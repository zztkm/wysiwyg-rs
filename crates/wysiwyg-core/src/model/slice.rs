use serde::{Deserialize, Serialize};

use super::node::Fragment;

/// A slice of a document — used for replace operations and clipboard.
///
/// `open_start` and `open_end` indicate how many levels of nesting are "open"
/// (i.e., the slice starts/ends in the middle of a node at that depth).
///
/// For example, cutting `<p>hel|lo</p><p>wor|ld</p>` gives:
///   content: Fragment[text("lo"), text("wor")]
///   open_start: 1   (the first paragraph is still open at the cut point)
///   open_end:   1   (the second paragraph is still open at the cut point)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub content: Fragment,
    /// How many levels of the document tree are open at the start.
    pub open_start: usize,
    /// How many levels of the document tree are open at the end.
    pub open_end: usize,
}

impl Slice {
    pub fn new(content: Fragment, open_start: usize, open_end: usize) -> Self {
        Slice {
            content,
            open_start,
            open_end,
        }
    }

    /// A completely empty slice (no content, not open on either side).
    pub fn empty() -> Self {
        Slice {
            content: Fragment::empty(),
            open_start: 0,
            open_end: 0,
        }
    }

    /// The size of this slice's content in logical position units.
    pub fn size(&self) -> usize {
        self.content.size
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{mark::MarkSet, node::Node, node::NodeTypeId};
    use std::sync::Arc;

    const TEXT_TYPE: NodeTypeId = NodeTypeId(2);

    fn text_node(s: &str) -> Arc<Node> {
        Arc::new(Node::text(TEXT_TYPE, s, MarkSet::empty()))
    }

    #[test]
    fn empty_slice() {
        let s = Slice::empty();
        assert!(s.is_empty());
        assert_eq!(s.size(), 0);
    }

    #[test]
    fn slice_size() {
        let s = Slice::new(Fragment::from_node(text_node("hello")), 0, 0);
        assert_eq!(s.size(), 5);
    }
}
