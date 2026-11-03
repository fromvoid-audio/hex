//! View-local selection transitions shared by the editor and sandbox.
use super::NodeId;

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
