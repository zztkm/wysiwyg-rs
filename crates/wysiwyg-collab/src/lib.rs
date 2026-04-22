// wysiwyg-collab: collaborative editing layer using yrs (CRDT)
//
// Implemented scope:
//   - Flat document: doc > [block*] (paragraph, heading, code_block)
//   - Text insertions and deletions within a single block
//   - Block split (Enter key, split_block) and block merge (Backspace at start)
//   - Mark sync (AddMarkStep / RemoveMarkStep → XmlText::format)
//   - Block type changes (heading / code_block / paragraph)
//   - Selection clamp after remote update
//   - Two-peer convergence verified by tests
//
// TODO: nested block support (blockquote, list_item containing blocks)
// TODO: cross-paragraph deletions (ReplaceStep where from and to span multiple blocks)
// TODO: marks lost on block merge (block B's mark info is stripped — plain text only)
// TODO: handle ReplaceAroundStep (wrap / lift operations)

use std::collections::HashMap;
use std::sync::Arc;

use wysiwyg_core::{
    model::{
        attrs::{AttrValue, Attrs},
        mark::{Mark, MarkSet},
        node::{Fragment, Node},
        schema::{basic_schema, Schema},
    },
    state::{EditorState, Selection},
    transform::{
        mark_step::{AddMarkStep, RemoveMarkStep},
        replace_step::ReplaceStep,
        step::Step,
    },
};
use yrs::{
    types::xml::XmlOut, updates::decoder::Decode, Doc, GetString, ReadTxn, Text, Transact,
    WriteTxn, Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim,
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
// Position helpers
// ---------------------------------------------------------------------------

/// PM 位置 `[from, to]` に重なる全ブロックの yrs テキスト座標を返す。
///
/// 戻り値: `Vec<(block_idx, text_char_start, text_char_len)>`
///
/// # Limitations
///
/// TODO: UTF-16 非対応 — サロゲートペア文字は char count と UTF-16 code unit 数が異なる。
///       BMP 外文字を含むドキュメントでは mark 範囲がずれる可能性がある。
pub fn block_ranges(doc: &Arc<Node>, from: usize, to: usize) -> Vec<(u32, u32, u32)> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    for (idx, block) in doc.content.children.iter().enumerate() {
        let block_size = block.node_size();
        let text_start = offset + 1;
        let text_end = offset + block_size - 1;
        let range_from = from.max(text_start);
        let range_to = to.min(text_end);
        if range_from < range_to {
            let char_start = (range_from - text_start) as u32;
            let char_len = (range_to - range_from) as u32;
            result.push((idx as u32, char_start, char_len));
        }
        offset += block_size;
    }
    result
}

/// PM 位置がブロック境界（開きトークン直前）である場合に yrs XmlFragment の
/// 挿入インデックスを返す。
fn block_index_from_boundary(doc: &Arc<Node>, pm_pos: usize) -> Option<u32> {
    let mut offset = 0usize;
    for (idx, block) in doc.content.children.iter().enumerate() {
        if offset == pm_pos {
            return Some(idx as u32);
        }
        offset += block.node_size();
    }
    if offset == pm_pos {
        return Some(doc.content.children.len() as u32);
    }
    None
}

/// yrs XmlText::get_string() が返す XML マークアップ文字列を
/// `(text, mark_names)` セグメントのリストに分解する。
///
/// 例: `"h<bold>ell</bold>o"` → `[("h", []), ("ell", ["bold"]), ("o", [])]`
///
/// # Limitations
///
/// TODO: タグ属性は無視する (e.g. `<link href="...">`)。link mark の href などは
///       失われる。属性付き mark に対応するには属性をパースする必要がある。
pub fn parse_yrs_xml_segments(s: &str) -> Vec<(String, Vec<String>)> {
    let mut result: Vec<(String, Vec<String>)> = Vec::new();
    let mut mark_stack: Vec<String> = Vec::new();
    let mut pending_text = String::new();
    let mut remaining = s;

    while !remaining.is_empty() {
        if let Some(tag_start) = remaining.find('<') {
            if tag_start > 0 {
                pending_text.push_str(&remaining[..tag_start]);
            }
            remaining = &remaining[tag_start..];

            if remaining.starts_with("</") {
                if !pending_text.is_empty() {
                    result.push((pending_text.clone(), mark_stack.clone()));
                    pending_text.clear();
                }
                if let Some(end) = remaining.find('>') {
                    let tag_name = remaining[2..end].to_string();
                    if mark_stack.last() == Some(&tag_name) {
                        mark_stack.pop();
                    }
                    remaining = &remaining[end + 1..];
                } else {
                    break;
                }
            } else {
                if !pending_text.is_empty() {
                    result.push((pending_text.clone(), mark_stack.clone()));
                    pending_text.clear();
                }
                if let Some(end) = remaining.find('>') {
                    // タグ名のみ取得 (属性は無視)
                    let tag_content = &remaining[1..end];
                    let tag_name = tag_content
                        .split_whitespace()
                        .next()
                        .unwrap_or(tag_content)
                        .to_string();
                    mark_stack.push(tag_name);
                    remaining = &remaining[end + 1..];
                } else {
                    break;
                }
            }
        } else {
            pending_text.push_str(remaining);
            remaining = "";
        }
    }

    if !pending_text.is_empty() {
        result.push((pending_text, mark_stack));
    }

    result
}

/// XML マークアップを除いたプレーンテキストを返す。
pub fn strip_xml_tags(s: &str) -> String {
    parse_yrs_xml_segments(s)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

// ---------------------------------------------------------------------------
// Reconstruct PM doc from yrs XmlFragment (reverse sync)
// ---------------------------------------------------------------------------

/// Read the yrs XmlFragment "content" and rebuild an `Arc<Node>` PM document.
///
/// Supports `paragraph`, `heading` (with `level` attribute), and `code_block`.
/// Mark information is reconstructed from the XML markup returned by
/// `XmlText::get_string()` (e.g. `"h<bold>ell</bold>o"`).
///
/// # Limitations
///
/// TODO: nested block reconstruction (blockquote, list_item).
/// TODO: link mark attrs (href, title) are not reconstructed — parse_yrs_xml_segments
///       ignores tag attributes.
pub fn build_pm_doc_from_yrs<T: ReadTxn>(
    content: &yrs::XmlFragmentRef,
    txn: &T,
    schema: &Arc<Schema>,
) -> Arc<Node> {
    let doc_type = schema.node_type_by_name("doc").unwrap();
    let para_type = schema.node_type_by_name("paragraph").unwrap();
    let text_type = schema.node_type_by_name("text").unwrap();

    let mut blocks: Vec<Arc<Node>> = Vec::new();

    let len = content.len(txn);
    for i in 0..len {
        let Some(child) = content.get(txn, i) else {
            continue;
        };
        let XmlOut::Element(elem) = child else {
            continue;
        };

        let tag = elem.tag();
        let tag_str = tag.as_ref();

        // ブロックの PM ノード型を tag 名から解決
        let block_type = match schema.node_type_by_name(tag_str) {
            Some(nt) => nt,
            None => continue, // 未知の要素はスキップ
        };

        // heading の level 属性を復元
        let block_attrs = if tag_str == "heading" {
            if let Some(level_str) = elem.get_attribute(txn, "level") {
                let level: i64 = level_str.parse().unwrap_or(1);
                Attrs::empty().with("level", AttrValue::Int(level))
            } else {
                block_type.default_attrs()
            }
        } else {
            Attrs::empty()
        };

        // XmlText の内容から mark 付きテキストノードを生成
        let mut text_nodes: Vec<Arc<Node>> = Vec::new();
        let elem_len = elem.len(txn);
        for j in 0..elem_len {
            if let Some(XmlOut::Text(xml_text)) = elem.get(txn, j) {
                let xml_str = xml_text.get_string(txn);
                let segments = parse_yrs_xml_segments(&xml_str);
                for (text, mark_names) in segments {
                    if text.is_empty() {
                        continue;
                    }
                    let marks_vec: Vec<Mark> = mark_names
                        .iter()
                        .filter_map(|name| schema.mark_type_by_name(name))
                        .map(|mt| Mark::simple(mt.id))
                        .collect();
                    let mark_set = MarkSet::from_marks(marks_vec);
                    text_nodes.push(Arc::new(Node::text(text_type.id, text.as_str(), mark_set)));
                }
            }
        }

        let block_fragment = if text_nodes.is_empty() {
            Fragment::empty()
        } else {
            Fragment::from_nodes(text_nodes)
        };

        blocks.push(Arc::new(Node::new(
            block_type.id,
            block_attrs,
            block_fragment,
            MarkSet::empty(),
        )));
    }

    // 少なくとも 1 つの段落を保証する
    if blocks.is_empty() {
        blocks.push(Arc::new(Node::new(
            para_type.id,
            Attrs::empty(),
            Fragment::empty(),
            MarkSet::empty(),
        )));
    }

    Arc::new(Node::new(
        doc_type.id,
        Attrs::empty(),
        Fragment::from_nodes(blocks),
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
    /// Create a host `CollabState` with a single empty paragraph.
    ///
    /// Only one peer should call this. After editing, distribute the initial
    /// state to other peers via `encode_state_as_update` before they start
    /// editing, then have them call `join_guest`.
    pub fn create_host(client_id: u64) -> Self {
        let schema = basic_schema();
        let editor = EditorState::with_empty_doc(schema);
        let ydoc = Doc::with_client_id(client_id);
        {
            let mut txn = ydoc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        }
        CollabState { editor, ydoc }
    }

    /// Join as a guest peer, bootstrapping state from the host's update bytes.
    ///
    /// `initial_update` must be produced by `encode_state_as_update` on the
    /// host (or any peer that already has the canonical initial document).
    pub fn join_guest(client_id: u64, initial_update: &[u8]) -> Self {
        let schema = basic_schema();
        let ydoc = Doc::with_client_id(client_id);
        {
            let mut txn = ydoc.transact_mut();
            txn.apply_update(
                yrs::Update::decode_v1(initial_update).expect("join_guest: decode_v1 failed"),
            )
            .expect("join_guest: apply_update failed");
        }
        let new_doc = {
            let txn = ydoc.transact();
            let content = txn
                .get_xml_fragment("content")
                .expect("'content' fragment must exist after applying host update");
            build_pm_doc_from_yrs(&content, &txn, &schema)
        };
        let editor = EditorState::new(schema, new_doc, Selection::cursor(1));
        CollabState { editor, ydoc }
    }

    /// Apply a PM transaction to the editor state, and propagate text-only
    /// steps to the yrs document.
    ///
    /// Returns `true` if the transaction was applied successfully.
    pub fn apply_transaction(&mut self, tr: wysiwyg_core::state::Transaction) -> bool {
        match self.editor.apply(&tr) {
            Ok(new_state) => {
                self.editor = new_state;
                // Advance `cur` through each step so that position resolution
                // uses the correct pre-step document even in multi-step transactions
                // (e.g. split_block emits 3 sequential ReplaceSteps).
                let mut cur = tr.doc_before().clone();
                for step in tr.steps() {
                    match step {
                        Step::Replace(rs) => self.sync_replace_step_to_yrs(rs, &cur),
                        Step::AddMark(s) => self.sync_add_mark_step_to_yrs(s, &cur),
                        Step::RemoveMark(s) => self.sync_remove_mark_step_to_yrs(s, &cur),
                        // TODO: sync ReplaceAroundStep (wrap/lift operations)
                        Step::ReplaceAround(_) => {}
                    }
                    if let Ok((next, _)) = step.apply(&cur) {
                        cur = next;
                    }
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
    /// 既存の selection を新ドキュメントのサイズに clamp する。
    ///
    /// # Limitations
    ///
    /// TODO: selection を update の Mapping で厳密にリマップするのではなく、
    ///       単純に clamp しているため、ユーザーのカーソル意図が失われる場合がある。
    pub fn rebuild_pm_from_yrs(&mut self) {
        let txn = self.ydoc.transact();
        let content = txn
            .get_xml_fragment("content")
            .expect("'content' fragment must exist");
        let new_doc = build_pm_doc_from_yrs(&content, &txn, &self.editor.schema);
        drop(txn);
        let clamped_selection = self.editor.selection.clone().clamped(&new_doc);
        self.editor = EditorState::new(self.editor.schema.clone(), new_doc, clamped_selection);
    }

    /// Encode the yrs state as a v1 update that can be sent to a remote peer.
    pub fn encode_state_as_update(&self) -> Vec<u8> {
        let txn = self.ydoc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// 自ドキュメントの state vector を返す。
    ///
    /// 差分同期のため、対向 peer に送って「あなたから見て私の知らない op を送って」と
    /// 要求する用途に使う。ネットワーク転送する場合は `yrs::Encode::encode_v1()` で
    /// バイト列に直列化する。
    pub fn state_vector(&self) -> yrs::StateVector {
        self.ydoc.transact().state_vector()
    }

    /// `remote_sv` から見て自分が新たに持っている op のみを v1 update として encode する。
    ///
    /// 全量 `encode_state_as_update` と比較して、共有済みの op を除いた差分のみを返すため
    /// 転送量が小さくなる。戻り値は `apply_remote_update` で受け取れる。
    pub fn encode_diff(&self, remote_sv: &yrs::StateVector) -> Vec<u8> {
        self.ydoc.transact().encode_diff_v1(remote_sv)
    }

    // -----------------------------------------------------------------------
    // Internal: forward sync
    // -----------------------------------------------------------------------

    fn sync_replace_step_to_yrs(&self, rs: &ReplaceStep, doc_before: &Arc<Node>) {
        // --- ケース 1: open slice (ブロック結合 = Backspace at block start) ---
        if rs.slice.open_start == 1 && rs.slice.open_end == 1 && rs.slice.content.is_empty() {
            self.sync_block_merge_to_yrs(rs, doc_before);
            return;
        }

        if rs.slice.open_start != 0 || rs.slice.open_end != 0 {
            // depth > 1 の open slice は未対応
            return;
        }

        // --- ケース 2: 新規ブロック挿入 (split_block の step c) ---
        // from == to かつブロック境界で、slice が 1 つの非リーフノードを含む
        if rs.from == rs.to && resolve_text_pos(doc_before, rs.from).is_none() {
            if let Some(new_block) = rs.slice.content.child(0) {
                if !new_block.is_leaf() {
                    self.sync_block_insert_to_yrs(rs.from, new_block, doc_before);
                    return;
                }
            }
        }

        // --- ケース 3: ブロックタイプ変更 (set_block_type) ---
        // from != to、両端がブロック境界、slice が 1 つの非リーフノード
        if rs.from != rs.to
            && resolve_text_pos(doc_before, rs.from).is_none()
            && resolve_text_pos(doc_before, rs.to).is_none()
        {
            if let Some(new_block) = rs.slice.content.child(0) {
                if !new_block.is_leaf() {
                    self.sync_block_type_change_to_yrs(rs.from, new_block, doc_before);
                    return;
                }
            }
        }

        // --- ケース 4: テキスト挿入/削除 (既存ロジック) ---
        let del_from = resolve_text_pos(doc_before, rs.from);
        let del_to = resolve_text_pos(doc_before, rs.to);
        let insert_text: Option<String> = extract_flat_text_from_slice(&rs.slice);

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        if rs.from < rs.to {
            if let (Some((bi, char_from)), Some((bi_to, char_to))) = (del_from, del_to) {
                if bi == bi_to {
                    if let Some(XmlOut::Element(para)) = content.get(&txn, bi) {
                        if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                            let delete_len = char_to - char_from;
                            xml_text.remove_range(&mut txn, char_from, delete_len);
                        }
                    }
                }
                // TODO: cross-paragraph deletions (bi != bi_to)
            }
        }

        if let Some(text) = insert_text {
            if let Some((bi, char_offset)) = resolve_text_pos(doc_before, rs.from) {
                if let Some(XmlOut::Element(para)) = content.get(&txn, bi) {
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

    /// open-slice ブロック結合を yrs に同期する。
    /// ブロック B のプレーンテキストをブロック A 末尾に追記し、ブロック B を削除する。
    ///
    /// # Limitations
    ///
    /// TODO: ブロック B のマーク情報は失われる。プレーンテキストのみ移動する。
    fn sync_block_merge_to_yrs(&self, rs: &ReplaceStep, doc_before: &Arc<Node>) {
        let (bi_a, _) = match resolve_text_pos(doc_before, rs.from) {
            Some(v) => v,
            None => return,
        };
        let (bi_b, _) = match resolve_text_pos(doc_before, rs.to) {
            Some(v) => v,
            None => return,
        };
        if bi_b != bi_a + 1 {
            return;
        }

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        // ブロック A の XmlText 末尾位置を取得
        let len_a: u32 = if let Some(XmlOut::Element(para_a)) = content.get(&txn, bi_a) {
            if let Some(XmlOut::Text(xml_text_a)) = para_a.get(&txn, 0) {
                xml_text_a.len(&txn)
            } else {
                0
            }
        } else {
            return;
        };

        // ブロック B のプレーンテキストを取得 (mark なし)
        let b_text: String = if let Some(XmlOut::Element(para_b)) = content.get(&txn, bi_b) {
            if let Some(XmlOut::Text(xml_text_b)) = para_b.get(&txn, 0) {
                strip_xml_tags(&xml_text_b.get_string(&txn))
            } else {
                String::new()
            }
        } else {
            return;
        };

        // ブロック A 末尾にブロック B のテキストを追記
        if !b_text.is_empty() {
            if let Some(XmlOut::Element(para_a)) = content.get(&txn, bi_a) {
                if para_a.len(&txn) == 0 {
                    para_a.insert(&mut txn, 0, XmlTextPrelim::new(""));
                }
                if let Some(XmlOut::Text(xml_text_a)) = para_a.get(&txn, 0) {
                    xml_text_a.insert(&mut txn, len_a, &b_text);
                }
            }
        }

        // ブロック B を削除
        content.remove_range(&mut txn, bi_b, 1);
    }

    /// 新しいブロックを yrs XmlFragment に挿入する。
    fn sync_block_insert_to_yrs(
        &self,
        pm_pos: usize,
        new_block: &Arc<wysiwyg_core::model::node::Node>,
        doc_before: &Arc<Node>,
    ) {
        let insert_idx = match block_index_from_boundary(doc_before, pm_pos) {
            Some(i) => i,
            None => return,
        };

        let type_name = self.editor.schema.node_type(new_block.type_id).name.clone();
        let text = extract_flat_text_from_block(new_block);

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        let new_elem = content.insert(
            &mut txn,
            insert_idx,
            XmlElementPrelim::empty(type_name.as_ref()),
        );
        // heading の level 属性を設定
        if type_name.as_ref() == "heading" {
            if let Some(level) = new_block.attrs.get("level") {
                new_elem.insert_attribute(&mut txn, "level", attr_value_to_string(level));
            }
        }
        // XmlText を追加してテキスト内容を設定
        let xml_text_prelim = XmlTextPrelim::new(text.as_str());
        new_elem.insert(&mut txn, 0, xml_text_prelim);
    }

    /// ブロックタイプ変更を yrs に同期する (heading ↔ paragraph ↔ code_block)。
    /// 旧 XmlElement の後に新タグの XmlElement を挿入し、テキストをコピーして旧を削除する。
    fn sync_block_type_change_to_yrs(
        &self,
        pm_pos: usize,
        new_block: &Arc<wysiwyg_core::model::node::Node>,
        doc_before: &Arc<Node>,
    ) {
        let block_idx = match block_index_from_boundary(doc_before, pm_pos) {
            Some(i) => i,
            None => return,
        };

        let new_type_name = self.editor.schema.node_type(new_block.type_id).name.clone();

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        // 旧ブロックのプレーンテキストを取得
        let old_text: String = if let Some(XmlOut::Element(old_elem)) = content.get(&txn, block_idx)
        {
            if let Some(XmlOut::Text(xml_text)) = old_elem.get(&txn, 0) {
                strip_xml_tags(&xml_text.get_string(&txn))
            } else {
                String::new()
            }
        } else {
            return;
        };

        // 新 XmlElement を旧の位置に挿入
        let new_elem = content.insert(
            &mut txn,
            block_idx,
            XmlElementPrelim::empty(new_type_name.as_ref()),
        );

        // heading level 属性を設定
        if new_type_name.as_ref() == "heading" {
            if let Some(level) = new_block.attrs.get("level") {
                new_elem.insert_attribute(&mut txn, "level", attr_value_to_string(level));
            }
        }

        // テキスト内容をコピー
        new_elem.insert(&mut txn, 0, XmlTextPrelim::new(old_text.as_str()));

        // 旧ブロック (インデックスが 1 ずれた位置) を削除
        content.remove_range(&mut txn, block_idx + 1, 1);
    }

    /// AddMarkStep を yrs に同期する。mark が適用される各ブロックの XmlText に
    /// `format()` でフォーマット属性を設定する。
    fn sync_add_mark_step_to_yrs(&self, step: &AddMarkStep, cur_doc: &Arc<Node>) {
        let mark_name = self.editor.schema.mark_type(step.mark.type_id).name.clone();
        let ranges = block_ranges(cur_doc, step.from, step.to);
        if ranges.is_empty() {
            return;
        }

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        for (block_idx, char_start, char_len) in ranges {
            if let Some(XmlOut::Element(para)) = content.get(&txn, block_idx) {
                if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                    let mut attrs: HashMap<Arc<str>, yrs::Any> = HashMap::new();
                    attrs.insert(mark_name.clone(), yrs::Any::Bool(true));
                    xml_text.format(&mut txn, char_start, char_len, attrs);
                }
            }
        }
    }

    /// RemoveMarkStep を yrs に同期する。mark が除去される各ブロックの XmlText に
    /// `format()` で `Any::Null` を設定して属性を削除する。
    fn sync_remove_mark_step_to_yrs(&self, step: &RemoveMarkStep, cur_doc: &Arc<Node>) {
        let mark_name = self.editor.schema.mark_type(step.mark.type_id).name.clone();
        let ranges = block_ranges(cur_doc, step.from, step.to);
        if ranges.is_empty() {
            return;
        }

        let mut txn = self.ydoc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");

        for (block_idx, char_start, char_len) in ranges {
            if let Some(XmlOut::Element(para)) = content.get(&txn, block_idx) {
                if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                    let mut attrs: HashMap<Arc<str>, yrs::Any> = HashMap::new();
                    attrs.insert(mark_name.clone(), yrs::Any::Null);
                    xml_text.format(&mut txn, char_start, char_len, attrs);
                }
            }
        }
    }
}

/// ブロックノードのフラットテキスト内容を返す (mark を無視)。
fn extract_flat_text_from_block(block: &Arc<Node>) -> String {
    let mut text = String::new();
    for child in block.content.children.iter() {
        if let Some(t) = &child.text {
            text.push_str(t.as_ref());
        }
    }
    text
}

/// `AttrValue` を yrs attribute 文字列に変換する。
fn attr_value_to_string(v: &AttrValue) -> String {
    match v {
        AttrValue::String(s) => s.to_string(),
        AttrValue::Int(i) => i.to_string(),
        AttrValue::Bool(b) => b.to_string(),
        AttrValue::Null => String::new(),
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
    use wysiwyg_core::commands::{insert_text, set_block_type, split_block, toggle_bold};

    fn collect_text(node: &Arc<Node>) -> String {
        if let Some(t) = &node.text {
            return t.to_string();
        }
        node.content.children.iter().map(collect_text).collect()
    }

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

        assert_eq!(resolve_text_pos(&doc, 0), None); // doc-level gap before para
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
            Arc::new(Node::new(
                para_type.id,
                Attrs::empty(),
                Fragment::from_node(t),
                MarkSet::empty(),
            ))
        };

        let doc = Arc::new(Node::new(
            doc_type.id,
            Attrs::empty(),
            Fragment::from_nodes(vec![make_para("ab"), make_para("cd")]),
            MarkSet::empty(),
        ));

        assert_eq!(resolve_text_pos(&doc, 0), None); // doc-level gap before para(ab)
        assert_eq!(resolve_text_pos(&doc, 1), Some((0, 0))); // before 'a'
        assert_eq!(resolve_text_pos(&doc, 2), Some((0, 1))); // before 'b'
        assert_eq!(resolve_text_pos(&doc, 3), Some((0, 2))); // after 'b' (end of para(ab))
        assert_eq!(resolve_text_pos(&doc, 4), None); // doc-level gap before para(cd)
        assert_eq!(resolve_text_pos(&doc, 5), Some((1, 0))); // before 'c'
        assert_eq!(resolve_text_pos(&doc, 6), Some((1, 1))); // before 'd'
        assert_eq!(resolve_text_pos(&doc, 7), Some((1, 2))); // after 'd' (end of para(cd))
    }

    #[test]
    fn two_peer_text_convergence() {
        // ホストが空ドキュメントを作成し、ゲストが initial state を受け取ってから
        // 両者が独立に編集し、最後に同期する。

        let mut host = CollabState::create_host(1);
        let initial = host.encode_state_as_update();
        let mut guest = CollabState::join_guest(2, &initial);

        // ホストが "hello" を挿入
        let tr_a = insert_text(&host.editor, "hello").unwrap();
        assert!(host.apply_transaction(tr_a));

        // ゲストが "world" を挿入
        let tr_b = insert_text(&guest.editor, "world").unwrap();
        assert!(guest.apply_transaction(tr_b));

        // 全量 update を交換
        let update_a = host.encode_state_as_update();
        let update_b = guest.encode_state_as_update();
        host.apply_remote_update(&update_b);
        guest.apply_remote_update(&update_a);

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

        let text_host = read_text(&host);
        let text_guest = read_text(&guest);

        assert_eq!(text_host, text_guest, "peers did not converge");
        assert!(
            text_host.contains("hello") && text_host.contains("world"),
            "merged text '{text_host}' is missing content"
        );

        // factory pattern では段落が 1 つだけ存在する（重複なし）
        let para_count = {
            let txn = host.ydoc.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            content.len(&txn)
        };
        assert_eq!(para_count, 1, "段落が重複している");
    }

    #[test]
    fn host_guest_initial_share() {
        // ホストが "Hello" を入力し、その後ゲストが join_guest で参加する。
        // ゲストの PM doc が正しく "Hello" を含んでいること、かつ段落が 1 つであることを確認。

        let mut host = CollabState::create_host(1);

        let tr = insert_text(&host.editor, "Hello").unwrap();
        assert!(host.apply_transaction(tr));

        let update = host.encode_state_as_update();
        let guest = CollabState::join_guest(2, &update);

        assert_eq!(collect_text(&guest.editor.doc), "Hello");
        assert_eq!(guest.editor.doc.child_count(), 1, "段落が 1 つであるべき");
    }

    #[test]
    fn two_peer_block_split_convergence() {
        // ホストが "hello" を挿入し、ゲストが参加した後にホストが split_block する。
        // 両 peer の yrs 側段落数が 2 で一致することを確認する。

        let mut host = CollabState::create_host(1);
        let tr = insert_text(&host.editor, "hello").unwrap();
        assert!(host.apply_transaction(tr));

        let initial = host.encode_state_as_update();
        let mut guest = CollabState::join_guest(2, &initial);

        // カーソルを pos=3 ("he" の後) に置いてブロックを分割
        let split_state = EditorState::new(
            host.editor.schema.clone(),
            host.editor.doc.clone(),
            Selection::cursor(3),
        );
        let tr = split_block(&split_state).unwrap();
        assert!(host.apply_transaction(tr));

        // ホストの update をゲストへ送信
        let update = host.encode_state_as_update();
        guest.apply_remote_update(&update);

        let para_count = |state: &CollabState| {
            let txn = state.ydoc.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            content.len(&txn)
        };

        assert_eq!(para_count(&host), 2, "ホストの段落数が 2 であるべき");
        assert_eq!(para_count(&guest), 2, "ゲストの段落数が 2 であるべき");
    }

    #[test]
    fn two_peer_mark_convergence() {
        // ホストが "hello" に bold を適用し、ゲストへ同期する。
        // ゲストの PM doc に bold mark が反映されることを確認する。

        let mut host = CollabState::create_host(1);
        let tr = insert_text(&host.editor, "hello").unwrap();
        assert!(host.apply_transaction(tr));

        let initial = host.encode_state_as_update();
        let mut guest = CollabState::join_guest(2, &initial);

        // テキスト全体 (pos 1–6) を選択して bold を toggle
        let bold_state = EditorState::new(
            host.editor.schema.clone(),
            host.editor.doc.clone(),
            Selection::text(1, 6),
        );
        let tr = toggle_bold(&bold_state).unwrap();
        assert!(host.apply_transaction(tr));

        let update = host.encode_state_as_update();
        guest.apply_remote_update(&update);

        let bold_id = guest.editor.schema.mark_type_by_name("bold").unwrap().id;
        let para = guest.editor.doc.child(0).unwrap();
        let has_bold = para
            .content
            .children
            .iter()
            .any(|n| n.marks.contains(bold_id));
        assert!(has_bold, "ゲストの doc に bold mark があるべき");
    }

    #[test]
    fn two_peer_block_type_convergence() {
        // ホストが段落を heading level=2 に変更し、ゲストへ同期する。
        // ゲストの PM doc でブロックが heading かつ level=2 であることを確認する。

        let mut host = CollabState::create_host(1);
        let tr = insert_text(&host.editor, "hello").unwrap();
        assert!(host.apply_transaction(tr));

        let initial = host.encode_state_as_update();
        let mut guest = CollabState::join_guest(2, &initial);

        // 段落を heading level=2 に変更
        let heading_state = EditorState::new(
            host.editor.schema.clone(),
            host.editor.doc.clone(),
            Selection::cursor(1),
        );
        let tr = set_block_type(
            &heading_state,
            "heading",
            Attrs::empty().with("level", AttrValue::Int(2)),
        )
        .unwrap();
        assert!(host.apply_transaction(tr));

        let update = host.encode_state_as_update();
        guest.apply_remote_update(&update);

        let heading_type = guest.editor.schema.node_type_by_name("heading").unwrap();
        let block = guest.editor.doc.child(0).unwrap();
        assert_eq!(
            block.type_id, heading_type.id,
            "ゲストの doc でブロックが heading であるべき"
        );
        assert_eq!(
            block.attrs.get("level"),
            Some(&AttrValue::Int(2)),
            "level が 2 であるべき"
        );
    }

    #[test]
    fn two_peer_incremental_sync() {
        // 差分同期の動作確認。
        // 1. host/guest を初期化し同期
        // 2. 各々が独立に編集
        // 3. state vector を交換 → 差分 update を計算して適用
        // 4. 両者が収束し、かつ差分 update の方が全量 update より小さい

        let mut host = CollabState::create_host(1);
        let tr = insert_text(&host.editor, "Hello").unwrap();
        assert!(host.apply_transaction(tr));

        let initial = host.encode_state_as_update();
        let mut guest = CollabState::join_guest(2, &initial);

        // 各々が独立に編集 (host は末尾に追記、guest も同様)
        let host_end = collect_text(&host.editor.doc).chars().count() + 1;
        let host_at_end = EditorState::new(
            host.editor.schema.clone(),
            host.editor.doc.clone(),
            Selection::cursor(host_end),
        );
        let tr = insert_text(&host_at_end, ", host").unwrap();
        assert!(host.apply_transaction(tr));

        let guest_end = collect_text(&guest.editor.doc).chars().count() + 1;
        let guest_at_end = EditorState::new(
            guest.editor.schema.clone(),
            guest.editor.doc.clone(),
            Selection::cursor(guest_end),
        );
        let tr = insert_text(&guest_at_end, " & guest").unwrap();
        assert!(guest.apply_transaction(tr));

        // 差分同期
        let sv_host = host.state_vector();
        let sv_guest = guest.state_vector();
        let diff_for_host = guest.encode_diff(&sv_host);
        let diff_for_guest = host.encode_diff(&sv_guest);

        host.apply_remote_update(&diff_for_host);
        guest.apply_remote_update(&diff_for_guest);

        // 収束確認
        let host_text = collect_text(&host.editor.doc);
        let guest_text = collect_text(&guest.editor.doc);
        assert_eq!(host_text, guest_text, "テキストが収束していない");
        assert!(host_text.contains("Hello"));
        assert!(host_text.contains("host"));
        assert!(host_text.contains("guest"));

        // 差分 update の方が全量 update より小さい (共有済み "Hello" 部が省かれる)
        let full_update = host.encode_state_as_update();
        assert!(
            diff_for_guest.len() < full_update.len(),
            "差分 update ({}) が全量 update ({}) 以上のサイズになっている",
            diff_for_guest.len(),
            full_update.len(),
        );
    }
}

#[cfg(test)]
mod api_probe {
    use yrs::{
        types::xml::XmlOut, Doc, GetString, ReadTxn, Text, Transact, WriteTxn, XmlElementPrelim,
        XmlFragment, XmlTextPrelim,
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

    /// yrs XmlText::format でマーク適用後もプレーンテキストが保持されることを確認する。
    /// PR2 の mark 同期実装の前提 API 検証。
    #[test]
    fn yrs_xmltext_format_roundtrip() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use yrs::Any;

        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            let para = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            para.insert(&mut txn, 0, XmlTextPrelim::new("hello"));
        }
        // "ell" (chars 1-3) に bold フォーマットを適用
        {
            let mut txn = doc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            if let Some(XmlOut::Element(para)) = content.get(&txn, 0) {
                if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                    let mut attrs: HashMap<Arc<str>, Any> = HashMap::new();
                    attrs.insert(Arc::from("bold"), Any::Bool(true));
                    xml_text.format(&mut txn, 1, 3, attrs);
                }
            }
        }
        // XmlText::get_string() は format 情報を XML マークアップとして埋め込んだ文字列を返す。
        // プレーンテキストの復元には diff/iter が必要 (PR2 の mark 再構成で実装予定)。
        {
            let txn = doc.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            if let Some(XmlOut::Element(para)) = content.get(&txn, 0) {
                if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                    let s = xml_text.get_string(&txn);
                    // format 後: bold タグが埋め込まれる
                    assert!(s.contains("bold"), "bold マークアップが含まれるはず: {s}");
                    // テキスト内容はすべて含まれる
                    assert!(
                        s.contains('h') && s.contains("ell") && s.contains('o'),
                        "テキスト内容が欠落している: {s}"
                    );
                } else {
                    panic!("XmlText ノードが見つからない");
                }
            } else {
                panic!("paragraph が見つからない");
            }
        }
    }

    /// format() 後に段落が子 1 つ (XmlText) のままで、get_string() が XML markup を返すことを確認。
    #[test]
    fn yrs_xmltext_children_after_format() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use yrs::{
            types::xml::XmlOut, Any, Doc, ReadTxn, Transact, WriteTxn, XmlElementPrelim,
            XmlFragment, XmlTextPrelim,
        };

        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            let para = content.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            para.insert(&mut txn, 0, XmlTextPrelim::new("hello"));
        }
        {
            let mut txn = doc.transact_mut();
            let content = txn.get_or_insert_xml_fragment("content");
            if let Some(XmlOut::Element(para)) = content.get(&txn, 0) {
                if let Some(XmlOut::Text(xml_text)) = para.get(&txn, 0) {
                    let mut attrs: HashMap<Arc<str>, Any> = HashMap::new();
                    attrs.insert(Arc::from("bold"), Any::Bool(true));
                    xml_text.format(&mut txn, 1, 3, attrs);
                }
            }
        }
        {
            let txn = doc.transact();
            let content = txn.get_xml_fragment("content").unwrap();
            if let Some(XmlOut::Element(para)) = content.get(&txn, 0) {
                let child_count = para.len(&txn);
                // format() が inline element を生成するなら child_count > 1
                // XmlText 内部に埋め込むなら child_count == 1
                assert!(child_count >= 1);
                // 子ノードを走査してテキスト内容を収集する
                let mut collected = String::new();
                let mut inline_elem_count = 0u32;
                for i in 0..child_count {
                    match para.get(&txn, i) {
                        Some(XmlOut::Text(t)) => collected.push_str(&t.get_string(&txn)),
                        Some(XmlOut::Element(e)) => {
                            inline_elem_count += 1;
                            for j in 0..e.len(&txn) {
                                if let Some(XmlOut::Text(t2)) = e.get(&txn, j) {
                                    collected.push_str(&t2.get_string(&txn));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                // format() は inline element を生成しない — child_count == 1 のまま
                assert_eq!(child_count, 1, "段落の子が 1 つであるべき");
                assert_eq!(inline_elem_count, 0, "inline element は生成されない");
                // get_string() は XML markup を含む文字列を返す
                assert!(
                    collected.contains("bold"),
                    "bold マークアップが含まれるはず: {collected}"
                );
            }
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
        let update1 = doc1
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());
        let update2 = doc2
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());

        // Apply each to the other
        doc1.transact_mut()
            .apply_update(Update::decode_v1(&update2).unwrap())
            .unwrap();
        doc2.transact_mut()
            .apply_update(Update::decode_v1(&update1).unwrap())
            .unwrap();

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
