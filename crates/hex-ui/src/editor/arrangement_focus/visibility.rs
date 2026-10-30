//! Pure section-focus track visibility and ancestor closure.
use hex_project::{ClipId, Project, SectionId, Track, TrackId};
use std::{collections::BTreeSet, rc::Rc};

pub(super) fn interval_visible(start: u64, end: u64, ranges: &[(u64, u64)]) -> bool {
    ranges.is_empty() || (start < end && ranges.iter().any(|r| start < r.1 && end > r.0))
}
/// Section intervals choose whole tracks; clip visibility never crops those tracks.
/// Empty intervals retain the ordinary unfiltered view.
pub(crate) fn track_intersects_sections(
    project: &Project,
    track: &Track,
    ranges: &[(u64, u64)],
) -> bool {
    ranges.is_empty()
        || track.clips().iter().any(|clip| {
            clip.start()
                .checked_add(clip.duration())
                .is_some_and(|end| interval_visible(clip.start(), end, ranges))
        })
        || track.midi_clips().iter().any(|clip| {
            let Some(end) = clip.start().raw().checked_add(clip.length().raw()) else {
                return false;
            };
            let map = project.tempo_map();
            match (
                map.beat_to_frame(clip.start()),
                map.beat_to_frame(hex_project::BeatTime::from_raw(end)),
            ) {
                (Ok(start), Ok(end)) => interval_visible(start, end, ranges),
                _ => false,
            }
        })
}

/// Returns matching track IDs and their group ancestors for nonempty ranges.
pub(super) fn visible_tracks(
    project: &Project,
    ranges: &[(u64, u64)],
    new_track: Option<TrackId>,
) -> BTreeSet<TrackId> {
    let mut visible = BTreeSet::new();
    if !ranges.is_empty() {
        for track in project.tracks() {
            if new_track != Some(track.id()) && !track_intersects_sections(project, track, ranges) {
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
}

/// Cached, view-only projection; no widget, scroll or audio ownership.
#[derive(Default)]
pub(crate) struct Projection {
    pub(crate) enabled: bool,
    stamp: Option<Stamp>,
    selected: Vec<SectionId>,
    pub(crate) new_track: Option<TrackId>,
    folded: BTreeSet<TrackId>,
    fold_revision: u64,
    pub(crate) ranges: Rc<[(u64, u64)]>,
    pub(crate) indices: Vec<usize>,
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
    pub(crate) fn folded(&self, id: TrackId) -> bool {
        self.folded.contains(&id)
    }
    pub(crate) fn fold_revision(&self) -> u64 {
        self.fold_revision
    }
    pub(crate) fn filtering(&self) -> bool {
        !self.ranges.is_empty() || !self.folded.is_empty()
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
        self.indices.clear();
        self.console_indices.clear();
        self.tracks.clear();
        self.clips.clear();
        let visible = visible_tracks(project, &self.ranges, self.new_track);
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
