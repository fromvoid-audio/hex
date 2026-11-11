//! Canonical selected subtree rows for one revision-stamped drag.
use hex_project::{Invalid, MAX_TRACKS, Project, TrackId, TrackKind, TrackSelectionIds};
use std::{collections::BTreeMap, ops::Range};

pub(crate) struct SourceRows {
    selected: [bool; MAX_TRACKS],
    groups: BTreeMap<TrackId, Range<usize>>,
    end: usize,
    contiguous: bool,
    common_parent: Option<Option<TrackId>>,
}
impl SourceRows {
    pub(crate) fn capture(project: &Project, tracks: &TrackSelectionIds) -> Result<Self, Invalid> {
        let mut selected = [false; MAX_TRACKS];
        let mut groups = BTreeMap::new();
        let common_parent = if project.has_group_tracks() {
            let hierarchy = project.track_hierarchy()?;
            if tracks.ids().iter().any(|id| hierarchy.row(*id).is_none()) {
                return Err(Invalid("selected track no longer exists"));
            }
            let mut parent = None;
            let mut initialized = false;
            let mut same_parent = true;
            for row in hierarchy.rows() {
                let range = row.subtree_range();
                if row.kind() == TrackKind::Group {
                    groups.insert(row.id(), range.clone());
                }
                if !tracks.ids().contains(&row.id())
                    || selected.get(range.start).copied().unwrap_or(false)
                {
                    continue;
                }
                if initialized {
                    same_parent &= parent == row.parent_group();
                } else {
                    parent = row.parent_group();
                    initialized = true;
                }
                selected
                    .get_mut(range)
                    .ok_or(Invalid("selected subtree exceeds track inventory"))?
                    .fill(true);
            }
            same_parent.then_some(parent)
        } else {
            let mut found = 0;
            for (index, track) in project.tracks().iter().enumerate() {
                if tracks.ids().contains(&track.id()) {
                    let row = selected
                        .get_mut(index)
                        .ok_or(Invalid("track inventory exceeds limit"))?;
                    *row = true;
                    found += 1;
                }
            }
            if found != tracks.ids().len() {
                return Err(Invalid("selected track no longer exists"));
            }
            Some(None)
        };
        let first = selected
            .iter()
            .position(|selected| *selected)
            .ok_or(Invalid("track selection is empty"))?;
        let end = selected
            .iter()
            .rposition(|selected| *selected)
            .and_then(|index| index.checked_add(1))
            .ok_or(Invalid("selected subtree exceeds track inventory"))?;
        let contiguous = selected[first..end].iter().all(|selected| *selected);
        Ok(Self {
            selected,
            groups,
            end,
            contiguous,
            common_parent,
        })
    }
    /// Inside the selection is invalid; its trailing boundary is a no-op only
    /// if every selected root would retain its current parent.
    pub(crate) fn permits(&self, boundary: usize, destination_parent: Option<TrackId>) -> bool {
        !(self.selected.get(boundary).copied().unwrap_or(false)
            || self.contiguous
                && boundary == self.end
                && self.common_parent == Some(destination_parent))
    }
    /// A group can close across hidden rows. Compare the next visible row
    /// with the canonical subtree end captured at the start of the drag.
    pub(crate) fn group_ends_by(&self, group: TrackId, next_row: usize) -> bool {
        self.groups
            .get(&group)
            .is_some_and(|range| range.end <= next_row)
    }

    /// An append target cannot lie in a selected subtree or preserve a trailing no-op.
    pub(crate) fn permits_group(&self, group: TrackId) -> bool {
        self.groups.get(&group).is_some_and(|range| {
            !(self.selected.get(range.start).copied().unwrap_or(true)
                || self.contiguous
                    && range.end == self.end
                    && self.common_parent == Some(Some(group)))
        })
    }
}
