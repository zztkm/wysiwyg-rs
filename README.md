# wysiwyg-rs

A WYSIWYG editor core library written in Rust, inspired by ProseMirror/Tiptap.

## Overview

`wysiwyg-rs` provides a platform-agnostic document model and editing primitives that can be used in WebAssembly (browser) or native GUI environments.

## Crates

| Crate | Description |
|-------|-------------|
| `wysiwyg-core` | Document model, schema, transforms, selection, editor state, undo/redo, commands |
| `wysiwyg-collab` | Collaborative editing via [yrs](https://github.com/y-crdt/yrs) (stub, Phase 4) |
| `wysiwyg-wasm` | `wasm-bindgen` bindings for browser use (stub, Phase 3) |

## Architecture

The design follows ProseMirror's document model:

- **Schema** — registry of `NodeType` and `MarkType` definitions
- **Node / Fragment** — immutable tree of `Arc<Node>` for cheap structural sharing
- **Step** — atomic document mutation (`ReplaceStep`, `AddMarkStep`, `RemoveMarkStep`, `ReplaceAroundStep`)
- **Transaction** — accumulates steps; maps selection through each step
- **EditorState** — immutable snapshot: `(schema, doc, selection, history)`
- **Commands** — functions `(&EditorState) -> Option<Transaction>`

### Position model

Positions follow ProseMirror's logical position convention:

- Text node size = number of Unicode scalar values (`char` count)
- Non-text leaf node size = 1
- Branch node size = `content_size + 2` (opening + closing token)

## Development Status

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | Complete | Document model, `ReplaceStep`, `Transform` |
| Phase 2 | Complete | Marks, `AddMarkStep`/`RemoveMarkStep`, `Selection`, `EditorState`, `HistoryState`, built-in commands |
| Phase 3 | Planned | `wysiwyg-wasm` with `wasm-bindgen` |
| Phase 4 | Planned | Collaborative editing with `yrs` |

## Usage

```rust
use wysiwyg_core::{
    model::schema::basic_schema,
    state::{EditorState, Selection},
    commands::{toggle_bold, toggle_heading},
};

let schema = basic_schema();
let state = EditorState::with_empty_doc(schema);

// Toggle heading level 1 on current selection
if let Some(tr) = toggle_heading(&state, 1) {
    let new_state = state.apply(&tr).unwrap();
    // new_state.doc now has a heading node
}
```

## License

MIT
