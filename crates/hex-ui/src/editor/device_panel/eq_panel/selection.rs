//! View-local selection transitions shared by the editor and sandbox.
use super::NodeId;

/// Select a pressed node and retain a possible group-collapse intent until
/// release. The caller validates node availability and owns gesture custody.
pub(super) fn select_node(
    selected: &mut Option<NodeId>,
    group: &mut Vec<NodeId>,
    collapse: &mut Option<NodeId>,
    id: NodeId,
    extend: bool,
) {
    *collapse = None;
    if extend {
        if group.is_empty() {
            group.extend(*selected);
        }
        if !group.contains(&id) {
            group.push(id);
        }
        if group.len() < 2 {
            group.clear();
        }
    } else if group.contains(&id) {
        *collapse = Some(id);
    } else {
        group.clear();
    }
    *selected = Some(id);
}

/// Apply complete marquee membership, retaining the focused node if it is
/// still selected. A single node is represented by focus without a group.
pub(super) fn apply_marquee(selected: &mut Option<NodeId>, group: &mut Vec<NodeId>, ids: &[u32]) {
    let members: Vec<NodeId> = ids
        .iter()
        .filter_map(|id| NodeId::new(u64::from(*id)).ok())
        .collect();
    *selected = selected
        .filter(|id| members.contains(id))
        .or_else(|| members.last().copied());
    *group = if members.len() > 1 {
        members
    } else {
        Vec::new()
    };
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn press_selection_extends_without_duplicates_and_defers_group_collapse() {
        let one = NodeId::new(1).unwrap();
        let two = NodeId::new(2).unwrap();
        let three = NodeId::new(3).unwrap();
        let mut selected = Some(one);
        let mut group = Vec::new();
        let mut collapse = None;
        select_node(&mut selected, &mut group, &mut collapse, one, true);
        assert_eq!(selected, Some(one));
        assert!(group.is_empty());
        select_node(&mut selected, &mut group, &mut collapse, two, true);
        assert_eq!(group, [one, two]);
        assert_eq!(selected, Some(two));
        select_node(&mut selected, &mut group, &mut collapse, two, true);
        assert_eq!(group, [one, two]);
        select_node(&mut selected, &mut group, &mut collapse, one, false);
        assert_eq!(group, [one, two]);
        assert_eq!(collapse, Some(one));
        assert_eq!(selected, Some(one));
        select_node(&mut selected, &mut group, &mut collapse, three, false);
        assert_eq!(selected, Some(three));
        assert!(group.is_empty());
        assert_eq!(collapse, None);
    }
}
