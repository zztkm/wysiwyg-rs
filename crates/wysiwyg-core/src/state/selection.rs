use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    model::node::Node,
    transform::step_map::Mapping,
};

/// A text selection defined by an anchor and head.
///
/// - `anchor`: The fixed end (where the selection started).
/// - `head`: The moving end (where the cursor is).
///
/// If `anchor == head`, this is a cursor (collapsed selection).
/// The selected range is `[from()..to())`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSelection {
    pub anchor: usize,
    pub head: usize,
}

impl TextSelection {
    pub fn new(anchor: usize, head: usize) -> Self {
        TextSelection { anchor, head }
    }

    /// Create a collapsed cursor at `pos`.
    pub fn cursor(pos: usize) -> Self {
        TextSelection { anchor: pos, head: pos }
    }

    /// The start of the selected range (min of anchor and head).
    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The end of the selected range (max of anchor and head).
    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// Whether the selection is collapsed (cursor).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Map this selection through a `Mapping`.
    pub fn map(&self, mapping: &Mapping) -> TextSelection {
        TextSelection {
            anchor: mapping.map_left(self.anchor),
            head: mapping.map_left(self.head),
        }
    }
}

/// A node selection — selects an entire block or inline node.
///
/// `pos` is the position immediately *before* the selected node.
/// The selected node occupies `pos..pos+node_size` in the parent's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSelection {
    pub pos: usize,
}

impl NodeSelection {
    pub fn new(pos: usize) -> Self {
        NodeSelection { pos }
    }

    pub fn map(&self, mapping: &Mapping) -> NodeSelection {
        NodeSelection {
            pos: mapping.map_left(self.pos),
        }
    }
}

/// Selects the entire document content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllSelection;

impl AllSelection {
    /// Map through a mapping — an AllSelection always covers the entire doc,
    /// so it maps to itself (positions don't matter for this variant).
    pub fn map(&self, _mapping: &Mapping) -> AllSelection {
        AllSelection
    }
}

/// The selection state of the editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Selection {
    Text(TextSelection),
    Node(NodeSelection),
    All(AllSelection),
}

impl Selection {
    /// Create a collapsed cursor at `pos`.
    pub fn cursor(pos: usize) -> Self {
        Selection::Text(TextSelection::cursor(pos))
    }

    /// Create a text selection from `anchor` to `head`.
    pub fn text(anchor: usize, head: usize) -> Self {
        Selection::Text(TextSelection::new(anchor, head))
    }

    /// Select the entire document.
    pub fn all() -> Self {
        Selection::All(AllSelection)
    }

    /// The start of the selection range.
    pub fn from(&self) -> usize {
        match self {
            Selection::Text(s) => s.from(),
            Selection::Node(s) => s.pos,
            Selection::All(_) => 0,
        }
    }

    /// The end of the selection range.
    pub fn to(&self, doc: &Arc<Node>) -> usize {
        match self {
            Selection::Text(s) => s.to(),
            Selection::Node(s) => s.pos + 1, // minimal — caller should use actual node size
            Selection::All(_) => doc.content.size,
        }
    }

    /// Whether the selection is a cursor (no range).
    pub fn is_cursor(&self) -> bool {
        matches!(self, Selection::Text(s) if s.is_empty())
    }

    /// Map the selection through a `Mapping`.
    pub fn map(&self, mapping: &Mapping) -> Selection {
        match self {
            Selection::Text(s) => Selection::Text(s.map(mapping)),
            Selection::Node(s) => Selection::Node(s.map(mapping)),
            Selection::All(s) => Selection::All(s.map(mapping)),
        }
    }

    /// Clamp the selection to be valid within `doc`.
    pub fn clamped(self, doc: &Arc<Node>) -> Selection {
        let max = doc.content.size;
        match self {
            Selection::Text(mut s) => {
                s.anchor = s.anchor.min(max);
                s.head = s.head.min(max);
                Selection::Text(s)
            }
            Selection::Node(mut s) => {
                s.pos = s.pos.min(max.saturating_sub(1));
                Selection::Node(s)
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::step_map::{Mapping, StepMap};

    #[test]
    fn cursor_is_empty() {
        let s = TextSelection::cursor(5);
        assert!(s.is_empty());
        assert_eq!(s.from(), 5);
        assert_eq!(s.to(), 5);
    }

    #[test]
    fn text_selection_range() {
        let s = TextSelection::new(8, 3); // head < anchor = backward selection
        assert_eq!(s.from(), 3);
        assert_eq!(s.to(), 8);
    }

    #[test]
    fn selection_map_through_insertion() {
        // Insert 3 chars at position 5.
        let mut mapping = Mapping::new();
        mapping.append_map(StepMap::from_ranges([(5, 0, 3)]));

        // Cursor at position 7 (after insertion point): shifts right by 3.
        let sel = Selection::cursor(7);
        let mapped = sel.map(&mapping);
        assert_eq!(mapped.from(), 10);

        // Cursor at position 3 (before insertion): unchanged.
        let sel2 = Selection::cursor(3);
        let mapped2 = sel2.map(&mapping);
        assert_eq!(mapped2.from(), 3);
    }

    #[test]
    fn selection_map_through_deletion() {
        // Delete 4 chars at position 2.
        let mut mapping = Mapping::new();
        mapping.append_map(StepMap::from_ranges([(2, 4, 0)]));

        // Cursor inside deleted range: maps to start of deletion.
        let sel = Selection::cursor(4);
        let mapped = sel.map(&mapping);
        assert_eq!(mapped.from(), 2);

        // Cursor after deleted range: shifts left by 4.
        let sel2 = Selection::cursor(8);
        let mapped2 = sel2.map(&mapping);
        assert_eq!(mapped2.from(), 4);
    }
}
