use std::collections::BTreeSet;

/// Set of per-replica counters that have been accepted, stored as a
/// contiguous prefix plus a sparse tail so that gaps (op 6 before op 5) and
/// duplicates are handled exactly. Counters start at 1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeenSet {
    contiguous: u64,
    sparse: BTreeSet<u64>,
}

impl SeenSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, counter: u64) -> bool {
        counter != 0 && (counter <= self.contiguous || self.sparse.contains(&counter))
    }

    /// Returns `true` if the counter was not seen before.
    pub fn insert(&mut self, counter: u64) -> bool {
        if self.contains(counter) || counter == 0 {
            return false;
        }
        if counter == self.contiguous + 1 {
            self.contiguous = counter;
            while self.sparse.remove(&(self.contiguous + 1)) {
                self.contiguous += 1;
            }
        } else {
            self.sparse.insert(counter);
        }
        true
    }

    pub fn contiguous(&self) -> u64 {
        self.contiguous
    }

    pub fn sparse_len(&self) -> usize {
        self.sparse.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    proptest! {
        #[test]
        fn matches_a_plain_set(inserts in prop::collection::vec(1u64..200, 0..400)) {
            let mut seen = SeenSet::new();
            let mut model = HashSet::new();
            for c in inserts {
                let new = seen.insert(c);
                prop_assert_eq!(new, model.insert(c));
                prop_assert!(seen.contains(c));
            }
            for c in 1..210u64 {
                prop_assert_eq!(seen.contains(c), model.contains(&c));
            }
            // Everything up to the first gap has been folded into the prefix.
            let first_gap = (1..).find(|c| !model.contains(c)).unwrap();
            prop_assert_eq!(seen.contiguous(), first_gap - 1);
            prop_assert_eq!(seen.sparse_len(), model.iter().filter(|&&c| c >= first_gap).count());
        }
    }
}
