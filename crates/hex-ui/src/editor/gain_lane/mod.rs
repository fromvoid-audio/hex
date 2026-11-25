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
#[derive(Clone, Debug)]
pub(super) struct MarqueeDraft {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) track: TrackId,
    pub(super) viewport: Viewport,
    pub(super) rect: [f32; 4],
    pub(super) marquee: hex_widgets::Marquee,
    pub(super) mode: hex_widgets::MarqueeMode,
    pub(super) base_selection: Vec<BeatTime>,
    pub(super) base_primary: Option<BeatTime>,
    pub(super) explicit: bool,
}
#[derive(Clone, Debug)]
pub(super) struct GroupDraft {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) track: TrackId,
    pub(super) grabbed: GainPoint,
    pub(super) group: Vec<GainPoint>,
    pub(super) rest: Vec<GainPoint>,
    pub(super) preview: Vec<GainPoint>,
    pub(super) selected_after: Vec<BeatTime>,
    pub(super) primary_after: BeatTime,
    pub(super) origin: [f32; 2],
    pub(super) viewport: Viewport,
    pub(super) rect: [f32; 4],
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
    pub(super) selected_points: Vec<BeatTime>,
    pub(super) draft: Option<Draft>,
    pub(super) group_draft: Option<GroupDraft>,
    pub(super) marquee: Option<MarqueeDraft>,
    pub(super) pending: Option<PendingGain>,
    gain: Option<NumericBinding>,
    pub(super) error: Option<String>,
}
impl GainLane {
    pub(super) fn effective_selected(&self) -> Vec<BeatTime> {
        let mut beats = self.selected_points.clone();
        if let Some(primary) = self.selected
            && !beats.contains(&primary)
        {
            beats.push(primary);
        }
        beats.sort_unstable();
        beats.dedup();
        beats
    }

    pub(super) fn is_selected(&self, beat: BeatTime) -> bool {
        self.selected == Some(beat) || self.selected_points.binary_search(&beat).is_ok()
    }

    pub(super) fn set_single_selection(&mut self, beat: Option<BeatTime>) {
        self.selected = beat;
        self.selected_points = beat.into_iter().collect();
    }

    pub(super) fn set_selection(&mut self, mut beats: Vec<BeatTime>, primary: Option<BeatTime>) {
        beats.sort_unstable();
        beats.dedup();
        self.selected = primary
            .filter(|beat| beats.binary_search(beat).is_ok())
            .or_else(|| beats.first().copied());
        self.selected_points = beats;
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
        self.selected_points.clear();
    }

    pub(super) fn numeric_draft_active(&self) -> bool {
        self.gain
            .as_ref()
            .is_some_and(|binding| binding.edit_state.is_editing())
    }

    pub(super) fn pointer_is_active(&self) -> bool {
        self.draft.is_some_and(|draft| draft.pointer.is_some())
            || self.group_draft.is_some()
            || self.marquee.is_some()
    }

    pub(super) fn change_owner(&mut self, track: TrackId, project: &hex_project::Project) {
        if self.track.is_none() || self.track == Some(track) {
            return;
        }
        self.track = Some(track);
        self.set_single_selection(
            project
                .track(track)
                .and_then(|track| track.gain_envelope().first())
                .map(|point| point.beat),
        );
        self.draft = None;
        self.group_draft = None;
        self.marquee = None;
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
        let draft_stale = self.draft.is_some_and(|draft| {
            replaced
                || draft.generation != generation
                || draft.revision != revision
                || project.track(draft.track).is_none()
        });
        let group_stale = self.group_draft.as_ref().is_some_and(|draft| {
            replaced
                || draft.generation != generation
                || draft.revision != revision
                || project.track(draft.track).is_none()
        });
        let marquee_stale = self.marquee.as_ref().is_some_and(|marquee| {
            replaced
                || marquee.generation != generation
                || marquee.revision != revision
                || project.track(marquee.track).is_none()
        });
        if draft_stale || group_stale || marquee_stale {
            self.draft = None;
            self.group_draft = None;
            self.marquee = None;
            self.gain = None;
            if draft_stale || group_stale {
                self.error = Some("draft canceled · project changed".into());
            }
        }
        if replaced
            || self
                .track
                .is_some_and(|track| project.track(track).is_none())
        {
            self.track = None;
            self.clear_selection();
            self.gain = None;
        }
        if let Some(track) = self.track.and_then(|track| project.track(track)) {
            self.selected_points.retain(|beat| {
                track
                    .gain_envelope()
                    .iter()
                    .any(|point| point.beat == *beat)
            });
            if self
                .selected
                .is_some_and(|beat| !track.gain_envelope().iter().any(|point| point.beat == beat))
            {
                self.selected = self
                    .selected_points
                    .first()
                    .copied()
                    .or_else(|| track.gain_envelope().first().map(|point| point.beat));
            }
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
        if busy
            || self.gain_lane.draft.is_some()
            || self.gain_lane.group_draft.is_some()
            || self.gain_lane.marquee.is_some()
            || self.gain_lane.pending.is_some()
        {
            self.gain_lane.error = Some("finish or cancel the local draft first".into());
            return false;
        }
        true
    }

    pub(super) fn select_touched_track_gain_lane(&mut self, track: TrackId) {
        if !self.automation_view.enabled {
            return;
        }
        self.automation_view
            .devices
            .clear_main(super::device_panel::DevicePanelOwner::Track(track));
        let changed = self.gain_lane.track != Some(track);
        self.gain_lane.track = Some(track);
        if changed {
            let first = self
                .state
                .as_ref()
                .and_then(|state| state.project.track(track))
                .and_then(|source| source.gain_envelope().first())
                .map(|point| point.beat);
            self.gain_lane.set_single_selection(first);
            self.gain_lane.draft = None;
            self.gain_lane.group_draft = None;
            self.gain_lane.marquee = None;
            self.gain_lane.gain = None;
            self.gain_lane.error = None;
        }
    }

    /// Live track gain value for unautomated lanes: in-flight gesture or pending
    /// recovery preview first, then canonical track gain. Keeps the released
    /// gesture value across repaints until the exact terminal installs state.
    pub(super) fn live_track_gain_display(&self, track: TrackId, canonical: f32) -> f32 {
        self.track_gain_controls
            .displayed_track_gain(self.project_generation, track)
            .or_else(|| {
                self.device_panel
                    .gesture
                    .preview_track_gain(self.project_generation, track)
            })
            .filter(|value| value.is_finite())
            .unwrap_or(canonical)
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
        self.audio_detail.clip_view_open = false;
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
        let marquee_cancelled = if let Some(marquee) = self.gain_lane.marquee.take() {
            self.gain_lane
                .set_selection(marquee.base_selection, marquee.base_primary);
            true
        } else {
            false
        };
        let group_cancelled = self.gain_lane.group_draft.take().is_some();
        let changed = self.gain_lane.draft.take().is_some()
            || group_cancelled
            || marquee_cancelled
            || interacting;
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
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.device_panel.gesture.snapshot().is_some()
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
        self.gain_lane.set_single_selection(Some(draft.point.beat));
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
        self.gain_lane
            .set_single_selection(Some(points[index].beat));
        self.gain_lane.gain = None;
    }

    pub(super) fn delete_selected_gain_points(&mut self, cx: &Context<Self>) -> bool {
        let selected = self.gain_lane.effective_selected();
        if selected.is_empty() {
            return false;
        }
        if !self.gain_action_admitted(cx) || self.edit_pending() {
            self.gain_lane.error = Some("finish or cancel the local draft first".into());
            return false;
        }
        let Some(track_id) = self.gain_lane.track else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(track) = state.project.track(track_id) else {
            return false;
        };
        if !track.gain_lane().spans().is_empty() {
            self.gain_lane.error = Some(
                "recorded touch spans are read-only · clear lane before editing the base curve"
                    .into(),
            );
            return false;
        }
        let edit = if selected.len() == 1 {
            GainLaneEdit::Remove(selected[0])
        } else {
            let remaining: Vec<GainPoint> = track
                .gain_envelope()
                .iter()
                .copied()
                .filter(|point| selected.binary_search(&point.beat).is_err())
                .collect();
            if remaining.len() == track.gain_envelope().len() {
                return false;
            }
            if remaining.is_empty() {
                GainLaneEdit::Clear
            } else {
                if let Err(reason) = hex_project::validate_gain_envelope(&remaining) {
                    self.gain_lane.error = Some(reason.0.into());
                    return false;
                }
                GainLaneEdit::Replace(remaining)
            }
        };
        self.gain_lane.clear_selection();
        self.gain_lane.gain = None;
        self.gain_lane.error = None;
        self.gain_edit(edit, cx);
        true
    }

    fn start_gain_pointer(
        &mut self,
        state: &ViewState,
        viewport: Viewport,
        rect: [f32; 4],
        pos: [f32; 2],
        modifiers: gpui::Modifiers,
    ) -> bool {
        let Some(track_id) = self.gain_lane.track else {
            return false;
        };
        let Some(track) = state.project.track(track_id) else {
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
        let free = modifiers.platform;
        let selected = self.gain_lane.selected;
        let hit = track
            .gain_envelope()
            .iter()
            .filter_map(|point| {
                let frame = state.project.tempo_map().beat_to_frame(point.beat).ok()?;
                let x = rect[0] + viewport.fraction(frame)? * rect[2];
                let y = rect[1] + rect[3] * (1.0 - gain_display_position(point.gain));
                ((pos[0] - x).abs() <= 6.0 && (pos[1] - y).abs() <= 6.0).then_some(*point)
            })
            .min_by_key(|point| if Some(point.beat) == selected { 0 } else { 1 });

        if let Some(hit_point) = hit {
            if modifiers.shift || modifiers.platform {
                let mode = if modifiers.platform {
                    hex_widgets::MarqueeMode::Toggle
                } else {
                    hex_widgets::MarqueeMode::Add
                };
                let base = self.gain_lane.effective_selected();
                let next = mode.combine(&base, &[hit_point.beat]);
                let primary = if next.binary_search(&hit_point.beat).is_ok() {
                    Some(hit_point.beat)
                } else {
                    next.first().copied()
                };
                self.gain_lane.set_selection(next, primary);
                self.gain_lane.gain = None;
                self.gain_lane.error = None;
                return true;
            }
            let current_selected = self.gain_lane.effective_selected();
            if current_selected.len() > 1 && self.gain_lane.is_selected(hit_point.beat) {
                if !track.gain_lane().spans().is_empty() {
                    self.gain_lane.error = Some(
                        "recorded touch spans are read-only · clear lane before editing the base curve"
                            .into(),
                    );
                    return false;
                }
                if self.edit_pending()
                    || self.gain_lane.pending.is_some()
                    || self.persistence.opening_project
                    || self.persistence.close_prompt_open
                    || self.device_panel.gesture.snapshot().is_some()
                {
                    return false;
                }
                let mut group = Vec::new();
                let mut rest = Vec::new();
                for &pt in track.gain_envelope() {
                    if current_selected.binary_search(&pt.beat).is_ok() {
                        group.push(pt);
                    } else {
                        rest.push(pt);
                    }
                }
                self.gain_lane.group_draft = Some(GroupDraft {
                    generation: self.project_generation,
                    revision: state.revision,
                    track: track_id,
                    grabbed: hit_point,
                    group,
                    rest,
                    preview: track.gain_envelope().to_vec(),
                    selected_after: current_selected,
                    primary_after: hit_point.beat,
                    origin: pos,
                    viewport,
                    rect,
                });
                self.gain_lane.gain = None;
                self.gain_lane.error = None;
                return true;
            }
            if !self.begin_gain_draft(Some(hit_point.beat), hit_point) {
                return false;
            }
            self.gain_lane.set_single_selection(Some(hit_point.beat));
            self.gain_lane.gain = None;
            if let Some(draft) = self.gain_lane.draft.as_mut() {
                draft.pointer = Some((pos, viewport, rect));
            }
            return true;
        }

        // Blank-space press: Shift/Cmd starts marquee immediately; plain press arms
        // a blank-click point draft that converts to a marquee once dragged past threshold.
        let Some(marquee) = hex_widgets::Marquee::new(pos) else {
            return false;
        };
        let mode = if modifiers.platform {
            hex_widgets::MarqueeMode::Toggle
        } else if modifiers.shift {
            hex_widgets::MarqueeMode::Add
        } else {
            hex_widgets::MarqueeMode::Replace
        };
        let explicit = modifiers.shift || modifiers.platform;
        let base_selection = self.gain_lane.effective_selected();
        let base_primary = self.gain_lane.selected;
        if !explicit {
            let Some(beat) = pointer_beat(
                &state.project,
                self.snap_grid.with_platform_modifier(free),
                viewport,
                rect,
                pos[0],
            ) else {
                return false;
            };
            let point = GainPoint {
                incoming_gain: None,
                beat,
                gain: pointer_gain(rect, pos[1]),
                outgoing: hex_project::GainInterpolation::Linear,
            };
            if !self.begin_gain_draft(None, point) {
                return false;
            }
            self.gain_lane.set_single_selection(Some(point.beat));
            self.gain_lane.gain = None;
            if let Some(draft) = self.gain_lane.draft.as_mut() {
                draft.pointer = Some((pos, viewport, rect));
            }
        }
        self.gain_lane.marquee = Some(MarqueeDraft {
            generation: self.project_generation,
            revision: state.revision,
            track: track_id,
            viewport,
            rect,
            marquee,
            mode,
            base_selection,
            base_primary,
            explicit,
        });
        true
    }

    fn move_gain_pointer(&mut self, pos: [f32; 2], free: bool) -> bool {
        if pos.iter().any(|value| !value.is_finite()) {
            return false;
        }
        let Some(state) = self.state.as_ref().cloned() else {
            return false;
        };
        if let Some(marquee_draft) = self.gain_lane.marquee.as_mut() {
            marquee_draft.marquee.update(pos);
            if marquee_draft.explicit || marquee_draft.marquee.crossed_threshold() {
                // Dragging across blank space promotes from blank-click add to marquee selection.
                self.gain_lane.draft = None;
                let track_id = marquee_draft.track;
                let viewport = marquee_draft.viewport;
                let rect = marquee_draft.rect;
                let mode = marquee_draft.mode;
                let base_selection = marquee_draft.base_selection.clone();
                let marquee = marquee_draft.marquee;
                if let Some(track) = state.project.track(track_id) {
                    let hits = marquee.hits(track.gain_envelope().iter().filter_map(|point| {
                        let frame = state.project.tempo_map().beat_to_frame(point.beat).ok()?;
                        let x = rect[0] + viewport.fraction(frame)? * rect[2];
                        let y = rect[1] + rect[3] * (1.0 - gain_display_position(point.gain));
                        Some((point.beat, [x, y]))
                    }));
                    let combined = mode.combine(&base_selection, &hits);
                    self.gain_lane.set_selection(combined, None);
                    self.gain_lane.gain = None;
                    self.gain_lane.error = None;
                }
                return true;
            }
        }
        if let Some(group_draft) = self.gain_lane.group_draft.as_mut() {
            let origin = group_draft.origin;
            let viewport = group_draft.viewport;
            let rect = group_draft.rect;
            let grabbed = group_draft.grabbed;
            let new_beat = if (pos[0] - origin[0]).abs() > 2.0 {
                let Some(beat) = pointer_beat(
                    &state.project,
                    self.snap_grid.with_platform_modifier(free),
                    viewport,
                    rect,
                    pos[0],
                ) else {
                    return false;
                };
                beat
            } else {
                grabbed.beat
            };
            let new_display = if (pos[1] - origin[1]).abs() > 2.0 {
                (1.0 - (pos[1] - rect[1]) / rect[3]).clamp(0.0, 1.0)
            } else {
                gain_display_position(grabbed.gain)
            };
            let beat_delta = i128::from(new_beat.raw()) - i128::from(grabbed.beat.raw());
            let display_delta = new_display - gain_display_position(grabbed.gain);

            let mut moved_group = Vec::with_capacity(group_draft.group.len());
            let mut moved_beats = Vec::with_capacity(group_draft.group.len());
            for &original in &group_draft.group {
                let Some(raw) = i128::from(original.beat.raw())
                    .checked_add(beat_delta)
                    .and_then(|v| u64::try_from(v).ok())
                else {
                    self.gain_lane.error = Some("group move refused · beat out of range".into());
                    return true;
                };
                let beat = BeatTime::from_raw(raw);
                let display =
                    (gain_display_position(original.gain) + display_delta).clamp(0.0, 1.0);
                let gain = gain_from_display_position(display);
                moved_beats.push(beat);
                moved_group.push(GainPoint {
                    beat,
                    gain,
                    ..original
                });
            }
            let mut candidate = Vec::with_capacity(group_draft.rest.len() + moved_group.len());
            candidate.extend_from_slice(&group_draft.rest);
            candidate.extend(moved_group);
            candidate.sort_by_key(|pt| pt.beat);
            let has_duplicate_beat = candidate
                .windows(2)
                .any(|pair| pair[0].beat == pair[1].beat);
            if has_duplicate_beat {
                self.gain_lane.error = Some("group move refused · beat is occupied".into());
                return true;
            }
            if let Err(reason) = hex_project::validate_gain_envelope(&candidate) {
                self.gain_lane.error = Some(reason.0.into());
                return true;
            }
            group_draft.preview = candidate;
            group_draft.selected_after = moved_beats;
            group_draft.primary_after = new_beat;
            self.gain_lane.error = None;
            return true;
        }
        let Some(draft) = self.gain_lane.draft.as_mut() else {
            return false;
        };
        let Some((origin, viewport, rect)) = draft.pointer else {
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

    fn finish_gain_pointer(&mut self, pos: [f32; 2], free: bool) -> bool {
        if let Some(marquee_draft) = self.gain_lane.marquee.take() {
            let mut marquee = marquee_draft.marquee;
            marquee.update(pos);
            if marquee_draft.explicit || marquee.crossed_threshold() {
                self.gain_lane.draft = None;
                if let Some(state) = self.state.as_ref()
                    && let Some(track) = state.project.track(marquee_draft.track)
                {
                    let hits = marquee.hits(track.gain_envelope().iter().filter_map(|point| {
                        let frame = state.project.tempo_map().beat_to_frame(point.beat).ok()?;
                        let x = marquee_draft.rect[0]
                            + marquee_draft.viewport.fraction(frame)? * marquee_draft.rect[2];
                        let y = marquee_draft.rect[1]
                            + marquee_draft.rect[3] * (1.0 - gain_display_position(point.gain));
                        Some((point.beat, [x, y]))
                    }));
                    let combined = marquee_draft
                        .mode
                        .combine(&marquee_draft.base_selection, &hits);
                    self.gain_lane.set_selection(combined, None);
                    self.gain_lane.gain = None;
                    self.gain_lane.error = None;
                }
                return true;
            }
            // Blank click below marquee threshold: fall through to commit the blank-click point draft.
        }
        if self.gain_lane.group_draft.is_some() {
            self.move_gain_pointer(pos, free);
            let Some(group_draft) = self.gain_lane.group_draft.take() else {
                return false;
            };
            let Some(state) = self.state.as_ref() else {
                return true;
            };
            if group_draft.generation != self.project_generation
                || group_draft.revision != state.revision
                || self.gain_lane.track != Some(group_draft.track)
            {
                self.gain_lane.error = Some("gain draft refused · project changed".into());
                return true;
            }
            let Some(track) = state.project.track(group_draft.track) else {
                return true;
            };
            if group_draft.preview == track.gain_envelope() {
                // Zero-motion click on a member of a multi-selection collapses to that point.
                self.gain_lane
                    .set_single_selection(Some(group_draft.grabbed.beat));
                self.gain_lane.gain = None;
                self.gain_lane.error = None;
                return true;
            }
            let edit = GainLaneEdit::Replace(group_draft.preview.clone());
            self.gain_lane
                .set_selection(group_draft.selected_after, Some(group_draft.primary_after));
            self.gain_lane.gain = None;
            self.gain_lane.pending = Some(PendingGain::new_group(
                group_draft.generation,
                group_draft.revision,
                group_draft.track,
                group_draft.preview,
                edit.clone(),
            ));
            if !self.enqueue(Request::GainLane {
                project_generation: group_draft.generation,
                revision: group_draft.revision,
                track: group_draft.track,
                edit,
            }) {
                self.retire_pending_gain();
            }
            return true;
        }
        if self.gain_lane.draft.is_none_or(|d| d.pointer.is_none()) {
            return false;
        }
        self.move_gain_pointer(pos, free);
        self.finish_gain_draft();
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
fn track_gain_display_spec() -> Option<hex_widgets::NumericSpec> {
    hex_widgets::NumericSpec::new(
        0.0,
        2.0,
        1.0,
        hex_widgets::NumericScale::LinearAmplitudeDecibels,
        1,
        None::<&str>,
    )
    .ok()
}

/// Normalized vertical lane position (`0.0` bottom .. `1.0` top) matching
/// `track_gain.rs`'s `NumericScale::LinearAmplitudeDecibels` fader scale.
pub(super) fn gain_display_position(gain: f32) -> f32 {
    if !gain.is_finite() {
        return 0.0;
    }
    let clamped = f64::from(gain.clamp(0.0, 2.0));
    track_gain_display_spec()
        .and_then(|spec| spec.to_normalized(clamped).ok())
        .map(|position| position.clamp(0.0, 1.0) as f32)
        .unwrap_or((clamped / 2.0) as f32)
}

fn gain_from_display_position(display: f32) -> f32 {
    if !display.is_finite() {
        return 0.0;
    }
    let clamped = f64::from(display.clamp(0.0, 1.0));
    track_gain_display_spec()
        .and_then(|spec| spec.from_normalized(clamped).ok())
        .map(|gain| (gain as f32).clamp(0.0, 2.0))
        .unwrap_or((clamped * 2.0) as f32)
}

fn pointer_gain(rect: [f32; 4], y: f32) -> f32 {
    let display = (1.0 - (y - rect[1]) / rect[3]).clamp(0.0, 1.0);
    gain_from_display_position(display)
}
