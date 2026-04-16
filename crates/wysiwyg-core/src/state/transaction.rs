use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    model::{mark::Mark, node::{Fragment, Node}, schema::Schema, slice::Slice},
    transform::{
        mark_step::{AddMarkStep, RemoveMarkStep},
        replace_step::ReplaceStep,
        step::{Step, StepError},
        step_map::Mapping,
        transform::Transform,
    },
};

use super::selection::Selection;

/// Arbitrary metadata attached to a transaction.
///
/// Used to communicate intent between the editor core and plugins/history.
/// For example, `"addToHistory": Bool(false)` prevents a transaction from
/// being recorded in the undo stack.
#[derive(Debug, Clone)]
pub enum MetaValue {
    Bool(bool),
    String(String),
    Int(i64),
}

/// A transaction: a set of steps to apply to an `EditorState`.
///
/// Built by calling `state.transaction()`, then calling methods to accumulate
/// changes. Applied via `state.apply(&tr)` to produce a new `EditorState`.
///
/// A `Transaction` is a `Transform` plus:
/// - Selection tracking (the selection after the transaction).
/// - Metadata (arbitrary key-value annotations).
/// - A reference to the `EditorState` it was built from (for type checking etc.).
pub struct Transaction {
    /// The underlying transform.
    pub(super) transform: Transform,
    /// The schema (shared reference).
    pub schema: Arc<Schema>,
    /// The selection at the time the transaction was created (before any steps).
    pub selection_before: Selection,
    /// Selection after the transaction.  Starts as a copy of `selection_before`,
    /// mapped through each step as they are applied.
    pub selection: Selection,
    /// Arbitrary metadata.
    metadata: HashMap<String, MetaValue>,
}

impl Transaction {
    pub(super) fn new(doc: Arc<Node>, schema: Arc<Schema>, selection: Selection) -> Self {
        Transaction {
            transform: Transform::new(doc),
            schema,
            selection_before: selection.clone(),
            selection,
            metadata: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Document mutation (delegate to Transform)
    // -----------------------------------------------------------------------

    /// Apply a step, updating the document, mapping, and selection.
    pub fn step(&mut self, step: Step) -> Result<&mut Self, StepError> {
        let map = step.get_map();
        self.transform.step(step)?;
        // Map the selection through the new step.
        self.selection = self.selection.clone().map(&{
            let mut m = Mapping::new();
            m.append_map(map);
            m
        });
        Ok(self)
    }

    /// Replace the range `[from..to)` with `slice`.
    pub fn replace(&mut self, from: usize, to: usize, slice: Slice) -> Result<&mut Self, StepError> {
        self.step(Step::Replace(ReplaceStep::new(from, to, slice)))
    }

    /// Insert `content` at `pos`.
    pub fn insert(&mut self, pos: usize, content: Fragment) -> Result<&mut Self, StepError> {
        self.replace(pos, pos, Slice::new(content, 0, 0))
    }

    /// Delete the range `[from..to)`.
    pub fn delete(&mut self, from: usize, to: usize) -> Result<&mut Self, StepError> {
        self.replace(from, to, Slice::empty())
    }

    /// Add `mark` to all inline content in `[from..to)`.
    pub fn add_mark(&mut self, from: usize, to: usize, mark: Mark) -> Result<&mut Self, StepError> {
        self.step(Step::AddMark(AddMarkStep::new(from, to, mark)))
    }

    /// Remove `mark` from all inline content in `[from..to)`.
    pub fn remove_mark(&mut self, from: usize, to: usize, mark: Mark) -> Result<&mut Self, StepError> {
        self.step(Step::RemoveMark(RemoveMarkStep::new(from, to, mark)))
    }

    // -----------------------------------------------------------------------
    // Selection
    // -----------------------------------------------------------------------

    /// Explicitly set the selection after this transaction.
    pub fn set_selection(&mut self, selection: Selection) -> &mut Self {
        self.selection = selection;
        self
    }

    // -----------------------------------------------------------------------
    // Metadata
    // -----------------------------------------------------------------------

    pub fn set_meta(&mut self, key: impl Into<String>, value: MetaValue) -> &mut Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn get_meta(&self, key: &str) -> Option<&MetaValue> {
        self.metadata.get(key)
    }

    /// Whether this transaction should be added to the undo history.
    /// Defaults to `true`; set `"addToHistory": Bool(false)` to opt out.
    pub fn add_to_history(&self) -> bool {
        match self.metadata.get("addToHistory") {
            Some(MetaValue::Bool(v)) => *v,
            _ => true,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn doc(&self) -> &Arc<Node> {
        &self.transform.doc
    }

    pub fn doc_before(&self) -> &Arc<Node> {
        &self.transform.doc_before
    }

    pub fn steps(&self) -> &[Step] {
        &self.transform.steps
    }

    pub fn mapping(&self) -> &Mapping {
        &self.transform.mapping
    }

    pub fn doc_changed(&self) -> bool {
        self.transform.doc_changed()
    }
}
