//! Block-level commands: set block type, toggle heading.

use std::sync::Arc;

use crate::{
    model::{
        attrs::{AttrValue, Attrs},
        mark::MarkSet,
        node::{Node, NodeTypeId},
    },
    state::{EditorState, Transaction},
};

/// Change all block nodes that overlap the current selection to `type_name`
/// with the given `attrs`.
///
/// Returns `None` if no block in the selection needs to be changed.
pub fn set_block_type(state: &EditorState, type_name: &str, attrs: Attrs) -> Option<Transaction> {
    let node_type = state.schema.node_type_by_name(type_name)?;
    let type_id = node_type.id;

    // Save selection before we apply steps (it will be remapped through the step).
    let saved_selection = state.selection.clone();

    let mut tr = state.transaction();

    // Apply one ReplaceStep per affected block, so positions are preserved as
    // much as possible.  For each block node that needs to change type, replace
    // [node_start .. node_start+node_size) with the new node.
    let mut offset = 0usize;
    let sel_from = state.selection.from();
    let sel_to = state.selection.to(&state.doc);

    for child in state.doc.content.children.iter() {
        let child_size = child.node_size();
        let child_end = offset + child_size;

        if child_end > sel_from && offset < sel_to && !child.is_text() && !child.is_leaf() {
            // Build the replacement node.
            let new_child = Arc::new(Node::new(
                type_id,
                attrs.clone(),
                child.content.clone(),
                MarkSet::empty(),
            ));
            if new_child.type_id != child.type_id || new_child.attrs != child.attrs {
                use crate::model::slice::Slice;
                use crate::transform::replace_step::ReplaceStep;
                use crate::transform::step::Step;
                let frag = crate::model::node::Fragment::from_node(new_child);
                let step =
                    Step::Replace(ReplaceStep::new(offset, child_end, Slice::new(frag, 0, 0)));
                tr.step(step).ok()?;
            }
        }

        offset = child_end;
    }

    if !tr.doc_changed() {
        return None;
    }

    // Restore the selection (clamped to the new doc).
    tr.set_selection(saved_selection);

    Some(tr)
}

/// Toggle a heading of the given `level` on the current selection.
///
/// - If all selected blocks are already headings of `level`, revert to
///   paragraph.
/// - Otherwise, set them to heading with `level`.
pub fn toggle_heading(state: &EditorState, level: i64) -> Option<Transaction> {
    let heading_type = state.schema.node_type_by_name("heading")?;
    // Verify "paragraph" exists in schema before proceeding.
    state.schema.node_type_by_name("paragraph")?;

    let sel = &state.selection;
    let from = sel.from();
    let to = sel.to(&state.doc);

    // Check if all blocks in selection are already this heading level.
    let all_heading = all_blocks_are(
        &state.doc.content,
        from,
        to,
        heading_type.id,
        Some(("level", &AttrValue::Int(level))),
    );

    if all_heading {
        set_block_type(state, "paragraph", Attrs::empty())
    } else {
        let attrs = Attrs::empty().with("level", AttrValue::Int(level));
        set_block_type(state, "heading", attrs)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true if all block nodes in `[from..to)` within `fragment` have
/// `type_id` and (optionally) an attr matching `(key, value)`.
fn all_blocks_are(
    fragment: &crate::model::node::Fragment,
    from: usize,
    to: usize,
    type_id: NodeTypeId,
    attr_check: Option<(&str, &AttrValue)>,
) -> bool {
    let mut offset = 0usize;
    let mut found = false;
    let mut all_match = true;

    for child in fragment.children.iter() {
        let child_size = child.node_size();
        let child_end = offset + child_size;

        if child_end > from && offset < to && !child.is_text() && !child.is_leaf() {
            found = true;
            if child.type_id != type_id {
                all_match = false;
            } else if let Some((key, val)) = attr_check {
                if child.attrs.get(key) != Some(val) {
                    all_match = false;
                }
            }
        }

        offset = child_end;
    }

    found && all_match
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::schema::basic_schema,
        model::{
            attrs::Attrs,
            mark::MarkSet,
            node::{Fragment, Node},
        },
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

        let len = text.chars().count();
        EditorState::new(schema, doc, Selection::text(1, len + 1))
    }

    #[test]
    fn set_block_type_to_heading() {
        let state = state_with_paragraph("hello");
        let heading_type = state.schema.node_type_by_name("heading").unwrap();

        let tr = set_block_type(
            &state,
            "heading",
            Attrs::empty().with("level", AttrValue::Int(2)),
        )
        .expect("should produce a transaction");

        let new_state = state.apply(&tr).unwrap();
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.type_id, heading_type.id);
        assert_eq!(block.attrs.get("level"), Some(&AttrValue::Int(2)));
    }

    #[test]
    fn set_block_type_back_to_paragraph() {
        let state = state_with_paragraph("hello");
        let para_type = state.schema.node_type_by_name("paragraph").unwrap();

        // First set to heading, then back to paragraph.
        let tr1 = set_block_type(
            &state,
            "heading",
            Attrs::empty().with("level", AttrValue::Int(1)),
        )
        .unwrap();
        let state2 = state.apply(&tr1).unwrap();

        let tr2 = set_block_type(&state2, "paragraph", Attrs::empty()).unwrap();
        let state3 = state2.apply(&tr2).unwrap();

        let block = state3.doc.child(0).unwrap();
        assert_eq!(block.type_id, para_type.id);
    }

    #[test]
    fn toggle_heading_sets_heading() {
        let state = state_with_paragraph("hello");
        let heading_type = state.schema.node_type_by_name("heading").unwrap();

        let tr = toggle_heading(&state, 1).unwrap();
        let new_state = state.apply(&tr).unwrap();

        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.type_id, heading_type.id);
        assert_eq!(block.attrs.get("level"), Some(&AttrValue::Int(1)));
    }

    #[test]
    fn toggle_heading_reverts_to_paragraph_if_already_heading() {
        let state = state_with_paragraph("hello");
        let para_type = state.schema.node_type_by_name("paragraph").unwrap();

        // Set to heading first.
        let tr1 = toggle_heading(&state, 2).unwrap();
        let state2 = state.apply(&tr1).unwrap();

        // Toggle again at same level → back to paragraph.
        let tr2 = toggle_heading(&state2, 2).unwrap();
        let state3 = state2.apply(&tr2).unwrap();

        let block = state3.doc.child(0).unwrap();
        assert_eq!(block.type_id, para_type.id);
    }
}
