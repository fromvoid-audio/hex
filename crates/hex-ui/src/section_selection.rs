//! Shared extended section selection; hosts own focus, gestures and projects.

use hex_project::BeatTime;

/// Return the inclusive range in song order, retaining exact canonical starts.
pub(crate) fn range<K, I>(sections: I, anchor: K, clicked: K) -> Vec<K>
where
    K: Copy + Eq,
    I: IntoIterator<Item = (K, BeatTime)>,
    I::IntoIter: Clone,
{
    let sections = sections.into_iter();
    let start = |key| {
        sections
            .clone()
            .find(|(id, _)| *id == key)
            .map(|(_, beat)| beat)
    };
    let (Some(a), Some(b)) = (start(anchor), start(clicked)) else {
        return vec![clicked];
    };
    let (low, high) = (a.min(b), a.max(b));
    let mut sections: Vec<_> = sections
        .filter(|(_, beat)| low <= *beat && *beat <= high)
        .collect();
    sections.sort_by_key(|(_, beat)| *beat);
    sections.into_iter().map(|(id, _)| id).collect()
}

pub(crate) struct Extended<K> {
    pub(crate) selected: Option<K>,
    pub(crate) members: Vec<K>,
    pub(crate) anchor: K,
}

/// Shift selects the caller's song-ordered range; Command+Shift toggles one ID.
/// An anchor no longer in the selection falls back to focus, then the click.
pub(crate) fn extend<K: Copy + Eq>(
    mut previous: Vec<K>,
    selected: Option<K>,
    anchor: Option<K>,
    clicked: K,
    toggle: bool,
    range: impl FnOnce(K, K) -> Vec<K>,
) -> Extended<K> {
    let anchor = anchor
        .filter(|anchor| previous.contains(anchor))
        .or(selected)
        .unwrap_or(clicked);
    let (members, anchor) = if toggle {
        if previous.contains(&clicked) {
            previous.retain(|old| *old != clicked);
        } else {
            previous.push(clicked);
        }
        (previous, clicked)
    } else {
        (range(anchor, clicked), anchor)
    };
    let selected = if members.contains(&clicked) {
        Some(clicked)
    } else {
        members.last().copied()
    };
    Extended {
        selected,
        members,
        anchor,
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn exact_range_is_inclusive_ordered_and_handles_missing_endpoints() {
        let a = BeatTime::ONE;
        let b = BeatTime::from_raw(a.raw() + 1);
        let c = BeatTime::from_raw(a.raw() + 2);
        assert_eq!(a.raw() as f32, b.raw() as f32);
        let items = [(3, c), (1, a), (2, b)];
        assert_eq!(range(items, 2, 3), [2, 3]);
        assert_eq!(range(items, 3, 2), [2, 3]);
        assert_eq!(range(items, 2, 2), [2]);
        assert_eq!(range(items, 9, 2), [2]);
        assert_eq!(range(items, 2, 9), [9]);
    }

    #[test]
    fn range_uses_retained_anchor_and_caller_song_order() {
        let result = extend(vec![2], Some(2), Some(2), 1, false, |a, b| {
            assert_eq!((a, b), (2, 1));
            vec![1, 2]
        });
        assert_eq!(result.members, [1, 2]);
        assert_eq!(result.selected, Some(1));
        assert_eq!(result.anchor, 2);
    }

    #[test]
    fn stale_anchor_falls_back_to_focus_then_click() {
        let result = extend(vec![2], Some(2), Some(1), 3, false, |a, b| vec![a, b]);
        assert_eq!(result.members, [2, 3]);
        assert_eq!(result.anchor, 2);
        let result = extend(Vec::new(), None, Some(1), 3, false, |a, b| vec![a, b]);
        assert_eq!(result.anchor, 3);
    }

    #[test]
    fn toggle_preserves_order_and_focuses_remaining_last_member() {
        let result = extend(vec![2, 1], Some(1), Some(2), 1, true, |_, _| unreachable!());
        assert_eq!(result.members, [2]);
        assert_eq!(result.selected, Some(2));
        assert_eq!(result.anchor, 1);
        let empty = extend(vec![2], Some(2), Some(2), 2, true, |_, _| unreachable!());
        assert!(empty.members.is_empty());
        assert_eq!(empty.selected, None);
        assert_eq!(empty.anchor, 2);
    }
}
