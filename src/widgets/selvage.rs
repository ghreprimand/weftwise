//! Persistent Selvage component boundary.
//!
//! Selvage marks are retained across renders rather than destroyed and rebuilt
//! every frame. [`diff_marks`] is the pure, GTK-free core of that reconciliation:
//! given the keys of the previously retained marks and the keys of the next
//! projection, it produces an ordered plan of [`MarkOp`] operations that reuse
//! matching widgets in place, append only genuinely new marks, remove only
//! vanished ones, and reorder only when the order changed. Keeping widgets alive
//! avoids recreating tooltips and accessible objects on every render, which also
//! removes a class of widget-lifetime churn during rapid state updates.

/// One reconciliation operation applied to the retained mark widgets.
///
/// The renderer holds a `Vec` of retained `(key, widget)` marks in display
/// order. It applies every [`MarkOp::Remove`] first, then walks the
/// [`MarkOp::Reuse`] and [`MarkOp::Create`] operations in `next` order to build
/// the new retained list, reusing existing widgets where a key matched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkOp {
    /// The retained widget at this index in the previous list has no match in
    /// the next projection and must be unparented and dropped.
    Remove {
        /// Index into the previous retained list.
        prev: usize,
    },
    /// Reuse the retained widget at `prev`, update it from `next`, and place it
    /// at slot `next` in the new order.
    Reuse {
        /// Index into the previous retained list.
        prev: usize,
        /// Index into the next projection (its final display slot).
        next: usize,
    },
    /// Build a new widget for the projection at slot `next`.
    Create {
        /// Index into the next projection (its final display slot).
        next: usize,
    },
}

/// Diff two key sequences into an ordered reconciliation plan.
///
/// Keys identify a mark's stable identity (a workspace id, or a stable region
/// slot for status marks). Each next key reuses at most one previous widget with
/// the same key, matched left-to-right so duplicate keys degrade to positional
/// matching. Removals are emitted first (ascending previous index), then the
/// placement operations follow in `next` order.
#[must_use]
pub fn diff_marks<K: Eq>(prev: &[K], next: &[K]) -> Vec<MarkOp> {
    let mut consumed = vec![false; prev.len()];
    let mut placement: Vec<MarkOp> = Vec::with_capacity(next.len());
    for (n, key) in next.iter().enumerate() {
        let matched = prev
            .iter()
            .enumerate()
            .find(|&(i, candidate)| !consumed[i] && candidate == key)
            .map(|(i, _)| i);
        match matched {
            Some(prev_index) => {
                consumed[prev_index] = true;
                placement.push(MarkOp::Reuse {
                    prev: prev_index,
                    next: n,
                });
            }
            None => placement.push(MarkOp::Create { next: n }),
        }
    }

    let mut ops: Vec<MarkOp> = consumed
        .iter()
        .enumerate()
        .filter_map(|(i, done)| (!done).then_some(MarkOp::Remove { prev: i }))
        .collect();
    ops.extend(placement);
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_keys_reuse_every_widget_in_place() {
        let ops = diff_marks(&[1, 2, 3], &[1, 2, 3]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Reuse { prev: 0, next: 0 },
                MarkOp::Reuse { prev: 1, next: 1 },
                MarkOp::Reuse { prev: 2, next: 2 },
            ]
        );
    }

    #[test]
    fn an_appended_key_reuses_the_rest_and_creates_the_new_mark() {
        let ops = diff_marks(&[1, 2], &[1, 2, 3]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Reuse { prev: 0, next: 0 },
                MarkOp::Reuse { prev: 1, next: 1 },
                MarkOp::Create { next: 2 },
            ]
        );
    }

    #[test]
    fn a_vanished_middle_key_is_removed_and_the_rest_reused() {
        let ops = diff_marks(&[1, 2, 3], &[1, 3]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Remove { prev: 1 },
                MarkOp::Reuse { prev: 0, next: 0 },
                MarkOp::Reuse { prev: 2, next: 1 },
            ]
        );
    }

    #[test]
    fn a_reordered_sequence_reuses_widgets_at_new_slots() {
        let ops = diff_marks(&[1, 2, 3], &[3, 1, 2]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Reuse { prev: 2, next: 0 },
                MarkOp::Reuse { prev: 0, next: 1 },
                MarkOp::Reuse { prev: 1, next: 2 },
            ]
        );
    }

    #[test]
    fn a_disjoint_projection_removes_all_and_creates_all() {
        let ops = diff_marks(&[1, 2], &[3, 4]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Remove { prev: 0 },
                MarkOp::Remove { prev: 1 },
                MarkOp::Create { next: 0 },
                MarkOp::Create { next: 1 },
            ]
        );
    }

    #[test]
    fn empty_next_removes_every_previous_widget() {
        let ops = diff_marks(&[1, 2, 3], &[] as &[i32]);
        assert_eq!(
            ops,
            vec![
                MarkOp::Remove { prev: 0 },
                MarkOp::Remove { prev: 1 },
                MarkOp::Remove { prev: 2 },
            ]
        );
    }

    #[test]
    fn empty_prev_creates_every_next_widget() {
        let ops = diff_marks(&[] as &[i32], &[1, 2]);
        assert_eq!(
            ops,
            vec![MarkOp::Create { next: 0 }, MarkOp::Create { next: 1 }]
        );
    }
}
