use std::sync::Arc;

use crate::model::{
    mark::Mark,
    node::{Fragment, Node},
    slice::Slice,
};

use super::{
    mark_step::{AddMarkStep, RemoveMarkStep},
    replace_step::ReplaceStep,
    step::{Step, StepError},
    step_map::Mapping,
};

/// Accumulates a sequence of `Step`s applied to a document.
///
/// Each `step()` call applies the step immediately, updating the current
/// document and the accumulated `Mapping`.  The original document is preserved
/// so that each step can be inverted.
pub struct Transform {
    /// The original document (before any steps).
    pub doc_before: Arc<Node>,
    /// The current document (after all steps so far).
    pub doc: Arc<Node>,
    /// Steps applied so far.
    pub steps: Vec<Step>,
    /// Accumulated position mapping across all steps.
    pub mapping: Mapping,
}

impl Transform {
    /// Start a new transform on `doc`.
    pub fn new(doc: Arc<Node>) -> Self {
        Transform {
            doc_before: doc.clone(),
            doc,
            steps: Vec::new(),
            mapping: Mapping::new(),
        }
    }

    /// Apply a step, updating the document and mapping.
    ///
    /// Returns an error (and leaves state unchanged) if the step cannot be applied.
    pub fn step(&mut self, step: Step) -> Result<&mut Self, StepError> {
        let (new_doc, step_map) = step.apply(&self.doc)?;
        self.steps.push(step);
        self.mapping.append_map(step_map);
        self.doc = new_doc;
        Ok(self)
    }

    // -----------------------------------------------------------------------
    // Convenience methods
    // -----------------------------------------------------------------------

    /// Replace the range `[from..to)` with `slice`.
    pub fn replace(
        &mut self,
        from: usize,
        to: usize,
        slice: Slice,
    ) -> Result<&mut Self, StepError> {
        self.step(Step::Replace(ReplaceStep::new(from, to, slice)))
    }

    /// Insert `content` at `pos` (equivalent to `replace(pos, pos, Slice::closed(content))`).
    pub fn insert(&mut self, pos: usize, content: Fragment) -> Result<&mut Self, StepError> {
        self.replace(pos, pos, Slice::new(content, 0, 0))
    }

    /// Delete the range `[from..to)`.
    pub fn delete(&mut self, from: usize, to: usize) -> Result<&mut Self, StepError> {
        self.replace(from, to, Slice::empty())
    }

    /// Add `mark` to all inline content in `[from..to)`.
    pub fn add_mark(&mut self, from: usize, to: usize, mark: Mark) -> Result<&mut Self, StepError> {
        self.step(Step::AddMark(AddMarkStep::new(from, to, mark)))
    }

    /// Remove `mark` from all inline content in `[from..to)`.
    pub fn remove_mark(
        &mut self,
        from: usize,
        to: usize,
        mark: Mark,
    ) -> Result<&mut Self, StepError> {
        self.step(Step::RemoveMark(RemoveMarkStep::new(from, to, mark)))
    }

    /// Whether the transform has changed the document.
    pub fn doc_changed(&self) -> bool {
        !self.steps.is_empty()
    }

    /// 全ステップが最終ドキュメントに与えた変更の境界ボックスを返す。
    ///
    /// 各ステップが生成したポスト適用範囲を後続ステップのマッピングで最終ドキュメント座標へ
    /// 変換し、全ステップの最小 from・最大 to を返す。
    /// ステップがない場合は `None`。
    pub fn changed_range(&self) -> Option<(usize, usize)> {
        if self.steps.is_empty() {
            return None;
        }
        let maps = self.mapping.maps();
        let mut min_from = usize::MAX;
        let mut max_to = 0usize;

        for (i, step) in self.steps.iter().enumerate() {
            let (from, to) = match step {
                Step::Replace(rs) => (rs.from, rs.from + rs.slice.size()),
                Step::AddMark(s) => (s.from, s.to),
                Step::RemoveMark(s) => (s.from, s.to),
                Step::ReplaceAround(s) => (s.from, s.to),
            };
            let mut mapped_from = from;
            let mut mapped_to = to;
            for map in &maps[i + 1..] {
                mapped_from = map.map_left(mapped_from);
                mapped_to = map.map_right(mapped_to);
            }
            if mapped_from < min_from {
                min_from = mapped_from;
            }
            if mapped_to > max_to {
                max_to = mapped_to;
            }
        }

        Some((min_from, max_to))
    }

    /// The accumulated `Mapping`.
    pub fn mapping(&self) -> &Mapping {
        &self.mapping
    }

    /// Map a position in the original document to the current document using
    /// right-bias.
    pub fn map_right(&self, pos: usize) -> usize {
        self.mapping.map_right(pos)
    }
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
        node::{Node, NodeTypeId},
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

    fn collect_text(node: &Arc<Node>) -> String {
        if let Some(t) = &node.text {
            return t.to_string();
        }
        node.content.children.iter().map(collect_text).collect()
    }

    fn simple_doc() -> Arc<Node> {
        doc(vec![para(vec![text_node("hello")])])
    }

    #[test]
    fn insert_and_delete() {
        let d = simple_doc();
        let mut tr = Transform::new(d);

        // Insert " world" at pos 6 (after "hello" inside the para).
        tr.insert(6, Fragment::from_node(text_node(" world")))
            .unwrap();
        assert_eq!(collect_text(&tr.doc), "hello world");

        // Delete the first word "hello " (now at positions 1..7).
        tr.delete(1, 7).unwrap();
        assert_eq!(collect_text(&tr.doc), "world");
    }

    #[test]
    fn two_step_mapping() {
        let d = simple_doc(); // "hello", positions 1..6 = content
        let mut tr = Transform::new(d);

        // Step 1: Insert "XX" at pos 1 → "XXhello"
        tr.insert(1, Fragment::from_node(text_node("XX"))).unwrap();
        // Step 2: Insert "YY" at pos 9 (old pos 7, mapped right = 9)
        tr.insert(9, Fragment::from_node(text_node("YY"))).unwrap();
        assert_eq!(collect_text(&tr.doc), "XXhelloYY");
    }

    #[test]
    fn replace_convenience() {
        let d = simple_doc();
        let mut tr = Transform::new(d);
        tr.replace(
            1,
            6,
            Slice::new(Fragment::from_node(text_node("world")), 0, 0),
        )
        .unwrap();
        assert_eq!(collect_text(&tr.doc), "world");
    }

    #[test]
    fn doc_changed_flag() {
        let d = simple_doc();
        let tr = Transform::new(d.clone());
        assert!(!tr.doc_changed());

        let mut tr2 = Transform::new(d);
        tr2.delete(1, 3).unwrap();
        assert!(tr2.doc_changed());
    }

    #[test]
    fn invalid_step_does_not_change_state() {
        let d = simple_doc();
        let mut tr = Transform::new(d.clone());
        let result = tr.delete(5, 2); // invalid: from > to
        assert!(result.is_err());
        // Document and step list are unchanged.
        assert_eq!(tr.doc, d);
        assert!(!tr.doc_changed());
    }

    #[test]
    fn changed_range_single_insert() {
        let d = simple_doc(); // "hello", content at 1..6
        let mut tr = Transform::new(d);
        tr.insert(3, Fragment::from_node(text_node("XX"))).unwrap();
        // 位置 3 に "XX"(size=2) を挿入 → 出力範囲は (3, 5)
        assert_eq!(tr.changed_range(), Some((3, 5)));
    }

    #[test]
    fn changed_range_delete() {
        let d = simple_doc();
        let mut tr = Transform::new(d);
        tr.delete(2, 4).unwrap();
        // 削除 → 出力は空点 (2, 2)
        assert_eq!(tr.changed_range(), Some((2, 2)));
    }

    #[test]
    fn changed_range_two_steps_bounding_box() {
        let d = simple_doc();
        let mut tr = Transform::new(d);
        // Step 0: insert "AB" at pos 1 → 最終ドキュメントで (1, 3)
        tr.insert(1, Fragment::from_node(text_node("AB"))).unwrap();
        // Step 1: insert "CD" at pos 9 → 最終ドキュメントで (9, 11)
        tr.insert(9, Fragment::from_node(text_node("CD"))).unwrap();
        // bounding box は (1, 11)
        let (from, to) = tr.changed_range().unwrap();
        assert_eq!(from, 1);
        assert_eq!(to, 11);
    }

    #[test]
    fn changed_range_none_when_no_steps() {
        let d = simple_doc();
        let tr = Transform::new(d);
        assert_eq!(tr.changed_range(), None);
    }

    #[test]
    fn position_mapped_through_steps() {
        let d = simple_doc(); // "hello", content at 1..6
        let mut tr = Transform::new(d);
        // Insert 3 chars at position 3 → "heXXXllo"
        tr.insert(3, Fragment::from_node(text_node("XXX"))).unwrap();
        // Original position 4 should map to 7 (shifted right by 3).
        assert_eq!(tr.map_right(4), 7);
    }
}
