use std::sync::Arc;

use crate::{
    model::{node::Node, schema::Schema},
    transform::step::StepError,
};

use super::{history::HistoryState, selection::Selection, transaction::Transaction};

/// Errors that can occur when applying a transaction.
#[derive(Debug, Clone)]
pub enum ApplyError {
    Step(StepError),
    InvalidSelection,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::Step(e) => write!(f, "Step error: {e}"),
            ApplyError::InvalidSelection => write!(f, "Invalid selection after transaction"),
        }
    }
}

impl std::error::Error for ApplyError {}

impl From<StepError> for ApplyError {
    fn from(e: StepError) -> Self {
        ApplyError::Step(e)
    }
}

/// The immutable editor state.
///
/// Cloning is cheap: the document tree is shared via `Arc`.  The `apply`
/// method is the **only** way to produce a new state.
///
/// ```text
/// let tr = state.transaction();
/// tr.insert(pos, content)?;
/// let new_state = state.apply(tr)?;
/// ```
#[derive(Clone)]
pub struct EditorState {
    /// The schema (never changes after construction).
    pub schema: Arc<Schema>,
    /// The current document.
    pub doc: Arc<Node>,
    /// The current selection.
    pub selection: Selection,
    /// Undo/redo history (Phase 2 — directly embedded, no plugin indirection).
    pub history: HistoryState,
    /// ドキュメントが変更されるたびに単調増加するバージョン番号。
    /// 差分更新の O(1) 変更検出に使用する。
    pub doc_version: u64,
}

impl EditorState {
    /// Create a new state.
    ///
    /// `doc` must conform to `schema`'s top-level node type.
    pub fn new(schema: Arc<Schema>, doc: Arc<Node>, selection: Selection) -> Self {
        EditorState {
            schema,
            doc,
            selection,
            history: HistoryState::new(),
            doc_version: 0,
        }
    }

    /// Create an editor state with an empty document for `schema`.
    ///
    /// The document is `doc -> [paragraph -> []]` — a single empty paragraph.
    pub fn with_empty_doc(schema: Arc<Schema>) -> Self {
        use crate::model::{
            attrs::Attrs,
            mark::MarkSet,
            node::{Fragment, Node},
        };

        let para_type = schema
            .node_type_by_name("paragraph")
            .expect("schema must have a 'paragraph' node type");
        let doc_type = schema
            .node_type_by_name("doc")
            .expect("schema must have a 'doc' node type");

        let para = Arc::new(Node::new(
            para_type.id,
            Attrs::empty(),
            Fragment::empty(),
            MarkSet::empty(),
        ));
        let doc = Arc::new(Node::new(
            doc_type.id,
            Attrs::empty(),
            Fragment::from_node(para),
            MarkSet::empty(),
        ));

        EditorState {
            schema,
            doc,
            selection: Selection::cursor(1), // inside the empty paragraph
            history: HistoryState::new(),
            doc_version: 0,
        }
    }

    /// Start a new transaction based on the current state.
    pub fn transaction(&self) -> Transaction {
        Transaction::new(
            self.doc.clone(),
            self.schema.clone(),
            self.selection.clone(),
        )
    }

    /// Apply a transaction, returning the next `EditorState`.
    ///
    /// This is the **only** way to create a new state.
    pub fn apply(&self, tr: &Transaction) -> Result<EditorState, ApplyError> {
        let new_doc = tr.doc().clone();
        let new_selection = tr.selection.clone().clamped(&new_doc);

        // Update history if this transaction modifies the document.
        let new_history = if tr.doc_changed() && tr.add_to_history() {
            self.history.record(tr)
        } else {
            self.history.clone()
        };

        Ok(EditorState {
            schema: self.schema.clone(),
            doc: new_doc,
            selection: new_selection,
            history: new_history,
            doc_version: if tr.doc_changed() {
                self.doc_version.wrapping_add(1)
            } else {
                self.doc_version
            },
        })
    }

    // -----------------------------------------------------------------------
    // Undo / Redo
    // -----------------------------------------------------------------------

    /// Undo the last recorded action, returning the new state (or `None` if
    /// there is nothing to undo).
    pub fn undo(&self) -> Option<EditorState> {
        let (new_history, tr) = self.history.undo(self)?;
        let new_doc = tr.doc().clone();
        let new_selection = tr.selection.clone().clamped(&new_doc);
        Some(EditorState {
            schema: self.schema.clone(),
            doc: new_doc,
            selection: new_selection,
            history: new_history,
            doc_version: self.doc_version.wrapping_add(1),
        })
    }

    /// Redo the last undone action, returning the new state (or `None` if
    /// there is nothing to redo).
    pub fn redo(&self) -> Option<EditorState> {
        let (new_history, tr) = self.history.redo(self)?;
        let new_doc = tr.doc().clone();
        let new_selection = tr.selection.clone().clamped(&new_doc);
        Some(EditorState {
            schema: self.schema.clone(),
            doc: new_doc,
            selection: new_selection,
            history: new_history,
            doc_version: self.doc_version.wrapping_add(1),
        })
    }

    /// Whether there are steps that can be undone.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there are steps that can be redone.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::schema::basic_schema;

    fn collect_text(node: &Arc<Node>) -> String {
        if let Some(t) = &node.text {
            return t.to_string();
        }
        node.content.children.iter().map(collect_text).collect()
    }

    #[test]
    fn with_empty_doc_creates_valid_state() {
        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema);
        assert_eq!(state.doc.child_count(), 1); // one empty paragraph
        assert_eq!(state.selection.from(), 1); // cursor inside the paragraph
    }

    #[test]
    fn apply_transaction_updates_doc() {
        use crate::model::{
            mark::MarkSet,
            node::{Fragment, Node},
        };

        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema.clone());

        // Insert "hello" into the empty paragraph.
        let text_type = schema.node_type_by_name("text").unwrap();
        let text_node = Arc::new(Node::text(text_type.id, "hello", MarkSet::empty()));

        let mut tr = state.transaction();
        tr.insert(1, Fragment::from_node(text_node)).unwrap();
        let new_state = state.apply(&tr).unwrap();

        assert_eq!(collect_text(&new_state.doc), "hello");
    }

    #[test]
    fn undo_reverts_change() {
        use crate::model::{mark::MarkSet, node::Fragment, node::Node};

        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema.clone());
        let text_type = schema.node_type_by_name("text").unwrap();

        let text_node = Arc::new(Node::text(text_type.id, "hello", MarkSet::empty()));
        let mut tr = state.transaction();
        tr.insert(1, Fragment::from_node(text_node)).unwrap();
        let state2 = state.apply(&tr).unwrap();
        assert_eq!(collect_text(&state2.doc), "hello");

        // Undo.
        let state3 = state2.undo().expect("should be able to undo");
        assert_eq!(collect_text(&state3.doc), "");
    }

    #[test]
    fn redo_after_undo() {
        use crate::model::{mark::MarkSet, node::Fragment, node::Node};

        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema.clone());
        let text_type = schema.node_type_by_name("text").unwrap();

        let text_node = Arc::new(Node::text(text_type.id, "hi", MarkSet::empty()));
        let mut tr = state.transaction();
        tr.insert(1, Fragment::from_node(text_node)).unwrap();
        let state2 = state.apply(&tr).unwrap();

        let state3 = state2.undo().unwrap();
        assert_eq!(collect_text(&state3.doc), "");

        let state4 = state3.redo().unwrap();
        assert_eq!(collect_text(&state4.doc), "hi");
    }

    #[test]
    fn transaction_not_added_to_history_when_flagged() {
        use crate::model::{mark::MarkSet, node::Fragment, node::Node};
        use crate::state::transaction::MetaValue;

        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema.clone());
        let text_type = schema.node_type_by_name("text").unwrap();

        let text_node = Arc::new(Node::text(text_type.id, "secret", MarkSet::empty()));
        let mut tr = state.transaction();
        tr.insert(1, Fragment::from_node(text_node)).unwrap();
        tr.set_meta("addToHistory", MetaValue::Bool(false));
        let state2 = state.apply(&tr).unwrap();

        // Cannot undo — not in history.
        assert!(!state2.can_undo());
    }
}
