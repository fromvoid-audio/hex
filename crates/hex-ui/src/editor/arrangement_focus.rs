//! Read-only Arrangement visibility over the canonical loop interval. Rebuild once
//! per document/revision/range change, never per row. Canonical indices and IDs
//! remain authoritative for edits; list indices are translated only at the view.
use super::EditorView;
use hex_project::{ClipId, Project, Track, TrackId};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    generation: u64,
    revision: u64,
    range: Option<(u64, u64)>,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) enabled: bool,
    stamp: Option<Stamp>,
    indices: Vec<usize>,
    tracks: BTreeSet<TrackId>,
    clips: BTreeSet<ClipId>,
    restore_scroll: Option<(TrackId, gpui::Pixels)>,
    pub(super) pending_restore: Option<(TrackId, gpui::Pixels)>,
}
fn intersects(start: u64, end: u64, range: (u64, u64)) -> bool {
    start < end && start < range.1 && end > range.0
}
impl State {
    fn rebuild(&mut self, generation: u64, revision: u64, project: &Project) -> bool {
        if self.stamp.is_some_and(|s| s.generation != generation) {
            self.enabled = false;
            self.restore_scroll = None;
            self.pending_restore = None;
        }
        if self.enabled && project.loop_region().is_none() {
            self.enabled = false;
            self.pending_restore = self.restore_scroll.take();
        }
        let range = self
            .enabled
            .then(|| project.loop_region())
            .flatten()
            .map(|r| (r.start(), r.end()));
        let stamp = Stamp {
            generation,
            revision,
            range,
        };
        if self.stamp == Some(stamp) {
            return false;
        }
        self.indices.clear();
        self.tracks.clear();
        self.clips.clear();
        for (index, track) in project.tracks().iter().enumerate() {
            if let Some(range) = range {
                for clip in track.clips() {
                    if clip
                        .start()
                        .checked_add(clip.duration())
                        .is_some_and(|end| intersects(clip.start(), end, range))
                    {
                        self.clips.insert(clip.id());
                    }
                }
                for clip in track.midi_clips() {
                    let end = clip
                        .start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .map(hex_project::BeatTime::from_raw);
                    if let Some((Ok(start), Ok(end))) = end.map(|end| {
                        (
                            project.tempo_map().beat_to_frame(clip.start()),
                            project.tempo_map().beat_to_frame(end),
                        )
                    }) && intersects(start, end, range)
                    {
                        self.clips.insert(clip.id());
                    }
                }
                if !track.clips().iter().any(|c| self.clips.contains(&c.id()))
                    && !track
                        .midi_clips()
                        .iter()
                        .any(|c| self.clips.contains(&c.id()))
                {
                    continue;
                }
            }
            self.indices.push(index);
            if self.enabled {
                self.tracks.insert(track.id());
            }
        }
        self.stamp = Some(stamp);
        true
    }
}
impl EditorView {
    pub(super) fn arrangement_focus_range(&self) -> Option<(u64, u64)> {
        self.arrangement_focus.stamp.and_then(|s| s.range)
    }
    pub(super) fn arrangement_clip_visible(&self, id: ClipId) -> bool {
        !self.arrangement_focus.enabled || self.arrangement_focus.clips.contains(&id)
    }
    pub(super) fn arrangement_track_visible(&self, id: TrackId) -> bool {
        !self.arrangement_focus.enabled || self.arrangement_focus.tracks.contains(&id)
    }
    pub(super) fn arrangement_tracks<'a>(
        &'a self,
        project: &'a Project,
    ) -> impl Iterator<Item = (usize, &'a Track)> {
        project
            .tracks()
            .iter()
            .enumerate()
            .filter(|(_, track)| self.arrangement_track_visible(track.id()))
    }
    pub(super) fn arrangement_track_index(&self, row: usize) -> Option<usize> {
        self.arrangement_focus.indices.get(row).copied()
    }
    pub(super) fn arrangement_row_index(&self, canonical: usize) -> Option<usize> {
        self.arrangement_focus
            .indices
            .binary_search(&canonical)
            .ok()
    }
    pub(super) fn focused_neighbor_track(
        &self,
        forward: bool,
    ) -> Option<(usize, TrackId, Option<ClipId>)> {
        let project = &self.state.as_ref()?.project;
        let indices = &self.arrangement_focus.indices;
        let last = indices.len().checked_sub(1)?;
        let current = self
            .selected_track
            .and_then(|id| project.tracks().iter().position(|t| t.id() == id));
        let row = match current {
            Some(index) => match indices.binary_search(&index) {
                Ok(row) if forward => row.saturating_add(1).min(last),
                Ok(row) => row.saturating_sub(1),
                Err(row) if forward => row.min(last),
                Err(row) => row.saturating_sub(1),
            },
            None if forward => 0,
            None => last,
        };
        let track = project.tracks().get(*indices.get(row)?)?;
        let clip = track
            .clips()
            .iter()
            .map(|c| c.id())
            .chain(track.midi_clips().iter().map(|c| c.id()))
            .find(|id| self.arrangement_clip_visible(*id));
        Some((row, track.id(), clip))
    }
    pub(super) fn reconcile_arrangement_focus(&mut self) {
        if let Some(state) = &self.state {
            let was_enabled = self.arrangement_focus.enabled;
            self.arrangement_focus
                .rebuild(self.project_generation, state.revision, &state.project);
            if was_enabled && !self.arrangement_focus.enabled {
                self.status = "loop focus ended · all tracks visible".into();
            }
        } else {
            self.arrangement_focus = State::default();
        }
    }
    pub(super) fn toggle_arrangement_focus(&mut self) {
        self.reconcile_arrangement_focus();
        if self.arrangement_focus.enabled {
            self.arrangement_focus.enabled = false;
            self.arrangement_focus.pending_restore = self.arrangement_focus.restore_scroll.take();
            self.status = "all arrangement tracks · q focuses loop content".into();
        } else {
            if self
                .state
                .as_ref()
                .and_then(|s| s.project.loop_region())
                .is_none()
            {
                self.status = "set a loop range before focusing its content".into();
                return;
            }
            let top = self.track_scroll.list.logical_scroll_top();
            self.arrangement_focus.restore_scroll = self
                .state
                .as_ref()
                .and_then(|s| s.project.tracks().get(top.item_ix))
                .map(|t| (t.id(), top.offset_in_item));
            self.arrangement_focus.enabled = true;
            self.status = "loop focus · q restores all tracks".into();
        }
        self.reconcile_arrangement_focus();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
