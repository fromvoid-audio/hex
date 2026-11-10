//! Cached section/tag visibility and ancestor closure; never audio state.
use hex_project::{ClipId, Project, SectionId, Track, TrackId, TrackTag};
use std::{collections::BTreeSet, rc::Rc};

use super::interval::track_intersects_sections;

/// Returns matching track IDs and their group ancestors for nonempty ranges.
pub(super) fn visible_tracks(
    project: &Project,
    ranges: &[(u64, u64)],
    new_track: Option<TrackId>,
    tag: Option<&TrackTag>,
) -> BTreeSet<TrackId> {
    let mut visible = BTreeSet::new();
    if !ranges.is_empty() || tag.is_some() {
        for track in project.tracks() {
            if tag.is_some_and(|tag| !track.has_tag(tag))
                || (new_track != Some(track.id())
                    && !track_intersects_sections(project, track, ranges))
            {
                continue;
            }
            visible.insert(track.id());
            let mut parent = track.parent_group();
            for _ in 0..hex_project::MAX_TRACKS {
                let Some(id) = parent else { break };
                if !visible.insert(id) {
                    break;
                }
                parent = project.track(id).and_then(|track| track.parent_group());
            }
        }
    }
    visible
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    generation: u64,
    revision: u64,
    enabled: bool,
    new_track: Option<TrackId>,
    fold_revision: u64,
    tag_revision: u64,
}

/// Cached, view-only projection; no widget, scroll or audio ownership.
#[derive(Default)]
pub(crate) struct Projection {
    pub(crate) enabled: bool,
    tag: Option<TrackTag>,
    tag_revision: u64,
    stamp: Option<Stamp>,
    selected: Vec<SectionId>,
    pub(crate) new_track: Option<TrackId>,
    folded: BTreeSet<TrackId>,
    fold_revision: u64,
    pub(crate) ranges: Rc<[(u64, u64)]>,
    pub(crate) indices: Vec<usize>,
    // Count from the same snapshot as indices, including hidden trailing rows.
    canonical_count: usize,
    pub(crate) console_indices: Vec<usize>,
    pub(crate) tracks: BTreeSet<TrackId>,
    pub(crate) clips: BTreeSet<ClipId>,
}
fn section_intervals(project: &Project, selected: &[SectionId]) -> Rc<[(u64, u64)]> {
    let mut intervals = selected
        .iter()
        .filter_map(|id| {
            let section = project.arrangement_section(*id)?;
            let map = project.tempo_map();
            let start = map.beat_to_frame(section.start()).ok()?;
            let end = map.beat_to_frame(section.end()).ok()?;
            (start < end).then_some((start, end))
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut union: Vec<(u64, u64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = union.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            union.push((start, end));
        }
    }
    union.into()
}
impl Projection {
    /// Gap cues are for selected section intervals, not ordinary tags or folds.
    fn section_markers_active(&self) -> bool {
        self.enabled && !self.ranges.is_empty()
    }
    pub(super) fn hidden_before(&self, index: usize) -> bool {
        if !self.section_markers_active() {
            return false;
        }
        let Ok(row) = self.indices.binary_search(&index) else {
            return false;
        };
        row.checked_sub(1)
            .and_then(|previous| self.indices.get(previous))
            .map_or(index > 0, |previous| index.saturating_sub(*previous) > 1)
    }
    /// Interior gaps belong to the next row, so no boundary is painted twice.
    pub(super) fn hidden_after(&self, index: usize) -> bool {
        self.section_markers_active()
            && self.indices.last() == Some(&index)
            && index
                .checked_add(1)
                .is_some_and(|next| next < self.canonical_count)
    }
    pub(super) fn all_tracks_hidden(&self) -> bool {
        self.section_markers_active() && self.canonical_count > 0 && self.indices.is_empty()
    }

    pub(crate) fn tag_filter(&self) -> Option<&TrackTag> {
        self.tag.as_ref()
    }
    pub(crate) fn set_tag_filter(&mut self, tag: Option<TrackTag>) {
        if self.tag != tag {
            self.tag = tag;
            self.tag_revision = self.tag_revision.wrapping_add(1);
        }
    }

    pub(crate) fn folded(&self, id: TrackId) -> bool {
        self.folded.contains(&id)
    }
    pub(crate) fn fold_revision(&self) -> u64 {
        self.fold_revision
    }
    pub(crate) fn filtering(&self) -> bool {
        self.tag.is_some() || !self.ranges.is_empty() || !self.folded.is_empty()
    }
    pub(crate) fn toggle_fold(&mut self, id: TrackId) {
        if !self.folded.remove(&id) {
            self.folded.insert(id);
        }
        self.fold_revision = self.fold_revision.wrapping_add(1);
    }
    pub(super) fn generation_changed(&self, generation: u64) -> bool {
        self.stamp.is_some_and(|s| s.generation != generation)
    }
    pub(super) fn observe_active_track(&mut self, active: Option<TrackId>) {
        if self.new_track != active {
            self.new_track = None;
        }
    }
    pub(super) fn reveal_created_track(&mut self, track: TrackId) {
        self.new_track = (self.enabled && !self.ranges.is_empty()).then_some(track);
    }
    pub(super) fn rebuild(
        &mut self,
        generation: u64,
        revision: u64,
        project: &Project,
        selected: &[SectionId],
        active_track: Option<TrackId>,
    ) -> bool {
        if self.stamp.is_some_and(|s| s.generation != generation) {
            self.enabled = false;
            self.new_track = None;
            self.folded.clear();
            self.set_tag_filter(None);
        }
        self.observe_active_track(active_track);
        if !self.enabled || self.selected != selected {
            self.new_track = None;
        }
        let mut stamp = Stamp {
            generation,
            revision,
            enabled: self.enabled,
            new_track: self.new_track,
            fold_revision: self.fold_revision,
            tag_revision: self.tag_revision,
        };
        if self.stamp == Some(stamp) && self.selected == selected {
            return false;
        }
        let ranges = if self.enabled {
            section_intervals(project, selected)
        } else {
            Rc::from([])
        };
        // Validate track existence only on an invalidated projection, so an
        // unchanged repaint retains the constant-time stamp path.
        if self.ranges != ranges || self.new_track.is_some_and(|id| project.track(id).is_none()) {
            self.new_track = None;
            stamp.new_track = None;
        }
        self.folded
            .retain(|id| project.track(*id).is_some_and(Track::is_group));
        self.selected.clear();
        self.selected.extend_from_slice(selected);
        self.ranges = ranges;
        self.canonical_count = project.tracks().len();
        self.indices.clear();
        self.console_indices.clear();
        self.tracks.clear();
        self.clips.clear();
        let visible = visible_tracks(project, &self.ranges, self.new_track, self.tag.as_ref());
        // Console ignores section focus, while sharing tags and group folds.
        let tagged = self
            .tag
            .as_ref()
            .map(|tag| visible_tracks(project, &[], None, Some(tag)));
        let mut ancestors: Vec<(TrackId, bool)> = Vec::with_capacity(hex_project::MAX_GROUP_DEPTH);
        for (index, track) in project.tracks().iter().enumerate() {
            while ancestors.last().map(|(id, _)| *id) != track.parent_group()
                && !ancestors.is_empty()
            {
                ancestors.pop();
            }
            let hidden = ancestors.last().is_some_and(|(_, hidden)| *hidden);
            if track.is_group() && ancestors.len() < hex_project::MAX_GROUP_DEPTH {
                ancestors.push((track.id(), hidden || self.folded.contains(&track.id())));
            }
            if hidden {
                continue;
            }
            if tagged
                .as_ref()
                .is_some_and(|visible| !visible.contains(&track.id()))
            {
                continue;
            }
            self.console_indices.push(index);
            if !self.ranges.is_empty() && !visible.contains(&track.id()) {
                continue;
            }
            if self.filtering() {
                self.tracks.insert(track.id());
                self.clips.extend(
                    track
                        .clips()
                        .iter()
                        .map(|clip| clip.id())
                        .chain(track.midi_clips().iter().map(|clip| clip.id())),
                );
            }
            self.indices.push(index);
        }
        self.stamp = Some(stamp);
        true
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    #[test]
    fn tag_filter_is_view_only_cached_clearable_and_generation_bound() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut tracks = Vec::new();
        for name in ["kick", "hat", "bass"] {
            let Created::Track(track) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            tracks.push(track);
        }
        let drums = TrackTag::new("drums").unwrap();
        for track in &tracks[..2] {
            editor
                .execute(Command::SetTrackTags {
                    track: *track,
                    tags: vec![drums.clone()],
                })
                .unwrap();
        }
        let canonical = editor.project().clone();
        let mut projection = Projection::default();
        projection.rebuild(1, 1, &canonical, &[], None);
        projection.set_tag_filter(Some(drums));
        assert!(projection.rebuild(1, 1, &canonical, &[], None));
        assert_eq!(projection.indices, vec![0, 1]);
        assert_eq!(projection.console_indices, vec![0, 1]);
        let ptr = projection.indices.as_ptr();
        for _ in 0..1000 {
            assert!(!projection.rebuild(1, 1, &canonical, &[], None));
        }
        assert_eq!(projection.indices.as_ptr(), ptr);
        projection.set_tag_filter(Some(TrackTag::new("no match").unwrap()));
        projection.rebuild(1, 1, &canonical, &[], None);
        assert!(projection.indices.is_empty());
        assert!(projection.console_indices.is_empty());
        projection.set_tag_filter(None);
        projection.rebuild(1, 1, &canonical, &[], None);
        assert_eq!(projection.indices, vec![0, 1, 2]);
        projection.set_tag_filter(Some(TrackTag::new("drums").unwrap()));
        projection.rebuild(2, 1, &canonical, &[], None);
        assert!(projection.tag_filter().is_none());
        assert_eq!(projection.indices, vec![0, 1, 2]);
        assert_eq!(editor.project(), &canonical);
    }
    #[test]
    fn hundred_track_filter_and_cached_repaint_cost_remain_separate() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let tag = TrackTag::new("drums").unwrap();
        for index in 0..100 {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            if index % 2 == 0 {
                editor
                    .execute(Command::SetTrackTags {
                        track,
                        tags: vec![tag.clone()],
                    })
                    .unwrap();
            }
        }
        let project = editor.project();
        let mut projection = Projection::default();
        projection.rebuild(1, 1, project, &[], None);
        let before = std::time::Instant::now();
        for _ in 0..10000 {
            assert!(!std::hint::black_box(&mut projection).rebuild(1, 1, project, &[], None));
        }
        let unfiltered = before.elapsed();
        projection.set_tag_filter(Some(tag));
        let before = std::time::Instant::now();
        projection.rebuild(1, 1, project, &[], None);
        let rebuild = before.elapsed();
        assert_eq!(projection.indices.len(), 50);
        let before = std::time::Instant::now();
        for _ in 0..10000 {
            assert!(!std::hint::black_box(&mut projection).rebuild(1, 1, project, &[], None));
        }
        eprintln!(
            "tag filter100tracks unoptimized: rebuild {rebuild:?}; cached10000 unfiltered {unfiltered:?}, filtered {:?}",
            before.elapsed()
        );
    }
}
