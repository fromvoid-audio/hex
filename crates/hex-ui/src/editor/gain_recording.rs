//! Native touch recording UI; project edits are made only from delivered receipts.
use super::*;
use gpui::{Div, Entity, Subscription, div, prelude::*, px};
use hex_audio::gain_recording::{Receipt, Target};
use hex_session::gain_recording::{AppliedGain, GainTakeEnd};
use hex_widgets::{
    NumericControl, NumericControlAppearance, NumericControlConfig, NumericControlEvent,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};
use std::sync::atomic::Ordering::Acquire;

#[derive(Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Idle,
    Arming,
    Ready,
    Touching,
    Finishing,
}
struct Binding {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}
#[derive(Default)]
pub(super) struct Recorder {
    mode: Mode,
    track: Option<TrackId>,
    generation: u64,
    revision: u64,
    audio_revision: Option<u64>,
    serial: u64,
    target: Option<Target>,
    events: Vec<AppliedGain>,
    pending_gain: Option<f32>,
    cancelled: bool,
    binding: Option<Binding>,
}
impl Recorder {
    pub(super) fn discard(&mut self) {
        self.mode = Mode::Idle;
        self.track = None;
        self.target = None;
        self.audio_revision = None;
        self.pending_gain = None;
        self.events.clear();
        self.binding = None;
        self.cancelled = true;
    }
    pub(super) fn busy(&self) -> bool {
        matches!(self.mode, Mode::Arming | Mode::Touching | Mode::Finishing)
    }
    pub(super) fn blocks_edits(&self) -> bool {
        self.busy() || self.mode == Mode::Ready
    }
    pub(super) fn numeric_editing(&self, cx: &Context<EditorView>) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|binding| binding.entity.read(cx).is_editing())
    }
    pub(super) fn arm_failed(&mut self) {
        if self.mode == Mode::Arming {
            self.discard();
        }
    }
    pub(super) fn published(&mut self, track: Option<TrackId>, revision: u64) {
        if self.mode == Mode::Arming && track == self.track {
            self.audio_revision = Some(revision);
        }
    }
}
impl EditorView {
    fn toggle_gain_recording(&mut self, track: TrackId, cx: &mut Context<Self>) {
        if self.gain_recorder.mode != Mode::Idle {
            self.cancel_gain_recording();
            cx.notify();
            return;
        }
        if self.edit_pending()
            || self.device_gesture.snapshot().is_some()
            || self.pending_playback.is_some()
            || !self.playback_candidates.is_empty()
        {
            self.error = Some("finish pending edits before arming gain recording".into());
            return;
        }
        let Some(state) = self.state.clone() else {
            return;
        };
        if self.audio.is_none() {
            self.error = Some("gain recording requires native audio".into());
            return;
        }
        if !self.enqueue(Request::ArmGainRecording {
            project_generation: self.project_generation,
            revision: state.revision,
            track,
        }) {
            return;
        }
        self.gain_recorder.mode = Mode::Arming;
        self.gain_recorder.track = Some(track);
        self.gain_recorder.generation = self.project_generation;
        self.gain_recorder.revision = state.revision;
        self.gain_recorder.audio_revision = None;
        self.gain_recorder.cancelled = false;
        self.gain_recorder.pending_gain = None;
        self.gain_recorder.binding = None;
        self.status = "preparing gain recording · playback starts before touch".into();
        if let Some(audio) = self.audio.as_mut() {
            if audio
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .is_err()
            {
                self.error = Some("play is queued; retry after pending transport controls".into());
            }
        }
        cx.notify();
    }
    fn begin_gain_touch(&mut self, gain: f32) {
        if self.gain_recorder.mode != Mode::Ready {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(track) = self
            .gain_recorder
            .track
            .and_then(|id| state.project.track(id))
        else {
            return;
        };
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let Some(audio_revision) = self.gain_recorder.audio_revision else {
            return;
        };
        if !audio.control.status.gain_recording_ready.load(Acquire) {
            self.error = Some("gain recording waits for aligned playback".into());
            return;
        }
        let Some(epoch) = track.gain_lane().recording_epoch().checked_add(1) else {
            self.error = Some("gain recording epoch exhausted".into());
            return;
        };
        let Some(serial) = self.gain_recorder.serial.checked_add(1) else {
            self.error = Some("gain recording gesture identity exhausted".into());
            return;
        };
        self.gain_recorder.events.clear();
        self.gain_recorder.pending_gain = None;
        if self
            .gain_recorder
            .events
            .try_reserve_exact(hex_audio::gain_recording::MAX_TAKE_EVENTS as usize)
            .is_err()
        {
            self.error = Some("could not reserve gain take events".into());
            return;
        }
        let target = Target {
            track_id: track.id().value(),
            project_generation: self.project_generation,
            canonical_revision: state.revision,
            audio_revision,
            recording_epoch: epoch,
            transport_epoch: audio.control.status.transport_epoch.load(Acquire),
            gesture_serial: serial,
            sample_rate: state.project.sample_rate(),
        };
        match audio.control.gain_recording.begin(target, gain) {
            Ok(()) => {
                self.gain_recorder.serial = serial;
                self.gain_recorder.target = Some(target);
                self.gain_recorder.mode = Mode::Touching;
                self.gain_recorder.cancelled = false;
                self.status = "recording gain touch · release to return to read".into();
            }
            Err(reason) => self.error = Some(reason.into()),
        }
    }
    fn change_gain_touch(&mut self, gain: f32) {
        if self.gain_recorder.mode == Mode::Touching {
            self.gain_recorder.pending_gain = Some(gain);
            self.flush_gain_touch();
        }
    }
    fn flush_gain_touch(&mut self) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        if let Some(gain) = self.gain_recorder.pending_gain {
            match audio.control.gain_recording.change(gain) {
                Ok(()) => self.gain_recorder.pending_gain = None,
                Err(reason) => {
                    self.error = Some(reason.into());
                    if reason == "gain take is full; release to save it" {
                        self.gain_recorder.pending_gain = None;
                        self.gain_recorder.mode = Mode::Finishing;
                    } else {
                        return;
                    }
                }
            }
        }
        if self.gain_recorder.mode == Mode::Finishing && !self.gain_recorder.cancelled {
            if let Err(reason) = audio.control.gain_recording.release() {
                self.error = Some(reason.into());
            }
        }
    }
    fn end_gain_touch(&mut self) {
        if self.gain_recorder.mode == Mode::Touching {
            self.gain_recorder.mode = Mode::Finishing;
            self.flush_gain_touch();
            self.status = "gain touch release pending · original lane retained until commit".into();
        }
    }
    pub(super) fn cancel_gain_recording(&mut self) {
        self.gain_recorder.pending_gain = None;
        self.gain_recorder.cancelled = true;
        if self.gain_recorder.target.is_some() {
            if let Some(audio) = self.audio.as_mut() {
                if let Err(reason) = audio.control.gain_recording.cancel() {
                    self.error = Some(reason.into());
                }
            }
            self.gain_recorder.mode = Mode::Finishing;
        } else {
            self.gain_recorder.mode = Mode::Idle;
            self.gain_recorder.binding = None;
        }
        self.status = "gain take cancelled · original lane retained".into();
    }
    pub(super) fn tick_gain_recording(&mut self) -> bool {
        if self.gain_recorder.mode == Mode::Idle {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.project_generation != self.gain_recorder.generation
            || state.revision != self.gain_recorder.revision
        {
            self.cancel_gain_recording();
        }
        let Some(audio) = self.audio.as_mut() else {
            self.gain_recorder.discard();
            self.error =
                Some("audio unavailable · gain take discarded; original lane retained".into());
            return true;
        };
        let mut receipts = Vec::new();
        for _ in 0..66 {
            let Some(receipt) = audio.control.gain_recording.try_receipt() else {
                break;
            };
            receipts.push(receipt);
        }
        let mut changed = !receipts.is_empty();
        if self.gain_recorder.mode == Mode::Arming
            && self.gain_recorder.audio_revision
                == Some(audio.control.status.applied_revision.load(Acquire))
            && audio.control.status.gain_recording_ready.load(Acquire)
        {
            self.gain_recorder.mode = Mode::Ready;
            self.status = "gain recording ready · drag touch gain; release returns to read".into();
            changed = true;
        }
        if audio.control.status.device_fault.load(Acquire)
            || audio.control.status.gain_recording_fault.load(Acquire)
        {
            let _ = audio.control.gain_recording.cancel();
            audio.control.stop();
            // The sink may never call back after device loss. Discard the
            // uncommitted UI take without waiting for a receipt or pretending
            // that the callback quiesced. Existing recovery retires this output.
            self.gain_recorder.discard();
            self.error =
                Some("audio failed · gain take discarded; recover audio before recording".into());
            return true;
        }
        if self.gain_recorder.mode == Mode::Arming {
            if let Some((revision, reason)) = audio.control.status.rejected_revision() {
                if self.gain_recorder.audio_revision == Some(revision) {
                    self.gain_recorder.mode = Mode::Idle;
                    self.gain_recorder.pending_gain = None;
                    self.gain_recorder.binding = None;
                    self.error = Some(format!("gain recording arm refused · {}", reason.message()));
                    return true;
                }
            }
        }
        for receipt in receipts {
            if Some(receipt.target()) != self.gain_recorder.target {
                continue;
            }
            match receipt {
                Receipt::Applied {
                    sequence,
                    performance_frame,
                    content_frame,
                    loop_pass,
                    gain,
                    ..
                } => {
                    if self.gain_recorder.events.len()
                        < hex_audio::gain_recording::MAX_TAKE_EVENTS as usize
                    {
                        self.gain_recorder.events.push(AppliedGain {
                            sequence,
                            performance_frame,
                            content_frame,
                            loop_pass,
                            gain,
                        });
                    } else {
                        self.cancel_gain_recording();
                    }
                }
                Receipt::Superseded { .. } => {}
                Receipt::Finished {
                    target,
                    performance_frame,
                    content_frame,
                    loop_pass,
                } => {
                    let events = std::mem::take(&mut self.gain_recorder.events);
                    let track = self.gain_recorder.track;
                    let cancelled = self.gain_recorder.cancelled;
                    self.gain_recorder.target = None;
                    self.gain_recorder.pending_gain = None;
                    self.gain_recorder.mode = Mode::Idle;
                    self.gain_recorder.binding = None;
                    if cancelled {
                        continue;
                    }
                    if let (Some(state), Some(track)) = (self.state.as_ref(), track) {
                        let expected_epoch = target.recording_epoch - 1;
                        let context = hex_project::GainTouchContext {
                            recording_epoch: target.recording_epoch,
                            project_generation: target.project_generation,
                            canonical_revision: target.canonical_revision,
                            audio_revision: target.audio_revision,
                            transport_epoch: target.transport_epoch,
                            gesture_epoch: target.transport_epoch,
                            gesture_serial: target.gesture_serial,
                            sample_rate: target.sample_rate,
                            loop_pass: events.first().map_or(loop_pass, |event| event.loop_pass),
                        };
                        match hex_session::gain_recording::finish_take(
                            &state.project,
                            track,
                            expected_epoch,
                            context,
                            &events,
                            GainTakeEnd {
                                performance_frame,
                                content_frame,
                                loop_pass,
                            },
                        ) {
                            Ok(spans) if !spans.is_empty() => {
                                self.enqueue(Request::GainLane {
                                    project_generation: target.project_generation,
                                    revision: target.canonical_revision,
                                    track,
                                    edit: hex_session::service::GainLaneEdit::SpliceTake {
                                        expected_epoch,
                                        spans,
                                    },
                                });
                            }
                            Ok(_) => self.status = "empty gain touch · project unchanged".into(),
                            Err(reason) => {
                                self.error = Some(format!("gain take refused · {reason}"))
                            }
                        }
                    }
                }
                Receipt::Aborted { reason, .. } => {
                    self.gain_recorder.target = None;
                    self.gain_recorder.pending_gain = None;
                    self.gain_recorder.events.clear();
                    self.gain_recorder.mode = Mode::Idle;
                    self.gain_recorder.binding = None;
                    self.error = Some(
                        format!("gain take aborted · {reason:?} · original lane retained")
                            .to_lowercase(),
                    );
                }
            }
        }
        self.flush_gain_touch();
        changed
    }
}

pub(super) fn controls(view: &mut EditorView, track: TrackId, cx: &mut Context<EditorView>) -> Div {
    let armed = view.gain_recorder.mode != Mode::Idle;
    let mut controls = div().flex().items_center().gap_1().child(
        super::waveform::button(
            if armed { "■" } else { "●" },
            "arm gain touch recording · stop cancels the uncommitted take",
            ("gain-record-arm", track.value()),
        )
        .on_click(cx.listener(move |view, _, _, cx| view.toggle_gain_recording(track, cx))),
    );
    if !armed || view.gain_recorder.track != Some(track) {
        return controls;
    }
    if view.gain_recorder.binding.is_none() {
        let gain = view
            .state
            .as_ref()
            .and_then(|state| {
                state
                    .gain_preview
                    .as_ref()
                    .ok()
                    .and_then(|preview| preview.value_at(track, view.cursor_position()))
                    .or_else(|| state.project.track(track).map(|owner| owner.gain()))
            })
            .unwrap_or(1.0);
        let config = NumericSpec::new(
            0.0,
            200.0,
            f64::from(gain) * 100.0,
            NumericScale::Linear,
            2,
            Some("%"),
        )
        .and_then(|spec| {
            NumericControlConfig::new(
                "gain-record-value",
                theme::text("touch gain"),
                NumericControlKind::HorizontalSlider,
                spec,
                f64::from(gain) * 100.0,
                super::device_panel::control_style(),
            )
        });
        let config = match config {
            Ok(value) => value.with_appearance(NumericControlAppearance::InlineValue),
            Err(reason) => {
                view.error = Some(reason.to_string());
                return controls;
            }
        };
        let entity = cx.new(|cx| NumericControl::from_config(cx, config));
        let subscription = cx.subscribe(&entity, |view, _, event: &NumericControlEvent, cx| {
            let gain = (event.value / 100.0) as f32;
            match event.phase {
                NumericEventPhase::Begin => view.begin_gain_touch(gain),
                NumericEventPhase::Change => view.change_gain_touch(gain),
                NumericEventPhase::Commit => {
                    view.change_gain_touch(gain);
                    view.end_gain_touch();
                }
                NumericEventPhase::Cancel => {
                    // Disabling a full take cancels the local widget gesture,
                    // but its reserved release is already saving delivered data.
                    if matches!(view.gain_recorder.mode, Mode::Ready | Mode::Touching) {
                        view.cancel_gain_recording();
                    }
                }
            }
            cx.notify();
        });
        view.gain_recorder.binding = Some(Binding {
            entity,
            _subscription: subscription,
        });
    }
    if let Some(binding) = view.gain_recorder.binding.as_ref() {
        if view.gain_recorder.mode == Mode::Ready {
            let read_gain = view
                .state
                .as_ref()
                .and_then(|state| state.gain_preview.as_ref().ok())
                .and_then(|preview| preview.value_at(track, view.cursor_position()));
            if let Some(gain) = read_gain {
                binding.entity.update(cx, |numeric, cx| {
                    if let Err(reason) = numeric.set_external_value(f64::from(gain) * 100.0, cx) {
                        view.error = Some(reason.to_string());
                    }
                });
            }
        }
        let enabled = matches!(view.gain_recorder.mode, Mode::Ready | Mode::Touching);
        binding
            .entity
            .update(cx, |numeric, cx| numeric.set_enabled(enabled, cx));
        controls = controls.child(div().w(px(84.0)).h(px(20.0)).child(binding.entity.clone()));
    }
    controls
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use hex_project::{Command, Created, Editor, Project};
    use std::sync::Arc;

    #[gpui::test]
    fn unavailable_output_discards_uncommitted_take_and_releases_edit_fence(
        cx: &mut TestAppContext,
    ) {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "touch".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let project = editor.project().clone();
        let (_, entity) =
            super::super::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
        entity.update(cx, |view, _| {
            let revision = view.state.as_ref().unwrap().revision;
            view.gain_recorder.mode = Mode::Touching;
            view.gain_recorder.track = Some(track);
            view.gain_recorder.generation = view.project_generation;
            view.gain_recorder.revision = revision;
            view.gain_recorder.serial = 7;
            view.gain_recorder.pending_gain = Some(1.5);
            view.gain_recorder.events.push(AppliedGain {
                sequence: 1,
                performance_frame: 1,
                content_frame: 1,
                loop_pass: 0,
                gain: 0.5,
            });
            assert!(view.gain_recorder.blocks_edits());
            assert!(view.tick_gain_recording());
            assert!(!view.gain_recorder.blocks_edits());
            assert!(view.gain_recorder.pending_gain.is_none());
            assert!(view.gain_recorder.events.is_empty());
            assert!(view.gain_recorder.track.is_none());
            assert_eq!(
                view.gain_recorder.serial, 7,
                "discard never reuses gesture identity"
            );
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &project);
            assert!(view.pending_requests.is_empty());
            assert!(
                view.error
                    .as_deref()
                    .unwrap()
                    .contains("original lane retained")
            );
        });
    }
}
