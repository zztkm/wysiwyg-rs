//! Wasm bindings for wysiwyg-rs.
//!
//! Exposes [`WasmEditor`] via `wasm-bindgen` for use from JavaScript.
//!
//! # Document JSON format
//!
//! Documents follow the ProseMirror JSON convention:
//!
//! ```json
//! {
//!   "type": "doc",
//!   "content": [
//!     {
//!       "type": "paragraph",
//!       "content": [
//!         { "type": "text", "text": "Hello", "marks": [{"type": "bold"}] }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! # Selection JSON format
//!
//! ```json
//! { "anchor": 1, "head": 6 }
//! ```

use std::sync::Arc;

use wasm_bindgen::prelude::*;

use wysiwyg_core::{
    commands::{insert_text, set_block_type, toggle_bold, toggle_code, toggle_italic, toggle_heading},
    model::{
        attrs::{AttrValue, Attrs},
        mark::{Mark, MarkSet},
        node::{Fragment, Node},
        schema::{basic_schema, Schema},
    },
    state::{EditorState, Selection},
};

// ---------------------------------------------------------------------------
// JSON ↔ internal model conversion
// ---------------------------------------------------------------------------

fn attr_value_to_json(v: &AttrValue) -> serde_json::Value {
    match v {
        AttrValue::String(s) => serde_json::Value::String(s.as_ref().into()),
        AttrValue::Int(i) => serde_json::Value::Number((*i).into()),
        AttrValue::Bool(b) => serde_json::Value::Bool(*b),
        AttrValue::Null => serde_json::Value::Null,
    }
}

fn attrs_to_json(attrs: &Attrs) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = attrs
        .0
        .iter()
        .map(|(k, v)| (k.as_ref().to_string(), attr_value_to_json(v)))
        .collect();
    serde_json::Value::Object(map)
}

fn mark_to_json(mark: &Mark, schema: &Schema) -> serde_json::Value {
    let type_name = schema.mark_type(mark.type_id).name.as_ref().to_string();
    let mut obj = serde_json::Map::new();
    obj.insert("type".into(), serde_json::Value::String(type_name));
    if !mark.attrs.0.is_empty() {
        obj.insert("attrs".into(), attrs_to_json(&mark.attrs));
    }
    serde_json::Value::Object(obj)
}

fn node_to_json(node: &Node, schema: &Schema) -> serde_json::Value {
    // Text node
    if let Some(text) = &node.text {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), serde_json::Value::String("text".into()));
        obj.insert(
            "text".into(),
            serde_json::Value::String(text.as_ref().into()),
        );
        if !node.marks.is_empty() {
            let marks: Vec<serde_json::Value> =
                node.marks.iter().map(|m| mark_to_json(m, schema)).collect();
            obj.insert("marks".into(), serde_json::Value::Array(marks));
        }
        return serde_json::Value::Object(obj);
    }

    let type_name = schema.node_type(node.type_id).name.as_ref().to_string();
    let mut obj = serde_json::Map::new();
    obj.insert("type".into(), serde_json::Value::String(type_name));

    if !node.attrs.0.is_empty() {
        obj.insert("attrs".into(), attrs_to_json(&node.attrs));
    }

    if !node.content.is_empty() {
        let content: Vec<serde_json::Value> = node
            .content
            .children
            .iter()
            .map(|child| node_to_json(child, schema))
            .collect();
        obj.insert("content".into(), serde_json::Value::Array(content));
    }

    serde_json::Value::Object(obj)
}

fn json_attr_value(val: &serde_json::Value) -> AttrValue {
    match val {
        serde_json::Value::String(s) => AttrValue::String(Arc::from(s.as_str())),
        serde_json::Value::Number(n) => {
            AttrValue::Int(n.as_i64().unwrap_or(0))
        }
        serde_json::Value::Bool(b) => AttrValue::Bool(*b),
        _ => AttrValue::Null,
    }
}

fn json_to_attrs(val: &serde_json::Value) -> Attrs {
    if let Some(obj) = val.as_object() {
        let mut attrs = Attrs::empty();
        for (k, v) in obj {
            attrs = attrs.with(k.as_str(), json_attr_value(v));
        }
        attrs
    } else {
        Attrs::empty()
    }
}

fn json_to_mark(val: &serde_json::Value, schema: &Schema) -> Result<Mark, String> {
    let obj = val.as_object().ok_or("mark must be an object")?;
    let type_name = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("mark missing 'type'")?;
    let mark_type = schema
        .mark_type_by_name(type_name)
        .ok_or_else(|| format!("unknown mark type: {type_name}"))?;
    let attrs = obj
        .get("attrs")
        .map(json_to_attrs)
        .unwrap_or_else(Attrs::empty);
    Ok(Mark::new(mark_type.id, attrs))
}

fn json_to_node(val: &serde_json::Value, schema: &Schema) -> Result<Arc<Node>, String> {
    let obj = val.as_object().ok_or("node must be an object")?;
    let type_name = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("node missing 'type'")?;

    if type_name == "text" {
        let text = obj
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("text node missing 'text'")?;
        let text_type = schema
            .node_type_by_name("text")
            .ok_or("schema has no 'text' type")?;
        let marks = if let Some(marks_val) = obj.get("marks") {
            marks_val
                .as_array()
                .ok_or("'marks' must be an array")?
                .iter()
                .map(|m| json_to_mark(m, schema))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![]
        };
        return Ok(Arc::new(Node::text(
            text_type.id,
            text,
            MarkSet::from_marks(marks),
        )));
    }

    let node_type = schema
        .node_type_by_name(type_name)
        .ok_or_else(|| format!("unknown node type: {type_name}"))?;

    let attrs = obj
        .get("attrs")
        .map(json_to_attrs)
        .unwrap_or_else(Attrs::empty);

    let content = if let Some(content_val) = obj.get("content") {
        let children = content_val
            .as_array()
            .ok_or("'content' must be an array")?
            .iter()
            .map(|c| json_to_node(c, schema))
            .collect::<Result<Vec<_>, _>>()?;
        Fragment::from_nodes(children)
    } else {
        Fragment::empty()
    };

    let node = if node_type.spec.atom {
        Node::atom(node_type.id, attrs, MarkSet::empty())
    } else {
        Node::new(node_type.id, attrs, content, MarkSet::empty())
    };

    Ok(Arc::new(node))
}

// ---------------------------------------------------------------------------
// WasmEditor
// ---------------------------------------------------------------------------

/// A stateful WYSIWYG editor instance exposed to JavaScript via wasm-bindgen.
///
/// Create with `new WasmEditor()` (starts with an empty document), or
/// `WasmEditor.from_doc(jsonString)` to initialise from a JSON document.
#[wasm_bindgen]
pub struct WasmEditor {
    state: EditorState,
}

#[wasm_bindgen]
impl WasmEditor {
    /// Create a new editor with an empty document (basic schema).
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmEditor {
        let schema = basic_schema();
        let state = EditorState::with_empty_doc(schema);
        WasmEditor { state }
    }

    /// Create an editor from a JSON document string.
    ///
    /// Returns an error string if the JSON is invalid or references unknown types.
    pub fn from_doc(doc_json: &str) -> Result<WasmEditor, String> {
        let schema = basic_schema();
        let val: serde_json::Value =
            serde_json::from_str(doc_json).map_err(|e| e.to_string())?;
        let doc = json_to_node(&val, &schema)?;
        let state = EditorState::new(schema, doc, Selection::cursor(1));
        Ok(WasmEditor { state })
    }

    // -----------------------------------------------------------------------
    // Document / selection access
    // -----------------------------------------------------------------------

    /// Return the current document as a JSON string.
    pub fn get_doc(&self) -> String {
        let val = node_to_json(&self.state.doc, &self.state.schema);
        serde_json::to_string(&val).unwrap_or_default()
    }

    /// Return the current selection as a JSON string `{"anchor": N, "head": N}`.
    pub fn get_selection(&self) -> String {
        let from = self.state.selection.from();
        let to = self.state.selection.to(&self.state.doc);
        serde_json::json!({ "anchor": from, "head": to }).to_string()
    }

    /// Move the cursor to `pos`.
    pub fn set_cursor(&mut self, pos: usize) {
        let mut tr = self.state.transaction();
        tr.set_selection(Selection::cursor(pos));
        if let Ok(new_state) = self.state.apply(&tr) {
            self.state = new_state;
        }
    }

    /// Set the selection to `[anchor, head)`.
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        let mut tr = self.state.transaction();
        tr.set_selection(Selection::text(anchor, head));
        if let Ok(new_state) = self.state.apply(&tr) {
            self.state = new_state;
        }
    }

    // -----------------------------------------------------------------------
    // Text input
    // -----------------------------------------------------------------------

    /// Insert `text` at the current selection. Returns `true` on success.
    pub fn insert_text(&mut self, text: &str) -> bool {
        self.dispatch(insert_text(&self.state, text))
    }

    // -----------------------------------------------------------------------
    // Mark commands
    // -----------------------------------------------------------------------

    /// Toggle **bold** on the current selection.
    pub fn toggle_bold(&mut self) -> bool {
        self.dispatch(toggle_bold(&self.state))
    }

    /// Toggle **italic** on the current selection.
    pub fn toggle_italic(&mut self) -> bool {
        self.dispatch(toggle_italic(&self.state))
    }

    /// Toggle **inline code** on the current selection.
    pub fn toggle_code(&mut self) -> bool {
        self.dispatch(toggle_code(&self.state))
    }

    // -----------------------------------------------------------------------
    // Block commands
    // -----------------------------------------------------------------------

    /// Toggle a heading of the given level (1–6) on the current selection.
    pub fn toggle_heading(&mut self, level: i32) -> bool {
        self.dispatch(toggle_heading(&self.state, level as i64))
    }

    /// Change all selected blocks to `type_name` (e.g. "paragraph", "code_block").
    pub fn set_block_type(&mut self, type_name: &str) -> bool {
        use wysiwyg_core::model::attrs::Attrs;
        self.dispatch(set_block_type(&self.state, type_name, Attrs::empty()))
    }

    // -----------------------------------------------------------------------
    // History
    // -----------------------------------------------------------------------

    /// Undo the last change. Returns `true` if there was something to undo.
    pub fn undo(&mut self) -> bool {
        if let Some(new_state) = self.state.undo() {
            self.state = new_state;
            true
        } else {
            false
        }
    }

    /// Redo the last undone change. Returns `true` if there was something to redo.
    pub fn redo(&mut self) -> bool {
        if let Some(new_state) = self.state.redo() {
            self.state = new_state;
            true
        } else {
            false
        }
    }

    pub fn can_undo(&self) -> bool {
        self.state.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.state.can_redo()
    }
}

impl WasmEditor {
    fn dispatch(&mut self, tr: Option<wysiwyg_core::state::Transaction>) -> bool {
        if let Some(tr) = tr {
            if let Ok(new_state) = self.state.apply(&tr) {
                self.state = new_state;
                return true;
            }
        }
        false
    }
}
