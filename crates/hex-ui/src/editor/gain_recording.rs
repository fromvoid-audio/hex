//! Native touch recording UI; project edits are made only from delivered receipts.
use super::*;
use gpui::{Div, Entity, Subscription, div, prelude::*, px};
use hex_audio::gain_recording::{Receipt, Target};
use hex_session::gain_recording::{AppliedGain, GainTakeEnd};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::sync::atomic::Ordering::Acquire;
mod pending;
mod retirement;
use pending::SealedTake;

#[derive(Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Idle,
    Arming,
    Ready,
    Touching,
    Finishing,
    Retained,
    Committing,
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
    sealed: Option<SealedTake>,
    retired: Option<retirement::RetiredCapture>,
}
impl Recorder {
    pub(super) fn discard(&mut self) {
        self.mode = Mode::Idle;
        self.track = None;
        self.target = None;
        self.audio_revision = None;
        self.pending_gain = None;
        self.events.clear();
        self.sealed = None;
        self.retired = None;
        self.binding = None;
        self.cancelled = true;
    }
    pub(super) fn busy(&self) -> bool {
        matches!(self.mode, Mode::Arming | Mode::Touching | Mode::Finishing)
    }
    pub(super) fn blocks_edits(&self) -> bool {
        self.busy() || self.mode == Mode::Ready || self.has_retained_take()
    }
    pub(super) fn numeric_editing(&self, cx: &gpui::App) -> bool {
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
        if self.gain_lane.draft.is_some()
            || self.gain_lane.numeric_draft_active()
            || self.sampler_parameter.numeric_draft_active()
            || self.automation_view.devices.point_draft_active()
        {
            self.error = Some("finish local drafts before arming gain recording".into());
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
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error = Some("gain recording is unavailable for verified streaming audio".into());
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
        let Some(status) = audio.control.status() else {
            self.error = Some("audio output is closed".into());
            return;
        };
        if !status.gain_recording_ready.load(Acquire) {
            self.error = Some("gain recording waits for aligned playback".into());
            return;
        }
        let transport_epoch = status.transport_epoch.load(Acquire);
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
            transport_epoch,
            gesture_serial: serial,
            sample_rate: state.project.sample_rate(),
        };
        match audio.control.gain_begin(target, gain) {
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
            match audio.control.gain_change(gain) {
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
            if let Err(reason) = audio.control.gain_release() {
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
        if self.gain_recorder.has_retained_take() {
            self.error = Some(
                "gain take retained · wait for acknowledgment or retry/discard explicitly".into(),
            );
            return;
        }
        self.gain_recorder.pending_gain = None;
        self.gain_recorder.cancelled = true;
        if self.gain_recorder.target.is_some() {
            if let Some(audio) = self.audio.as_mut() {
                if let Err(reason) = audio.control.gain_cancel() {
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
        let retired_changed = self.poll_retired_gain_receipts();
        if self.gain_recorder.mode == Mode::Idle || self.gain_recorder.has_retained_take() {
            return retired_changed;
        }
        let Some(state) = self.state.as_ref() else {
            return retired_changed;
        };
        let source_changed = self.project_generation != self.gain_recorder.generation
            || state.revision != self.gain_recorder.revision;
        let Some(audio) = self.audio.as_mut() else {
            self.gain_recorder.audio_lost();
            self.error = Some(
                "audio unavailable · incomplete gain take retained for explicit discard".into(),
            );
            return true;
        };
        if source_changed {
            // Invalidate unfinished capture without treating an already queued
            // Finished receipt as an explicit user discard.
            let _ = audio.control.gain_cancel();
            self.gain_recorder.pending_gain = None;
            if self.gain_recorder.target.is_some() {
                self.gain_recorder.mode = Mode::Finishing;
            } else {
                self.gain_recorder.audio_lost();
                return true;
            }
        }
        let mut receipts = Vec::new();
        for _ in 0..66 {
            let Some(receipt) = audio.control.gain_receipt() else {
                break;
            };
            receipts.push(receipt);
        }
        let mut changed = retired_changed || !receipts.is_empty();
        let status_values = audio.control.status().map(|status| {
            (
                status.applied_revision.load(Acquire),
                status.gain_recording_ready.load(Acquire),
                status.device_fault.load(Acquire),
                status.render_fault.load(Acquire),
                status.gain_recording_fault.load(Acquire),
            )
        });
        if self.gain_recorder.mode == Mode::Arming
            && status_values.is_some_and(
                |(revision, ready, device_fault, render_fault, recording_fault)| {
                    self.gain_recorder.audio_revision == Some(revision)
                        && ready
                        && !device_fault
                        && !render_fault
                        && !recording_fault
                },
            )
        {
            self.gain_recorder.mode = Mode::Ready;
            self.status = "gain recording ready · drag touch gain; release returns to read".into();
            changed = true;
        }
        let output_failed = status_values
            .is_none_or(|(_, _, device_fault, render_fault, _)| device_fault || render_fault);
        let audio_failed = output_failed
            || status_values.is_some_and(|(_, _, _, _, recording_fault)| recording_fault);
        if audio_failed {
            let _ = audio.control.gain_cancel();
            // The output's fault fence already stops playback. A priority Stop
            // here would falsely turn automatic recovery into the user's rewind.
            if !output_failed {
                audio.control.stop();
            }
            // Process already-delivered Finished below before retiring capture.
            // Device loss cannot erase a completed take awaiting control polling.
        }
        if self.gain_recorder.mode == Mode::Arming {
            if let Some((revision, reason)) = audio
                .control
                .status()
                .and_then(|status| status.rejected_revision())
            {
                if self.gain_recorder.audio_revision == Some(revision) {
                    self.gain_recorder.mode = Mode::Idle;
                    self.gain_recorder.pending_gain = None;
                    self.gain_recorder.binding = None;
                    self.error =
                        Some(format!("gain recording arm refused · {}", reason.message()).into());
                    return true;
                }
            }
        }
        for receipt in receipts {
            self.gain_receipt(receipt);
        }
        self.flush_gain_touch();
        if audio_failed {
            // Keep the target until the output's quiescent retirement transfer.
            // A late sink Finished may still be delivered after this status read.
            self.gain_recorder.pending_gain = None;
            self.gain_recorder.binding = None;
            if self.gain_recorder.target.is_none() && !self.gain_recorder.has_retained_take() {
                self.gain_recorder.audio_lost();
                self.error =
                    Some("audio failed · gain recording stopped · restart audio to retry".into());
            } else {
                self.error = Some(
                    "audio failed · gain take retained · recover audio to finish receipt handoff"
                        .into(),
                );
            }
            return true;
        }
        changed
    }
    fn gain_receipt(&mut self, receipt: Receipt) {
        if Some(receipt.target()) != self.gain_recorder.target {
            return;
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
                    self.error =
                        Some("gain take exceeded its event limit · original lane retained".into());
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
                if self.gain_recorder.cancelled {
                    self.gain_recorder.discard();
                } else {
                    self.seal_gain_take(
                        target,
                        GainTakeEnd {
                            performance_frame,
                            content_frame,
                            loop_pass,
                        },
                    );
                }
            }
            Receipt::Aborted { reason, .. } => {
                let expected_cancel = self.gain_recorder.cancelled
                    && reason == hex_audio::gain_recording::AbortReason::Cancelled;
                if self.gain_recorder.cancelled {
                    self.gain_recorder.discard();
                } else {
                    self.gain_recorder.audio_lost();
                }
                if !expected_cancel {
                    self.error = Some(
                        format!(
                            "gain take aborted · {} · original lane retained",
                            super::user_errors::gain_abort(reason)
                        )
                        .into(),
                    );
                }
            }
        }
    }
}

/// Recovery stays visible even when the recorded track/source was replaced.
pub(super) fn retained_controls(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> gpui::AnyElement {
    if !view.gain_recorder.has_retained_take() {
        return div().into_any_element();
    }
    let retry_available = view.gain_take_retry_available();
    let waiting = view
        .gain_recorder
        .sealed
        .as_ref()
        .is_some_and(SealedTake::awaiting_commit);
    div()
        .id("retained-gain-take")
        .flex()
        .items_center()
        .child(theme::text(if waiting {
            "saving gain take"
        } else if view.gain_recorder.retired.is_some() {
            "recovering gain take"
        } else if view
            .gain_recorder
            .sealed
            .as_ref()
            .is_some_and(|take| !take.complete())
        {
            "incomplete gain take"
        } else {
            "gain take retained"
        }))
        .child(
            super::waveform::button(
                "↻",
                if retry_available {
                    "retry retained gain take against its original source"
                } else {
                    "retry unavailable · keep or explicitly discard the retained gain take"
                },
                ("gain-take-retry", 0),
            )
            .opacity(if retry_available { 1.0 } else { 0.4 })
            .on_click(cx.listener(|view, _, _, cx| {
                view.retry_gain_take();
                cx.notify();
            })),
        )
        .child(
            super::waveform::button(
                "×",
                "explicitly discard retained gain take · unavailable while applying",
                ("gain-take-discard", 0),
            )
            .opacity(if waiting { 0.4 } else { 1.0 })
            .on_click(cx.listener(|view, _, _, cx| {
                view.discard_gain_take();
                cx.notify();
            })),
        )
        .into_any_element()
}

pub(super) fn controls(view: &mut EditorView, track: TrackId, cx: &mut Context<EditorView>) -> Div {
    let armed = view.gain_recorder.mode != Mode::Idle;
    let mut controls = div().flex().items_center().gap_1().child(
        super::waveform::button(
            if armed { "■" } else { "●" },
            if view.gain_recorder.has_retained_take() {
                "gain take retained · use the transport retry/discard controls"
            } else {
                "arm gain touch recording · stop cancels the uncommitted take"
            },
            ("gain-record-arm", track.value()),
        )
        .on_click(cx.listener(move |view, _, _, cx| view.toggle_gain_recording(track, cx))),
    );
    if !armed || view.gain_recorder.track != Some(track) || view.gain_recorder.has_retained_take() {
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
                crate::theme::control_style(),
            )
        });
        let config = match config {
            Ok(value) => value.with_presentation(NumericControlPresentation::InlineValue),
            Err(reason) => {
                view.error = Some(reason.to_string().into());
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
                        view.error = Some(reason.to_string().into());
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
    fn header_volume_begin_obeys_recording_fence_even_before_next_render(cx: &mut TestAppContext) {
        let mut model = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: "record guard".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        struct BrowserDirectory(std::path::PathBuf);
        impl Drop for BrowserDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("hex-header-volume-record-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let browser = BrowserDirectory(directory);
        std::fs::create_dir(browser.0.join("samples")).unwrap();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, &browser.0)
                });
                editor.update(cx, |view, cx| {
                    assert_eq!(view.browser.directory, browser.0.join("samples"));
                    assert_eq!(view.browser.places, vec![browser.0.join("samples")]);
                    view.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::new(model.project().clone()),
                        waveforms: Default::default(),
                        source_overviews: Default::default(),
                        audio_clip_waveforms: Default::default(),
                        midi_preview: Arc::default(),
                        gain_preview: Ok(Arc::default()),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision: 1,
                        unsaved: false,
                        document_path: None,
                    }));
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let rail = visual
            .debug_bounds(Box::leak(
                format!("track-volume-{}-slider", track.value()).into_boxed_str(),
            ))
            .unwrap();
        // The widget was enabled at the last render. Arming can happen before
        // its next render; admission must still reject the direct gesture.
        editor.update(cx, |view, _| {
            view.gain_recorder.mode = Mode::Ready;
        });
        visual.simulate_mouse_down(
            rail.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual.simulate_mouse_move(
            gpui::point(rail.right(), rail.center().y),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        visual.simulate_mouse_up(
            rail.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        editor.read_with(cx, |view, _| {
            assert!(view.device_gesture.snapshot().is_none());
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.pending_requests.is_empty());
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .gain(),
                1.0
            );
        });
    }

    #[gpui::test]
    fn unavailable_output_retains_captured_take_until_explicit_discard(cx: &mut TestAppContext) {
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
            view.gain_recorder.target = Some(Target {
                track_id: track.value(),
                project_generation: view.project_generation,
                canonical_revision: revision,
                audio_revision: 1,
                recording_epoch: 1,
                transport_epoch: 1,
                gesture_serial: 7,
                sample_rate: 48_000,
            });
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
            assert!(view.gain_recorder.blocks_edits());
            assert!(view.gain_recorder.pending_gain.is_none());
            assert_eq!(view.gain_recorder.events.len(), 1);
            assert!(!view.gain_recorder.sealed.as_ref().unwrap().complete());
            assert!(!view.gain_take_retry_available());
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
                    .contains("incomplete gain take retained")
            );
            view.discard_gain_take();
            assert!(!view.gain_recorder.blocks_edits());
            assert!(view.gain_recorder.events.is_empty());
            assert_eq!(view.gain_recorder.serial, 7);
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "gain_recording/tests.rs"]
mod custody_tests;
