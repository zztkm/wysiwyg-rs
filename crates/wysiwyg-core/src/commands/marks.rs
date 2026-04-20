//! Mark-related commands: toggle bold, italic, code, and arbitrary marks.

use crate::{
    model::{
        attrs::Attrs,
        mark::{Mark, MarkTypeId},
    },
    state::{EditorState, Transaction},
};

/// Toggle the mark with `type_id` on the current selection.
///
/// - If **all** inline content in the selection already has this mark,
///   the mark is removed from the entire selection.
/// - Otherwise, the mark is added to the entire selection.
///
/// Returns `None` if the selection is empty (cursor) and the schema does not
/// allow this mark at the cursor position.
pub fn toggle_mark(state: &EditorState, type_id: MarkTypeId, attrs: Attrs) -> Option<Transaction> {
    let sel = &state.selection;
    let from = sel.from();
    let to = sel.to(&state.doc);

    if from >= to {
        // Cursor — nothing to toggle over a range.
        // (Future: toggling a mark at a cursor stores "stored marks" for the
        //  next inserted text.  Phase 3 feature.)
        return None;
    }

    let mark = Mark::new(type_id, attrs);
    let already_applied = range_has_mark(&state.doc, from, to, type_id);

    let mut tr = state.transaction();
    if already_applied {
        tr.remove_mark(from, to, mark).ok()?;
    } else {
        tr.add_mark(from, to, mark).ok()?;
    }

    Some(tr)
}

/// Toggle **bold** on the current selection.
pub fn toggle_bold(state: &EditorState) -> Option<Transaction> {
    let bold_id = state.schema.mark_type_by_name("bold")?.id;
    toggle_mark(state, bold_id, Attrs::empty())
}

/// Toggle **italic** on the current selection.
pub fn toggle_italic(state: &EditorState) -> Option<Transaction> {
    let italic_id = state.schema.mark_type_by_name("italic")?.id;
    toggle_mark(state, italic_id, Attrs::empty())
}

/// Toggle **code** (inline code) on the current selection.
pub fn toggle_code(state: &EditorState) -> Option<Transaction> {
    let code_id = state.schema.mark_type_by_name("code")?.id;
    toggle_mark(state, code_id, Attrs::empty())
}

// ---------------------------------------------------------------------------
// Helper: check whether ALL text in [from..to) has the given mark.
// ---------------------------------------------------------------------------

fn range_has_mark(
    doc: &std::sync::Arc<crate::model::node::Node>,
    from: usize,
    to: usize,
    type_id: MarkTypeId,
) -> bool {
    check_fragment_has_mark(&doc.content, from, to, type_id)
}

fn check_fragment_has_mark(
    fragment: &crate::model::node::Fragment,
    from: usize,
    to: usize,
    type_id: MarkTypeId,
) -> bool {
    let mut offset = 0usize;
    let mut found_inline = false;
    let mut all_marked = true;

    for child in fragment.children.iter() {
        let child_size = child.node_size();
        let child_end = offset + child_size;

        if child_end > from && offset < to {
            if child.is_text() || child.is_leaf() {
                found_inline = true;
                if !child.marks.contains(type_id) {
                    all_marked = false;
                }
            } else {
                let inner_from = from.saturating_sub(offset + 1);
                let inner_to = to.saturating_sub(offset + 1).min(child.content.size);
                let sub_found =
                    check_fragment_has_mark(&child.content, inner_from, inner_to, type_id);
                if !sub_found {
                    all_marked = false;
                }
                // count as "found inline" if there's any inline content
                found_inline = true;
            }
        }

        offset = child_end;
    }

    found_inline && all_marked
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::schema::basic_schema,
        model::{mark::MarkSet, node::Fragment, node::Node},
        state::{EditorState, Selection},
    };
    use std::sync::Arc;

    fn make_state_with_text(text: &str) -> EditorState {
        let schema = basic_schema();
        let text_type = schema.node_type_by_name("text").unwrap();
        let para_type = schema.node_type_by_name("paragraph").unwrap();
        let doc_type = schema.node_type_by_name("doc").unwrap();

        let text_node = Arc::new(Node::text(text_type.id, text, MarkSet::empty()));
        let para = Arc::new(Node::new(
            para_type.id,
            crate::model::attrs::Attrs::empty(),
            Fragment::from_node(text_node),
            MarkSet::empty(),
        ));
        let doc = Arc::new(Node::new(
            doc_type.id,
            crate::model::attrs::Attrs::empty(),
            Fragment::from_node(para),
            MarkSet::empty(),
        ));

        // Select the whole paragraph content (1..=len+1).
        let len = text.chars().count();
        EditorState::new(schema, doc, Selection::text(1, len + 1))
    }

    #[test]
    fn toggle_bold_adds_mark() {
        let state = make_state_with_text("hello");
        // Select all: 1..6 (5 chars)
        let tr = toggle_bold(&state).expect("should return transaction");
        let new_state = state.apply(&tr).unwrap();

        let bold_id = state.schema.mark_type_by_name("bold").unwrap().id;
        let para = new_state.doc.child(0).unwrap();
        let text = para.content.child(0).unwrap();
        assert!(text.marks.contains(bold_id));
    }

    #[test]
    fn toggle_bold_twice_removes_mark() {
        let state = make_state_with_text("hello");
        let bold_id = state.schema.mark_type_by_name("bold").unwrap().id;

        // First toggle: add bold.
        let tr1 = toggle_bold(&state).unwrap();
        let state2 = state.apply(&tr1).unwrap();

        // Second toggle: remove bold.
        let tr2 = toggle_bold(&state2).unwrap();
        let state3 = state2.apply(&tr2).unwrap();

        let para = state3.doc.child(0).unwrap();
        let text = para.content.child(0).unwrap();
        assert!(!text.marks.contains(bold_id));
    }

    #[test]
    fn toggle_returns_none_for_cursor() {
        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema);
        // Cursor (no range) — toggle should return None.
        assert!(toggle_bold(&state).is_none());
    }
}
