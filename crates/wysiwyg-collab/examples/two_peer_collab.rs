//! 2 peer による協調編集のデモ。
//!
//! `CollabState::create_host` でホストを起動し、
//! `CollabState::join_guest` でゲストが参加するフローを示す。
//!
//! シナリオ:
//!   1. ホストがテキストを入力し、段落を heading level=2 に変換する。
//!   2. ゲストが参加し、ホストの初期状態を受け取る。
//!   3. ゲストがテキストを追記し、ホストが bold mark を適用する (並行編集)。
//!   4. 全量 state update を交換して収束を確認する。
//!
//! 実行:
//!   cargo run --example two_peer_collab -p wysiwyg-collab
//!
//! 注意: 現在の同期は全量 state update を交換している。
//!       実運用では差分同期 (encode_diff_v1 + StateVector) への置き換えを推奨する。
//! TODO: CollabState に state_vector / encode_diff API を追加し、
//!       two_peer_incremental example として差分同期を実演する。

use std::sync::Arc;

use wysiwyg_collab::CollabState;
use wysiwyg_core::{
    commands::{insert_text, set_block_type, toggle_bold},
    model::{
        attrs::{AttrValue, Attrs},
        node::Node,
    },
    state::{EditorState, Selection},
};
use yrs::{ReadTxn, Transact, XmlFragment};

fn main() {
    // -----------------------------------------------------------------------
    // 1. ホスト起動とドキュメント構築
    //    ホストが "Hello, " を入力後、段落を heading level=2 に変換する。
    //    ゲストが参加する前にブロックタイプを確定させることで、
    //    並行編集による CRDT 競合を避ける。
    // -----------------------------------------------------------------------
    println!("=== 1. ホスト起動とドキュメント構築 ===");
    let mut host = CollabState::create_host(1);

    let tr = insert_text(&host.editor, "Hello, ").expect("挿入コマンドが None を返した");
    assert!(host.apply_transaction(tr), "トランザクションの適用に失敗");

    // 段落を heading level=2 に変換
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
    .expect("set_block_type が None を返した");
    assert!(host.apply_transaction(tr), "heading 変換に失敗");

    let heading_type = host.editor.schema.node_type_by_name("heading").unwrap();
    let block = host.editor.doc.child(0).unwrap();
    println!("テキスト: {:?}", collect_text(&host.editor.doc));
    println!(
        "block type: {} (level: {:?})",
        if block.type_id == heading_type.id {
            "heading"
        } else {
            "paragraph"
        },
        block.attrs.get("level"),
    );
    println!("段落数 (yrs): {}", yrs_para_count(&host));

    // -----------------------------------------------------------------------
    // 2. ゲスト参加
    //    ホストの確定済み状態 (heading + "Hello, ") を受け取る。
    //    TODO: 実運用では差分同期 (encode_diff_v1 + StateVector) を使うべき
    // -----------------------------------------------------------------------
    println!("\n=== 2. ゲスト参加 ===");
    let initial_update = host.encode_state_as_update();
    let mut guest = CollabState::join_guest(2, &initial_update);
    println!("Guest テキスト: {:?}", collect_text(&guest.editor.doc));
    println!("Guest 段落数 (yrs): {}", yrs_para_count(&guest));

    // -----------------------------------------------------------------------
    // 3. 並行編集
    //    ゲスト: "world!" をテキスト末尾に追記
    //    ホスト: "Hello, " に bold mark を適用
    //    両操作は異なる CRDT 属性に作用するため競合なしに収束する。
    // -----------------------------------------------------------------------
    println!("\n=== 3. 並行編集 (Guest: テキスト追記 / Host: bold 適用) ===");

    // ゲスト — テキスト末尾に "world!" を追記
    // join_guest はカーソルを pos=1 に設定するため、末尾位置を計算して移動する。
    let guest_text_len = collect_text(&guest.editor.doc).chars().count();
    let end_pos = guest_text_len + 1; // para/heading 開きトークン(1) + テキスト長
    let guest_at_end = EditorState::new(
        guest.editor.schema.clone(),
        guest.editor.doc.clone(),
        Selection::cursor(end_pos),
    );
    let tr = insert_text(&guest_at_end, "world!").expect("挿入コマンドが None を返した");
    assert!(
        guest.apply_transaction(tr),
        "Guest のトランザクション適用に失敗"
    );
    println!("Guest テキスト: {:?}", collect_text(&guest.editor.doc));

    // ホスト — "Hello, " (pos 1〜8) を選択して bold を適用
    let bold_state = EditorState::new(
        host.editor.schema.clone(),
        host.editor.doc.clone(),
        Selection::text(1, 8),
    );
    let tr = toggle_bold(&bold_state).expect("toggle_bold が None を返した");
    assert!(host.apply_transaction(tr), "Host の bold 適用に失敗");

    let bold_id = host.editor.schema.mark_type_by_name("bold").unwrap().id;
    let block = host.editor.doc.child(0).unwrap();
    let host_has_bold = block
        .content
        .children
        .iter()
        .any(|n| n.marks.contains(bold_id));
    println!("Host テキスト: {:?}", collect_text(&host.editor.doc));
    println!("Host bold mark: {host_has_bold}");

    // -----------------------------------------------------------------------
    // 4. 相互同期と収束確認
    //    TODO: 実運用では encode_diff_v1 + StateVector で差分のみ交換すべき
    // -----------------------------------------------------------------------
    println!("\n=== 4. 相互同期 + 収束確認 ===");
    let update_from_host = host.encode_state_as_update();
    let update_from_guest = guest.encode_state_as_update();
    host.apply_remote_update(&update_from_guest);
    guest.apply_remote_update(&update_from_host);

    let host_text = collect_text(&host.editor.doc);
    let guest_text = collect_text(&guest.editor.doc);
    println!("Host  最終テキスト: {:?}", host_text);
    println!("Guest 最終テキスト: {:?}", guest_text);
    assert_eq!(host_text, guest_text, "テキストが収束していない");
    assert!(
        host_text.contains("Hello, ") && host_text.contains("world!"),
        "両者のテキストに期待するコンテンツが含まれていない: {host_text:?}",
    );

    let host_para = yrs_para_count(&host);
    let guest_para = yrs_para_count(&guest);
    println!("Host 段落数: {host_para}, Guest 段落数: {guest_para}");
    assert_eq!(host_para, guest_para, "段落数が収束していない");

    // heading type と level の確認
    let heading_type = guest.editor.schema.node_type_by_name("heading").unwrap();
    let block = guest.editor.doc.child(0).unwrap();
    let is_heading = block.type_id == heading_type.id;
    println!(
        "Guest block type: {} (level: {:?})",
        if is_heading { "heading" } else { "paragraph" },
        block.attrs.get("level"),
    );
    assert!(is_heading, "Guest のブロックが heading になっていない");
    assert_eq!(
        block.attrs.get("level"),
        Some(&AttrValue::Int(2)),
        "level が 2 になっていない",
    );

    // bold mark の確認 — "Hello, " 部分に bold が付いているはず
    let bold_id = guest.editor.schema.mark_type_by_name("bold").unwrap().id;
    let has_bold = block
        .content
        .children
        .iter()
        .any(|n| n.marks.contains(bold_id));
    println!("Guest bold mark: {has_bold}");
    assert!(has_bold, "Guest の doc に bold mark が反映されていない");

    println!("\n両 peer が同一状態に収束しました");
}

// ---------------------------------------------------------------------------
// ヘルパー
// ---------------------------------------------------------------------------

/// ドキュメントツリーを再帰的に辿ってテキストを結合する。
fn collect_text(node: &Arc<Node>) -> String {
    if let Some(t) = &node.text {
        return t.to_string();
    }
    node.content.children.iter().map(collect_text).collect()
}

/// yrs XmlFragment 内のブロック数を返す。
fn yrs_para_count(state: &CollabState) -> u32 {
    let txn = state.ydoc.transact();
    let content = txn
        .get_xml_fragment("content")
        .expect("'content' fragment が存在するべき");
    content.len(&txn)
}
