use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::attrs::Attrs;
use super::mark::MarkSet;

/// Interned identifier for a node type within a `Schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeTypeId(pub u16);

/// A single node in the document tree.
///
/// Wrapped in `Arc` everywhere so that cloning an `EditorState` is cheap —
/// only the path from the modified node to the root is reallocated on each
/// transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Which type this node is (looked up in the `Schema`).
    pub type_id: NodeTypeId,
    /// Attributes specific to this node instance.
    pub attrs: Attrs,
    /// Ordered child nodes.  Empty for leaf/text nodes.
    pub content: Fragment,
    /// Marks applied to this node (meaningful for inline nodes only).
    pub marks: MarkSet,
    /// For text nodes only: the text content.
    pub text: Option<Arc<str>>,
    /// True for atom (indivisible) nodes such as `hard_break`.
    /// Atom nodes always have size 1 regardless of content.
    /// Defaults to `false` for regular branch nodes and text nodes.
    #[serde(default)]
    pub is_atom: bool,
}

impl Node {
    /// Create a branch node (has children, no text).
    pub fn new(type_id: NodeTypeId, attrs: Attrs, content: Fragment, marks: MarkSet) -> Self {
        Node {
            type_id,
            attrs,
            content,
            marks,
            text: None,
            is_atom: false,
        }
    }

    /// Create a text node.
    pub fn text(type_id: NodeTypeId, text: impl Into<Arc<str>>, marks: MarkSet) -> Self {
        Node {
            type_id,
            attrs: Attrs::empty(),
            content: Fragment::empty(),
            marks,
            text: Some(text.into()),
            is_atom: false,
        }
    }

    /// Create an atom (indivisible) node such as `hard_break`.
    /// Atom nodes have size 1 and are treated as leaves.
    pub fn atom(type_id: NodeTypeId, attrs: Attrs, marks: MarkSet) -> Self {
        Node {
            type_id,
            attrs,
            content: Fragment::empty(),
            marks,
            text: None,
            is_atom: true,
        }
    }

    /// The size of this node in logical position units:
    /// - text node: number of `char`s (Unicode scalar values)
    /// - atom leaf: 1
    /// - branch node: `content.size + 2` (opening + closing token)
    ///   — this is 2 even for empty branch nodes (e.g., empty paragraph)
    pub fn node_size(&self) -> usize {
        if let Some(t) = &self.text {
            t.chars().count()
        } else if self.is_atom {
            1
        } else {
            self.content.size + 2
        }
    }

    pub fn is_text(&self) -> bool {
        self.text.is_some()
    }

    /// A node is a leaf if it is a text node or an atom.
    /// Empty branch nodes (e.g., empty paragraphs) are NOT leaves.
    pub fn is_leaf(&self) -> bool {
        self.text.is_some() || self.is_atom
    }

    /// Number of direct children.
    pub fn child_count(&self) -> usize {
        self.content.child_count()
    }

    pub fn child(&self, index: usize) -> Option<&Arc<Node>> {
        self.content.child(index)
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.attrs == other.attrs
            && self.content == other.content
            && self.marks == other.marks
            && self.text == other.text
            && self.is_atom == other.is_atom
    }
}

/// An ordered sequence of child nodes.
///
/// Stores children as `Arc<[Arc<Node>]>` so that appending/prepending creates
/// a new allocation but all unchanged children are shared.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Fragment {
    pub children: Arc<[Arc<Node>]>,
    /// Cached total size of all children.
    pub size: usize,
}

impl Fragment {
    pub fn empty() -> Self {
        Fragment {
            children: Arc::from(vec![].into_boxed_slice()),
            size: 0,
        }
    }

    pub fn from_nodes(nodes: Vec<Arc<Node>>) -> Self {
        let size = nodes.iter().map(|n| n.node_size()).sum();
        Fragment {
            children: Arc::from(nodes.into_boxed_slice()),
            size,
        }
    }

    pub fn from_node(node: Arc<Node>) -> Self {
        let size = node.node_size();
        Fragment {
            children: Arc::from(vec![node].into_boxed_slice()),
            size,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    pub fn child(&self, index: usize) -> Option<&Arc<Node>> {
        self.children.get(index)
    }

    /// Concatenate two fragments.
    pub fn append(&self, other: &Fragment) -> Fragment {
        let mut nodes: Vec<Arc<Node>> = self.children.iter().cloned().collect();
        nodes.extend(other.children.iter().cloned());
        Fragment::from_nodes(nodes)
    }

    /// Cut a sub-fragment from logical position `from` to `to`.
    ///
    /// Returns the slice of children (possibly with trimmed text nodes) that
    /// falls within `[from, to)`.
    pub fn cut(&self, from: usize, to: usize) -> Fragment {
        if from == 0 && to == self.size {
            return self.clone();
        }
        let mut nodes: Vec<Arc<Node>> = Vec::new();
        let mut offset = 0usize;
        for child in self.children.iter() {
            let child_size = child.node_size();
            if offset + child_size > from && offset < to {
                let cut_from = from.saturating_sub(offset);
                let cut_to = (to - offset).min(child_size);
                if child.is_text() {
                    let text = child.text.as_ref().unwrap();
                    if cut_from == 0 && cut_to == child_size {
                        nodes.push(child.clone());
                    } else {
                        // Slice the text by char indices
                        let sliced: Arc<str> = text
                            .chars()
                            .skip(cut_from)
                            .take(cut_to - cut_from)
                            .collect::<String>()
                            .into();
                        let mut new_node = (**child).clone();
                        new_node.text = Some(sliced);
                        nodes.push(Arc::new(new_node));
                    }
                } else {
                    nodes.push(child.clone());
                }
            }
            offset += child_size;
            if offset >= to {
                break;
            }
        }
        Fragment::from_nodes(nodes)
    }

    /// Replace children in range `[from..to)` with `replacement`.
    pub fn replace_child_range(&self, from: usize, to: usize, replacement: Fragment) -> Fragment {
        let before = self.cut(0, from);
        let after = self.cut(to, self.size);
        before.append(&replacement).append(&after)
    }
}

impl PartialEq for Fragment {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.children == other.children
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mark::MarkSet;

    const DOC_TYPE: NodeTypeId = NodeTypeId(0);
    const PARA_TYPE: NodeTypeId = NodeTypeId(1);
    const TEXT_TYPE: NodeTypeId = NodeTypeId(2);

    fn text_node(s: &str) -> Arc<Node> {
        Arc::new(Node::text(TEXT_TYPE, s, MarkSet::empty()))
    }

    fn para(children: Vec<Arc<Node>>) -> Arc<Node> {
        Arc::new(Node::new(
            PARA_TYPE,
            Attrs::empty(),
            Fragment::from_nodes(children),
            MarkSet::empty(),
        ))
    }

    #[test]
    fn text_node_size() {
        let n = text_node("hello");
        assert_eq!(n.node_size(), 5);
    }

    #[test]
    fn text_node_size_unicode() {
        // "日本語" is 3 chars
        let n = text_node("日本語");
        assert_eq!(n.node_size(), 3);
    }

    #[test]
    fn branch_node_size() {
        // paragraph containing "hello" (5 chars): size = 5 + 2 = 7
        let p = para(vec![text_node("hello")]);
        assert_eq!(p.node_size(), 7);
    }

    #[test]
    fn fragment_size() {
        let f = Fragment::from_nodes(vec![text_node("ab"), text_node("cd")]);
        assert_eq!(f.size, 4);
    }

    #[test]
    fn fragment_cut_text() {
        let f = Fragment::from_nodes(vec![text_node("hello world")]);
        let cut = f.cut(6, 11);
        assert_eq!(cut.size, 5);
        assert_eq!(cut.child(0).unwrap().text.as_deref(), Some("world"));
    }

    #[test]
    fn fragment_cut_across_children() {
        // Two text nodes: "foo" (3) + "bar" (3) = size 6
        let f = Fragment::from_nodes(vec![text_node("foo"), text_node("bar")]);
        // cut [1..5) → "oo" + "ba"
        let cut = f.cut(1, 5);
        assert_eq!(cut.size, 4);
        assert_eq!(cut.child(0).unwrap().text.as_deref(), Some("oo"));
        assert_eq!(cut.child(1).unwrap().text.as_deref(), Some("ba"));
    }

    #[test]
    fn empty_branch_node_size_is_two() {
        // An empty paragraph (no text, no children, not atom) must have size 2.
        let p = Arc::new(Node::new(
            PARA_TYPE,
            Attrs::empty(),
            Fragment::empty(),
            MarkSet::empty(),
        ));
        assert_eq!(p.node_size(), 2);
        assert!(!p.is_leaf(), "empty branch is NOT a leaf");
    }

    #[test]
    fn atom_node_size_is_one() {
        const HARD_BREAK: NodeTypeId = NodeTypeId(8);
        let hb = Arc::new(Node::atom(HARD_BREAK, Attrs::empty(), MarkSet::empty()));
        assert_eq!(hb.node_size(), 1);
        assert!(hb.is_leaf(), "atom IS a leaf");
    }

    #[test]
    fn doc_size() {
        // doc -> [para("hello")] => para.size=7, doc.size = 7+2 = 9
        let doc = Arc::new(Node::new(
            DOC_TYPE,
            Attrs::empty(),
            Fragment::from_node(para(vec![text_node("hello")])),
            MarkSet::empty(),
        ));
        assert_eq!(doc.node_size(), 9);
    }
}
