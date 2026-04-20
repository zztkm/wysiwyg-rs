/// A position map produced by a single step.
///
/// Internally stores a flat array of `(old_pos, old_size, new_size)` triples.
/// Each triple describes a range that was deleted (`old_size`) and the content
/// that was inserted in its place (`new_size`).  The rest of the document is
/// considered unchanged ("unaffected" ranges sit between the triples).
#[derive(Debug, Clone, Default)]
pub struct StepMap {
    /// Flat: [old_pos_0, old_size_0, new_size_0, old_pos_1, ...]
    ranges: Vec<usize>,
}

impl StepMap {
    /// Construct from `(old_pos, old_size, new_size)` triples.
    pub fn from_ranges(ranges: impl IntoIterator<Item = (usize, usize, usize)>) -> Self {
        StepMap {
            ranges: ranges.into_iter().flat_map(|(p, o, n)| [p, o, n]).collect(),
        }
    }

    /// A map that makes no changes (identity).
    pub fn identity() -> Self {
        StepMap::default()
    }

    /// Map an absolute position through this step.
    ///
    /// `bias`:
    /// - `-1` (or any negative) → the position maps to the *left* side of an
    ///   insertion point (i.e., stays before inserted content).
    /// - `1` (or any positive) → maps to the *right* side (i.e., moves past
    ///   inserted content).
    ///
    /// Returns the mapped position in the *new* document.
    pub fn map(&self, pos: usize, bias: i8) -> usize {
        let pos_i = pos as isize;
        // Accumulated shift from ranges we've passed through.
        let mut offset = 0isize;

        let mut i = 0;
        while i < self.ranges.len() {
            let range_start = self.ranges[i] as isize;
            let old_size = self.ranges[i + 1] as isize;
            let new_size = self.ranges[i + 2] as isize;

            if range_start > pos_i {
                // All remaining ranges are after our position — stop.
                break;
            }

            if old_size == 0 && range_start == pos_i {
                // Pure insertion at our position.
                return if bias >= 0 {
                    (range_start + offset + new_size) as usize
                } else {
                    (range_start + offset) as usize
                };
            }

            if range_start + old_size > pos_i {
                // Position is inside the deleted/replaced range.
                return if bias >= 0 {
                    (range_start + offset + new_size) as usize
                } else {
                    (range_start + offset) as usize
                };
            }

            // Position is after this range — accumulate the shift.
            offset += new_size - old_size;
            i += 3;
        }

        (pos_i + offset) as usize
    }

    /// Map a position through this step, returning the mapped position.
    /// Convenience wrapper with right-bias.
    pub fn map_right(&self, pos: usize) -> usize {
        self.map(pos, 1)
    }

    /// Map a position through this step with left-bias.
    pub fn map_left(&self, pos: usize) -> usize {
        self.map(pos, -1)
    }
}

/// A chain of `StepMap`s for mapping positions through multiple steps.
///
/// Also supports "mirror" entries: `(i, j)` means `maps[i]` and `maps[j]` are
/// inverse of each other, enabling smarter mapping for undo/redo.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    maps: Vec<StepMap>,
    /// Mirror pairs: each element is `(forward_idx, backward_idx)`.
    mirror: Vec<(usize, usize)>,
}

impl Mapping {
    pub fn new() -> Self {
        Mapping::default()
    }

    /// Append a `StepMap` to the chain.
    pub fn append_map(&mut self, map: StepMap) {
        self.maps.push(map);
    }

    /// Append a `StepMap` and record that the step at `mirror_of` index is its
    /// inverse (used for undo tracking).
    pub fn append_map_with_mirror(&mut self, map: StepMap, mirror_of: usize) {
        let idx = self.maps.len();
        self.maps.push(map);
        self.mirror.push((mirror_of, idx));
    }

    /// Map a position through the entire chain.
    pub fn map(&self, pos: usize, bias: i8) -> usize {
        let mut p = pos;
        for m in &self.maps {
            p = m.map(p, bias);
        }
        p
    }

    pub fn map_right(&self, pos: usize) -> usize {
        self.map(pos, 1)
    }

    pub fn map_left(&self, pos: usize) -> usize {
        self.map(pos, -1)
    }

    pub fn maps(&self) -> &[StepMap] {
        &self.maps
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_map() {
        let m = StepMap::identity();
        assert_eq!(m.map(5, 1), 5);
        assert_eq!(m.map(0, 1), 0);
    }

    #[test]
    fn insertion_at_position() {
        // Insert 3 chars at position 5 (old_size=0, new_size=3).
        let m = StepMap::from_ranges([(5, 0, 3)]);
        // Positions before insertion are unchanged.
        assert_eq!(m.map(4, 1), 4);
        // Right-bias: position 5 maps to 8 (after the insertion).
        assert_eq!(m.map(5, 1), 8);
        // Left-bias: position 5 maps to 5 (before the insertion).
        assert_eq!(m.map(5, -1), 5);
        // Position after insertion shifts right.
        assert_eq!(m.map(6, 1), 9);
    }

    #[test]
    fn deletion() {
        // Delete 3 chars at position 2 (old_size=3, new_size=0).
        let m = StepMap::from_ranges([(2, 3, 0)]);
        // Position before deletion: unchanged.
        assert_eq!(m.map(1, 1), 1);
        // Position within deleted range: maps to start of deletion.
        assert_eq!(m.map(3, 1), 2);
        // Position after deletion: shifts left by 3.
        assert_eq!(m.map(5, 1), 2);
        assert_eq!(m.map(6, 1), 3);
    }

    #[test]
    fn replacement() {
        // Replace 2 chars at position 3 with 4 chars.
        let m = StepMap::from_ranges([(3, 2, 4)]);
        assert_eq!(m.map(2, 1), 2);
        assert_eq!(m.map(5, 1), 7); // after the changed region, shift +2
    }

    #[test]
    fn mapping_chain() {
        // First step: insert 2 at pos 5.
        // Second step: insert 1 at pos 8 (in new coords).
        let mut mapping = Mapping::new();
        mapping.append_map(StepMap::from_ranges([(5, 0, 2)]));
        mapping.append_map(StepMap::from_ranges([(8, 0, 1)]));
        // pos=4 → unchanged by both steps → 4
        assert_eq!(mapping.map(4, 1), 4);
        // pos=5 right-bias → 7 after step1 (past the 2 inserted chars).
        // In step2, pos=7 < 8 (insertion point), so no shift → final = 7.
        assert_eq!(mapping.map(5, 1), 7);
    }
}
