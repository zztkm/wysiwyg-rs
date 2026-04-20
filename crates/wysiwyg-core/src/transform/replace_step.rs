use std::sync::Arc;

use crate::model::{
    node::{Fragment, Node},
    slice::Slice,
};

use super::{
    step::{Step, StepError, StepResult},
    step_map::{Mapping, StepMap},
};

/// Replace the range `[from..to)` in the document with `slice`.
///
/// This is the fundamental step — all text editing operations can be expressed
/// as `ReplaceStep`s.
///
/// # Position model
/// - `from` and `to` are absolute positions within `doc.content` (not counting
///   the doc node's own opening/closing tokens).
/// - For a closed slice (`open_start == 0`, `open_end == 0`), the replacement
///   is straightforward: content before `from`, then `slice.content`, then
///   content from `to` onwards.
/// - For open slices, the first/last nodes of the slice are merged with the
///   content at the cut boundaries (enabling "join paragraph" operations).
///   **Phase 1 implements closed slices; open boundary merging is Phase 2.**
#[derive(Debug, Clone)]
pub struct ReplaceStep {
    pub from: usize,
    pub to: usize,
    pub slice: Slice,
}

impl ReplaceStep {
    pub fn new(from: usize, to: usize, slice: Slice) -> Self {
        ReplaceStep { from, to, slice }
    }

    /// Returns the `StepMap` this step produces without applying it.
    pub fn get_map(&self) -> StepMap {
        StepMap::from_ranges([(self.from, self.to - self.from, self.slice.size())])
    }

    /// Apply the step to `doc`.
    pub fn apply(&self, doc: &Arc<Node>) -> StepResult {
        if self.from > self.to {
            return Err(StepError::InvalidRange {
                from: self.from,
                to: self.to,
            });
        }
        if self.to > doc.content.size {
            return Err(StepError::InvalidPosition(self.to));
        }

        let new_content = replace_in_fragment(&doc.content, self.from, self.to, &self.slice)?;
        let new_doc = Arc::new({
            let mut n = (**doc).clone();
            n.content = new_content;
            n
        });

        let map = StepMap::from_ranges([(self.from, self.to - self.from, self.slice.size())]);
        Ok((new_doc, map))
    }

    /// Produce the inverse of this step.
    ///
    /// `doc` must be the document **before** the step is applied.
    pub fn invert(&self, doc: &Arc<Node>) -> Step {
        // Extract the content that will be removed.
        let old_content = extract_fragment(&doc.content, self.from, self.to);
        let inv_slice = Slice::new(old_content, self.slice.open_start, self.slice.open_end);
        Step::Replace(ReplaceStep {
            from: self.from,
            to: self.from + self.slice.size(),
            slice: inv_slice,
        })
    }

    /// Map this step through a `Mapping`.
    pub fn map(&self, mapping: &Mapping) -> Option<Step> {
        let from = mapping.map_left(self.from);
        let to = mapping.map_right(self.to);
        if from > to {
            return None;
        }
        Some(Step::Replace(ReplaceStep {
            from,
            to,
            slice: self.slice.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Core replacement algorithm
// ---------------------------------------------------------------------------

/// Replace `[from..to)` in `fragment` with `slice.content`.
///
/// Positions `from` and `to` are relative to the start of `fragment`.
///
/// The algorithm:
///  1. Try to find a single branch child that fully contains both `from` and `to`,
///     and recurse into it.
///  2. If no such child exists (the range spans multiple children, or touches
///     text nodes at this level), perform a flat replacement.
fn replace_in_fragment(
    fragment: &Fragment,
    from: usize,
    to: usize,
    slice: &Slice,
) -> Result<Fragment, StepError> {
    let mut offset = 0usize;

    // Pass 1: look for a single branch child that contains both endpoints.
    for (i, child) in fragment.children.iter().enumerate() {
        let child_size = child.node_size();
        let child_start = offset;

        if !child.is_text() && !child.is_leaf() {
            // Content of this branch node lives at [child_start+1 .. child_start+1+content_size].
            let inner_start = child_start + 1;
            let inner_end = inner_start + child.content.size;

            if from >= inner_start && to <= inner_end {
                // Both endpoints are inside this child's content.
                let inner_from = from - inner_start;
                let inner_to = to - inner_start;
                let new_child_content =
                    replace_in_fragment(&child.content, inner_from, inner_to, slice)?;
                let new_child = Arc::new({
                    let mut n = (**child).clone();
                    n.content = new_child_content;
                    n
                });
                let mut children: Vec<Arc<Node>> = fragment.children.iter().cloned().collect();
                children[i] = new_child;
                return Ok(Fragment::from_nodes(children));
            }
        }

        offset += child_size;
    }

    // Pass 2: flat replacement at this level.
    flat_replace(fragment, from, to, slice)
}

/// Perform a flat replacement of `[from..to)` within `fragment`.
///
/// - Content before `from` is kept.
/// - `slice.content` is inserted.
/// - Content from `to` onwards is kept.
///
/// # Open boundaries (Phase 1 limitation)
///
/// When `slice.open_start > 0`, the first node in the slice should be merged
/// with the left boundary node (i.e. the node that contains position `from`).
/// Similarly for `open_end > 0` and the right boundary.
///
/// Phase 1 implements this for the simple "single-level" merge case only.
/// Full depth-n merging is Phase 2.
fn flat_replace(
    fragment: &Fragment,
    from: usize,
    to: usize,
    slice: &Slice,
) -> Result<Fragment, StepError> {
    // Split the fragment at the cut points.
    let (before, after) = split_at_range(fragment, from, to)?;

    if slice.open_start == 0 && slice.open_end == 0 {
        // Closed slice: simple concatenation.
        let result = before.append(&slice.content).append(&after);
        return Ok(result);
    }

    // Open slice: merge boundary nodes.
    // Extract the "inner" slice nodes (excluding the boundary-merged ones).
    merge_open_slice(before, slice, after)
}

/// Merge an open slice with the before/after fragments.
///
/// For `open_start == 1` and `open_end == 1` (the most common case, e.g.
/// merging two paragraphs):
///
/// - The *last* node of `before` contributes its content to the *first* node
///   of the slice.
/// - The *first* node of `after` contributes its content to the *last* node
///   of the slice.
///
/// Phase 1 handles depth-1 open boundaries only.
fn merge_open_slice(
    before: Fragment,
    slice: &Slice,
    after: Fragment,
) -> Result<Fragment, StepError> {
    let open_start = slice.open_start;
    let open_end = slice.open_end;

    if open_start > 1 || open_end > 1 {
        // TODO (Phase 2): implement deep open boundary merging.
        return Err(StepError::InvalidContent(
            "Open boundary depth > 1 is not yet supported".into(),
        ));
    }

    // Depth-1 merge: merge into the innermost block layer.
    let slice_children: Vec<Arc<Node>> = slice.content.children.iter().cloned().collect();
    if slice_children.is_empty() {
        // Empty slice with open boundaries: merge before's last child content
        // with after's first child content.
        return merge_boundary_nodes(before, after);
    }

    let mut result_children: Vec<Arc<Node>> = Vec::new();

    // Before nodes (excluding the last one which will be merged).
    let before_children: Vec<Arc<Node>> = before.children.iter().cloned().collect();
    if open_start > 0 {
        let before_body = &before_children[..before_children.len().saturating_sub(1)];
        result_children.extend_from_slice(before_body);
    } else {
        result_children.extend_from_slice(&before_children);
    }

    // Slice content, with boundary merging.
    let first_slice = &slice_children[0];
    let last_slice = &slice_children[slice_children.len() - 1];

    let merged_first = if open_start > 0 {
        if let Some(before_last) = before_children.last() {
            // Prepend before_last's content to first_slice's content.
            let merged_content = before_last.content.append(&first_slice.content);
            Arc::new({
                let mut n = (**first_slice).clone();
                n.content = merged_content;
                n
            })
        } else {
            first_slice.clone()
        }
    } else {
        first_slice.clone()
    };

    if slice_children.len() == 1 {
        // Single slice node: merge both boundaries into it.
        let merged = if open_end > 0 {
            if let Some(after_first) = after.children.first() {
                let merged_content = merged_first.content.append(&after_first.content);
                Arc::new({
                    let mut n = (*merged_first).clone();
                    n.content = merged_content;
                    n
                })
            } else {
                merged_first
            }
        } else {
            merged_first
        };
        result_children.push(merged);
    } else {
        // Multiple slice nodes.
        result_children.push(merged_first);
        for s in &slice_children[1..slice_children.len() - 1] {
            result_children.push(s.clone());
        }
        let merged_last = if open_end > 0 {
            if let Some(after_first) = after.children.first() {
                let merged_content = last_slice.content.append(&after_first.content);
                Arc::new({
                    let mut n = (**last_slice).clone();
                    n.content = merged_content;
                    n
                })
            } else {
                last_slice.clone()
            }
        } else {
            last_slice.clone()
        };
        result_children.push(merged_last);
    }

    // After nodes (excluding the first one which was merged).
    let after_children: Vec<Arc<Node>> = after.children.iter().cloned().collect();
    if open_end > 0 {
        if after_children.len() > 1 {
            result_children.extend_from_slice(&after_children[1..]);
        }
    } else {
        result_children.extend_from_slice(&after_children);
    }

    Ok(Fragment::from_nodes(result_children))
}

/// Merge the last node of `before` with the first node of `after`.
/// Used when the slice is empty but has open boundaries (e.g. joining two blocks).
fn merge_boundary_nodes(before: Fragment, after: Fragment) -> Result<Fragment, StepError> {
    let mut result: Vec<Arc<Node>> = Vec::new();
    let before_children: Vec<Arc<Node>> = before.children.iter().cloned().collect();
    let after_children: Vec<Arc<Node>> = after.children.iter().cloned().collect();

    if before_children.is_empty() && after_children.is_empty() {
        return Ok(Fragment::empty());
    }

    result.extend_from_slice(&before_children[..before_children.len().saturating_sub(1)]);

    match (before_children.last(), after_children.first()) {
        (Some(b), Some(a)) => {
            let merged_content = b.content.append(&a.content);
            let merged = Arc::new({
                let mut n = (**b).clone();
                n.content = merged_content;
                n
            });
            result.push(merged);
            if after_children.len() > 1 {
                result.extend_from_slice(&after_children[1..]);
            }
        }
        (Some(b), None) => result.push(b.clone()),
        (None, Some(a)) => result.push(a.clone()),
        (None, None) => {}
    }

    Ok(Fragment::from_nodes(result))
}

/// Split `fragment` at `from` and `to`, returning `(before, after)`.
///
/// `before` = everything in the fragment before position `from`.
/// `after`  = everything in the fragment from position `to` onwards.
///
/// For branch nodes that are partially in the range:
/// - The node is split, with its left part going into `before` and its right
///   part going into `after` (these become the "open" nodes that open_start
///   and open_end refer to).
///
/// For text nodes that are partially in the range:
/// - The text is sliced at the character level.
fn split_at_range(
    fragment: &Fragment,
    from: usize,
    to: usize,
) -> Result<(Fragment, Fragment), StepError> {
    let mut before: Vec<Arc<Node>> = Vec::new();
    let mut after: Vec<Arc<Node>> = Vec::new();
    let mut offset = 0usize;

    for child in fragment.children.iter() {
        let child_size = child.node_size();
        let child_start = offset;
        let child_end = offset + child_size;

        if child_end <= from {
            before.push(child.clone());
        } else if child_start >= to {
            after.push(child.clone());
        } else if child.is_text() {
            // Text node partially overlaps.
            let text = child.text.as_ref().unwrap();
            let chars: Vec<char> = text.chars().collect();
            let local_from = from.saturating_sub(child_start);
            let local_to = to.saturating_sub(child_start).min(chars.len());

            if local_from > 0 {
                let s: Arc<str> = chars[..local_from].iter().collect::<String>().into();
                let mut n = (**child).clone();
                n.text = Some(s);
                before.push(Arc::new(n));
            }
            if local_to < chars.len() {
                let s: Arc<str> = chars[local_to..].iter().collect::<String>().into();
                let mut n = (**child).clone();
                n.text = Some(s);
                after.push(Arc::new(n));
            }
        } else {
            // Branch node partially overlaps the range.
            let inner_start = child_start + 1;
            let inner_end = inner_start + child.content.size;

            if from > child_start && from <= inner_end {
                // `from` is inside this child — left part goes into `before`.
                let inner_cut = from - inner_start;
                let left_content = child.content.cut(0, inner_cut);
                let mut n = (**child).clone();
                n.content = left_content;
                before.push(Arc::new(n));
            }

            if to >= inner_start && to < child_end {
                // `to` is strictly inside this child — right part goes into `after`.
                // When to == child_end the node is completely consumed by the range.
                let inner_cut = to.saturating_sub(inner_start);
                let right_content = child.content.cut(inner_cut, child.content.size);
                let mut n = (**child).clone();
                n.content = right_content;
                after.push(Arc::new(n));
            }
        }

        offset = child_end;
    }

    Ok((Fragment::from_nodes(before), Fragment::from_nodes(after)))
}

/// Extract the content of `fragment` in the range `[from..to)`.
///
/// Returns a `Fragment` representing what was (or will be) at that range.
/// Used by `invert` to capture the original content.
fn extract_fragment(fragment: &Fragment, from: usize, to: usize) -> Fragment {
    if from == to {
        return Fragment::empty();
    }

    let mut offset = 0usize;

    // First try to recurse into a single child.
    for child in fragment.children.iter() {
        let child_size = child.node_size();
        let child_start = offset;

        if !child.is_text() && !child.is_leaf() {
            let inner_start = child_start + 1;
            let inner_end = inner_start + child.content.size;

            if from >= inner_start && to <= inner_end {
                return extract_fragment(&child.content, from - inner_start, to - inner_start);
            }
        }

        offset += child_size;
    }

    // Fall back to flat extraction.
    fragment.cut(from, to)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    /// Helper: collect all text from a doc recursively.
    fn collect_text(node: &Arc<Node>) -> String {
        if let Some(t) = &node.text {
            return t.to_string();
        }
        node.content.children.iter().map(collect_text).collect()
    }

    /// doc -> [para("hello world")]
    /// doc.content.size = 13 (para.node_size = 11+2 = 13)
    fn simple_doc() -> Arc<Node> {
        doc(vec![para(vec![text_node("hello world")])])
    }

    #[test]
    fn insert_text_at_end_of_paragraph() {
        // para("hello world"), insert "!" at position 12 (after the last char, inside para).
        // Positions inside doc:
        //   0: before para opening
        //   1..12: "hello world" (11 chars at positions 1..=11)
        //   12: after last char (before para closing)
        //   13: after para
        let d = simple_doc();
        assert_eq!(d.content.size, 13);

        let step = ReplaceStep::new(
            12,
            12,
            Slice::new(Fragment::from_node(text_node("!")), 0, 0),
        );
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), "hello world!");
    }

    #[test]
    fn delete_text_range() {
        // Delete " world" (positions 6..12 inside the para content).
        // Absolute positions: para opens at 0, content at 1..12.
        // " world" = chars 5..11 of "hello world", absolute positions 6..12.
        let d = simple_doc();
        let step = ReplaceStep::new(6, 12, Slice::empty());
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), "hello");
    }

    #[test]
    fn replace_text_range() {
        // Replace "world" with "Rust" (positions 7..12 are "world").
        let d = simple_doc();
        let step = ReplaceStep::new(
            7,
            12,
            Slice::new(Fragment::from_node(text_node("Rust")), 0, 0),
        );
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), "hello Rust");
    }

    #[test]
    fn insert_at_beginning() {
        let d = simple_doc();
        // Insert ">> " at position 1 (start of paragraph content).
        let step = ReplaceStep::new(
            1,
            1,
            Slice::new(Fragment::from_node(text_node(">> ")), 0, 0),
        );
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), ">> hello world");
    }

    #[test]
    fn invert_delete() {
        let d = simple_doc();
        // Delete "hello" (positions 1..6 in the para's content → absolute 1..6).
        let step = ReplaceStep::new(1, 6, Slice::empty());
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), " world");

        // Invert: should restore "hello".
        let inv = step.invert(&d);
        let (restored, _) = inv.apply(&new_doc).unwrap();
        assert_eq!(collect_text(&restored), "hello world");
    }

    #[test]
    fn invert_insert() {
        let d = simple_doc();
        let step = ReplaceStep::new(
            12,
            12,
            Slice::new(Fragment::from_node(text_node("!!!")), 0, 0),
        );
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), "hello world!!!");

        let inv = step.invert(&d);
        let (restored, _) = inv.apply(&new_doc).unwrap();
        assert_eq!(collect_text(&restored), "hello world");
    }

    #[test]
    fn step_map_tracks_insertion() {
        let d = simple_doc();
        // Insert 3 chars at pos 6.
        let step = ReplaceStep::new(
            6,
            6,
            Slice::new(Fragment::from_node(text_node("XYZ")), 0, 0),
        );
        let (_, map) = step.apply(&d).unwrap();
        // Positions before insertion are unchanged.
        assert_eq!(map.map_right(5), 5);
        // Right-bias at insertion point moves past the inserted text.
        assert_eq!(map.map_right(6), 9);
        // Left-bias stays before.
        assert_eq!(map.map_left(6), 6);
        // Positions after the insertion shift right by 3.
        assert_eq!(map.map_right(7), 10);
    }

    #[test]
    fn invalid_range_returns_error() {
        let d = simple_doc();
        let step = ReplaceStep::new(5, 3, Slice::empty()); // from > to
        assert!(step.apply(&d).is_err());
    }

    #[test]
    fn out_of_bounds_returns_error() {
        let d = simple_doc();
        let step = ReplaceStep::new(0, 100, Slice::empty()); // to > content size
        assert!(step.apply(&d).is_err());
    }

    #[test]
    fn insert_block_at_doc_level() {
        // Insert a new paragraph after the existing one.
        // doc.content.size = 13, so position 13 is the end.
        let d = simple_doc();
        let new_para = para(vec![text_node("second")]);
        let step = ReplaceStep::new(13, 13, Slice::new(Fragment::from_node(new_para), 0, 0));
        let (new_doc, _map) = step.apply(&d).unwrap();
        assert_eq!(collect_text(&new_doc), "hello worldsecond");
        assert_eq!(new_doc.child_count(), 2);
    }

    #[test]
    fn merge_paragraphs_with_open_slice() {
        // doc -> [para("hello"), para("world")]
        // Join the two paragraphs by replacing from end of first content to
        // start of second content, with an open-start, open-end empty slice.
        // Positions:
        //   para1 occupies 0..7 (node_size = 5+2 = 7)
        //     content at 1..6
        //   para2 occupies 7..14
        //     content at 8..13
        // To merge: from=6 (end of para1 content), to=8 (start of para2 content)
        // Slice: empty, open_start=1, open_end=1
        let d = doc(vec![
            para(vec![text_node("hello")]),
            para(vec![text_node("world")]),
        ]);
        assert_eq!(d.content.size, 14);

        let step = ReplaceStep::new(6, 8, Slice::new(Fragment::empty(), 1, 1));
        let (new_doc, _) = step.apply(&d).unwrap();
        assert_eq!(new_doc.child_count(), 1);
        assert_eq!(collect_text(&new_doc), "helloworld");
    }
}
