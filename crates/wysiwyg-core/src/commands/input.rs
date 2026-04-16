//! Text-input commands.

use std::sync::Arc;

use crate::{
    model::{
        mark::MarkSet,
        node::{Fragment, Node},
        slice::Slice,
    },
    state::{EditorState, Selection, Transaction},
    transform::{replace_step::ReplaceStep, step::Step},
};

/// Insert `text` at the current selection, replacing any selected range.
///
/// After insertion the cursor is placed immediately after the new text.
/// Returns `None` if `text` is empty or the schema has no "text" node type.
pub fn insert_text(state: &EditorState, text: &str) -> Option<Transaction> {
    if text.is_empty() {
        return None;
    }

    let text_type = state.schema.node_type_by_name("text")?;
    let from = state.selection.from();
    let to = state.selection.to(&state.doc);

    let text_node = Arc::new(Node::text(text_type.id, text, MarkSet::empty()));
    let slice = Slice::new(Fragment::from_node(text_node), 0, 0);
    let step = Step::Replace(ReplaceStep::new(from, to, slice));

    let mut tr = state.transaction();
    tr.step(step).ok()?;

    // Cursor after the inserted text.
    let new_pos = from + text.chars().count();
    tr.set_selection(Selection::cursor(new_pos));

    Some(tr)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{attrs::Attrs, mark::MarkSet, node::{Fragment, Node}, schema::basic_schema},
        state::{EditorState, Selection},
    };
    use std::sync::Arc;

    fn state_with_paragraph(text: &str) -> EditorState {
        let schema = basic_schema();
        let text_type = schema.node_type_by_name("text").unwrap();
        let para_type = schema.node_type_by_name("paragraph").unwrap();
        let doc_type = schema.node_type_by_name("doc").unwrap();

        let text_node = Arc::new(Node::text(text_type.id, text, MarkSet::empty()));
        let para = Arc::new(Node::new(
            para_type.id,
            Attrs::empty(),
            Fragment::from_node(text_node),
            MarkSet::empty(),
        ));
        let doc = Arc::new(Node::new(
            doc_type.id,
            Attrs::empty(),
            Fragment::from_node(para),
            MarkSet::empty(),
        ));
        EditorState::new(schema, doc, Selection::cursor(1))
    }

    #[test]
    fn insert_at_cursor() {
        let state = state_with_paragraph("world");
        // Cursor at position 1 (start of paragraph).
        let tr = insert_text(&state, "hello ").unwrap();
        let new_state = state.apply(&tr).unwrap();

        let para = new_state.doc.child(0).unwrap();
        // Content should start with "hello ".
        let first_text = para.content.child(0).unwrap();
        assert!(first_text.text.as_deref().unwrap_or("").starts_with("hello"));

        // Cursor should be at 1 + 6 = 7 (after "hello ").
        assert_eq!(new_state.selection.from(), 7);
        assert!(new_state.selection.is_cursor());
    }

    #[test]
    fn insert_into_empty_paragraph() {
        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema);
        // Cursor at 1 (inside the empty paragraph).
        let tr = insert_text(&state, "hi").unwrap();
        let new_state = state.apply(&tr).unwrap();

        let para = new_state.doc.child(0).unwrap();
        let text_node = para.content.child(0).unwrap();
        assert_eq!(text_node.text.as_deref(), Some("hi"));
        // Cursor at 1 + 2 = 3.
        assert_eq!(new_state.selection.from(), 3);
    }

    #[test]
    fn insert_replaces_selection() {
        let state = state_with_paragraph("hello");
        // Select all text: anchor=1, head=6.
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let sel_state = EditorState::new(schema, doc, Selection::text(1, 6));
        let tr = insert_text(&sel_state, "world").unwrap();
        let new_state = sel_state.apply(&tr).unwrap();

        let para = new_state.doc.child(0).unwrap();
        let text_node = para.content.child(0).unwrap();
        assert_eq!(text_node.text.as_deref(), Some("world"));
    }

    #[test]
    fn insert_empty_string_returns_none() {
        let state = state_with_paragraph("hello");
        assert!(insert_text(&state, "").is_none());
    }
}
