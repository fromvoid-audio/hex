//! Shared extended section selection; hosts own focus, gestures and projects.

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

#[cfg(test)]
mod tests {
    use super::*;

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
