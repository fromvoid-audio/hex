//! Authored half-open group output mute ranges in absolute Arrangement frames.
//! They follow the group identity through track edits and history. Moving
//! individual clips, changing tempo, and changing the group mixer do not retime
//! these frame coordinates. This module owns metadata only.
use crate::{Invalid, Project, Track, TrackId, TrackKind};
use std::sync::Arc;

pub const MAX_GROUP_OUTPUT_MUTE_RANGES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupOutputMuteRange {
    start: u64,
    end: u64,
}

impl GroupOutputMuteRange {
    pub fn new(start: u64, end: u64) -> Result<Self, Invalid> {
        if start >= end {
            return Err(Invalid("group output mute range must be positive"));
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> u64 {
        self.start
    }

    pub fn end(self) -> u64 {
        self.end
    }

    pub fn contains(self, frame: u64) -> bool {
        (self.start..self.end).contains(&frame)
    }
}

impl Track {
    /// Sorted, nonoverlapping absolute Arrangement frame ranges. Adjacent
    /// ranges are represented by their union. Empty for an ordinary track.
    pub fn group_output_mutes(&self) -> &[GroupOutputMuteRange] {
        &self.group_output_mutes
    }
}

impl Project {
    /// Union one authored interval into a private model candidate. The
    /// committing command owns history, preparation and publication.
    pub fn add_group_output_mute(
        &mut self,
        group: TrackId,
        mut range: GroupOutputMuteRange,
    ) -> Result<(), Invalid> {
        let track = self.track_mut(group)?;
        if !track.is_group() {
            return Err(Invalid("output mute ranges require a group"));
        }
        let current = track.group_output_mutes();
        let first = current.partition_point(|old| old.end < range.start);
        let after = first + current[first..].partition_point(|old| old.start <= range.end);
        if first < after {
            range.start = range.start.min(current[first].start);
            range.end = range.end.max(current[after - 1].end);
            if after == first + 1 && range == current[first] {
                return Ok(());
            }
        }
        let len = current.len() - (after - first) + 1;
        if len > MAX_GROUP_OUTPUT_MUTE_RANGES {
            return Err(Invalid("group output mute range limit reached"));
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(len)
            .map_err(|_| Invalid("could not reserve group output mute ranges"))?;
        ranges.extend_from_slice(&current[..first]);
        ranges.push(range);
        ranges.extend_from_slice(&current[after..]);
        track.group_output_mutes = ranges.into();
        Ok(())
    }
}

pub(crate) fn snapshot_ranges(
    kind: TrackKind,
    ranges: Vec<GroupOutputMuteRange>,
) -> Result<Arc<[GroupOutputMuteRange]>, Invalid> {
    if !ranges.is_empty() && kind != TrackKind::Group {
        return Err(Invalid("output mute ranges require a group"));
    }
    if ranges.len() > MAX_GROUP_OUTPUT_MUTE_RANGES
        || ranges.windows(2).any(|pair| pair[0].end >= pair[1].start)
    {
        return Err(Invalid(
            "group output mute ranges must be bounded, sorted and disjoint",
        ));
    }
    Ok(ranges.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, TrackSelectionIds};

    fn fixture() -> (Project, TrackId, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(child) = editor
            .execute(Command::AddTrack {
                name: "child".into(),
            })
            .unwrap()
        else {
            panic!("child");
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[child]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };
        (editor.project().clone(), group, child)
    }

    #[test]
    fn union_preserves_half_open_boundaries_and_refuses_invalid_owners() {
        let (mut project, group, child) = fixture();
        assert!(GroupOutputMuteRange::new(3, 3).is_err());
        assert!(GroupOutputMuteRange::new(3, 2).is_err());
        for (start, end) in [(20, 30), (10, 15), (15, 20), (11, 14), (50, u64::MAX)] {
            project
                .add_group_output_mute(group, GroupOutputMuteRange::new(start, end).unwrap())
                .unwrap();
        }
        let ranges = project.track(group).unwrap().group_output_mutes();
        assert_eq!(
            ranges,
            &[
                GroupOutputMuteRange::new(10, 30).unwrap(),
                GroupOutputMuteRange::new(50, u64::MAX).unwrap()
            ]
        );
        assert!(!ranges[0].contains(9));
        assert!(ranges[0].contains(10) && ranges[0].contains(29));
        assert!(!ranges[0].contains(30));
        assert!(ranges[1].contains(u64::MAX - 1) && !ranges[1].contains(u64::MAX));
        let before = project.clone();
        let invalid_owner_range = GroupOutputMuteRange::new(1, 2).unwrap();
        for owner in [child, TrackId(u64::MAX)] {
            assert!(
                project
                    .add_group_output_mute(owner, invalid_owner_range)
                    .is_err()
            );
            assert_eq!(project, before);
        }
    }

    #[test]
    fn bounded_inventory_can_merge_at_capacity_and_undo_group_removal() {
        let (mut project, group, child) = fixture();
        for index in 0..MAX_GROUP_OUTPUT_MUTE_RANGES as u64 {
            let start = 3 * index;
            project
                .add_group_output_mute(group, GroupOutputMuteRange::new(start, start + 1).unwrap())
                .unwrap();
        }
        let full = project.clone();
        assert!(
            project
                .add_group_output_mute(group, GroupOutputMuteRange::new(500, 501).unwrap())
                .is_err()
        );
        assert_eq!(project, full);
        project
            .add_group_output_mute(group, GroupOutputMuteRange::new(1, 3).unwrap())
            .unwrap();
        assert_eq!(
            project.track(group).unwrap().group_output_mutes().len(),
            MAX_GROUP_OUTPUT_MUTE_RANGES - 1
        );

        let mut malformed = project.snapshot();
        malformed.tracks[0].group_output_mutes.swap(0, 1);
        assert!(Project::from_snapshot(malformed).is_err());
        let mut misplaced = project.snapshot();
        misplaced.tracks[1]
            .group_output_mutes
            .push(GroupOutputMuteRange::new(1, 2).unwrap());
        assert!(Project::from_snapshot(misplaced).is_err());

        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::UngroupTrack(group)).unwrap();
        assert!(editor.project().track(group).is_none());
        assert_eq!(editor.project().track(child).unwrap().parent_group(), None);
        assert!(editor.undo());
        assert_eq!(editor.project(), &project);
        assert!(editor.redo());
        assert!(editor.project().track(group).is_none());
    }

    #[test]
    fn track_copy_retains_arrangement_ranges_only_when_it_copies_content() {
        let (mut project, group, _) = fixture();
        let range = GroupOutputMuteRange::new(10, 20).unwrap();
        project.add_group_output_mute(group, range).unwrap();
        let mut editor = Editor::new(project, 8).unwrap();
        for without_clips in [false, true] {
            let Created::Tracks { first, .. } = editor
                .execute(Command::DuplicateTracks {
                    tracks: TrackSelectionIds::new(&[group]).unwrap(),
                    without_clips,
                })
                .unwrap()
            else {
                panic!("group copy");
            };
            let expected: &[GroupOutputMuteRange] = if without_clips { &[] } else { &[range] };
            assert_eq!(
                editor.project().track(first).unwrap().group_output_mutes(),
                expected
            );
            assert_eq!(
                editor.project().track(group).unwrap().group_output_mutes(),
                &[range]
            );
        }
    }
}
