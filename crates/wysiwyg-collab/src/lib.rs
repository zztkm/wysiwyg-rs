// wysiwyg-collab: collaborative editing layer using yrs (CRDT)
//
// Prototype scope (Phase 4):
//   - Flat document only: doc > [paragraph*]
//   - Text insertions and deletions (ReplaceStep with text content)
//   - Mark changes and block-type changes are NOT synced to yrs in this prototype
//   - Two-peer convergence is the exit criterion
//
// TODO: nested block support (blockquote, list_item containing blocks)
// TODO: sync AddMarkStep / RemoveMarkStep to yrs as XmlElement attributes
// TODO: sync block-type changes (heading, code_block) to yrs element tag names
// TODO: handle ReplaceAroundStep (wrap / lift operations)
// TODO: proper initial-state sharing — CollabState::new() creates a paragraph per peer,
//       leading to N paragraphs after CRDT merge; real use requires one peer to initialise
//       and distribute state via encode_state_as_update → apply_remote_update before editing

use std::sync::Arc;

use wysiwyg_core::{
    model::{
        attrs::Attrs,
        mark::MarkSet,
        node::{Fragment, Node},
        schema::{basic_schema, Schema},
    },
    state::{EditorState, Selection},
    transform::{replace_step::ReplaceStep, step::Step},
};
use yrs::{
    types::xml::XmlOut, updates::decoder::Decode, Doc, GetString, ReadTxn, Text, Transact,
    WriteTxn, XmlElementPrelim, XmlFragment, XmlTextPrelim,
};

// ---------------------------------------------------------------------------
// Position mapping: PM logical position → (block_index, char_offset)
// ---------------------------------------------------------------------------

/// Map an absolute PM position inside a flat doc (doc → [paragraph*]) to
/// `(block_index, char_offset_within_block)`.
///
/// Returns `None` if the position is on a block boundary (not inside text content)
/// or out of range.
///
/// # Limitations
///
/// TODO: only handles a flat `doc > [block*]` structure; nested blocks
///       (e.g. `blockquote > paragraph`) are not resolved correctly because
///       the function iterates only the top-level children of `doc`.
pub fn resolve_text_pos(doc: &Arc<Node>, pm_pos: usize) -> Option<(u32, u32)> {
    let mut offset = 0usize;
    for (idx, child) in doc.content.children.iter().enumerate() {
        let child_size = child.node_size(); // e.g. para: text_size + 2
        // Text positions inside this block: [offset+1 .. offset+child_size-1]
        let text_start = offset + 1;
        let text_end = offset + child_size - 1; // exclusive end = offset + 1 + content_size
        if pm_pos >= text_start && pm_pos <= text_end {
            let char_offset = pm_pos - text_start;
            return Some((idx as u32, char_offset as u32));
        }
        offset += child_size;
    }
    None
}

// ---------------------------------------------------------------------------
// Reconstruct PM doc from yrs XmlFragment (reverse sync)
// ---------------------------------------------------------------------------

/// Read the yrs XmlFragment "content" and rebuild an `Arc<Node>` PM document.
///
/// Only `<paragraph>` elements containing `XmlText` children are supported.
/// Unknown elements are skipped.
///
/// # Limitations
///
/// TODO: heading / code_block support — map yrs element tag names (e.g. `"heading"`)
///       back to the correct PM node type and restore attrs (e.g. `level`).
/// TODO: mark reconstruction — yrs XmlText attributes should map back to PM marks
///       (bold, italic, code, link) when mark sync is implemented.
/// TODO: nested block reconstruction (blockquote, list_item).
pub fn build_pm_doc_from_yrs<T: ReadTxn>(
    content: &yrs::XmlFragmentRef,
    txn: &T,
    schema: &Arc<Schema>,
) -> Arc<Node> {
    let doc_type = schema.node_type_by_name("doc").unwrap();
    let para_type = schema.node_type_by_name("paragraph").unwrap();
    let text_type = schema.node_type_by_name("text").unwrap();

    let mut paragraphs: Vec<Arc<Node>> = Vec::new();

    let len = content.len(txn);
    for i in 0..len {
        let Some(child) = content.get(txn, i) else { continue };
        let XmlOut::Element(elem) = child else { continue };
        if elem.tag().as_ref() != "paragraph" {
            continue;
        }

        // Collect all XmlText strings in the element and concatenate.
        let mut text_content = String::new();
        let elem_len = elem.len(txn);
        for j in 0..elem_len {
            if let Some(XmlOut::Text(xml_text)) = elem.get(txn, j) {
                text_content.push_str(&xml_text.get_string(txn));
            }
        }

        let para_node = if text_content.is_empty() {
            // Empty paragraph
            Arc::new(Node::new(
                para_type.id,
                Attrs::empty(),
                Fragment::empty(),
                MarkSet::empty(),
            ))
        } else {
            let text_node = Arc::new(Node::text(text_type.id, text_content.as_str(), MarkSet::empty()));
            Arc::new(Node::new(
                para_type.id,
                Attrs::empty(),
                Fragment::from_node(text_node),
                MarkSet::empty(),
            ))
        };
        paragraphs.push(para_node);
    }

    // Ensure there is at least one paragraph.
    if paragraphs.is_empty() {
        paragraphs.push(Arc::new(Node::new(
            para_type.id,
            Attrs::empty(),
            Fragment::empty(),
            MarkSet::empty(),
        )));
    }

    Arc::new(Node::new(
        doc_type.id,
        Attrs::empty(),
        Fragment::from_nodes(paragraphs),
        MarkSet::empty(),
    ))
}

// ---------------------------------------------------------------------------
// CollabState
// ---------------------------------------------------------------------------

/// A collaborative editor state.
///
/// Wraps an `EditorState` and a `yrs::Doc`, keeping them in sync for text
/// insertions and deletions in flat (non-nested) documents.
pub struct CollabState {
    pub editor: EditorState,
    pub ydoc: Doc,
}

impl CollabState {
    /// Create a new `CollabState` from a `yrs::Doc` id (useful for multi-peer
    /// tests) and an empty document.
    ///
    /// # Known limitation
    ///
    /// Each peer that calls `new()` inserts its own initial empty paragraph into
    /// its private yrs document.  After a CRDT merge the two paragraphs coexist,
    /// resulting in N paragraphs for N peers rather than a single shared document.
    ///
    /// TODO: replace this constructor with a factory pattern where one designated
    ///       peer creates the document and distributes the initial state via
    ///       `encode_state_as_update` before any other peer starts editing.
    pub fn new(client_id: u64) -> Self {
        let schema = basic_schema();
        let editor = EditorState::with_empty_doc(schema);
        let ydoc = Doc::with_client_id(client_id);
        // Initialise the yrs side with an empty paragraph.
        {
            let mut txn = ydoc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        }
        CollabState { editor, ydoc }
    }

    /// Apply a PM transaction to the editor state, and propagate text-only
    /// steps to the yrs document.
    ///
    /// Returns `true` if the transaction was applied successfully.
    pub fn apply_transaction(
        &mut self,
        tr: wysiwyg_core::state::Transaction,
    ) -> bool {
        let doc_before = self.editor.doc.clone();
        match self.editor.apply(&tr) {
            Ok(new_state) => {
                self.editor = new_state;
                // Sync text steps to yrs.
                for step in tr.steps() {
                    if let Step::Replace(rs) = step {
                        self.sync_replace_step_to_yrs(rs, &doc_before);
                    }
                    // TODO: sync AddMarkStep / RemoveMarkStep — store mark state as
                    //       XmlText attributes in yrs so remote peers can reconstruct marks.
                    // TODO: sync ReplaceAroundStep — needed for wrap/lift (e.g. turning a
                    //       paragraph into a blockquote or list item).
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Apply a yrs state vector update from a remote peer and rebuild the
    /// PM editor state from the updated yrs document.
    pub fn apply_remote_update(&mut self, update: &[u8]) {
        let mut txn = self.ydoc.transact_mut();
        txn.apply_update(yrs::Update::decode_v1(update).expect("decode_v1 failed"))
            .expect("apply_update failed");
        drop(txn);
        self.rebuild_pm_from_yrs();
    }

    /// Read the yrs document and reconstruct the PM doc, updating the editor
    /// state in place.
    ///
    /// # Known limitation
    ///
    /// TODO: the selection is always reset to `cursor(1)` after a remote update.
    ///       The correct behaviour is to remap the existing selection through the
    ///       update's `Mapping` so the cursor follows the user's intended position.
    pub fn rebuild_pm_from_yrs(&mut self) {
        let txn = self.ydoc.transact();
        let content = txn
            .get_xml_fragment("content")
            .expect("'content' fragment must exist");
        let new_doc = build_pm_doc_from_yrs(&content, &txn, &self.editor.schema);
        self.editor = EditorState::new(
            self.editor.schema.clone(),
            new_doc,
            Selection::cursor(1), // TODO: remap selection through the update mapping
        );
    }

    /// Encode the yrs state as a v1 update that can be sent to a remote peer.
    pub fn encode_state_as_update(&self) -> Vec<u8> {
        let txn = self.ydoc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    // -----------------------------------------------------------------------
    // Internal: forward sync
    // -----------------------------------------------------------------------

    fn sync_replace_step_to_yrs(&self, rs: &ReplaceStep, doc_before: &Arc<Node>) {
        // Only handle pure text operations in flat docs.
        // A text insertion/deletion has from==to (cursor insert) or
        // from<to (range delete/replace) and a slice with 0 open ends.
        if rs.slice.open_start != 0 || rs.slice.open_end != 0 {
            // TODO: handle open slices — these represent block splits (Enter key) and
            //       block merges (Backspace at start of paragraph), which require
            //       inserting or removing XmlElement nodes from the yrs XmlFragment.
            return; // Block-level change — not synced in prototype
        }

        // Resolve deletion range in the pre-transaction doc.
        let del_from = resolve_text_pos(doc_before, rs.from);
        let del_to = resolve_text_pos(doc_before, rs.to);

        // Determine insertion text from the slice (must be a single text node in
        // a single paragraph).
        let insert_text: Option<String> = extract_flat_text_from_slice(&rs.slice);

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        // Delete the range if non-empty.
        if rs.from < rs.to {
            if let (Some((bi, char_from)), Some((bi_to, char_to))) = (del_from, del_to) {
                if bi == bi_to {
                    // Deletion within a single paragraph.
                    if let Some(XmlOut::Element(para)) = content.get(&txn, bi) {
                        if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                            let delete_len = char_to - char_from;
                            xml_text.remove_range(&mut txn, char_from, delete_len);
                        }
                    }
                }
                // TODO: cross-paragraph deletions (bi != bi_to) — requires removing
                //       text from the tail of the first paragraph, removing intermediate
                //       paragraphs entirely, and removing text from the head of the last
                //       paragraph, all within a single yrs transaction.
            }
        }

        // Insert text at the from position (in the post-deletion doc, which
        // we approximate by resolving in the current yrs document).
        if let Some(text) = insert_text {
            if let Some((bi, char_offset)) = resolve_text_pos(doc_before, rs.from) {
                if let Some(XmlOut::Element(para)) = content.get(&txn, bi) {
                    // Ensure there is an XmlText node.
                    if para.len(&txn) == 0 {
                        para.insert(&mut txn, 0, XmlTextPrelim::new(""));
                    }
                    if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                        xml_text.insert(&mut txn, char_offset, &text);
                    }
                }
            }
        }
    }
}

/// Extract the concatenated text from a `Slice` that contains exactly one
/// paragraph of flat text nodes.  Returns `None` if the slice has nested
/// structure or contains no text.
///
/// TODO: handle multi-paragraph slices (paste spanning multiple blocks).
/// TODO: preserve mark information — currently marks on text nodes are discarded,
///       which means pasted bold/italic text loses its formatting in the yrs layer.
fn extract_flat_text_from_slice(slice: &wysiwyg_core::model::slice::Slice) -> Option<String> {
    if slice.content.is_empty() {
        return None;
    }
    // Case 1: slice content is a single text node directly.
    if let Some(first) = slice.content.child(0) {
        if first.is_text() {
            return first.text.as_ref().map(|t| t.as_ref().to_string());
        }
        // Case 2: slice content is a paragraph containing text nodes.
        if !first.is_leaf() {
            let mut text = String::new();
            for child in first.content.children.iter() {
                if let Some(t) = &child.text {
                    text.push_str(t.as_ref());
                }
            }
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wysiwyg_core::commands::insert_text;

    #[test]
    fn resolve_text_pos_single_para() {
        // doc > [para("hello")]
        // The para node occupies content positions 0-6 (size=7):
        //   pos 0: gap before para (at doc level — NOT inside para content)
        //   pos 1: inside para, before 'h'  → char_offset 0
        //   pos 2: inside para, before 'e'  → char_offset 1
        //   pos 3: inside para, before 'l'  → char_offset 2
        //   pos 4: inside para, before 'l'  → char_offset 3
        //   pos 5: inside para, before 'o'  → char_offset 4
        //   pos 6: inside para, after 'o'   → char_offset 5  (valid end-of-para cursor)
        //   pos 7: gap after para (at doc level — NOT inside para content)
        let schema = basic_schema();
        let text_type = schema.node_type_by_name("text").unwrap();
        let para_type = schema.node_type_by_name("paragraph").unwrap();
        let doc_type = schema.node_type_by_name("doc").unwrap();
        let text_node = Arc::new(Node::text(text_type.id, "hello", MarkSet::empty()));
        let para = Arc::new(Node::new(para_type.id, Attrs::empty(), Fragment::from_node(text_node), MarkSet::empty()));
        let doc = Arc::new(Node::new(doc_type.id, Attrs::empty(), Fragment::from_node(para), MarkSet::empty()));

        assert_eq!(resolve_text_pos(&doc, 0), None);        // doc-level gap before para
        assert_eq!(resolve_text_pos(&doc, 1), Some((0, 0))); // before 'h'
        assert_eq!(resolve_text_pos(&doc, 3), Some((0, 2))); // before 'l'
        assert_eq!(resolve_text_pos(&doc, 5), Some((0, 4))); // before 'o'
        assert_eq!(resolve_text_pos(&doc, 6), Some((0, 5))); // after 'o' (end-of-para cursor)
        // Position 7 would be outside doc.content.size=7 range, handled by caller
    }

    #[test]
    fn resolve_text_pos_multi_para() {
        // doc > [para("ab"), para("cd")]
        // para("ab") at offset=0: size=4
        //   pos 0: doc-level gap before para(ab)
        //   pos 1: inside para(ab), before 'a' → char_offset 0
        //   pos 2: inside para(ab), before 'b' → char_offset 1
        //   pos 3: inside para(ab), after 'b'  → char_offset 2 (end-of-para cursor)
        // para("cd") at offset=4: size=4
        //   pos 4: doc-level gap before para(cd)
        //   pos 5: inside para(cd), before 'c' → char_offset 0
        //   pos 6: inside para(cd), before 'd' → char_offset 1
        //   pos 7: inside para(cd), after 'd'  → char_offset 2 (end-of-para cursor)
        let schema = basic_schema();
        let text_type = schema.node_type_by_name("text").unwrap();
        let para_type = schema.node_type_by_name("paragraph").unwrap();
        let doc_type = schema.node_type_by_name("doc").unwrap();

        let make_para = |s: &str| {
            let t = Arc::new(Node::text(text_type.id, s, MarkSet::empty()));
            Arc::new(Node::new(para_type.id, Attrs::empty(), Fragment::from_node(t), MarkSet::empty()))
        };

        let doc = Arc::new(Node::new(
            doc_type.id, Attrs::empty(),
            Fragment::from_nodes(vec![make_para("ab"), make_para("cd")]),
            MarkSet::empty(),
        ));

        assert_eq!(resolve_text_pos(&doc, 0), None);        // doc-level gap before para(ab)
        assert_eq!(resolve_text_pos(&doc, 1), Some((0, 0))); // before 'a'
        assert_eq!(resolve_text_pos(&doc, 2), Some((0, 1))); // before 'b'
        assert_eq!(resolve_text_pos(&doc, 3), Some((0, 2))); // after 'b' (end of para(ab))
        assert_eq!(resolve_text_pos(&doc, 4), None);        // doc-level gap before para(cd)
        assert_eq!(resolve_text_pos(&doc, 5), Some((1, 0))); // before 'c'
        assert_eq!(resolve_text_pos(&doc, 6), Some((1, 1))); // before 'd'
        assert_eq!(resolve_text_pos(&doc, 7), Some((1, 2))); // after 'd' (end of para(cd))
    }

    #[test]
    fn two_peer_text_convergence() {
        // Peer A and Peer B each start with an empty doc.
        // Peer A types "hello", Peer B types "world".
        // After syncing both should converge: yrs guarantees identical state.

        let mut peer_a = CollabState::new(1);
        let mut peer_b = CollabState::new(2);

        // Peer A inserts "hello"
        let tr_a = insert_text(&peer_a.editor, "hello").unwrap();
        assert!(peer_a.apply_transaction(tr_a));

        // Peer B inserts "world"
        let tr_b = insert_text(&peer_b.editor, "world").unwrap();
        assert!(peer_b.apply_transaction(tr_b));

        // Exchange full state updates.
        let update_a = peer_a.encode_state_as_update();
        let update_b = peer_b.encode_state_as_update();

        peer_a.apply_remote_update(&update_b);
        peer_b.apply_remote_update(&update_a);

        // Read merged text from yrs by concatenating all XmlText across all paragraphs.
        // Note: because each peer inserts a paragraph at index 0 in CollabState::new(),
        // after CRDT merge there are 2 paragraphs (one per peer).  Each paragraph holds
        // one peer's text, so we concatenate all paragraphs to get the full merged result.
        fn read_text(state: &CollabState) -> String {
            let txn = state.ydoc.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            let para_count = content.len(&txn);
            let mut out = String::new();
            for pi in 0..para_count {
                if let Some(XmlOut::Element(para)) = content.get(&txn, pi) {
                    let len = para.len(&txn);
                    for i in 0..len {
                        if let Some(XmlOut::Text(t)) = para.get(&txn, i) {
                            out.push_str(&t.get_string(&txn));
                        }
                    }
                }
            }
            out
        }

        let text_a = read_text(&peer_a);
        let text_b = read_text(&peer_b);

        // Both peers converge to the same text (yrs CRDT guarantee).
        assert_eq!(text_a, text_b, "peers did not converge");

        // The merged text must contain both strings.
        assert!(
            text_a.contains("hello") && text_a.contains("world"),
            "merged text '{text_a}' is missing content"
        );
    }
}

#[cfg(test)]
mod api_probe {
    use yrs::{
        types::xml::XmlOut, Doc, GetString, Transact, WriteTxn, XmlFragment,
        XmlElementPrelim, XmlTextPrelim, ReadTxn,
    };

    /// Verify that the yrs 0.21 XmlFragment API works as expected.
    #[test]
    fn yrs_xmlfragment_roundtrip() {
        let doc = Doc::new();

        {
            let mut txn = doc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            let para = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            para.insert(&mut txn, 0, XmlTextPrelim::new("hello"));
        }

        {
            let txn = doc.transact();
            let content = txn
                .get_xml_fragment("content")
                .expect("'content' fragment must exist");

            assert_eq!(content.len(&txn), 1);
            let first = content.get(&txn, 0).expect("first child");
            let para = match first {
                XmlOut::Element(e) => e,
                other => panic!("expected XmlElement, got {:?}", other),
            };
            assert_eq!(para.tag().as_ref(), "paragraph");
            assert_eq!(para.len(&txn), 1);
            let text_node = para.get(&txn, 0).expect("text node");
            let xml_text = match text_node {
                XmlOut::Text(t) => t,
                other => panic!("expected XmlText, got {:?}", other),
            };
            assert_eq!(xml_text.get_string(&txn), "hello");
        }
    }

    /// Verify that yrs update encoding and application (sync protocol) works.
    #[test]
    fn yrs_two_doc_convergence() {
        use yrs::{updates::decoder::Decode, Update};

        let doc1 = Doc::with_client_id(1);
        let doc2 = Doc::with_client_id(2);

        // doc1: add paragraph with "hello"
        {
            let mut txn = doc1.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            let para = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            let text = para.insert(&mut txn, 0, XmlTextPrelim::new("hello"));
            let _ = text;
        }

        // doc2: add paragraph with "world"
        {
            let mut txn = doc2.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            let para = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            let text = para.insert(&mut txn, 0, XmlTextPrelim::new("world"));
            let _ = text;
        }

        // Encode full state of each doc
        let update1 = doc1.transact().encode_state_as_update_v1(&yrs::StateVector::default());
        let update2 = doc2.transact().encode_state_as_update_v1(&yrs::StateVector::default());

        // Apply each to the other
        doc1.transact_mut().apply_update(Update::decode_v1(&update2).unwrap()).unwrap();
        doc2.transact_mut().apply_update(Update::decode_v1(&update1).unwrap()).unwrap();

        // Read merged text from doc1
        let text1 = {
            let txn = doc1.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            let mut out = String::new();
            let para_count = content.len(&txn);
            for i in 0..para_count {
                if let Some(XmlOut::Element(para)) = content.get(&txn, i) {
                    let len = para.len(&txn);
                    for j in 0..len {
                        if let Some(XmlOut::Text(t)) = para.get(&txn, j) {
                            out.push_str(&t.get_string(&txn));
                        }
                    }
                }
            }
            out
        };

        // Read merged text from doc2
        let text2 = {
            let txn = doc2.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            let mut out = String::new();
            let para_count = content.len(&txn);
            for i in 0..para_count {
                if let Some(XmlOut::Element(para)) = content.get(&txn, i) {
                    let len = para.len(&txn);
                    for j in 0..len {
                        if let Some(XmlOut::Text(t)) = para.get(&txn, j) {
                            out.push_str(&t.get_string(&txn));
                        }
                    }
                }
            }
            out
        };

        assert_eq!(text1, text2, "docs did not converge");
        assert!(
            text1.contains("hello") && text1.contains("world"),
            "merged text '{text1}' is missing content"
        );
    }
}
