//! Editor lifecycle for the armed track's stopped software monitor.
//! Session prepares the exact renderer and retains its storage hold. The input
//! worker owns the device until Finished. A take waits for both that lease and
//! the output callback's monitor return.
use super::{
    EditorMode, EditorView, MonitorAction, Monitoring, monitor_action, selected_input_config,
};
use hex_audio::{
    input_capture::{InputPhase, MonitorCapture, MonitorStream},
    input_devices::InputDeviceId,
    prepared::{AppliedAudioSource, input_monitor::MonitorStatus},
};
use hex_engine::PausedTrackMonitor;
use hex_session::service::{
    PausedInputMonitorRequest, Request, ResidentGraphSubmission, SubmitError,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, PartialEq, Eq)]
struct Target {
    request: PausedInputMonitorRequest,
    input: Option<InputDeviceId>,
    rate: u32,
}
impl Target {
    fn output(&self) -> u64 {
        self.request.source.output_identity
    }
    fn source(&self) -> AppliedAudioSource {
        AppliedAudioSource {
            context: self.request.source.context,
            audio_revision: self.request.source.audio_revision,
            output_identity: self.output(),
        }
    }
}

struct PreparedInput {
    capture: MonitorCapture,
    stream: Box<MonitorStream>,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Preparing {
        target: Target,
        reply: Receiver<Result<PausedTrackMonitor, String>>,
    },
    Opening {
        target: Target,
        monitor: PausedTrackMonitor,
        input: Receiver<Result<PreparedInput, String>>,
    },
    Installed {
        target: Target,
        capture: MonitorCapture,
        started: bool,
        audible: bool,
    },
    Stopping {
        capture: MonitorCapture,
        releasing_output: Option<u64>,
    },
}

#[derive(Default)]
pub(super) struct PausedInput {
    phase: Phase,
    failed: Option<Target>,
    #[cfg(all(
        test,
        feature = "ui-test-support",
        target_os = "macos",
        target_arch = "aarch64"
    ))]
    fixture_input: Option<Receiver<Result<PreparedInput, String>>>,
}
impl PausedInput {
    pub(super) fn pending(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }
    pub(super) fn user_changed_target(&mut self) {
        self.failed = None;
    }
    fn open(&mut self, target: &Target) -> Result<Receiver<Result<PreparedInput, String>>, String> {
        #[cfg(all(
            test,
            feature = "ui-test-support",
            target_os = "macos",
            target_arch = "aarch64"
        ))]
        if let Some(input) = self.fixture_input.take() {
            return Ok(input);
        }
        open_input(target)
    }
}

fn open_input(target: &Target) -> Result<Receiver<Result<PreparedInput, String>>, String> {
    let selected = target.input.clone();
    let rate = target.rate;
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("hex-monitor-input-setup".into())
        .spawn(move || {
            let result = (|| {
                let (id, config) = selected_input_config(rate, selected.as_ref())?;
                let (capture, feed) = MonitorCapture::open_on_control(id, config)
                    .map_err(|error| error.to_string())?;
                let stream = MonitorStream::new_on_control(feed, rate)
                    .map_err(|_| "input and output sample rates differ".to_owned())?;
                Ok(PreparedInput {
                    capture,
                    stream: Box::new(stream),
                })
            })();
            let _ = send.try_send(result);
        })
        .map_err(|error| error.to_string())?;
    Ok(receive)
}

impl EditorView {
    fn paused_input_target(&self) -> Option<Target> {
        let recorder = &self.audio_recorder;
        if !cfg!(all(target_os = "macos", target_arch = "aarch64"))
            || !recorder.software_monitor
            || recorder.blocks_edits()
            || recorder.retiring_monitor_output.is_some()
            || self.mode != EditorMode::Arrangement
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self.edit_pending()
        {
            return None;
        }
        let (generation, track) = recorder.armed_track?;
        let state = self.state.as_ref()?;
        let audio = self.audio.as_ref()?;
        let (source, transport_epoch) = audio.control.status()?.paused_input_source()?;
        if generation != self.project_generation
            || source.context.project_generation != generation
            || source.context.editor_revision != state.revision
        {
            return None;
        }
        Some(Target {
            request: PausedInputMonitorRequest {
                source: ResidentGraphSubmission {
                    output_identity: source.output_identity,
                    audio_revision: source.audio_revision,
                    context: source.context,
                },
                transport_epoch,
                track,
            },
            input: recorder.input.as_ref().map(|(id, _)| id.clone()),
            rate: audio.sample_rate,
        })
    }

    pub(super) fn tick_paused_input(&mut self) -> bool {
        let desired = self.paused_input_target();
        if self
            .audio_recorder
            .paused_input
            .failed
            .as_ref()
            .zip(desired.as_ref())
            .is_some_and(|(failed, current)| failed != current)
        {
            self.audio_recorder.paused_input.failed = None;
        }
        let phase = std::mem::take(&mut self.audio_recorder.paused_input.phase);
        let (phase, changed) = self.advance_paused_input(phase, desired.as_ref());
        self.audio_recorder.paused_input.phase = phase;
        changed
    }

    fn advance_paused_input(&mut self, phase: Phase, desired: Option<&Target>) -> (Phase, bool) {
        match phase {
            Phase::Idle => {
                let Some(target) = desired.filter(|target| {
                    Some(*target) != self.audio_recorder.paused_input.failed.as_ref()
                }) else {
                    return (Phase::Idle, false);
                };
                let (reply, receive) = mpsc::sync_channel(1);
                match self.service.submit(Request::PreparePausedInputMonitor {
                    request: target.request,
                    reply,
                }) {
                    Ok(()) => {
                        self.wake_poll();
                        self.status = self.audio_recorder.input_status("preparing input monitor");
                        (
                            Phase::Preparing {
                                target: target.clone(),
                                reply: receive,
                            },
                            true,
                        )
                    }
                    Err(SubmitError::Full(_)) => (Phase::Idle, false),
                    Err(SubmitError::Closed(_)) => {
                        self.paused_input_failed(target.clone(), "session worker is unavailable")
                    }
                    Err(SubmitError::Busy(_, _)) => {
                        self.paused_input_failed(target.clone(), "session preparation is busy")
                    }
                }
            }
            Phase::Preparing { target, reply } => match reply.try_recv() {
                Err(TryRecvError::Empty) => (Phase::Preparing { target, reply }, false),
                _ if desired != Some(&target) => (Phase::Idle, false),
                Ok(Ok(monitor)) => match self.audio_recorder.paused_input.open(&target) {
                    Ok(input) => (
                        Phase::Opening {
                            target,
                            monitor,
                            input,
                        },
                        false,
                    ),
                    Err(reason) => self.paused_input_failed(target, &reason),
                },
                Ok(Err(reason)) => self.paused_input_failed(target, &reason),
                Err(TryRecvError::Disconnected) => {
                    self.paused_input_failed(target, "session preparation ended")
                }
            },
            Phase::Opening {
                target,
                monitor,
                input,
            } => match input.try_recv() {
                Err(TryRecvError::Empty) => (
                    Phase::Opening {
                        target,
                        monitor,
                        input,
                    },
                    false,
                ),
                Ok(Ok(PreparedInput { capture, stream })) => {
                    if desired != Some(&target) {
                        capture.stop_on_control();
                        return (
                            Phase::Stopping {
                                capture,
                                releasing_output: None,
                            },
                            false,
                        );
                    }
                    let installed = self
                        .audio
                        .as_mut()
                        .filter(|audio| audio.control.output_identity() == Some(target.output()))
                        .and_then(|audio| audio.control.input_monitor())
                        .is_some_and(|control| {
                            control
                                .publish_paused(
                                    stream,
                                    monitor,
                                    target.source(),
                                    target.request.transport_epoch,
                                )
                                .is_ok()
                        });
                    if installed {
                        return (
                            Phase::Installed {
                                target,
                                capture,
                                started: false,
                                audible: false,
                            },
                            false,
                        );
                    }
                    capture.stop_on_control();
                    self.paused_input_failed(target, "output monitor is unavailable");
                    (
                        Phase::Stopping {
                            capture,
                            releasing_output: None,
                        },
                        true,
                    )
                }
                _ if desired != Some(&target) => (Phase::Idle, false),
                Ok(Err(reason)) => self.paused_input_failed(target, &reason),
                Err(TryRecvError::Disconnected) => {
                    self.paused_input_failed(target, "input setup ended")
                }
            },
            Phase::Installed {
                target,
                mut capture,
                mut started,
                mut audible,
            } => {
                if desired != Some(&target) {
                    let release = (!self.audio_recorder.software_monitor
                        || self.audio_recorder.armed_track.is_none())
                        && self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.control.status())
                            .and_then(|status| status.paused_input_source())
                            == Some((target.source(), target.request.transport_epoch));
                    self.retire_input_monitor(target.output(), release);
                    capture.stop_on_control();
                    return (
                        Phase::Stopping {
                            capture,
                            releasing_output: release.then(|| target.output()),
                        },
                        false,
                    );
                }
                let status = self
                    .audio
                    .as_mut()
                    .and_then(|audio| audio.control.input_monitor())
                    .map(|control| control.status());
                let action = monitor_action(
                    if started {
                        Monitoring::Active
                    } else {
                        Monitoring::Waiting
                    },
                    status,
                );
                let failure = capture
                    .take_open_error_on_control()
                    .or_else(|| match action {
                        MonitorAction::Failed(reason) => Some(reason.into()),
                        _ => None,
                    });
                if let Some(reason) = failure {
                    capture.stop_on_control();
                    self.stop_recording_monitor(target.output());
                    self.paused_input_failed(target, &reason);
                    return (
                        Phase::Stopping {
                            capture,
                            releasing_output: None,
                        },
                        true,
                    );
                }
                if action == MonitorAction::StartInput && capture.phase() == InputPhase::Ready {
                    started = capture.start_on_control();
                }
                let became_audible = !audible && status == Some(MonitorStatus::Running);
                if became_audible {
                    audible = true;
                    self.status = self
                        .audio_recorder
                        .input_status("monitoring armed audio input");
                }
                (
                    Phase::Installed {
                        target,
                        capture,
                        started,
                        audible,
                    },
                    became_audible,
                )
            }
            Phase::Stopping {
                capture,
                mut releasing_output,
            } => {
                if let Some(output) = releasing_output
                    && self.audio_recorder.retiring_monitor_output == Some(output)
                    && self.audio_recorder.software_monitor
                    && self.audio_recorder.armed_track.is_some()
                {
                    // A new activation should not wait for the old effects.
                    self.stop_recording_monitor(output);
                    releasing_output = None;
                }
                if matches!(capture.phase(), InputPhase::Finished(_))
                    && self.audio_recorder.retiring_monitor_output.is_none()
                {
                    (Phase::Idle, false)
                } else {
                    (
                        Phase::Stopping {
                            capture,
                            releasing_output,
                        },
                        false,
                    )
                }
            }
        }
    }

    fn paused_input_failed(&mut self, target: Target, reason: &str) -> (Phase, bool) {
        self.audio_recorder.paused_input.failed = Some(target);
        self.error = Some(format!("software monitoring unavailable · {reason}").into());
        (Phase::Idle, true)
    }
}

#[cfg(all(
    test,
    feature = "ui-test-support",
    target_os = "macos",
    target_arch = "aarch64"
))]
mod tests {
    use super::*;
    use crate::editor::worker_ui_fixture;
    use gpui::TestAppContext;
    use std::{
        sync::atomic::Ordering::Acquire,
        time::{Duration, Instant},
    };

    fn arm_monitor_fixture(
        cx: &mut TestAppContext,
        with_latency: bool,
    ) -> worker_ui_fixture::WorkerUiFixture {
        let mut commands = vec![hex_project::Command::AddTrack {
            name: "input".into(),
        }];
        if with_latency {
            commands.push(hex_project::Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            });
        }
        let mut fixture = worker_ui_fixture::open_for_paused_monitor(cx, commands);
        let (capture, feed, input) = hex_audio::worker_fixture::monitor_only_input().unwrap();
        let stream = Box::new(MonitorStream::new_on_control(feed, 48_000).ok().unwrap());
        let (send, receive) = mpsc::sync_channel(1);
        send.try_send(Ok(PreparedInput { capture, stream }))
            .ok()
            .unwrap();
        fixture.editor.update(cx, |view, _| {
            view.audio_recorder.paused_input.fixture_input = Some(receive);
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.toggle_audio_arm(track, view.project_generation);
            view.toggle_software_monitor();
            assert!(view.audio_recorder.software_monitor);
        });
        let mut block = [[0.0; 2]; 64];
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            fixture.editor.update(cx, |view, cx| {
                view.tick(cx);
            });
            fixture.backend.render(&mut block).unwrap();
            if block.iter().all(|frame| frame[0] > 0.0 && frame[1] > 0.0) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "armed monitor never became audible"
            );
            std::thread::yield_now();
        }
        assert!(input.started());
        fixture
    }

    #[gpui::test]
    fn stopped_software_monitor_reaches_the_armed_track_and_holds_record_until_retired(
        cx: &mut TestAppContext,
    ) {
        let mut fixture = arm_monitor_fixture(cx, false);
        fixture.editor.update(cx, |view, _| {
            let audio = view.audio.as_mut().unwrap();
            assert_eq!(audio.status().unwrap().position.load(Acquire), 0);
            audio
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .unwrap();
        });
        let mut block = [[0.0; 2]; 64];
        fixture.backend.render(&mut block).unwrap();
        assert_eq!(
            block, [[0.0; 2]; 64],
            "playing audio includes the retired input"
        );
        fixture.editor.update(cx, |view, _| {
            view.toggle_audio_recording();
            assert!(view.audio_recorder.record_after_monitor.is_some());
            assert_eq!(
                view.audio_recorder.label(),
                "cancel audio recording preparation"
            );
            view.tick_audio_recording();
            assert!(view.audio_recorder.record_after_monitor.is_some());
            assert!(view.audio_recorder.paused_input.pending());
            // Cancel before authorizing the real physical recording path.
            view.toggle_audio_recording();
            assert!(view.audio_recorder.record_after_monitor.is_none());
        });
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            fixture.backend.render(&mut block).unwrap();
            let finished = fixture.editor.update(cx, |view, _| {
                view.tick_audio_recording();
                !view.audio_recorder.needs_poll()
            });
            if finished {
                break;
            }
            assert!(Instant::now() < deadline, "old monitor did not retire");
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    fn stopped_monitor_off_drains_buffered_main_effect_audio(cx: &mut TestAppContext) {
        let mut fixture = arm_monitor_fixture(cx, true);
        fixture.editor.update(cx, |view, _| {
            view.toggle_software_monitor();
            view.tick_audio_recording();
        });
        let mut block = [[0.0; 2]; 64];
        fixture.backend.render(&mut block).unwrap();
        assert!(block.iter().any(|frame| frame[0] > 0.0));
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            fixture.backend.render(&mut block).unwrap();
            let finished = fixture.editor.update(cx, |view, _| {
                let status = view.audio.as_ref().unwrap().status().unwrap();
                assert_eq!(status.position.load(Acquire), 0);
                view.tick_audio_recording();
                !view.audio_recorder.needs_poll()
            });
            if finished {
                assert_eq!(block, [[0.0; 2]; 64]);
                break;
            }
            assert!(Instant::now() < deadline, "monitor tail did not retire");
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    fn failed_stopped_monitor_stays_closed_after_returning_its_endpoints(cx: &mut TestAppContext) {
        let mut fixture = arm_monitor_fixture(cx, false);
        let (send, receive) = mpsc::sync_channel(1);
        send.try_send(Err("unexpected input reopen".into()))
            .ok()
            .unwrap();
        fixture.editor.update(cx, |view, _| {
            view.audio_recorder.paused_input.fixture_input = Some(receive);
        });
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let finished = fixture.editor.update(cx, |view, cx| {
                view.tick(cx);
                !view.audio_recorder.needs_poll()
            });
            if finished {
                break;
            }
            assert!(Instant::now() < deadline, "failed input did not retire");
            std::thread::yield_now();
        }
        fixture.editor.update(cx, |view, cx| {
            view.tick(cx);
            assert!(!view.audio_recorder.needs_poll());
            assert!(
                view.error
                    .as_ref()
                    .is_some_and(|error| error.contains("input monitor underrun"))
            );
            assert!(view.audio_recorder.paused_input.fixture_input.is_some());
        });
    }
}
