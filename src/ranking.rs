//! Bounded selection shared by graph and pack-native ranking.

use std::cmp::Ordering;

pub(crate) fn truncate_top_by<T>(
    results: &mut Vec<T>,
    top: usize,
    mut compare: impl FnMut(&T, &T) -> Ordering,
) {
    if top == 0 {
        results.clear();
        return;
    }
    if results.len() > top {
        results.select_nth_unstable_by(top, &mut compare);
        results.truncate(top);
    }
    results.sort_unstable_by(compare);
}
