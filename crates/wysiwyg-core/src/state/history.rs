use crate::transform::{step::Step, step_map::Mapping};

use super::{selection::Selection, transaction::Transaction};

/// A single item in the undo or redo stack.
///
/// Stores the *inverse* steps needed to revert the recorded transaction,
/// along with the selection and mapping from that point in history.
#[derive(Clone)]
pub struct HistoryItem {
    /// Inverse steps that, when applied in order, undo this recorded action.
    pub inverse_steps: Vec<Step>,
    /// The mapping from the original document's positions to the positions
    /// after this item was applied.  Used to remap positions when rebasing
    /// later undo items through earlier ones.
    pub mapping: Mapping,
    /// The selection immediately **before** this action was applied.
    /// Restored when the action is undone.
    pub selection_before: Selection,
}

/// Undo/redo history embedded directly in `EditorState`.
///
/// Design:
/// - `undo_stack`: items that can be undone (oldest at index 0, newest last).
/// - `redo_stack`: items that can be redone (most recently undone at the end).
/// - When a new transaction is recorded, the redo stack is cleared.
/// - When undo is called, the top of the undo stack is moved to the redo stack.
/// - When redo is called, the top of the redo stack is moved to the undo stack.
///
/// Max history depth is capped at `MAX_DEPTH`.
#[derive(Clone)]
pub struct HistoryState {
    undo_stack: Vec<HistoryItem>,
    redo_stack: Vec<HistoryItem>,
}

const MAX_DEPTH: usize = 100;

impl HistoryState {
    pub fn new() -> Self {
        HistoryState {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Record a transaction, returning an updated `HistoryState`.
    ///
    /// Computes the inverse steps from the transaction and pushes an item onto
    /// the undo stack.  Clears the redo stack.
    pub fn record(&self, tr: &Transaction) -> HistoryState {
        // Build inverse steps by replaying the transform with intermediate docs.
        let mut current_doc = tr.doc_before().clone();
        let mut inverse_steps: Vec<Step> = Vec::new();

        for step in tr.steps().iter() {
            let inv = step.invert(&current_doc);
            inverse_steps.push(inv);
            // Advance current_doc for the next step's inversion.
            if let Ok((next_doc, _)) = step.apply(&current_doc) {
                current_doc = next_doc;
            }
        }

        // The inverse steps should be applied in reverse order to undo.
        let item = HistoryItem {
            inverse_steps,
            mapping: tr.mapping().clone(),
            selection_before: tr.selection_before.clone(),
        };

        let mut new_undo = self.undo_stack.clone();
        new_undo.push(item);
        if new_undo.len() > MAX_DEPTH {
            new_undo.remove(0);
        }

        HistoryState {
            undo_stack: new_undo,
            redo_stack: Vec::new(), // clear redo on new action
        }
    }

    /// Undo the last action.
    ///
    /// Returns the new `HistoryState` and a completed `Transaction` that
    /// contains the inverse steps.  Returns `None` if the stack is empty.
    pub fn undo<'a>(
        &self,
        state: &'a super::editor_state::EditorState,
    ) -> Option<(HistoryState, Transaction)> {
        let item = self.undo_stack.last()?;

        let mut tr = state.transaction();
        // Apply inverse steps in reverse order.
        for step in item.inverse_steps.iter().rev() {
            if let Err(_) = tr.step(step.clone()) {
                return None; // step failed — history may be corrupted
            }
        }
        // Restore the selection from before the action.
        tr.set_selection(item.selection_before.clone());
        // Mark as NOT adding to history (so we don't loop).
        tr.set_meta("addToHistory", super::transaction::MetaValue::Bool(false));

        let mut new_undo = self.undo_stack.clone();
        let _undone_item = new_undo.pop().unwrap();

        let mut new_redo = self.redo_stack.clone();
        new_redo.push(HistoryItem {
            inverse_steps: tr.steps().iter().map(|s| s.invert(&state.doc)).collect(),
            // Note: invert of the undo = the original forward steps (approximately).
            // For redo, what we need is a way to re-apply the original action.
            // We store the undo-of-the-undo as the redo item.
            mapping: tr.mapping().clone(),
            selection_before: state.selection.clone(),
        });

        Some((
            HistoryState {
                undo_stack: new_undo,
                redo_stack: new_redo,
            },
            tr,
        ))
    }

    /// Redo the last undone action.
    pub fn redo<'a>(
        &self,
        state: &'a super::editor_state::EditorState,
    ) -> Option<(HistoryState, Transaction)> {
        let item = self.redo_stack.last()?;

        let mut tr = state.transaction();
        for step in item.inverse_steps.iter().rev() {
            if let Err(_) = tr.step(step.clone()) {
                return None;
            }
        }
        tr.set_selection(item.selection_before.clone());
        tr.set_meta("addToHistory", super::transaction::MetaValue::Bool(false));

        let mut new_redo = self.redo_stack.clone();
        let _redone = new_redo.pop().unwrap();

        let mut new_undo = self.undo_stack.clone();
        new_undo.push(HistoryItem {
            inverse_steps: tr.steps().iter().map(|s| s.invert(&state.doc)).collect(),
            mapping: tr.mapping().clone(),
            selection_before: state.selection.clone(),
        });

        Some((
            HistoryState {
                undo_stack: new_undo,
                redo_stack: new_redo,
            },
            tr,
        ))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }
}

impl Default for HistoryState {
    fn default() -> Self {
        Self::new()
    }
}
