use serde::{Deserialize, Serialize};

use super::attrs::Attrs;

/// Interned identifier for a mark type within a `Schema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarkTypeId(pub u16);

/// A mark applied to inline content (e.g., bold, italic, link).
///
/// Marks are compared by `(type_id, attrs)` — two marks are equal iff both match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub type_id: MarkTypeId,
    pub attrs: Attrs,
}

impl Mark {
    pub fn new(type_id: MarkTypeId, attrs: Attrs) -> Self {
        Mark { type_id, attrs }
    }

    pub fn simple(type_id: MarkTypeId) -> Self {
        Mark {
            type_id,
            attrs: Attrs::empty(),
        }
    }

    /// Returns true if the mark sets belong to the same type and attrs.
    pub fn same_mark(&self, other: &Mark) -> bool {
        self == other
    }
}

/// A sorted, deduplicated set of marks on an inline node.
///
/// Invariant: no two marks with the same `type_id` can coexist (marks with
/// the same type_id but different attrs are considered different; callers
/// enforce exclusivity constraints at the schema level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MarkSet(pub Vec<Mark>);

impl MarkSet {
    pub fn empty() -> Self {
        MarkSet(Vec::new())
    }

    pub fn from_marks(mut marks: Vec<Mark>) -> Self {
        marks.sort_by_key(|m| m.type_id.0);
        marks.dedup_by(|a, b| a.type_id == b.type_id && a.attrs == b.attrs);
        MarkSet(marks)
    }

    /// Add a mark, replacing any existing mark of the same `type_id`.
    pub fn add(&self, mark: Mark) -> Self {
        let mut marks: Vec<Mark> = self
            .0
            .iter()
            .filter(|m| m.type_id != mark.type_id)
            .cloned()
            .collect();
        marks.push(mark);
        marks.sort_by_key(|m| m.type_id.0);
        MarkSet(marks)
    }

    /// Remove a mark by `type_id`.
    pub fn remove(&self, type_id: MarkTypeId) -> Self {
        MarkSet(
            self.0
                .iter()
                .filter(|m| m.type_id != type_id)
                .cloned()
                .collect(),
        )
    }

    pub fn contains(&self, type_id: MarkTypeId) -> bool {
        self.0.iter().any(|m| m.type_id == type_id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Mark> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Mark {
        Mark::simple(MarkTypeId(0))
    }
    fn italic() -> Mark {
        Mark::simple(MarkTypeId(1))
    }

    #[test]
    fn add_mark() {
        let set = MarkSet::empty().add(bold());
        assert!(set.contains(MarkTypeId(0)));
        assert!(!set.contains(MarkTypeId(1)));
    }

    #[test]
    fn add_replaces_same_type() {
        use super::super::attrs::AttrValue;
        let m1 = Mark::new(
            MarkTypeId(2),
            Attrs::empty().with("href", AttrValue::String("a".into())),
        );
        let m2 = Mark::new(
            MarkTypeId(2),
            Attrs::empty().with("href", AttrValue::String("b".into())),
        );
        let set = MarkSet::empty().add(m1).add(m2.clone());
        assert_eq!(set.0.len(), 1);
        assert_eq!(&set.0[0], &m2);
    }

    #[test]
    fn remove_mark() {
        let set = MarkSet::empty().add(bold()).add(italic());
        let set2 = set.remove(MarkTypeId(0));
        assert!(!set2.contains(MarkTypeId(0)));
        assert!(set2.contains(MarkTypeId(1)));
    }
}
