//! undo / redo 操作の例。
//!
//! テキストを複数回挿入して履歴を積み上げ、
//! undo で段階的に巻き戻し、redo で再適用する流れを示す。
//!
//! 実行:
//!   cargo run --example undo_redo -p wysiwyg-core

use std::sync::Arc;

use wysiwyg_core::{
    commands::insert_text,
    model::{node::Node, schema::basic_schema},
    state::EditorState,
};

fn main() {
    let schema = basic_schema();
    let state0 = EditorState::with_empty_doc(schema);

    println!("=== 初期状態 ===");
    println!("テキスト: {:?}", collect_text(&state0.doc));
    println!("undo 可能: {}", state0.can_undo());

    // 1 回目の挿入: "foo"
    let tr = insert_text(&state0, "foo").unwrap();
    let state1 = state0.apply(&tr).unwrap();

    println!("\n=== \"foo\" 挿入後 ===");
    println!("テキスト: {:?}", collect_text(&state1.doc));
    println!("undo 可能: {}", state1.can_undo());

    // 2 回目の挿入: "bar"
    let tr = insert_text(&state1, "bar").unwrap();
    let state2 = state1.apply(&tr).unwrap();

    println!("\n=== \"bar\" 追記後 ===");
    println!("テキスト: {:?}", collect_text(&state2.doc));
    println!("undo 可能: {}", state2.can_undo());
    println!("redo 可能: {}", state2.can_redo());

    // 1 回目の undo: "bar" を取り消す
    let state3 = state2.undo().expect("undo できるはずが None を返した");

    println!("\n=== undo 後 (\"bar\" を取り消し) ===");
    println!("テキスト: {:?}", collect_text(&state3.doc));
    println!("undo 可能: {}", state3.can_undo());
    println!("redo 可能: {}", state3.can_redo());

    // 2 回目の undo: "foo" も取り消す
    let state4 = state3.undo().expect("2 回目の undo が None を返した");

    println!("\n=== 2 回目の undo 後 (\"foo\" も取り消し) ===");
    println!("テキスト: {:?}", collect_text(&state4.doc));
    println!("undo 可能: {}", state4.can_undo());
    println!("redo 可能: {}", state4.can_redo());

    // redo で "foo" を再適用
    let state5 = state4.redo().expect("redo が None を返した");

    println!("\n=== redo 後 (\"foo\" を再適用) ===");
    println!("テキスト: {:?}", collect_text(&state5.doc));
    println!("undo 可能: {}", state5.can_undo());
    println!("redo 可能: {}", state5.can_redo());

    // さらに redo で "bar" を再適用
    let state6 = state5.redo().expect("2 回目の redo が None を返した");

    println!("\n=== 2 回目の redo 後 (\"bar\" を再適用) ===");
    println!("テキスト: {:?}", collect_text(&state6.doc));
    println!("undo 可能: {}", state6.can_undo());
    println!("redo 可能: {}", state6.can_redo());
}

/// ドキュメントツリーを再帰的に辿ってテキストを結合する
fn collect_text(node: &Arc<Node>) -> String {
    if let Some(t) = &node.text {
        return t.to_string();
    }
    node.content.children.iter().map(collect_text).collect()
}
