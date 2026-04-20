//! Text-input commands.

use std::sync::Arc;

use crate::{
    model::{
        attrs::Attrs,
        mark::MarkSet,
        node::{Fragment, Node},
        resolve::ResolvedPos,
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

/// 選択範囲を削除する。カーソル (選択なし) の場合は `None` を返す。
pub fn delete_selection(state: &EditorState) -> Option<Transaction> {
    if state.selection.is_cursor() {
        return None;
    }

    let from = state.selection.from();
    let to = state.selection.to(&state.doc);

    let step = Step::Replace(ReplaceStep::new(from, to, Slice::empty()));
    let mut tr = state.transaction();
    tr.step(step).ok()?;
    tr.set_selection(Selection::cursor(from));
    Some(tr)
}

/// カーソル左の 1 文字を削除する。選択がある場合は選択範囲を削除する。
///
/// # Limitations
/// - カーソルがブロック先頭にある場合は前ブロックと結合する (深さ 1 のみ)
/// - depth > 1 のネストブロックは未対応
pub fn backspace(state: &EditorState) -> Option<Transaction> {
    if !state.selection.is_cursor() {
        return delete_selection(state);
    }

    let pos = state.selection.from();
    if pos == 0 {
        return None;
    }

    let resolved = ResolvedPos::resolve(&state.doc, pos)?;

    let (from, to, slice, new_pos) = if resolved.parent_offset == 0 && resolved.depth >= 1 {
        // ブロック先頭 — 前ブロックと結合する
        if pos < 2 {
            // 最初のブロック先頭なので結合できない
            return None;
        }
        (pos - 2, pos, Slice::new(Fragment::empty(), 1, 1), pos - 2)
    } else {
        // テキスト中 — 1 文字削除
        (pos - 1, pos, Slice::empty(), pos - 1)
    };

    let step = Step::Replace(ReplaceStep::new(from, to, slice));
    let mut tr = state.transaction();
    tr.step(step).ok()?;
    tr.set_selection(Selection::cursor(new_pos));
    Some(tr)
}

/// カーソル位置でブロックを分割し、新しい段落を作成する。
///
/// - 選択がある場合は先に選択範囲を削除してから分割する
/// - 常に paragraph ノードで分割する
///
/// # Limitations
/// - TODO: 見出しブロックで Enter した場合も paragraph になる — 元のブロック種別を維持する実装が必要
pub fn split_block(state: &EditorState) -> Option<Transaction> {
    let para_type = state.schema.node_type_by_name("paragraph")?;
    let split_pos = state.selection.from();
    let to = state.selection.to(&state.doc);

    let mut tr = state.transaction();

    // 選択がある場合は先に削除
    if split_pos < to {
        tr.step(Step::Replace(ReplaceStep::new(
            split_pos,
            to,
            Slice::empty(),
        )))
        .ok()?;
    }

    // tr.doc() は最新の doc を返す
    let cur_doc = tr.doc().clone();

    // split_pos を含むブロックを探す
    let mut block_start = 0usize;
    let mut found = false;
    for block in cur_doc.content.children.iter() {
        let block_size = block.node_size();
        let inner_start = block_start + 1;
        let inner_end = inner_start + block.content.size;

        if split_pos >= inner_start && split_pos <= inner_end {
            let inner_from = split_pos - inner_start;
            // ブロックの右側コンテンツを抽出
            let right_content = block.content.cut(inner_from, block.content.size);

            // 右側コンテンツをブロックから削除
            if split_pos < inner_end {
                tr.step(Step::Replace(ReplaceStep::new(
                    split_pos,
                    inner_end,
                    Slice::empty(),
                )))
                .ok()?;
            }

            // ブロックの close トークンの直後の位置を計算
            // 削除後: block の content.size = inner_from、node_size = inner_from + 2
            let new_block_end = block_start + inner_from + 2;

            // 新しい段落を挿入
            let new_para = Arc::new(Node::new(
                para_type.id,
                Attrs::empty(),
                right_content,
                MarkSet::empty(),
            ));
            tr.step(Step::Replace(ReplaceStep::new(
                new_block_end,
                new_block_end,
                Slice::new(Fragment::from_node(new_para), 0, 0),
            )))
            .ok()?;

            // カーソルを新しい段落の先頭へ (close + open = 2 トークン分前進)
            tr.set_selection(Selection::cursor(split_pos + 2));
            found = true;
            break;
        }

        block_start += block_size;
    }

    if !found {
        return None;
    }

    Some(tr)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            attrs::Attrs,
            mark::MarkSet,
            node::{Fragment, Node},
            schema::basic_schema,
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
        assert!(first_text
            .text
            .as_deref()
            .unwrap_or("")
            .starts_with("hello"));

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

    fn collect_text(node: &crate::model::node::Node) -> String {
        if let Some(t) = &node.text {
            return t.to_string();
        }
        node.content
            .children
            .iter()
            .map(|c| collect_text(c))
            .collect()
    }

    fn state_with_two_paragraphs(text1: &str, text2: &str) -> EditorState {
        let schema = basic_schema();
        let text_type = schema.node_type_by_name("text").unwrap();
        let para_type = schema.node_type_by_name("paragraph").unwrap();
        let doc_type = schema.node_type_by_name("doc").unwrap();

        let mk_para = |t: &str| {
            let tn = Arc::new(Node::text(text_type.id, t, MarkSet::empty()));
            Arc::new(Node::new(
                para_type.id,
                Attrs::empty(),
                Fragment::from_node(tn),
                MarkSet::empty(),
            ))
        };

        let doc = Arc::new(Node::new(
            doc_type.id,
            Attrs::empty(),
            Fragment::from_nodes(vec![mk_para(text1), mk_para(text2)]),
            MarkSet::empty(),
        ));
        EditorState::new(schema, doc, Selection::cursor(1))
    }

    // ---- delete_selection ----

    #[test]
    fn delete_selection_non_empty() {
        let state = state_with_paragraph("hello");
        // pos 2-4 を選択 = doc 内の 'e','l' (inner offset 1-3)
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let sel_state = EditorState::new(schema, doc, Selection::text(2, 4));
        let tr = delete_selection(&sel_state).unwrap();
        let new_state = sel_state.apply(&tr).unwrap();

        let para = new_state.doc.child(0).unwrap();
        assert_eq!(collect_text(para), "hlo");
        assert_eq!(new_state.selection.from(), 2);
    }

    #[test]
    fn delete_selection_cursor_returns_none() {
        let state = state_with_paragraph("hello");
        assert!(delete_selection(&state).is_none());
    }

    // ---- backspace ----

    #[test]
    fn backspace_deletes_one_char() {
        let state = state_with_paragraph("hello");
        // pos=4 (first 'l' の後) でバックスペース → 'l' (inner pos 2) が消える
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let s = EditorState::new(schema, doc, Selection::cursor(4));
        let tr = backspace(&s).unwrap();
        let new_state = s.apply(&tr).unwrap();

        let para = new_state.doc.child(0).unwrap();
        assert_eq!(collect_text(para), "helo");
        assert_eq!(new_state.selection.from(), 3);
    }

    #[test]
    fn backspace_at_block_start_joins() {
        // "foo" (size=5) + "bar" (size=5). p2 先頭は pos=6
        let state = state_with_two_paragraphs("foo", "bar");
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let s = EditorState::new(schema, doc, Selection::cursor(6));
        let tr = backspace(&s).unwrap();
        let new_state = s.apply(&tr).unwrap();

        // 段落が 1 つに結合されるはず
        assert_eq!(new_state.doc.content.children.len(), 1);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(collect_text(para), "foobar");
        // カーソルは結合点 = "foo" の直後 = pos 4
        assert_eq!(new_state.selection.from(), 4);
    }

    #[test]
    fn backspace_at_first_block_start_returns_none() {
        let state = state_with_paragraph("hello");
        // pos=1 は最初のブロック先頭
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let s = EditorState::new(schema, doc, Selection::cursor(1));
        assert!(backspace(&s).is_none());
    }

    // ---- split_block ----

    #[test]
    fn split_block_creates_new_paragraph() {
        let state = state_with_paragraph("hello");
        // pos=3 (between 'he' and 'llo')
        let schema = state.schema.clone();
        let doc = state.doc.clone();
        let s = EditorState::new(schema, doc, Selection::cursor(3));
        let tr = split_block(&s).unwrap();
        let new_state = s.apply(&tr).unwrap();

        assert_eq!(new_state.doc.content.children.len(), 2);
        let p1 = new_state.doc.child(0).unwrap();
        let p2 = new_state.doc.child(1).unwrap();
        assert_eq!(collect_text(p1), "he");
        assert_eq!(collect_text(p2), "llo");
        // カーソルは p2 先頭 = 3 + 2 = 5
        assert_eq!(new_state.selection.from(), 5);
    }
}
