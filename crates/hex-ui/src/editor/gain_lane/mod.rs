//! Local drafts and a read-only prepared curve; the actor owns every committed edit.
mod controls;
mod paint;
#[cfg(test)]
mod tests;
#[cfg(test)]
use super::ViewState;
use super::musical_time::MusicalProject;
use super::{EditorView, viewport::Viewport};
use gpui::{Context, Entity, Subscription, Window};
use hex_project::{BeatTime, GainPoint, TrackId};
use hex_session::service::{GainLaneEdit, Request};
use hex_session::snapshot_exports::SnapshotTransportError;
use hex_widgets::NumericControl;
pub(super) use paint::view;

#[derive(Clone, Copy, Debug)]
pub(super) struct Draft {
    generation: u64,
    revision: u64,
    track: TrackId,
    from: Option<BeatTime>,
    point: GainPoint,
    pointer: Option<([f32; 2], Viewport, [f32; 4])>,
}
struct NumericBinding {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
    key: (TrackId, BeatTime, u64, u64),
}
#[derive(Default)]
pub(super) struct GainLane {
    pub(super) track: Option<TrackId>,
    plot_width: f32,
    pub(super) selected: Option<BeatTime>,
    pub(super) draft: Option<Draft>,
    gain: Option<NumericBinding>,
    pub(super) error: Option<String>,
}
impl GainLane {
    pub(super) fn pointer_is_active(&self) -> bool {
        self.draft.is_some_and(|draft| draft.pointer.is_some())
    }

    #[cfg(test)]
    pub(super) fn change_owner(&mut self, track: TrackId, project: &hex_project::Project) {
        let first = project
            .track(track)
            .and_then(|track| track.gain_envelope().first())
            .map(|point| point.beat);
        self.change_owner_facts(track, first);
    }
    pub(super) fn change_owner_facts(&mut self, track: TrackId, first: Option<BeatTime>) {
        if self.track.is_none() || self.track == Some(track) {
            return;
        }
        self.track = Some(track);
        self.selected = first;
        self.draft = None;
        self.gain = None;
        self.error = Some("draft canceled · selected track changed".into());
    }

    pub(super) fn reconcile_read(
        &mut self,
        generation: u64,
        revision: u64,
        project: hex_session::snapshot_exports::SnapshotProjectRead<'_>,
        replaced: bool,
    ) {
        let track = self.track.and_then(|id| project.track(id));
        let draft_exists = self
            .draft
            .is_none_or(|draft| project.track(draft.track).is_some());
        let first = track.and_then(|track| track.gain_points().first().map(|point| point.beat));
        let selected_exists = self.selected.is_none_or(|beat| {
            track.is_some_and(|track| track.gain_points().iter().any(|point| point.beat == beat))
        });
        self.reconcile_facts(
            generation,
            revision,
            replaced,
            track.is_some(),
            draft_exists,
            first,
            selected_exists,
        );
    }
    #[cfg(test)]
    pub(super) fn reconcile(
        &mut self,
        generation: u64,
        revision: u64,
        project: &hex_project::Project,
        replaced: bool,
    ) {
        let track = self.track.and_then(|id| project.track(id));
        let draft_exists = self
            .draft
            .is_none_or(|draft| project.track(draft.track).is_some());
        let first = track.and_then(|track| track.gain_envelope().first().map(|point| point.beat));
        let selected_exists = self.selected.is_none_or(|beat| {
            track.is_some_and(|track| track.gain_envelope().iter().any(|point| point.beat == beat))
        });
        self.reconcile_facts(
            generation,
            revision,
            replaced,
            track.is_some(),
            draft_exists,
            first,
            selected_exists,
        );
    }
    fn reconcile_facts(
        &mut self,
        generation: u64,
        revision: u64,
        replaced: bool,
        track_exists: bool,
        draft_exists: bool,
        first: Option<BeatTime>,
        selected_exists: bool,
    ) {
        if self.draft.is_some_and(|draft| {
            replaced
                || draft.generation != generation
                || draft.revision != revision
                || !draft_exists
        }) {
            self.draft = None;
            self.gain = None;
            self.error = Some("draft canceled · project changed".into());
        }
        if replaced || (self.track.is_some() && !track_exists) {
            self.track = None;
            self.selected = None;
            self.gain = None;
        }
        if self.track.is_some() && !selected_exists {
            self.selected = first;
        }
    }
}
fn replacement(
    points: &[GainPoint],
    from: Option<BeatTime>,
    point: GainPoint,
) -> Result<Vec<GainPoint>, &'static str> {
    if from != Some(point.beat) && points.iter().any(|old| old.beat == point.beat) && from.is_some()
    {
        return Err("point move refused · beat is occupied");
    }
    let mut candidate = points.to_vec();
    if let Some(from) = from {
        let Some(index) = candidate.iter().position(|old| old.beat == from) else {
            return Err("gain point no longer exists");
        };
        candidate.remove(index);
    }
    match candidate.binary_search_by_key(&point.beat, |point| point.beat) {
        Ok(index) => candidate[index] = point,
        Err(index) => candidate.insert(index, point),
    }
    hex_project::validate_gain_envelope(&candidate).map_err(|reason| reason.0)?;
    Ok(candidate)
}
#[cfg(test)]
fn preview_gain(state: &ViewState, track: TrackId, frame: u64) -> Option<f32> {
    let source = state.project.track(track)?;
    if source.gain_envelope().is_empty() && source.gain_lane().spans().is_empty() {
        Some(source.gain())
    } else {
        state.gain_preview.as_ref().ok()?.value_at(track, frame)
    }
}
fn presentation(
    error: Option<&str>,
    draft: bool,
    pending: bool,
    revision: u64,
    applied: Option<u64>,
) -> String {
    if let Some(error) = error {
        error.to_owned()
    } else if draft {
        "draft · not applied".into()
    } else if pending {
        "gain edit pending · canonical revision unchanged".into()
    } else if applied == Some(revision) {
        format!("revision {revision} applied · curve is a prepared preview")
    } else {
        format!("revision {revision} · audio application pending")
    }
}
impl EditorView {
    pub(super) fn gain_numeric_editing(&self, cx: &Context<Self>) -> bool {
        self.gain_recorder.numeric_editing(cx)
            || self
                .gain_lane
                .gain
                .as_ref()
                .is_some_and(|binding| binding.entity.read(cx).is_editing())
    }

    fn gain_action_admitted(&mut self, cx: &Context<Self>) -> bool {
        let busy = [&self.gain_lane.gain].into_iter().flatten().any(|binding| {
            binding.entity.read(cx).is_interacting() || binding.entity.read(cx).is_editing()
        });
        if busy || self.gain_lane.draft.is_some() {
            self.gain_lane.error = Some("finish or cancel the local draft first".into());
            return false;
        }
        true
    }

    pub(super) fn show_gain_lane(
        &mut self,
        track: TrackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Ok(Some((first, selection))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let first = project
                .track(track)?
                .gain_points()
                .first()
                .map(|point| point.beat);
            let mut selection = self.track_selection.clone();
            selection
                .select(&project, track, false, false)
                .then_some((first, selection))
        }) else {
            return;
        };
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        self.cancel_gain_draft(cx);
        // This compact editor replaces the open detail body, leaving real track rows at minimum size.
        self.clip_view_open = false;
        self.focus_selected_track(track, first, window);
        self.track_selection = selection;
        self.update_track_selection_status();
        self.gain_lane.track = Some(track);
        self.gain_lane.selected = first;
        self.gain_lane.gain = None;
        self.gain_lane.error = None;
        cx.notify();
    }
    pub(super) fn cancel_gain_draft(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gain_recorder.blocks_edits() {
            self.cancel_gain_recording();
            cx.notify();
            return true;
        }
        let interacting = [&self.gain_lane.gain].into_iter().flatten().any(|binding| {
            binding.entity.read(cx).is_interacting() || binding.entity.read(cx).is_editing()
        });
        let changed = self.gain_lane.draft.take().is_some() || interacting;
        if changed {
            // Dropping the local bindings cancels their text/gesture drafts without a commit event.
            self.gain_lane.gain = None;
            self.gain_lane.error = None;
            cx.notify();
        }
        changed
    }
    fn gain_point(&self) -> Result<Option<GainPoint>, SnapshotTransportError> {
        let (Some(state), Some(track)) = (self.state.as_ref(), self.gain_lane.track) else {
            return Ok(None);
        };
        state.read_with(|snapshot| {
            snapshot
                .model()
                .track(track)?
                .gain_points()
                .iter()
                .find(|point| Some(point.beat) == self.gain_lane.selected)
                .copied()
        })
    }
    fn begin_gain_draft(&mut self, from: Option<BeatTime>, point: GainPoint) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(track) = self.gain_lane.track else {
            return false;
        };
        let Ok(Some((revision, spans))) = state.read_with(|snapshot| {
            snapshot
                .model()
                .track(track)
                .map(|track| (snapshot.revision(), track.gain_spans().next().is_some()))
        }) else {
            return false;
        };
        if spans {
            self.gain_lane.error = Some(
                "recorded touch spans are read-only · clear lane before editing the base curve"
                    .into(),
            );
            return false;
        }
        if self.edit_pending()
            || self.opening_project
            || self.close_prompt_open
            || self.device_gesture.snapshot().is_some()
        {
            return false;
        }
        self.gain_lane.draft = Some(Draft {
            generation: self.project_generation,
            revision,
            track,
            from,
            point,
            pointer: None,
        });
        self.gain_lane.error = None;
        true
    }
    fn finish_gain_draft(&mut self) {
        let Some(draft) = self.gain_lane.draft else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            self.gain_lane.draft = None;
            return;
        };
        let edit = state.read_with(|snapshot| -> Result<Option<GainLaneEdit>, &'static str> {
            if draft.generation != self.project_generation
                || draft.revision != snapshot.revision()
                || self.gain_lane.track != Some(draft.track)
            {
                return Ok(None);
            }
            let Some(track) = snapshot.model().track(draft.track) else {
                return Ok(None);
            };
            let points = replacement(track.gain_points(), draft.from, draft.point)?;
            let old = track
                .gain_points()
                .iter()
                .find(|point| Some(point.beat) == self.gain_lane.selected);
            Ok(Some(
                if draft.from.is_none() || draft.from == Some(draft.point.beat) {
                    GainLaneEdit::Upsert(draft.point)
                } else if old.is_some_and(|old| {
                    old.gain == draft.point.gain && old.outgoing == draft.point.outgoing
                }) {
                    GainLaneEdit::Move {
                        from: draft.from.unwrap_or(draft.point.beat),
                        to: draft.point.beat,
                    }
                } else {
                    GainLaneEdit::Replace(points)
                },
            ))
        });
        let edit = match edit {
            Ok(Ok(Some(edit))) => edit,
            Ok(Ok(None)) => {
                self.gain_lane.draft = None;
                self.gain_lane.error = Some("gain draft refused · project changed".into());
                return;
            }
            Ok(Err(reason)) => {
                self.gain_lane.error = Some(reason.into());
                return;
            }
            Err(_) => return,
        };
        self.gain_lane.draft = None;
        self.gain_lane.selected = Some(draft.point.beat);
        self.gain_lane.gain = None;
        self.enqueue(Request::GainLane {
            project_generation: draft.generation,
            revision: draft.revision,
            track: draft.track,
            edit,
        });
    }
    fn gain_edit(&mut self, edit: GainLaneEdit, cx: &Context<Self>) {
        if !self.gain_action_admitted(cx) || self.edit_pending() {
            self.gain_lane.error = Some("finish or cancel the local draft first".into());
            return;
        }
        let Some(track) = self.gain_lane.track else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Ok(revision) = state.read_with(|snapshot| snapshot.revision()) else {
            return;
        };
        self.enqueue(Request::GainLane {
            project_generation: self.project_generation,
            revision,
            track,
            edit,
        });
    }
    fn cycle_gain_point(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !self.gain_action_admitted(cx) {
            return;
        }
        let (Some(state), Some(track)) = (self.state.as_ref(), self.gain_lane.track) else {
            return;
        };
        let Ok(Some(beat)) = state.read_with(|snapshot| {
            let track = snapshot.model().track(track)?;
            let points = track.gain_points();
            if points.is_empty() {
                return None;
            }
            let index = points
                .iter()
                .position(|point| Some(point.beat) == self.gain_lane.selected)
                .unwrap_or(0);
            let index = if forward {
                (index + 1) % points.len()
            } else {
                (index + points.len() - 1) % points.len()
            };
            Some(points[index].beat)
        }) else {
            return;
        };
        self.gain_lane.selected = Some(beat);
        self.gain_lane.gain = None;
    }
    fn start_gain_pointer(
        &mut self,
        viewport: Viewport,
        rect: [f32; 4],
        pos: [f32; 2],
        free: bool,
    ) -> bool {
        if rect
            .iter()
            .chain(pos.iter())
            .any(|value| !value.is_finite())
            || rect[2] <= 0.0
            || rect[3] <= 0.0
        {
            return false;
        }
        let (Some(state), Some(track)) = (self.state.as_ref(), self.gain_lane.track) else {
            return false;
        };
        let Ok(Some((point, from))) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let track = project.track(track)?;
            let selected = self.gain_lane.selected;
            let hit = track
                .gain_points()
                .iter()
                .filter_map(|point| {
                    let frame = project.tempo().beat_to_frame(point.beat).ok()?;
                    let x = rect[0] + viewport.fraction(frame)? * rect[2];
                    let y = rect[1] + rect[3] * (1.0 - point.gain / 2.0);
                    ((pos[0] - x).abs() <= 6.0 && (pos[1] - y).abs() <= 6.0).then_some(*point)
                })
                .min_by_key(|point| if Some(point.beat) == selected { 0 } else { 1 });
            let point = match hit {
                Some(point) => point,
                None => GainPoint {
                    beat: pointer_beat(
                        &project,
                        self.snap_grid.with_platform_modifier(free),
                        viewport,
                        rect,
                        pos[0],
                    )?,
                    gain: pointer_gain(rect, pos[1]),
                    outgoing: hex_project::GainInterpolation::Linear,
                },
            };
            Some((point, hit.map(|point| point.beat)))
        }) else {
            return false;
        };
        if !self.begin_gain_draft(from, point) {
            return false;
        }
        self.gain_lane.selected = Some(point.beat);
        self.gain_lane.gain = None;
        if let Some(draft) = self.gain_lane.draft.as_mut() {
            draft.pointer = Some((pos, viewport, rect));
        }
        true
    }
    fn move_gain_pointer(
        &mut self,
        pos: [f32; 2],
        free: bool,
    ) -> Result<bool, SnapshotTransportError> {
        if pos.iter().any(|value| !value.is_finite()) {
            return Ok(false);
        }
        let Some(mut draft) = self.gain_lane.draft else {
            return Ok(false);
        };
        let Some((origin, viewport, rect)) = draft.pointer else {
            return Ok(false);
        };
        let Some(state) = self.state.as_ref() else {
            return Ok(false);
        };
        let facts = state.read_with(|snapshot| {
            let project = snapshot.model();
            // A click preserves exact BeatTime even when several beats share a frame.
            if (pos[0] - origin[0]).abs() > 2.0 {
                draft.point.beat = pointer_beat(
                    &project,
                    self.snap_grid.with_platform_modifier(free),
                    viewport,
                    rect,
                    pos[0],
                )?;
            }
            if (pos[1] - origin[1]).abs() > 2.0 {
                draft.point.gain = pointer_gain(rect, pos[1]);
            }
            let error = project
                .track(draft.track)
                .and_then(|track| replacement(track.gain_points(), draft.from, draft.point).err())
                .map(str::to_owned);
            Some((draft, error))
        })?;
        let Some((draft, error)) = facts else {
            return Ok(false);
        };
        self.gain_lane.draft = Some(draft);
        self.gain_lane.error = error;
        Ok(true)
    }
}

fn pointer_beat(
    project: &(impl MusicalProject + ?Sized),
    grid: super::musical_time::SnapGrid,
    viewport: Viewport,
    rect: [f32; 4],
    x: f32,
) -> Option<BeatTime> {
    let frame = super::seek::pointer_to_frame(x, rect[0], rect[2], viewport)?;
    let beat = project.frame_to_beat(frame).ok()?;
    grid.snap_beat(project, beat, viewport, rect[2])
}
fn pointer_gain(rect: [f32; 4], y: f32) -> f32 {
    ((1.0 - (y - rect[1]) / rect[3]) * 2.0).clamp(0.0, 2.0)
}
