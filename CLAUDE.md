# CLAUDE.md — wysiwyg-rs 開発ガイド

## TODO コメント規約

未対応の機能や既知の制限には必ず `// TODO:` コメントを残す。
コードを読むだけで「何が動いて何が動かないか」を把握できるようにすることが目的。

```rust
// TODO: <何をすべきか> — <なぜ今やっていないか / どこが難しいか>
```

- 関数・型の制限はドキュメントコメント (`///`) 内に `# Limitations` セクションを設けて記載する。
- インライン TODO はロジックの直前または直後に置く。
- "not synced in prototype" のような曖昧な表現は避け、「何が必要か」を具体的に書く。

---

## クレート構成

```
wysiwyg-rs/
  crates/
    wysiwyg-core/    # ドキュメントモデル・Transform・EditorState（プラットフォーム非依存）
    wysiwyg-collab/  # yrs (CRDT) を使った協調編集レイヤー
    wysiwyg-wasm/    # wasm-bindgen による JS 向け API
```

依存方向: `wysiwyg-wasm` → `wysiwyg-core` + `wysiwyg-collab` → `wysiwyg-core` + `yrs`

---

## 既知の TODO・未対応事項

### wysiwyg-collab（Phase 4 プロトタイプ制限）

#### 初期状態の共有 (`CollabState::new`)

`CollabState::new()` を呼び出すたびに空の paragraph が yrs ドキュメントに追加される。
複数ピアが独立に `new()` を呼ぶと、CRDT マージ後に N 個の paragraph が残る。

**TODO**: 1 つのピアがドキュメントを作成し、`encode_state_as_update → apply_remote_update`
で初期状態を配布してから編集を開始するファクトリパターンに置き換える。

#### マーク同期 (`AddMarkStep` / `RemoveMarkStep`)

bold・italic・code・link のマーク変更は yrs に伝搬されない。
リモートピアで rebuild した PM ドキュメントにはマーク情報が失われる。

**TODO**: yrs `XmlText` の属性としてマーク状態を保存し、`build_pm_doc_from_yrs` で
属性 → PM マークへの逆変換を実装する。

#### ブロック型変更の同期 (`set_block_type` / `toggle_heading`)

heading・code_block などへのブロック型変更は yrs に伝搬されない。

**TODO**: yrs `XmlElement` のタグ名でブロック型を表現し、
`ReplaceAroundStep` を検出して yrs 側の要素を置き換える。

#### ブロック分割・結合 (`ReplaceStep` with open slice)

Enter キー（段落分割）や Backspace による段落結合は、スライスの `open_start`/`open_end`
が非ゼロになる `ReplaceStep` として表現される。現在はこの種のステップを無視している。

**TODO**: `open_start != 0 || open_end != 0` の場合に XmlElement の挿入・削除を行う。

#### 段落をまたぐ削除

選択範囲が複数の paragraph をまたぐ削除は yrs に反映されない。

**TODO**: 削除範囲が複数ブロックにまたがる場合、yrs トランザクション内で
- 先頭ブロックの末尾テキストを削除
- 中間ブロックを XmlFragment から削除
- 末尾ブロックの先頭テキストを削除
の 3 ステップを実行する。

#### リモート更新後のカーソル位置 (`rebuild_pm_from_yrs`)

`apply_remote_update` を呼ぶたびにカーソルが `position 1` にリセットされる。

**TODO**: yrs update の `Mapping` を使って既存の Selection を追跡し、
リモート更新後もカーソルをリマップする。

#### ネストしたブロック構造

`resolve_text_pos` と `build_pm_doc_from_yrs` はいずれも `doc > [block*]` の
フラット構造しか処理しない。

**TODO**: `blockquote > paragraph`・`list_item > paragraph` などの再帰的な構造に対応する
ために、位置解決と再構築を再帰化する。

#### スライスからのマーク情報

`extract_flat_text_from_slice` はテキスト内容のみ抽出し、マーク情報を捨てる。
ペーストしたテキストの書式が yrs レイヤーで失われる。

**TODO**: スライス内のテキストノードに付いたマークを yrs `XmlText` 属性として
書き込めるよう `extract_flat_text_from_slice` を拡張する。

---

## 位置カウント

ProseMirror 式の **論理位置（logical position）** を採用。

- テキストノードのサイズ = Unicode スカラー値の数（`char` 単位）
- 非テキストリーフのサイズ = 1
- ブランチノードのサイズ = `content_size + 2`（開始・終了タグ分）

段落内の有効なカーソル位置は `[offset+1 .. offset+child_size-1]`（両端含む）。
`offset+child_size-1` は「段落末尾の直後、閉じタグの直前」で **有効なカーソス位置**。

JS との境界（Wasm API）では UTF-16 ↔ Rust `char` の変換が必要。現状は未実装（TODO）。

---

## テスト戦略

- `cargo test --workspace` で全クレートのテストを実行。
- 各 `Step` には `apply` / `invert` / `map` のユニットテストを書く。
- 協調編集テストは 2 ピアの収束（両ピアのテキストが一致すること）を検証する。
- `api_probe` モジュールには yrs API の動作確認テストを置く（内部仕様の変化を検知するため）。
