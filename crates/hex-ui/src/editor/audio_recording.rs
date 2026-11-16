//! Physical-input Arrangement recording. GPUI holds only job handles and captured
//! identities; native streams, file writing, alignment and project edits retain
//! their existing audio and Session owners.
use super::{EditorMode, EditorView};
use hex_audio::{
    input_capture::{
        CoordinatedJob, CoordinatedOutcome, InputCapture, InputConfig, spawn_coordinated_take,
    },
    input_devices::{InputDevice, InputDeviceId, input_devices_on_control},
    prepared::AppliedAudioSource,
    sample_recording::Reader,
};
use hex_session::{import_batch_decode::AudioBatchContext, service::Request};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Release},
        },
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
struct Binding {
    source: AppliedAudioSource,
    track: hex_project::TrackId,
    context: AudioBatchContext,
}

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    Opening {
        binding: Binding,
        cancel: Arc<AtomicBool>,
        result: Receiver<Result<CoordinatedJob, String>>,
    },
    Capturing {
        binding: Binding,
        job: CoordinatedJob,
        started: bool,
        stopping: bool,
        invalidated: bool,
    },
}

#[derive(Default)]
pub(super) struct Recorder {
    stage: Stage,
    stop_transport_after: bool,
    pub(super) pending_commit_path: Option<PathBuf>,
    input: Option<(InputDeviceId, gpui::SharedString)>,
    listing_inputs: bool,
}
impl Recorder {
    pub(super) fn blocks_edits(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }
    pub(super) fn label(&self) -> &'static str {
        match self.stage {
            Stage::Idle => {
                "record selected track from chosen audio input · use hardware monitoring"
            }
            Stage::Opening { .. } => "cancel audio recording preparation",
            Stage::Capturing {
                started: false,
                stopping: false,
                ..
            } => "cancel audio recording preparation",
            Stage::Capturing {
                stopping: false, ..
            } => "stop audio recording",
            Stage::Capturing { stopping: true, .. } => "finishing audio recording",
        }
    }

    pub(super) fn input_label(&self) -> gpui::SharedString {
        self.input
            .as_ref()
            .map_or_else(|| "system default".into(), |(_, name)| name.clone())
    }

    fn input_status(&self, action: &str) -> String {
        format!(
            "{action} · {} · use hardware monitoring",
            hex_session::service::text::verbatim(self.input_label())
        )
    }
}

fn open_selected_take(
    reader: Reader,
    rate: u32,
    document: Option<PathBuf>,
    cancel: &AtomicBool,
    selected: Option<InputDeviceId>,
) -> Result<CoordinatedJob, String> {
    let inputs = input_devices_on_control().map_err(|error| error.to_string())?;
    let device = inputs
        .resolve_requested(selected.as_ref())
        .map_err(|error| error.to_string())?;
    let id = device.id.clone();
    let channels = [2, 1]
        .into_iter()
        .find(|channels| {
            device
                .formats
                .iter()
                .any(|format| format.supports_recording(rate, *channels))
        })
        .ok_or("selected audio input does not support f32 at the project sample rate")?;
    if cancel.load(Acquire) {
        return Err("audio recording canceled".into());
    }
    let input = InputCapture::open_on_control(
        id,
        InputConfig {
            sample_rate: rate,
            channels,
        },
    )
    .map_err(|error| error.to_string())?;
    let root = hex_session::service::sample_take_root(document.as_deref())?;
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path = root.join(format!("audio-take-{stamp}-{}.wav", std::process::id()));
    if cancel.load(Acquire) {
        return Err("audio recording canceled".into());
    }
    let job = spawn_coordinated_take(input, reader, path).map_err(|error| error.to_string())?;
    if cancel.load(Acquire) {
        job.stop_on_control();
    }
    Ok(job)
}

impl EditorView {
    pub(super) fn open_audio_input_menu(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.audio_recorder.blocks_edits() || self.audio_recorder.listing_inputs {
            return;
        }
        self.audio_recorder.listing_inputs = true;
        self.status = "scanning audio inputs".into();
        let position = window.mouse_position();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { input_devices_on_control() })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                view.audio_recorder.listing_inputs = false;
                match result {
                    Ok(inputs)
                        if !view.audio_recorder.blocks_edits() && view.context_menu.is_none() =>
                    {
                        view.show_audio_input_menu(inputs.devices().to_vec(), position, window, cx);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        view.error = Some(format!("audio input scan failed · {error}").into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_audio_input_menu(
        &mut self,
        inputs: Vec<InputDevice>,
        position: gpui::Point<gpui::Pixels>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use super::context_menu::EditorContextAction;
        use crate::context_menu::{MenuItem, PopupMenu};
        let focus = cx.focus_handle();
        let previous_focus = window.focused(cx);
        focus.focus(window);
        let selected = self.audio_recorder.input.as_ref().map(|(id, _)| id);
        if let Some((id, name)) = &self.audio_recorder.input
            && !inputs.iter().any(|device| &device.id == id)
        {
            self.error = Some(
                format!(
                    "selected audio input is unavailable · {}",
                    hex_session::service::text::verbatim(name)
                )
                .into(),
            );
        }
        let mut items = vec![MenuItem {
            label: "system default",
            swatch: None,
            checked: selected.is_none(),
            action: EditorContextAction::AudioInput {
                id: None,
                name: "system default".into(),
            },
        }];
        let mut counts = std::collections::HashMap::<&str, usize>::new();
        for device in &inputs {
            *counts.entry(&device.name).or_default() += 1;
        }
        items.extend(inputs.iter().map(|device| {
            let name = if counts.get(device.name.as_str()).copied().unwrap_or(0) > 1 {
                format!("{} · {}", device.name, device.id)
            } else {
                device.name.clone()
            };
            MenuItem {
                label: "audio input",
                swatch: None,
                checked: selected == Some(&device.id),
                action: EditorContextAction::AudioInput {
                    id: Some(device.id.clone()),
                    name: name.into(),
                },
            }
        }));
        let selected = items.iter().position(|item| item.checked).unwrap_or(0);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected,
            focus,
            previous_focus,
            items,
        });
        self.status = "choose audio input · use hardware monitoring".into();
    }

    pub(super) fn select_audio_input(
        &mut self,
        id: Option<InputDeviceId>,
        name: gpui::SharedString,
    ) {
        if self.audio_recorder.blocks_edits() {
            return;
        }
        self.audio_recorder.input = id.map(|id| (id, name.clone()));
        self.error = None;
        self.status = format!(
            "audio input · {}",
            hex_session::service::text::verbatim(&name)
        );
    }

    pub(super) fn toggle_audio_recording(&mut self) {
        self.wake_poll();
        match &mut self.audio_recorder.stage {
            Stage::Opening {
                binding, cancel, ..
            } => {
                cancel.store(true, Release);
                if let Some(audio) = self.audio.as_ref().filter(|audio| {
                    audio.control.output_identity() == Some(binding.source.output_identity)
                }) {
                    audio.control.stop_sample_recording();
                }
                self.status = "canceling audio recording".into();
                return;
            }
            Stage::Capturing {
                job,
                started,
                stopping,
                invalidated,
                ..
            } => {
                if !*stopping {
                    *invalidated |= !*started;
                    job.stop_on_control();
                    *stopping = true;
                }
                self.status = "finishing audio recording".into();
                return;
            }
            Stage::Idle => {}
        }
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            self.error = Some("audio recording currently requires macos arm64".into());
            return;
        }
        if self.mode != EditorMode::Arrangement
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self.edit_pending()
            || self.audio_batch.is_some()
            || self.pending_playback.is_some()
            || !self.playback_candidates.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.native_automation_recorder.busy()
            || self.automation_take_recovery.unresolved()
        {
            self.error = Some(
                "finish pending edits and use resident arrangement playback before recording audio"
                    .into(),
            );
            return;
        }
        let Some(state) = self.state.as_ref() else {
            self.error = Some("no project is available".into());
            return;
        };
        let Some(track) = self.selected_track.filter(|track| {
            state
                .project
                .track(*track)
                .is_some_and(|track| !track.is_group())
        }) else {
            self.error = Some("select an arrangement track before recording audio".into());
            return;
        };
        let context = AudioBatchContext {
            project_generation: self.project_generation,
            revision: state.revision,
            active_gesture: false,
        };
        let document = state.document_path.clone();
        let Some(audio) = self.audio.as_mut() else {
            self.error = Some("audio output is unavailable".into());
            return;
        };
        let Some(status) = audio.control.status() else {
            self.error = Some("audio output is unavailable".into());
            return;
        };
        if !status.playing.load(Acquire) {
            self.error = Some("start playback before recording audio".into());
            return;
        }
        let Some(source) = status.recording_source().filter(|source| {
            source.context.project_generation == context.project_generation
                && source.context.editor_revision == context.revision
        }) else {
            self.error = Some("wait for the current project audio before recording".into());
            return;
        };
        let rate = audio.sample_rate;
        let reader = match audio.control.start_bound_output_recording(source) {
            Ok(reader) => reader,
            Err(error) => {
                self.error = Some(error.to_string().into());
                return;
            }
        };
        let binding = Binding {
            source,
            track,
            context,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let input = self.audio_recorder.input.as_ref().map(|(id, _)| id.clone());
        let (sender, result) = mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("hex-record-input-setup".into())
            .spawn(move || {
                let job = open_selected_take(reader, rate, document, &worker_cancel, input);
                let _ = sender.send(job);
            })
        {
            self.error = Some(format!("audio recording setup failed · {error}").into());
            return;
        }
        self.audio_recorder.stage = Stage::Opening {
            binding,
            cancel,
            result,
        };
        self.audio_recorder.stop_transport_after = false;
        self.error = None;
        self.status = self.audio_recorder.input_status("preparing audio input");
    }

    /// Defer a transport Stop only while a running input take is being drained.
    pub(super) fn finish_audio_recording_before_stop(&mut self) -> bool {
        match &mut self.audio_recorder.stage {
            Stage::Opening {
                binding, cancel, ..
            } => {
                cancel.store(true, Release);
                if let Some(audio) = self.audio.as_ref().filter(|audio| {
                    audio.control.output_identity() == Some(binding.source.output_identity)
                }) {
                    audio.control.stop_sample_recording();
                }
                false
            }
            Stage::Capturing {
                job,
                started,
                stopping,
                invalidated,
                ..
            } => {
                *invalidated |= !*started;
                job.stop_on_control();
                *stopping = true;
                self.audio_recorder.stop_transport_after = true;
                self.status = "finishing audio recording before stop".into();
                true
            }
            Stage::Idle => false,
        }
    }

    pub(super) fn tick_audio_recording(&mut self) -> bool {
        let stage = std::mem::take(&mut self.audio_recorder.stage);
        match stage {
            Stage::Idle => false,
            Stage::Opening {
                binding,
                cancel,
                result,
            } => match result.try_recv() {
                Ok(Ok(job)) => {
                    let stopping = cancel.load(Acquire);
                    if stopping {
                        job.stop_on_control();
                    }
                    self.audio_recorder.stage = Stage::Capturing {
                        binding,
                        job,
                        started: false,
                        stopping,
                        invalidated: stopping,
                    };
                    self.status = if stopping {
                        "canceling audio recording".into()
                    } else {
                        self.audio_recorder.input_status("preparing audio input")
                    };
                    true
                }
                Ok(Err(error)) => {
                    if let Some(audio) = self.audio.as_mut().filter(|audio| {
                        audio.control.output_identity() == Some(binding.source.output_identity)
                    }) {
                        audio.control.reclaim_sample_recording();
                    }
                    if cancel.load(Acquire) {
                        self.status = "audio recording canceled".into();
                    } else {
                        self.error = Some(format!("audio recording failed · {error}").into());
                    }
                    true
                }
                Err(TryRecvError::Empty) => {
                    if self.project_generation != binding.context.project_generation
                        || self
                            .state
                            .as_ref()
                            .is_none_or(|state| state.revision != binding.context.revision)
                        || self
                            .audio
                            .as_ref()
                            .and_then(|audio| audio.control.output_identity())
                            != Some(binding.source.output_identity)
                    {
                        cancel.store(true, Release);
                        if let Some(audio) = self.audio.as_ref().filter(|audio| {
                            audio.control.output_identity() == Some(binding.source.output_identity)
                        }) {
                            audio.control.stop_sample_recording();
                        }
                    }
                    self.audio_recorder.stage = Stage::Opening {
                        binding,
                        cancel,
                        result,
                    };
                    false
                }
                Err(TryRecvError::Disconnected) => {
                    if let Some(audio) = self.audio.as_mut().filter(|audio| {
                        audio.control.output_identity() == Some(binding.source.output_identity)
                    }) {
                        audio.control.stop_sample_recording();
                        audio.control.reclaim_sample_recording();
                    }
                    self.error = Some("audio recording setup ended unexpectedly".into());
                    true
                }
            },
            Stage::Capturing {
                binding,
                mut job,
                mut started,
                mut stopping,
                mut invalidated,
            } => {
                if self.project_generation != binding.context.project_generation
                    || self
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != binding.context.revision)
                    || self
                        .audio
                        .as_ref()
                        .and_then(|audio| audio.control.output_identity())
                        != Some(binding.source.output_identity)
                {
                    job.stop_on_control();
                    stopping = true;
                    invalidated = true;
                }
                let became_started = !started && !stopping && job.input_capturing_on_control();
                if became_started {
                    started = true;
                    self.status = self
                        .audio_recorder
                        .input_status("recording audio input · click to stop");
                }
                match job.try_result_on_control() {
                    Ok(None) => {
                        self.audio_recorder.stage = Stage::Capturing {
                            binding,
                            job,
                            started,
                            stopping,
                            invalidated,
                        };
                        became_started
                    }
                    Ok(Some(outcome)) => {
                        if let Some(audio) = self.audio.as_mut().filter(|audio| {
                            audio.control.output_identity() == Some(binding.source.output_identity)
                        }) {
                            audio.control.reclaim_sample_recording();
                        }
                        let stop_after =
                            std::mem::take(&mut self.audio_recorder.stop_transport_after);
                        if stop_after {
                            self.stop();
                        }
                        self.finish_audio_take(binding, outcome, invalidated);
                        true
                    }
                    Err(_) => {
                        if let Some(audio) = self.audio.as_mut().filter(|audio| {
                            audio.control.output_identity() == Some(binding.source.output_identity)
                        }) {
                            audio.control.stop_sample_recording();
                            audio.control.reclaim_sample_recording();
                        }
                        if std::mem::take(&mut self.audio_recorder.stop_transport_after) {
                            self.stop();
                        }
                        self.error = Some(
                            format!(
                                "audio recording ended without a result · take path {}",
                                hex_session::service::text::verbatim(job.path().display())
                            )
                            .into(),
                        );
                        true
                    }
                }
            }
        }
    }

    fn finish_audio_take(
        &mut self,
        binding: Binding,
        outcome: CoordinatedOutcome,
        invalidated: bool,
    ) {
        if invalidated {
            let path = match &outcome {
                CoordinatedOutcome::Completed { path, .. }
                | CoordinatedOutcome::Failed { path, .. } => path,
            };
            self.error = Some(
                format!(
                    "audio recording canceled or project changed · take path {}",
                    hex_session::service::text::verbatim(path.display())
                )
                .into(),
            );
            return;
        }
        if let CoordinatedOutcome::Failed { path, reason, .. } = &outcome {
            self.error = Some(
                format!(
                    "audio recording failed · {reason} · take path {}",
                    hex_session::service::text::verbatim(path.display())
                )
                .into(),
            );
            return;
        }
        let take = match outcome.arrangement_take(binding.source) {
            Ok(take) => take,
            Err(error) => {
                let path = match &outcome {
                    CoordinatedOutcome::Completed { path, .. }
                    | CoordinatedOutcome::Failed { path, .. } => path,
                };
                self.error = Some(
                    format!(
                        "audio recording failed · {error} · take path {}",
                        hex_session::service::text::verbatim(path.display())
                    )
                    .into(),
                );
                return;
            }
        };
        self.commit_audio_take(binding, take.path.to_owned(), take.timeline_start);
    }

    fn commit_audio_take(&mut self, binding: Binding, path: PathBuf, timeline_start: u64) {
        if self.audio_batch.is_some() || self.device_gesture.snapshot().is_some() {
            self.error = Some(
                format!(
                    "audio recording could not be committed · take retained at {}",
                    hex_session::service::text::verbatim(path.display())
                )
                .into(),
            );
            return;
        }
        let (request, control) =
            match Request::recorded_audio_take(
                path.clone(),
                binding.track,
                timeline_start,
                binding.context,
            ) {
                Ok(request) => request,
                Err(error) => {
                    self.error = Some(format!(
                    "audio recording could not be committed · {error} · take retained at {}",
                    hex_session::service::text::verbatim(path.display())
                ).into());
                    return;
                }
            };
        if self.enqueue_recorded_audio(request) {
            self.audio_batch = Some(control);
            self.audio_batch_progress = None;
            self.audio_recorder.pending_commit_path = Some(path);
            self.status = "committing recorded audio".into();
        } else {
            self.error = Some(
                format!(
                    "audio recording could not be queued · take retained at {}",
                    hex_session::service::text::verbatim(path.display())
                )
                .into(),
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext};

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn input_menu_selects_exact_identity_and_preserves_device_name(cx: &mut TestAppContext) {
        let (project, _, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let first: InputDeviceId = "coreaudio:interface-a".parse().unwrap();
        let second: InputDeviceId = "coreaudio:interface-b".parse().unwrap();
        cx.update_window(window, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.show_audio_input_menu(
                    vec![
                        InputDevice {
                            id: first.clone(),
                            name: "Studio Input".into(),
                            formats: vec![],
                        },
                        InputDevice {
                            id: second.clone(),
                            name: "Studio Input".into(),
                            formats: vec![],
                        },
                    ],
                    gpui::point(gpui::px(100.0), gpui::px(100.0)),
                    window,
                    cx,
                );
                assert_eq!(view.context_menu.as_ref().unwrap().items.len(), 3);
                let label = super::super::context_menu::context_item_label(
                    &view.context_menu.as_ref().unwrap().items[2],
                );
                assert_eq!(
                    crate::theme::text(label).as_ref(),
                    "Studio Input · coreaudio:interface-b"
                );
                cx.notify();
            });
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let panel = visual.debug_bounds("context-menu-panel").unwrap();
        let second_row = gpui::point(panel.left() + gpui::px(40.0), panel.top() + gpui::px(62.0));
        visual.simulate_mouse_down(second_row, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(second_row, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.audio_recorder.input.as_ref().map(|(id, _)| id),
                Some(&second)
            );
            assert_eq!(
                view.audio_recorder.input_label().as_ref(),
                "Studio Input · coreaudio:interface-b"
            );
        });
        cx.update_window(window, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.show_audio_input_menu(
                    vec![InputDevice {
                        id: first,
                        name: "Studio Input".into(),
                        formats: vec![],
                    }],
                    gpui::point(gpui::px(100.0), gpui::px(100.0)),
                    window,
                    cx,
                );
                assert_eq!(
                    view.audio_recorder.input.as_ref().map(|(id, _)| id),
                    Some(&second)
                );
                assert!(
                    view.error
                        .as_ref()
                        .unwrap()
                        .contains("selected audio input is unavailable")
                );
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn transport_record_control_refuses_unavailable_output(cx: &mut TestAppContext) {
        let (project, track, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        editor.update(&mut visual, |view, _| {
            view.mode = EditorMode::Arrangement;
            view.selected_track = Some(track);
        });
        visual.run_until_parked();
        let button = visual.debug_bounds("audio-record-toggle").unwrap().center();
        visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("audio output is unavailable")
            );
            assert!(!view.audio_recorder.blocks_edits());
        });
    }

    #[gpui::test]
    fn close_is_refused_during_audio_preparation(cx: &mut TestAppContext) {
        let (project, track, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let (_sender, result) = mpsc::sync_channel(1);
        cx.update_window(window, |_, window, cx| {
            editor.update(cx, |view, cx| {
                let context = AudioBatchContext {
                    project_generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    active_gesture: false,
                };
                view.audio_recorder.stage = Stage::Opening {
                    binding: Binding {
                        source: AppliedAudioSource {
                            context: hex_engine::AudioSourceContext {
                                project_generation: context.project_generation,
                                editor_revision: context.revision,
                            },
                            audio_revision: 1,
                            output_identity: 2,
                        },
                        track,
                        context,
                    },
                    cancel: Arc::new(AtomicBool::new(false)),
                    result,
                };
                view.request_window_close(window, cx);
                assert!(view.error.as_ref().unwrap().contains("finish pending work"));
            });
        })
        .unwrap();
        assert!(!cx.windows().is_empty());
    }

    #[gpui::test]
    fn preparation_cancellation_and_failed_take_do_not_submit_a_clip(cx: &mut TestAppContext) {
        let (project, track, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        let path = std::env::temp_dir().join(format!(
            "Hex-Audio-Take-{}-MixedCase.wav",
            std::process::id()
        ));
        std::fs::write(&path, b"retained").unwrap();
        let (sender, result) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        editor.update(&mut visual, |view, _| {
            let binding = Binding {
                source: AppliedAudioSource {
                    context: hex_engine::AudioSourceContext {
                        project_generation: view.project_generation,
                        editor_revision: view.state.as_ref().unwrap().revision,
                    },
                    audio_revision: 1,
                    output_identity: 2,
                },
                track,
                context: AudioBatchContext {
                    project_generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    active_gesture: false,
                },
            };
            view.audio_recorder.stage = Stage::Opening {
                binding,
                cancel: Arc::clone(&cancel),
                result,
            };
            assert!(view.audio_recorder.blocks_edits());
            view.toggle_midi_recording();
            assert!(!view.midi_recorder.blocks_edits());
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("before recording midi")
            );
            assert!(
                view.begin_device_gesture(
                    hex_session::device_gesture::GestureTarget::TrackGain { track },
                    hex_session::device_gesture::ParameterValue::Float(1.0),
                )
                .is_err()
            );
            assert!(!view.enqueue(Request::Edit(hex_project::Command::AddTrack {
                name: "blocked".into()
            })));
            assert!(!view.finish_audio_recording_before_stop());
            assert!(cancel.load(Acquire));
            sender.send(Err("canceled".into())).unwrap();
            assert!(view.tick_audio_recording());
            assert!(!view.audio_recorder.blocks_edits());
            view.finish_audio_take(
                binding,
                CoordinatedOutcome::Failed {
                    path: path.clone(),
                    input: None,
                    reason: hex_audio::input_capture::CoordinatedFailure::InputInterrupted,
                },
                false,
            );
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("input recording was interrupted")
            );
            assert!(view.audio_batch.is_none());
            assert!(view.audio_recorder.pending_commit_path.is_none());
        });
        assert_eq!(std::fs::read(&path).unwrap(), b"retained");
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn recorded_take_commits_at_captured_frame_without_import_auto_warp(cx: &mut TestAppContext) {
        use hex_session::service::Service;
        use std::time::{Duration, Instant};

        fn next(service: &Service) -> hex_session::service::Event {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                if let Ok(event) = service.try_event() {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    return event;
                }
                assert!(Instant::now() < deadline, "session setup timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
        let path = files.wav("Recorded.wav", 48_000, 480);
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "record".into(),
            }))
            .unwrap();
        let state = next(&service).state;
        let track = state.project.tracks()[0].id();
        let window = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(window, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline ui test".into()))
                });
                editor.update(cx, |view, _| {
                    view.state = Some(state);
                    view.set_auto_warp_imports_for_test(true);
                    let context = AudioBatchContext {
                        project_generation: view.project_generation,
                        revision: view.state.as_ref().unwrap().revision,
                        active_gesture: false,
                    };
                    let binding = Binding {
                        source: AppliedAudioSource {
                            context: hex_engine::AudioSourceContext {
                                project_generation: context.project_generation,
                                editor_revision: context.revision,
                            },
                            audio_revision: 1,
                            output_identity: 2,
                        },
                        track,
                        context,
                    };
                    view.commit_audio_take(binding, path.clone(), 128);
                    assert!(view.audio_batch.is_some());
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(window, cx);
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let done = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.audio_batch.is_none() && view.worker_requests_pending == 0
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "recorded take commit timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let clips = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .clips();
            assert_eq!(clips.len(), 1);
            assert_eq!(clips[0].start(), 128);
            assert_eq!(clips[0].duration(), 480);
            assert!(
                clips[0]
                    .audio_edit()
                    .is_none_or(|edit| !edit.domain.warp_enabled)
            );
            assert!(view.audio_recorder.pending_commit_path.is_none());
        });
        assert!(path.exists());
    }
}
