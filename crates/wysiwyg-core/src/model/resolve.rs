use std::sync::Arc;

use super::node::Node;

/// One level in the path from the root to the resolved position.
#[derive(Debug, Clone)]
pub struct PathEntry {
    /// The node at this level.
    pub node: Arc<Node>,
    /// This node's index within its parent's children.
    pub index: usize,
    /// Absolute document offset at the *start* of this node's content
    /// (i.e., after its opening token).
    pub offset: usize,
}

/// A resolved position in the document.
///
/// # Position model (ProseMirror style)
///
/// Positions are integers counted across a linearised view of the document:
/// - Before each block node's opening: +1
/// - Inside a text node: +1 per `char`
/// - After each block node's closing: +1
///
/// `depth` 0 = inside the top-level document node (between its children).
/// `depth` increases with each level of nesting.
///
/// `parent_offset` is the position relative to the start of the deepest parent's
/// content — useful for character-level operations within a text node.
#[derive(Debug, Clone)]
pub struct ResolvedPos {
    /// The absolute position.
    pub pos: usize,
    /// Path from root (index 0) to the deepest node containing the position.
    /// The last entry is the immediate parent of the cursor.
    pub path: Vec<PathEntry>,
    /// The depth of the resolved position.  0 = doc level.
    pub depth: usize,
    /// Offset within the deepest parent's content.
    pub parent_offset: usize,
}

impl ResolvedPos {
    /// Resolve an absolute position `pos` within the document `doc`.
    ///
    /// Returns `None` if `pos` is out of range.
    pub fn resolve(doc: &Arc<Node>, pos: usize) -> Option<Self> {
        // Valid positions are 0..=doc.content.size (inside the doc).
        if pos > doc.content.size {
            return None;
        }
        let mut path: Vec<PathEntry> = Vec::new();
        let mut node = doc.clone();
        let mut remaining = pos;
        let mut absolute_offset = 0usize;

        // Walk from doc root down to the deepest node containing `pos`.
        'outer: loop {
            // `remaining` is the number of position units we still need to
            // traverse within `node.content`.
            let mut child_offset = 0usize;
            for (idx, child) in node.content.children.iter().enumerate() {
                let child_size = child.node_size();
                if remaining <= child_size {
                    // The position is inside or at the boundary of this child.
                    if child.is_text() || child.is_leaf() {
                        // Leaf or text: we've found the deepest node.
                        path.push(PathEntry {
                            node: child.clone(),
                            index: idx,
                            offset: absolute_offset + child_offset,
                        });
                        let parent_offset = child_offset + remaining;
                        return Some(ResolvedPos {
                            pos,
                            depth: path.len(),
                            parent_offset,
                            path,
                        });
                    }
                    // Branch node: descend into it.
                    path.push(PathEntry {
                        node: node.clone(),
                        index: idx,
                        offset: absolute_offset,
                    });
                    // Consume the opening token of the branch.
                    if remaining == 0 {
                        // Position is exactly at the opening of this child.
                        return Some(ResolvedPos {
                            pos,
                            depth: path.len() - 1,
                            parent_offset: child_offset,
                            path,
                        });
                    }
                    remaining -= 1; // opening token
                    absolute_offset += child_offset + 1;
                    node = child.clone();
                    continue 'outer;
                }
                child_offset += child_size;
                remaining -= child_size;
            }
            // Position is after all children — it's at the closing of `node`.
            return Some(ResolvedPos {
                pos,
                depth: path.len(),
                parent_offset: child_offset,
                path,
            });
        }
    }

    /// The immediate parent node (deepest node in the path).
    pub fn parent(&self) -> &Arc<Node> {
        if let Some(entry) = self.path.last() {
            &entry.node
        } else {
            // Should not happen for a valid resolved position, but return a
            // safe reference to satisfy the borrow checker.
            panic!("ResolvedPos has empty path")
        }
    }

    /// The node at the given `depth`.  `depth=0` returns the doc root.
    pub fn node_at_depth<'a>(&'a self, depth: usize, doc: &'a Arc<Node>) -> &'a Arc<Node> {
        if depth == 0 {
            return doc;
        }
        &self.path[depth - 1].node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        attrs::Attrs,
        mark::MarkSet,
        node::{Fragment, Node, NodeTypeId},
    };

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

    fn doc(children: Vec<Arc<Node>>) -> Arc<Node> {
        Arc::new(Node::new(
            DOC_TYPE,
            Attrs::empty(),
            Fragment::from_nodes(children),
            MarkSet::empty(),
        ))
    }

    /// Build: doc -> [para("hello")]
    /// Positions:
    ///   0: before para (opening of para)
    ///   1..6: inside "hello"
    ///   7: after para (closing of para)
    fn simple_doc() -> Arc<Node> {
        doc(vec![para(vec![text_node("hello")])])
    }

    #[test]
    fn resolve_start_of_paragraph() {
        let d = simple_doc();
        // pos=0 is before the paragraph (at doc level, child_offset=0).
        let rp = ResolvedPos::resolve(&d, 0).unwrap();
        assert_eq!(rp.pos, 0);
    }

    #[test]
    fn resolve_inside_text() {
        let d = simple_doc();
        // pos=3 is inside "hello" at char index 2 ('l').
        let rp = ResolvedPos::resolve(&d, 3).unwrap();
        assert_eq!(rp.pos, 3);
    }

    #[test]
    fn resolve_out_of_range() {
        let d = simple_doc();
        // doc content size = para.node_size() = 7, so pos=8 is invalid.
        assert!(ResolvedPos::resolve(&d, 8).is_none());
    }

    #[test]
    fn resolve_end_of_content() {
        let d = simple_doc();
        // pos=7 = doc.content.size (after the closing of para).
        let rp = ResolvedPos::resolve(&d, 7).unwrap();
        assert_eq!(rp.pos, 7);
    }
}
