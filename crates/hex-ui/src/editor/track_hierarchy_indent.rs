//! Revision-keyed view projection for group-header indentation.
use hex_project::{Project, TrackId};
use std::collections::BTreeMap;

/// Cache canonical hierarchy depths by project identity and revision. The
/// project inventory is validated preorder, so the bounded ancestor stack
/// needs no whole-project scan for each visible header.
#[derive(Default)]
pub(super) struct HierarchyIndent {
    stamp: Option<(u64, u64)>,
    depths: BTreeMap<TrackId, usize>,
}
impl HierarchyIndent {
    pub(super) fn reconcile(&mut self, generation: u64, revision: u64, project: &Project) {
        if self.stamp == Some((generation, revision)) {
            return;
        }
        self.depths.clear();
        let mut ancestors = Vec::with_capacity(hex_project::MAX_GROUP_DEPTH);
        for track in project.tracks() {
            while ancestors.last().copied() != track.parent_group() && !ancestors.is_empty() {
                ancestors.pop();
            }
            self.depths.insert(track.id(), ancestors.len());
            if track.is_group() && ancestors.len() < hex_project::MAX_GROUP_DEPTH {
                ancestors.push(track.id());
            }
        }
        self.stamp = Some((generation, revision));
    }
    pub(super) fn pixels(&self, track: TrackId) -> f32 {
        self.depths.get(&track).copied().unwrap_or(0) as f32 * 4.0
    }
}
