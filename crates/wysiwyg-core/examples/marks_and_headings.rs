//! マークとブロック操作の例。
//!
//! テキストを挿入した後、選択範囲を設定して bold / italic のトグルや
//! 見出し(heading)への変換を試みる。
//!
//! 実行:
//!   cargo run --example marks_and_headings -p wysiwyg-core

use std::sync::Arc;

use wysiwyg_core::{
    commands::{insert_text, toggle_bold, toggle_heading, toggle_italic},
    model::{node::Node, schema::basic_schema},
    state::{EditorState, Selection},
};

fn main() {
    let schema = basic_schema();

    // --- マーク操作 ---

    // "Hello Rust" を含む段落を用意（テキスト長 = 10 chars）
    let state = EditorState::with_empty_doc(schema.clone());
    let tr = insert_text(&state, "Hello Rust").unwrap();
    let state = state.apply(&tr).unwrap();

    println!("=== 元のテキスト ===");
    println!("テキスト: {:?}", collect_text(&state.doc));

    // "Hello Rust" 全体を選択して bold を適用（位置 1〜11）
    let state_sel = EditorState::new(schema.clone(), state.doc.clone(), Selection::text(1, 11));
    let tr = toggle_bold(&state_sel).expect("bold トグルが None を返した");
    let state = state_sel.apply(&tr).unwrap();

    let bold_id = schema.mark_type_by_name("bold").unwrap().id;
    let text_node = state.doc.child(0).unwrap().content.child(0).unwrap();
    println!("\n=== bold 適用後 ===");
    println!("bold マーク付き: {}", text_node.marks.contains(bold_id));

    // 同じ範囲に italic も追加
    let state_sel = EditorState::new(schema.clone(), state.doc.clone(), Selection::text(1, 11));
    let tr = toggle_italic(&state_sel).expect("italic トグルが None を返した");
    let state = state_sel.apply(&tr).unwrap();

    let italic_id = schema.mark_type_by_name("italic").unwrap().id;
    let text_node = state.doc.child(0).unwrap().content.child(0).unwrap();
    println!("\n=== italic 追加後 ===");
    println!("bold マーク付き:   {}", text_node.marks.contains(bold_id));
    println!("italic マーク付き: {}", text_node.marks.contains(italic_id));

    // bold のみ解除（2回目のトグルで削除）
    let state_sel = EditorState::new(schema.clone(), state.doc.clone(), Selection::text(1, 11));
    let tr = toggle_bold(&state_sel).expect("bold 解除トグルが None を返した");
    let state = state_sel.apply(&tr).unwrap();

    let text_node = state.doc.child(0).unwrap().content.child(0).unwrap();
    println!("\n=== bold 解除後 ===");
    println!("bold マーク付き:   {}", text_node.marks.contains(bold_id));
    println!("italic マーク付き: {}", text_node.marks.contains(italic_id));

    // --- ブロック操作 ---

    // 段落全体を選択して h1 見出しに変換
    let state_sel = EditorState::new(schema.clone(), state.doc.clone(), Selection::text(1, 11));
    let tr = toggle_heading(&state_sel, 1).expect("h1 見出しトグルが None を返した");
    let state = state_sel.apply(&tr).unwrap();

    let heading_type = schema.node_type_by_name("heading").unwrap();
    let block = state.doc.child(0).unwrap();
    println!("\n=== h1 見出し変換後 ===");
    println!("heading ノードか: {}", block.type_id == heading_type.id);
    println!("level 属性: {:?}", block.attrs.get("level"));

    // もう一度トグルすると paragraph に戻る
    let state_sel = EditorState::new(schema.clone(), state.doc.clone(), Selection::text(1, 11));
    let tr = toggle_heading(&state_sel, 1).expect("段落に戻すトグルが None を返した");
    let state = state_sel.apply(&tr).unwrap();

    let para_type = schema.node_type_by_name("paragraph").unwrap();
    let block = state.doc.child(0).unwrap();
    println!("\n=== 段落に戻した後 ===");
    println!("paragraph ノードか: {}", block.type_id == para_type.id);
}

/// ドキュメントツリーを再帰的に辿ってテキストを結合する
fn collect_text(node: &Arc<Node>) -> String {
    if let Some(t) = &node.text {
        return t.to_string();
    }
    node.content.children.iter().map(collect_text).collect()
}
