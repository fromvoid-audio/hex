//! One active local draft and read-only prepared curves; the actor owns every
//! committed edit. Detail and inline plots share pointer/command semantics.
//! Inline automation owns its target independently of background clip selection;
//! the detail lane follows the selected track only outside automation view.
//! Released markers retain their source-stamped ordered receipt until the exact
//! terminal and canonical state arrive in one receive; cancellation of held input
//! never revokes already-admitted display custody.
mod controls;
mod paint;
mod pending;
use pending::PendingGain;
#[cfg(test)]
pub(super) mod tests;
use super::{EditorView, ViewState, viewport::Viewport};
use gpui::{Context, Entity, Subscription, Window};
use hex_project::{BeatTime, GainPoint, TrackId};
use hex_session::service::{GainLaneEdit, Request};
use hex_widgets::NumericControl;
pub(super) use paint::{inline, view};

#[derive(Clone, Copy, Debug)]
pub(super) struct Draft {
    generation: u64,
    revision: u64,
    track: TrackId,
    from: Option<BeatTime>,
    pub(super) point: GainPoint,
    pointer: Option<([f32; 2], Viewport, [f32; 4])>,
}
struct NumericBinding {
    entity: Entity<NumericControl>,
    edit_state: hex_widgets::NumericEditState,
    _subscription: Subscription,
    key: (TrackId, BeatTime, u64, u64),
}
#[derive(Default)]
pub(super) struct GainLane {
    pub(super) track: Option<TrackId>,
    plot_width: f32,
    pub(super) selected: Option<BeatTime>,
    pub(super) draft: Option<Draft>,
    pub(super) pending: Option<PendingGain>,
    gain: Option<NumericBinding>,
    pub(super) error: Option<String>,
}
impl GainLane {
    pub(super) fn numeric_draft_active(&self) -> bool {
        self.gain
            .as_ref()
            .is_some_and(|binding| binding.edit_state.is_editing())
    }

    pub(super) fn pointer_is_active(&self) -> bool {
        self.draft.is_some_and(|draft| draft.pointer.is_some())
    }

    pub(super) fn change_owner(&mut self, track: TrackId, project: &hex_project::Project) {
        if self.track.is_none() || self.track == Some(track) {
            return;
        }
        self.track = Some(track);
        self.selected = project
            .track(track)
            .and_then(|track| track.gain_envelope().first())
            .map(|point| point.beat);
        self.draft = None;
        self.gain = None;
        self.error = Some("draft canceled · selected track changed".into());
    }

    pub(super) fn reconcile(
        &mut self,
        generation: u64,
        revision: u64,
        project: &hex_project::Project,
        replaced: bool,
    ) {
        if self.draft.is_some_and(|draft| {
            replaced
                || draft.generation != generation
                || draft.revision != revision
                || project.track(draft.track).is_none()
        }) {
            self.draft = None;
            self.gain = None;
            self.error = Some("draft canceled · project changed".into());
        }
        if replaced
            || self
                .track
                .is_some_and(|track| project.track(track).is_none())
        {
            self.track = None;
            self.selected = None;
            self.gain = None;
        }
        if let Some(track) = self.track.and_then(|track| project.track(track))
            && self
                .selected
                .is_some_and(|beat| !track.gain_envelope().iter().any(|point| point.beat == beat))
        {
            self.selected = track.gain_envelope().first().map(|point| point.beat);
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
    pub(super) fn gain_numeric_editing(&self, cx: &gpui::App) -> bool {
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
        if busy || self.gain_lane.draft.is_some() || self.gain_lane.pending.is_some() {
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
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        self.cancel_gain_draft(cx);
        // This compact editor replaces the open detail body, leaving real track rows at minimum size.
        self.clip_view_open = false;
        self.select_track(track, window);
        self.automation_view
            .devices
            .clear_main(super::device_panel::DevicePanelOwner::Track(track));
        self.gain_lane.track = Some(track);
        self.gain_lane.selected = self
            .state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.gain_envelope().first())
            .map(|point| point.beat);
        self.gain_lane.gain = None;
        self.gain_lane.error = None;
        cx.notify();
    }
    pub(super) fn cancel_gain_draft(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cancel_device_automation_draft(cx) {
            return true;
        }
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
    fn gain_point(&self) -> Option<GainPoint> {
        let lane = &self.gain_lane;
        self.state
            .as_ref()?
            .project
            .track(lane.track?)?
            .gain_envelope()
            .iter()
            .find(|point| Some(point.beat) == lane.selected)
            .copied()
    }
    fn begin_gain_draft(&mut self, from: Option<BeatTime>, point: GainPoint) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(track) = self.gain_lane.track else {
            return false;
        };
        if state
            .project
            .track(track)
            .is_some_and(|source| !source.gain_lane().spans().is_empty())
        {
            self.gain_lane.error = Some(
                "recorded touch spans are read-only · clear lane before editing the base curve"
                    .into(),
            );
            return false;
        }
        if self.edit_pending()
            || self.gain_lane.pending.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.device_gesture.snapshot().is_some()
        {
            return false;
        }
        self.gain_lane.draft = Some(Draft {
            generation: self.project_generation,
            revision: state.revision,
            track,
            from,
            point,
            pointer: None,
        });
        self.gain_lane.error = None;
        true
    }
    fn finish_gain_draft(&mut self) {
        let Some(draft) = self.gain_lane.draft.take() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if draft.generation != self.project_generation
            || draft.revision != state.revision
            || self.gain_lane.track != Some(draft.track)
        {
            self.gain_lane.error = Some("gain draft refused · project changed".into());
            return;
        }
        let Some(track) = state.project.track(draft.track) else {
            return;
        };
        if draft
            .from
            .and_then(|beat| track.gain_envelope().iter().find(|p| p.beat == beat))
            == Some(&draft.point)
        {
            self.gain_lane.selected = Some(draft.point.beat);
            self.gain_lane.gain = None;
            return;
        }
        let points = match replacement(track.gain_envelope(), draft.from, draft.point) {
            Ok(points) => points,
            Err(reason) => {
                self.gain_lane.error = Some(reason.into());
                self.gain_lane.draft = Some(draft);
                return;
            }
        };
        let edit = if draft.from.is_none() || draft.from == Some(draft.point.beat) {
            GainLaneEdit::Upsert(draft.point)
        } else if self
            .gain_point()
            .is_some_and(|old| old.gain == draft.point.gain && old.outgoing == draft.point.outgoing)
        {
            GainLaneEdit::Move {
                from: draft.from.unwrap_or(draft.point.beat),
                to: draft.point.beat,
            }
        } else {
            GainLaneEdit::Replace(points)
        };
        self.gain_lane.selected = Some(draft.point.beat);
        self.gain_lane.gain = None;
        self.gain_lane.pending = Some(PendingGain::new(draft, edit.clone()));
        if !self.enqueue(Request::GainLane {
            project_generation: draft.generation,
            revision: draft.revision,
            track: draft.track,
            edit,
        }) {
            self.retire_pending_gain();
        }
    }
    pub(super) fn gain_edit(&mut self, edit: GainLaneEdit, cx: &Context<Self>) {
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
        self.enqueue(Request::GainLane {
            project_generation: self.project_generation,
            revision: state.revision,
            track,
            edit,
        });
    }
    fn cycle_gain_point(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !self.gain_action_admitted(cx) {
            return;
        }
        let Some(track) = self
            .state
            .as_ref()
            .and_then(|state| self.gain_lane.track.and_then(|id| state.project.track(id)))
        else {
            return;
        };
        let points = track.gain_envelope();
        if points.is_empty() {
            return;
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
        self.gain_lane.selected = Some(points[index].beat);
        self.gain_lane.gain = None;
    }
    fn start_gain_pointer(
        &mut self,
        state: &ViewState,
        viewport: Viewport,
        rect: [f32; 4],
        pos: [f32; 2],
        free: bool,
    ) -> bool {
        let Some(track) = self.gain_lane.track.and_then(|id| state.project.track(id)) else {
            return false;
        };
        if rect
            .iter()
            .chain(pos.iter())
            .any(|value| !value.is_finite())
            || rect[2] <= 0.0
            || rect[3] <= 0.0
        {
            return false;
        }
        let selected = self.gain_lane.selected;
        let hit = track
            .gain_envelope()
            .iter()
            .filter_map(|point| {
                let frame = state.project.tempo_map().beat_to_frame(point.beat).ok()?;
                let x = rect[0] + viewport.fraction(frame)? * rect[2];
                let y = rect[1] + rect[3] * (1.0 - point.gain / 2.0);
                ((pos[0] - x).abs() <= 6.0 && (pos[1] - y).abs() <= 6.0).then_some(*point)
            })
            .min_by_key(|point| if Some(point.beat) == selected { 0 } else { 1 });
        let point = if let Some(point) = hit {
            point
        } else {
            let Some(beat) = pointer_beat(
                &state.project,
                self.snap_grid.with_platform_modifier(free),
                viewport,
                rect,
                pos[0],
            ) else {
                return false;
            };
            GainPoint {
                incoming_gain: None,
                beat,
                gain: pointer_gain(rect, pos[1]),
                outgoing: hex_project::GainInterpolation::Linear,
            }
        };
        if !self.begin_gain_draft(hit.map(|point| point.beat), point) {
            return false;
        }
        self.gain_lane.selected = Some(point.beat);
        self.gain_lane.gain = None;
        if let Some(draft) = self.gain_lane.draft.as_mut() {
            draft.pointer = Some((pos, viewport, rect));
        }
        true
    }
    fn move_gain_pointer(&mut self, pos: [f32; 2], free: bool) -> bool {
        if pos.iter().any(|value| !value.is_finite()) {
            return false;
        }
        let Some(draft) = self.gain_lane.draft.as_mut() else {
            return false;
        };
        let Some((origin, viewport, rect)) = draft.pointer else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        // A click preserves the exact authored BeatTime, even when several beats share a frame.
        if (pos[0] - origin[0]).abs() > 2.0 {
            let Some(beat) = pointer_beat(
                &state.project,
                self.snap_grid.with_platform_modifier(free),
                viewport,
                rect,
                pos[0],
            ) else {
                return false;
            };
            draft.point.beat = beat;
        }
        if (pos[1] - origin[1]).abs() > 2.0 {
            draft.point.gain = pointer_gain(rect, pos[1]);
        }
        self.gain_lane.error = state
            .project
            .track(draft.track)
            .and_then(|track| replacement(track.gain_envelope(), draft.from, draft.point).err())
            .map(str::to_owned);
        true
    }
}
fn pointer_beat(
    project: &hex_project::Project,
    grid: super::musical_time::SnapGrid,
    viewport: Viewport,
    rect: [f32; 4],
    x: f32,
) -> Option<BeatTime> {
    let frame = super::seek::pointer_to_frame(x, rect[0], rect[2], viewport)?;
    let beat = project.tempo_map().frame_to_beat(frame).ok()?;
    grid.snap_beat(project, beat, viewport, rect[2])
}
fn pointer_gain(rect: [f32; 4], y: f32) -> f32 {
    ((1.0 - (y - rect[1]) / rect[3]) * 2.0).clamp(0.0, 2.0)
}
