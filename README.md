# wysiwyg-rs

[ProseMirror](https://prosemirror.net/) の設計に着想を得た、Rust 製のプラットフォーム中立 WYSIWYG エディタコアライブラリです。

WebAssembly（ブラウザ）やネイティブ GUI 環境で使用できるドキュメントモデルと編集プリミティブを提供します。

## クレート構成

| クレート | 説明 |
|---|---|
| [`wysiwyg-core`](https://crates.io/crates/wysiwyg-core) | ドキュメントモデル、スキーマ、変換、選択、エディタ状態、Undo/Redo、コマンド |
| `wysiwyg-wasm` | ブラウザ向け wasm-bindgen バインディング（未公開） |
| `wysiwyg-collab` | yrs CRDT を使った協調編集レイヤー（未公開・プロトタイプ） |

## クイックスタート

`Cargo.toml` に追加:

```toml
[dependencies]
wysiwyg-core = "0.1"
```

## 使用例

```rust
use wysiwyg_core::{
    commands::{insert_text, toggle_bold, toggle_heading},
    model::schema::basic_schema,
    state::EditorState,
};

// 空ドキュメントで初期化
let schema = basic_schema();
let state = EditorState::with_empty_doc(schema);

// テキスト挿入
let tr = insert_text(&state, "Hello, world!").unwrap();
let state = state.apply(&tr).unwrap();

// 見出しに変換
let tr = toggle_heading(&state, 1).unwrap();
let state = state.apply(&tr).unwrap();

// 太字トグル
let tr = toggle_bold(&state).unwrap();
let state = state.apply(&tr).unwrap();

// Undo
let state = state.undo().unwrap();
```

その他の使用例は [`crates/wysiwyg-core/examples/`](crates/wysiwyg-core/examples/) を参照してください。

## アーキテクチャ

ProseMirror のドキュメントモデルに準拠した設計です。

```
EditorState（不変スナップショット）
  ├── Schema       … NodeType / MarkType のレジストリ
  ├── Node（Arc）  … 不変ドキュメントツリー
  ├── Selection    … TextSelection / NodeSelection / AllSelection
  └── HistoryState … Undo/Redo スタック（最大 100）

Command: (&EditorState) -> Option<Transaction>
Transaction → [Step, ...] → apply() → new EditorState
```

### 主要コンセプト

- **Schema** — `NodeType` と `MarkType` の定義レジストリ
- **Node / Fragment** — `Arc<Node>` による不変ツリー。変更時は変更パスのみ再割当で構造共有
- **Step** — 原子的ドキュメント変換（`ReplaceStep`, `AddMarkStep`, `RemoveMarkStep`, `ReplaceAroundStep`）
- **Transaction** — ステップを積み上げ、各ステップを通じて選択を再マップ
- **EditorState** — 不変スナップショット: `(schema, doc, selection, history)`
- **Command** — `(&EditorState) -> Option<Transaction>` のシグネチャを持つ関数

### 位置モデル

ProseMirror の論理位置規約に準拠:

- テキストノードサイズ = Unicode スカラー値の数（`char` 数）
- 非テキストリーフノードサイズ = 1
- 分岐ノードサイズ = `content_size + 2`（開始・終了トークン分）

### 組み込みスキーマ

`basic_schema()` で以下を提供します:

**ノード型**: `doc`, `paragraph`, `heading`, `code_block`, `blockquote`, `bullet_list`, `ordered_list`, `list_item`, `hard_break`, `text`

**マーク型**: `bold`, `italic`, `code`, `link`

## 開発状況

| フェーズ | 状態 | 内容 |
|---|---|---|
| Phase 1 | 完了 | ドキュメントモデル、`ReplaceStep`、`Transform` |
| Phase 2 | 完了 | Marks、`AddMarkStep`/`RemoveMarkStep`、`Selection`、`EditorState`、`HistoryState`、組み込みコマンド |
| Phase 3 | 実装済み | `wysiwyg-wasm`（wasm-bindgen バインディング）、ブラウザデモ |
| Phase 4 | プロトタイプ | `wysiwyg-collab`（yrs CRDT 協調編集） |

## ライセンス

MIT — 詳細は [LICENSE](LICENSE) を参照してください。
