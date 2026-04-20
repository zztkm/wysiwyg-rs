//! 基本的な編集操作の例。
//!
//! `EditorState::with_empty_doc` で空ドキュメントを作成し、
//! `insert_text` コマンドでテキストを挿入する最小限のシナリオを示す。
//!
//! 実行:
//!   cargo run --example basic_editing -p wysiwyg-core

use std::sync::Arc;

use wysiwyg_core::{
    commands::insert_text,
    model::{node::Node, schema::basic_schema},
    state::EditorState,
};

fn main() {
    // 基本スキーマで空ドキュメントの初期状態を作成
    let schema = basic_schema();
    let state = EditorState::with_empty_doc(schema);

    println!("=== 初期状態 ===");
    println!("テキスト: {:?}", collect_text(&state.doc));
    println!("カーソル位置: {}", state.selection.from());

    // "Hello, " を挿入
    let tr = insert_text(&state, "Hello, ").expect("テキスト挿入コマンドが None を返した");
    let state = state.apply(&tr).expect("トランザクションの適用に失敗");

    println!("\n=== \"Hello, \" 挿入後 ===");
    println!("テキスト: {:?}", collect_text(&state.doc));
    println!("カーソル位置: {}", state.selection.from());

    // カーソル直後に "world!" を追記
    let tr = insert_text(&state, "world!").expect("追記コマンドが None を返した");
    let state = state.apply(&tr).expect("トランザクションの適用に失敗");

    println!("\n=== \"world!\" 追記後 ===");
    println!("テキスト: {:?}", collect_text(&state.doc));
    println!("カーソル位置: {}", state.selection.from());
}

/// ドキュメントツリーを再帰的に辿ってテキストを結合する
fn collect_text(node: &Arc<Node>) -> String {
    if let Some(t) = &node.text {
        return t.to_string();
    }
    node.content.children.iter().map(collect_text).collect()
}
