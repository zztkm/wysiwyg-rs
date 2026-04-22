//! 差分同期 (incremental sync) のデモ。
//!
//! `state_vector()` と `encode_diff()` を使って、既に共有済みの op を除いた
//! 差分のみを交換するプロトコルを示す。全量 `encode_state_as_update()` と比較して
//! 転送量が減ることを数値で確認できる。
//!
//! シナリオ:
//!   1. ホストが起動し共有状態を作る。
//!   2. ゲストが参加 (初回は全量 update で bootstrap)。
//!   3. 双方が独立に編集する。
//!   4. 相手の state vector を使って差分 update を計算・交換する。
//!   5. 全量 update サイズと差分 update サイズを比較する。
//!
//! 実行:
//!   cargo run --example two_peer_incremental -p wysiwyg-collab

use std::sync::Arc;

use wysiwyg_collab::CollabState;
use wysiwyg_core::{
    commands::insert_text,
    model::node::Node,
    state::{EditorState, Selection},
};
use yrs::{updates::decoder::Decode, updates::encoder::Encode, StateVector};

fn main() {
    // -----------------------------------------------------------------------
    // 1. ホスト起動 + 初期テキスト
    // -----------------------------------------------------------------------
    println!("=== 1. ホスト起動 ===");
    let mut host = CollabState::create_host(1);
    let tr = insert_text(&host.editor, "Hello").expect("挿入に失敗");
    assert!(host.apply_transaction(tr));
    println!("Host テキスト: {:?}", collect_text(&host.editor.doc));

    // -----------------------------------------------------------------------
    // 2. ゲスト参加 (初回のみ全量 update を使って bootstrap)
    // -----------------------------------------------------------------------
    println!("\n=== 2. ゲスト参加 (初回は全量 update で bootstrap) ===");
    let initial_update = host.encode_state_as_update();
    let mut guest = CollabState::join_guest(2, &initial_update);
    println!("初期 bootstrap サイズ: {} bytes", initial_update.len());
    println!("Guest テキスト: {:?}", collect_text(&guest.editor.doc));

    // -----------------------------------------------------------------------
    // 3. 双方が独立に編集
    // -----------------------------------------------------------------------
    println!("\n=== 3. 双方が独立に編集 ===");
    let host_end = collect_text(&host.editor.doc).chars().count() + 1;
    let host_state = EditorState::new(
        host.editor.schema.clone(),
        host.editor.doc.clone(),
        Selection::cursor(host_end),
    );
    let tr = insert_text(&host_state, ", from host").expect("挿入に失敗");
    assert!(host.apply_transaction(tr));
    println!("Host テキスト: {:?}", collect_text(&host.editor.doc));

    let guest_end = collect_text(&guest.editor.doc).chars().count() + 1;
    let guest_state = EditorState::new(
        guest.editor.schema.clone(),
        guest.editor.doc.clone(),
        Selection::cursor(guest_end),
    );
    let tr = insert_text(&guest_state, " & greetings from guest").expect("挿入に失敗");
    assert!(guest.apply_transaction(tr));
    println!("Guest テキスト: {:?}", collect_text(&guest.editor.doc));

    // -----------------------------------------------------------------------
    // 4. 差分同期プロトコル
    //
    //    通常の sync protocol の流れ:
    //      (a) 各 peer が自分の state vector を相手に送る
    //      (b) 受け取った state vector で差分 update を encode して返す
    //      (c) 相手は差分 update を apply する
    //
    //    この example は単一プロセスなので network 送受信の代わりに
    //    encode_v1 / decode_v1 でバイト列への直列化を挟んで wire format を示す。
    // -----------------------------------------------------------------------
    println!("\n=== 4. 差分同期 ===");

    // (a) 各 peer が自分の state vector を相手に送る
    let sv_host_bytes = host.state_vector().encode_v1();
    let sv_guest_bytes = guest.state_vector().encode_v1();
    println!(
        "Host → Guest に送る state vector: {} bytes",
        sv_host_bytes.len()
    );
    println!(
        "Guest → Host に送る state vector: {} bytes",
        sv_guest_bytes.len()
    );

    // (b) 各 peer は受け取った state vector を decode し、差分 update を encode
    let sv_from_host = StateVector::decode_v1(&sv_host_bytes).expect("state vector decode 失敗");
    let sv_from_guest = StateVector::decode_v1(&sv_guest_bytes).expect("state vector decode 失敗");

    let diff_for_host = guest.encode_diff(&sv_from_host);
    let diff_for_guest = host.encode_diff(&sv_from_guest);
    println!(
        "Guest → Host に送る差分 update: {} bytes",
        diff_for_host.len()
    );
    println!(
        "Host → Guest に送る差分 update: {} bytes",
        diff_for_guest.len()
    );

    // 参考: 同じタイミングで全量 update を送った場合のサイズ
    let full_from_host = host.encode_state_as_update();
    let full_from_guest = guest.encode_state_as_update();
    println!(
        "(参考) Host 全量 update: {} bytes, Guest 全量 update: {} bytes",
        full_from_host.len(),
        full_from_guest.len()
    );

    // (c) 差分 update を apply
    host.apply_remote_update(&diff_for_host);
    guest.apply_remote_update(&diff_for_guest);

    // -----------------------------------------------------------------------
    // 5. 収束確認 + サイズ比較
    // -----------------------------------------------------------------------
    println!("\n=== 5. 収束確認 ===");
    let host_text = collect_text(&host.editor.doc);
    let guest_text = collect_text(&guest.editor.doc);
    println!("Host  最終テキスト: {:?}", host_text);
    println!("Guest 最終テキスト: {:?}", guest_text);
    assert_eq!(host_text, guest_text, "テキストが収束していない");

    // 差分 update の方が全量 update より小さいことを確認
    assert!(
        diff_for_guest.len() < full_from_host.len(),
        "差分 update が全量 update 以上のサイズになっている",
    );
    println!(
        "\n差分 update ({} bytes) は全量 update ({} bytes) より {} bytes 小さい",
        diff_for_guest.len(),
        full_from_host.len(),
        full_from_host.len() - diff_for_guest.len(),
    );

    println!("両 peer が差分同期で収束しました");
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
