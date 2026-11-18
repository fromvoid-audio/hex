//! Physical-input Arrangement recording. GPUI holds only job handles and captured
//! identities; native streams, file writing, alignment and project edits retain
//! their existing audio and Session owners.
use super::{EditorMode, EditorView};
mod paused_input;
use hex_audio::{
    input_capture::{
        CoordinatedJob, CoordinatedOutcome, InputCapture, InputConfig, MonitorStream,
        spawn_coordinated_take, spawn_coordinated_take_gated,
    },
    input_devices::{InputDevice, InputDeviceId, input_devices_on_control},
    prepared::{AppliedAudioSource, input_monitor::MonitorStatus},
    sample_recording::Reader,
};
use hex_session::{import_batch_decode::AudioBatchContext, service::Request};
use paused_input::PausedInput;
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

struct PreparedTake {
    job: CoordinatedJob,
    monitor: Option<Box<MonitorStream>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Monitoring {
    Hardware,
    Waiting,
    Active,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorAction {
    Wait,
    StartInput,
    Continue,
    Failed(&'static str),
}

fn monitor_action(phase: Monitoring, status: Option<MonitorStatus>) -> MonitorAction {
    match status {
        Some(MonitorStatus::Pending) => MonitorAction::Wait,
        Some(MonitorStatus::Priming | MonitorStatus::Running) if phase == Monitoring::Waiting => {
            MonitorAction::StartInput
        }
        Some(MonitorStatus::Priming | MonitorStatus::Running) => MonitorAction::Continue,
        Some(MonitorStatus::SourceChanged) => MonitorAction::Failed("audio source changed"),
        Some(MonitorStatus::Unsupported) => {
            MonitorAction::Failed("output configuration is unsupported")
        }
        Some(MonitorStatus::InputEnded) => MonitorAction::Failed("audio input ended"),
        Some(MonitorStatus::Underrun) => MonitorAction::Failed("input monitor underrun"),
        Some(MonitorStatus::Overrun) => MonitorAction::Failed("input monitor overrun"),
        Some(MonitorStatus::Discontinuity) => MonitorAction::Failed("input monitor discontinuity"),
        Some(MonitorStatus::OutputFault) => MonitorAction::Failed("audio output failed"),
        Some(MonitorStatus::Idle | MonitorStatus::Stopped) | None => {
            MonitorAction::Failed("audio output is unavailable")
        }
    }
}

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    Opening {
        binding: Binding,
        cancel: Arc<AtomicBool>,
        result: Receiver<Result<PreparedTake, String>>,
    },
    Capturing {
        binding: Binding,
        job: CoordinatedJob,
        started: bool,
        stopping: bool,
        invalidated: bool,
        monitoring: Monitoring,
        monitor_installed: bool,
        monitor_error: Option<&'static str>,
    },
}

#[derive(Default)]
pub(super) struct Recorder {
    stage: Stage,
    paused_input: PausedInput,
    record_after_monitor: Option<Binding>,
    stop_transport_after: bool,
    retiring_monitor_output: Option<u64>,
    pub(super) pending_commit_path: Option<PathBuf>,
    input: Option<(InputDeviceId, gpui::SharedString)>,
    listing_inputs: bool,
    pub(super) software_monitor: bool,
    armed_track: Option<(u64, hex_project::TrackId)>,
}
impl Recorder {
    pub(super) fn blocks_edits(&self) -> bool {
        !matches!(self.stage, Stage::Idle) || self.record_after_monitor.is_some()
    }
    pub(super) fn needs_poll(&self) -> bool {
        self.blocks_edits() || self.retiring_monitor_output.is_some() || self.paused_input.pending()
    }
    pub(super) fn label(&self) -> &'static str {
        match self.stage {
            Stage::Idle if self.record_after_monitor.is_some() => {
                "cancel audio recording preparation"
            }
            Stage::Idle if self.armed_track.is_some() => {
                "record armed track from chosen audio input"
            }
            Stage::Idle => "record selected track from chosen audio input",
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
            "{action} · {} · {} monitoring",
            hex_session::service::text::verbatim(self.input_label()),
            if self.software_monitor {
                "software"
            } else {
                "hardware"
            }
        )
    }

    pub(super) fn is_audio_armed(&self, generation: u64, track: hex_project::TrackId) -> bool {
        self.armed_track == Some((generation, track))
    }

    pub(super) fn reconcile_audio_arm(&mut self, project: &hex_project::Project, replaced: bool) {
        if replaced
            || self
                .armed_track
                .is_some_and(|(_, id)| project.track(id).is_none_or(|track| track.is_group()))
        {
            self.armed_track = None;
        }
    }

    fn target_track(
        &self,
        generation: u64,
        project: &hex_project::Project,
        selected: Option<hex_project::TrackId>,
    ) -> Option<hex_project::TrackId> {
        self.armed_track
            .filter(|(owner, _)| *owner == generation)
            .map(|(_, id)| id)
            .or(selected)
            .filter(|id| project.track(*id).is_some_and(|track| !track.is_group()))
    }
}

fn selected_input_config(
    rate: u32,
    selected: Option<&InputDeviceId>,
) -> Result<(InputDeviceId, InputConfig), String> {
    let inputs = input_devices_on_control().map_err(|error| error.to_string())?;
    let device = inputs
        .resolve_requested(selected)
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
    Ok((
        id,
        InputConfig {
            sample_rate: rate,
            channels,
        },
    ))
}

fn open_selected_take(
    reader: Reader,
    rate: u32,
    document: Option<PathBuf>,
    cancel: &AtomicBool,
    selected: Option<InputDeviceId>,
    software_monitor: bool,
) -> Result<PreparedTake, String> {
    let (id, config) = selected_input_config(rate, selected.as_ref())?;
    if cancel.load(Acquire) {
        return Err("audio recording canceled".into());
    }
    let (input, monitor) = if software_monitor {
        let (input, monitor) = InputCapture::open_with_monitor_on_control(id, config)
            .map_err(|error| error.to_string())?;
        let stream = MonitorStream::new_on_control(monitor, rate)
            .map_err(|_| "input and output sample rates differ".to_string())?;
        (input, Some(Box::new(stream)))
    } else {
        (
            InputCapture::open_on_control(id, config).map_err(|error| error.to_string())?,
            None,
        )
    };
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
    let job = if software_monitor {
        spawn_coordinated_take_gated(input, reader, path)
    } else {
        spawn_coordinated_take(input, reader, path)
    }
    .map_err(|error| error.to_string())?;
    if cancel.load(Acquire) {
        job.stop_on_control();
    }
    Ok(PreparedTake { job, monitor })
}

impl EditorView {
    pub(super) fn toggle_audio_arm(&mut self, track: hex_project::TrackId, generation: u64) {
        if generation != self.project_generation {
            return;
        }
        if self.audio_recorder.blocks_edits() {
            self.error = Some("finish the audio recording before changing its armed track".into());
            return;
        }
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident
            || self
                .state
                .as_ref()
                .and_then(|state| state.project.track(track))
                .is_none_or(|track| track.is_group())
        {
            self.error =
                Some("audio recording requires an ordinary resident arrangement track".into());
            return;
        }
        self.audio_recorder.armed_track = if self
            .audio_recorder
            .is_audio_armed(self.project_generation, track)
        {
            None
        } else {
            Some((self.project_generation, track))
        };
        self.audio_recorder.paused_input.user_changed_target();
        self.status = if self.audio_recorder.armed_track.is_some() {
            "track armed for audio recording"
        } else {
            "track disarmed for audio recording"
        }
        .into();
        self.wake_poll();
    }

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
        self.status = "choose audio input".into();
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
        self.audio_recorder.paused_input.user_changed_target();
        self.error = None;
        self.status = format!(
            "audio input · {}",
            hex_session::service::text::verbatim(&name)
        );
        self.wake_poll();
    }

    pub(super) fn toggle_software_monitor(&mut self) {
        if self.audio_recorder.blocks_edits() {
            self.error = Some("finish audio recording before changing monitoring".into());
            return;
        }
        self.audio_recorder.software_monitor = !self.audio_recorder.software_monitor;
        self.audio_recorder.paused_input.user_changed_target();
        self.status = if self.audio_recorder.software_monitor {
            "software monitoring enabled"
        } else {
            "hardware monitoring during recording"
        }
        .into();
        self.wake_poll();
    }

    pub(super) fn toggle_audio_recording(&mut self) {
        self.wake_poll();
        if self.audio_recorder.record_after_monitor.take().is_some() {
            self.status = "audio recording canceled".into();
            return;
        }
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
        if self.audio_recorder.retiring_monitor_output.is_some()
            && !self.audio_recorder.paused_input.pending()
        {
            self.error = Some("wait for software monitoring to finish stopping".into());
            return;
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
        let Some(track) = self.audio_recorder.target_track(
            self.project_generation,
            &state.project,
            self.selected_track,
        ) else {
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
        let binding = Binding {
            source,
            track,
            context,
        };
        if self.audio_recorder.paused_input.pending() {
            self.audio_recorder.record_after_monitor = Some(binding);
            self.error = None;
            self.status = "releasing input monitor before recording".into();
            return;
        }
        let rate = audio.sample_rate;
        let reader = match audio.control.start_bound_output_recording(source) {
            Ok(reader) => reader,
            Err(error) => {
                self.error = Some(error.to_string().into());
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let input = self.audio_recorder.input.as_ref().map(|(id, _)| id.clone());
        let software_monitor = self.audio_recorder.software_monitor;
        let (sender, result) = mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("hex-record-input-setup".into())
            .spawn(move || {
                let job = open_selected_take(
                    reader,
                    rate,
                    document,
                    &worker_cancel,
                    input,
                    software_monitor,
                );
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
        self.audio_recorder.record_after_monitor = None;
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

    fn stop_recording_monitor(&mut self, output_identity: u64) {
        if let Some(control) = self
            .audio
            .as_mut()
            .filter(|audio| audio.control.output_identity() == Some(output_identity))
            .and_then(|audio| audio.control.input_monitor())
        {
            control.stop();
            if control.reclaim().is_none() {
                self.audio_recorder.retiring_monitor_output = Some(output_identity);
            }
        }
    }

    pub(super) fn tick_audio_recording(&mut self) -> bool {
        if let Some(identity) = self.audio_recorder.retiring_monitor_output {
            let finished = match self
                .audio
                .as_mut()
                .filter(|audio| audio.control.output_identity() == Some(identity))
            {
                Some(audio)
                    if audio
                        .control
                        .status()
                        .is_some_and(|status| !status.device_fault.load(Acquire)) =>
                {
                    audio
                        .control
                        .input_monitor()
                        .is_none_or(|control| control.reclaim().is_some())
                }
                _ => true,
            };
            if finished {
                self.audio_recorder.retiring_monitor_output = None;
            }
        }
        let paused_changed = self.tick_paused_input();
        if !self.audio_recorder.paused_input.pending()
            && self.audio_recorder.retiring_monitor_output.is_none()
            && let Some(binding) = self.audio_recorder.record_after_monitor.take()
        {
            if self.record_target_is_current(binding) {
                self.toggle_audio_recording();
            } else {
                self.error = Some("audio recording canceled · input target changed".into());
            }
            return true;
        }
        let stage = std::mem::take(&mut self.audio_recorder.stage);
        match stage {
            Stage::Idle => paused_changed,
            Stage::Opening {
                binding,
                cancel,
                result,
            } => match result.try_recv() {
                Ok(Ok(PreparedTake { job, monitor })) => {
                    let mut stopping = cancel.load(Acquire)
                        || self.project_generation != binding.context.project_generation
                        || self
                            .state
                            .as_ref()
                            .is_none_or(|state| state.revision != binding.context.revision);
                    let mut monitoring = Monitoring::Hardware;
                    let mut monitor_installed = false;
                    let mut monitor_error = None;
                    if let Some(stream) = monitor {
                        monitoring = Monitoring::Failed;
                        if !stopping {
                            let installed = self
                                .audio
                                .as_mut()
                                .filter(|audio| {
                                    audio.control.output_identity()
                                        == Some(binding.source.output_identity)
                                        && audio
                                            .control
                                            .status()
                                            .and_then(|status| status.recording_source())
                                            == Some(binding.source)
                                })
                                .and_then(|audio| audio.control.input_monitor())
                                .is_some_and(|control| {
                                    control.reclaim();
                                    control
                                        .publish(
                                            stream,
                                            binding.source,
                                            hex_engine::OutputCaptureId::Track(binding.track),
                                        )
                                        .is_ok()
                                });
                            if installed {
                                monitor_installed = true;
                                monitoring = Monitoring::Waiting;
                            } else {
                                self.error = Some("software monitoring is unavailable · recording was not started".into());
                                monitor_error = Some("output is unavailable or busy");
                                stopping = true;
                            }
                        }
                    }
                    if stopping {
                        job.stop_on_control();
                    }
                    self.audio_recorder.stage = Stage::Capturing {
                        binding,
                        job,
                        started: false,
                        stopping,
                        invalidated: stopping,
                        monitoring,
                        monitor_installed,
                        monitor_error,
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
                mut monitoring,
                monitor_installed,
                mut monitor_error,
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
                let mut monitor_changed = false;
                if monitoring != Monitoring::Hardware && monitoring != Monitoring::Failed {
                    let monitor_status = self
                        .audio
                        .as_mut()
                        .filter(|audio| {
                            audio.control.output_identity() == Some(binding.source.output_identity)
                        })
                        .and_then(|audio| audio.control.input_monitor())
                        .map(|control| {
                            if stopping {
                                control.stop();
                            }
                            control.status()
                        });
                    if !stopping {
                        match monitor_action(monitoring, monitor_status) {
                            MonitorAction::StartInput => {
                                job.permit_input_start_on_control();
                                monitoring = Monitoring::Active;
                                monitor_changed = true;
                            }
                            MonitorAction::Failed(reason) => {
                                self.error =
                                    Some(format!("software monitoring ended · {reason}").into());
                                monitor_error = Some(reason);
                                if started {
                                    self.status =
                                        "recording audio input · monitoring unavailable".into();
                                }
                                if !started {
                                    job.stop_on_control();
                                    stopping = true;
                                    invalidated = true;
                                }
                                monitoring = Monitoring::Failed;
                                monitor_changed = true;
                            }
                            MonitorAction::Wait | MonitorAction::Continue => {}
                        }
                    }
                }
                match job.try_result_on_control() {
                    Ok(None) => {
                        self.audio_recorder.stage = Stage::Capturing {
                            binding,
                            job,
                            started,
                            stopping,
                            invalidated,
                            monitoring,
                            monitor_installed,
                            monitor_error,
                        };
                        became_started || monitor_changed
                    }
                    Ok(Some(outcome)) => {
                        if monitor_installed {
                            self.stop_recording_monitor(binding.source.output_identity);
                        }
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
                        let failed_path = invalidated.then(|| job.path().to_owned());
                        self.finish_audio_take(binding, outcome, invalidated);
                        if let (Some(reason), Some(path)) = (monitor_error, failed_path) {
                            self.error = Some(
                                format!(
                                    "software monitoring failed · {reason} · take path {}",
                                    hex_session::service::text::verbatim(path.display())
                                )
                                .into(),
                            );
                        }
                        true
                    }
                    Err(_) => {
                        if monitor_installed {
                            self.stop_recording_monitor(binding.source.output_identity);
                        }
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

    fn record_target_is_current(&self, binding: Binding) -> bool {
        self.project_generation == binding.context.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == binding.context.revision
                    && self.audio_recorder.target_track(
                        self.project_generation,
                        &state.project,
                        self.selected_track,
                    ) == Some(binding.track)
            })
            && self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.status())
                .and_then(|status| status.recording_source())
                == Some(binding.source)
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

    #[gpui::test]
    fn track_arm_is_exclusive_and_owns_recording_target_until_disarmed(cx: &mut TestAppContext) {
        let (project, first, second, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let click_arm = |visual: &mut VisualTestContext, track: hex_project::TrackId| {
            let selector = Box::leak(format!("track-audio-arm-{}", track.value()).into_boxed_str());
            let point = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_click(point, Modifiers::default());
            visual.run_until_parked();
        };
        click_arm(&mut visual, first);
        editor.update(&mut visual, |view, _| {
            view.selected_track = Some(second);
            assert!(
                view.audio_recorder
                    .is_audio_armed(view.project_generation, first)
            );
            assert_eq!(
                view.audio_recorder.target_track(
                    view.project_generation,
                    &view.state.as_ref().unwrap().project,
                    view.selected_track
                ),
                Some(first)
            );
        });
        click_arm(&mut visual, second);
        editor.read_with(&visual, |view, _| {
            assert!(
                !view
                    .audio_recorder
                    .is_audio_armed(view.project_generation, first)
            );
            assert!(
                view.audio_recorder
                    .is_audio_armed(view.project_generation, second)
            );
        });
        click_arm(&mut visual, second);
        editor.read_with(&visual, |view, _| {
            assert!(view.audio_recorder.armed_track.is_none());
            assert_eq!(
                view.audio_recorder.target_track(
                    view.project_generation,
                    &view.state.as_ref().unwrap().project,
                    view.selected_track
                ),
                Some(second)
            );
        });
    }

    #[gpui::test]
    fn audio_arm_refuses_changes_during_preparation_and_clears_stale_target(
        cx: &mut TestAppContext,
    ) {
        let (project, first, second, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        let (_sender, result) = mpsc::sync_channel(1);
        editor.update(&mut visual, |view, _| {
            view.toggle_audio_arm(first, view.project_generation);
            let state = view.state.as_ref().unwrap();
            let context = AudioBatchContext {
                project_generation: view.project_generation,
                revision: state.revision,
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
                    track: first,
                    context,
                },
                cancel: Arc::new(AtomicBool::new(false)),
                result,
            };
            view.toggle_audio_arm(second, view.project_generation);
            assert!(
                view.audio_recorder
                    .is_audio_armed(view.project_generation, first)
            );
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish the audio recording")
            );
            view.audio_recorder
                .reconcile_audio_arm(&hex_project::Project::new(48_000).unwrap(), false);
            assert!(view.audio_recorder.armed_track.is_none());
            assert!(matches!(&view.audio_recorder.stage, Stage::Opening { binding, .. } if binding.track == first));
            view.audio_recorder.stage = Stage::Idle;
            view.toggle_audio_arm(second, view.project_generation);
            let project = Arc::clone(&view.state.as_ref().unwrap().project);
            view.audio_recorder.reconcile_audio_arm(&project, true);
            assert!(view.audio_recorder.armed_track.is_none());
            view.toggle_audio_arm(second, view.project_generation + 1);
            assert!(view.audio_recorder.armed_track.is_none());
            view.document_audio_mode = hex_session::DocumentAudioMode::VerifiedStreaming;
            view.toggle_audio_arm(second, view.project_generation);
            assert!(view.audio_recorder.armed_track.is_none());
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("ordinary resident arrangement track")
            );
        });
    }

    fn monitored_fixture(
        cx: &mut TestAppContext,
    ) -> (
        super::super::worker_ui_fixture::WorkerUiFixture,
        hex_audio::worker_fixture::InputObserver,
        PathBuf,
    ) {
        use hex_project::Command;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let mut fixture = super::super::worker_ui_fixture::open_with_section_support(
            cx,
            vec![Command::AddTrack {
                name: "input".into(),
            }],
            false,
            true,
        );
        let editor = fixture.editor.clone();
        editor.update(cx, |view, _| {
            let audio = view.audio.as_mut().unwrap();
            audio
                .control
                .bind_resident_transport(view.project_generation);
            audio
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .unwrap();
            view.audio_recorder.software_monitor = true;
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let (binding, reader) = editor.update(cx, |view, _| {
            let context = AudioBatchContext {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                active_gesture: false,
            };
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            let audio = view.audio.as_mut().unwrap();
            let source = audio.control.status().unwrap().recording_source().unwrap();
            assert_eq!(
                source.context.project_generation,
                context.project_generation
            );
            assert_eq!(source.context.editor_revision, context.revision);
            let reader = audio.control.start_bound_output_recording(source).unwrap();
            (
                Binding {
                    source,
                    track,
                    context,
                },
                reader,
            )
        });
        let (input, monitor, observer) = hex_audio::worker_fixture::monitored_input().unwrap();
        let monitor = Box::new(MonitorStream::new_on_control(monitor, 48_000).ok().unwrap());
        let path = std::env::temp_dir().join(format!(
            "hex-monitor-ui-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let job = spawn_coordinated_take_gated(input, reader, path.clone()).unwrap();
        let (sender, result) = mpsc::sync_channel(1);
        sender
            .send(Ok(PreparedTake {
                job,
                monitor: Some(monitor),
            }))
            .unwrap();
        editor.update(cx, |view, _| {
            view.audio_recorder.stage = Stage::Opening {
                binding,
                cancel: Arc::new(AtomicBool::new(false)),
                result,
            };
            assert!(view.tick_audio_recording());
            assert!(matches!(
                view.audio_recorder.stage,
                Stage::Capturing {
                    monitoring: Monitoring::Waiting,
                    ..
                }
            ));
        });
        (fixture, observer, path)
    }

    #[cfg(target_os = "macos")]
    fn prime_recording_fixture(
        fixture: &mut super::super::worker_ui_fixture::WorkerUiFixture,
        observer: &hex_audio::worker_fixture::InputObserver,
        cx: &mut TestAppContext,
        deadline: std::time::Instant,
    ) -> u32 {
        let mut offset = 0;
        loop {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            assert!(!observer.started());
            let capturing = fixture.editor.read_with(cx, |view, _| {
                view.audio
                    .as_ref()
                    .unwrap()
                    .control
                    .sample_recording_phase()
                    == Some(hex_audio::sample_recording::Phase::Capturing)
            });
            if capturing {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "output timing did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(observer.set_capture_frame(offset + 64));
        let end = offset + 1536;
        while offset < end {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            assert!(!observer.started());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        offset
    }

    #[cfg(target_os = "macos")]
    fn assert_fixture_take(path: &std::path::Path) {
        let mut wav = hex_media::WavFrameReader::open(
            path,
            hex_media::WavReaderLimits::new(65_536, 2048, 32, 64),
        )
        .unwrap();
        assert_eq!(wav.read_frames(0, 64).unwrap(), vec![[0.25; 2]; 64]);
    }

    fn assert_monitor_reusable(
        fixture: &mut super::super::worker_ui_fixture::WorkerUiFixture,
        cx: &mut TestAppContext,
    ) {
        let (input, monitor, _) = hex_audio::worker_fixture::monitored_input().unwrap();
        let stream = Box::new(MonitorStream::new_on_control(monitor, 48_000).ok().unwrap());
        fixture.editor.update(cx, |view, _| {
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            let audio = view.audio.as_mut().unwrap();
            let source = audio.control.status().unwrap().recording_source().unwrap();
            let control = audio.control.input_monitor().unwrap();
            assert!(
                control
                    .publish(stream, source, hex_engine::OutputCaptureId::Track(track))
                    .is_ok()
            );
            control.stop();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        fixture.editor.update(cx, |view, _| {
            assert!(
                view.audio
                    .as_mut()
                    .unwrap()
                    .control
                    .input_monitor()
                    .unwrap()
                    .reclaim()
                    .is_some()
            )
        });
        drop(input);
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn monitor_callback_acknowledgment_gates_recording_and_stop_reclaims(cx: &mut TestAppContext) {
        let (mut fixture, observer, path) = monitored_fixture(cx);
        let editor = fixture.editor.clone();
        for _ in 0..4 {
            editor.update(cx, |view, _| {
                view.tick_audio_recording();
            });
            assert!(!observer.started());
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        // Continue output history while the editor withholds its permit.
        let mut offset = prime_recording_fixture(&mut fixture, &observer, cx, deadline);
        editor.update(cx, |view, _| {
            assert_eq!(
                view.audio
                    .as_mut()
                    .unwrap()
                    .control
                    .input_monitor()
                    .unwrap()
                    .status(),
                MonitorStatus::Priming
            );
            view.tick_audio_recording();
        });
        let mut heard = false;
        let mut capture_started = false;
        while !capture_started || !heard {
            let mut output = [[0.0; 2]; 64];
            fixture
                .backend
                .render_recording_fixture(&mut output, offset)
                .unwrap();
            heard |= output.iter().any(|frame| *frame != [0.0; 2]);
            offset += 64;
            capture_started = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(
                    view.audio_recorder.stage,
                    Stage::Capturing { started: true, .. }
                )
            });
            assert!(
                std::time::Instant::now() < deadline,
                "recording did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(observer.started());
        assert!(heard, "monitored input did not reach output");
        editor.update(cx, |view, _| view.toggle_audio_recording());
        loop {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            let done = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(view.audio_recorder.stage, Stage::Idle)
                    && view.audio_recorder.retiring_monitor_output.is_none()
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "recording did not stop"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(path.exists(), "completed take was not retained");
        assert_fixture_take(&path);
        assert_monitor_reusable(&mut fixture, cx);
        editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error)
        });
        fixture.finish(cx);
        std::fs::remove_file(path).unwrap();
    }

    #[gpui::test]
    fn cancel_before_monitor_ack_never_starts_input_and_retires_owner(cx: &mut TestAppContext) {
        let (mut fixture, observer, path) = monitored_fixture(cx);
        let editor = fixture.editor.clone();
        editor.update(cx, |view, _| view.toggle_audio_recording());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut offset = 0;
        loop {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            let done = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(view.audio_recorder.stage, Stage::Idle)
                    && view.audio_recorder.retiring_monitor_output.is_none()
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "canceled job did not stop"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!observer.started());
        assert!(!path.exists());
        assert_monitor_reusable(&mut fixture, cx);
        fixture.finish(cx);
    }

    #[gpui::test]
    fn output_fault_before_monitor_ack_stops_preparation_and_returns_owner(
        cx: &mut TestAppContext,
    ) {
        let (mut fixture, observer, path) = monitored_fixture(cx);
        let editor = fixture.editor.clone();
        fixture.backend.report_device_error().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut offset = 0;
        loop {
            let _ = fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset);
            offset += 64;
            let done = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(view.audio_recorder.stage, Stage::Idle)
                    && view.audio_recorder.retiring_monitor_output.is_none()
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "faulted job did not stop"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!observer.started());
        assert!(!path.exists());
        editor.read_with(cx, |view, _| assert!(view.error.is_some()));
        assert_monitor_reusable(&mut fixture, cx);
        fixture.finish(cx);
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn monitor_underrun_reports_error_without_discarding_input_take(cx: &mut TestAppContext) {
        let (mut fixture, observer, path) = monitored_fixture(cx);
        let editor = fixture.editor.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut offset = prime_recording_fixture(&mut fixture, &observer, cx, deadline);
        editor.update(cx, |view, _| {
            view.tick_audio_recording();
        });
        loop {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            let failed = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(
                    view.audio_recorder.stage,
                    Stage::Capturing {
                        started: true,
                        stopping: false,
                        monitoring: Monitoring::Failed,
                        ..
                    }
                )
            });
            if failed {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "monitor did not report underrun"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(observer.started());
        editor.read_with(cx, |view, _| {
            assert!(view.error.as_ref().unwrap().contains("underrun"))
        });
        editor.update(cx, |view, _| view.toggle_audio_recording());
        loop {
            fixture
                .backend
                .render_recording_fixture(&mut [[0.0; 2]; 64], offset)
                .unwrap();
            offset += 64;
            let done = editor.update(cx, |view, _| {
                view.tick_audio_recording();
                matches!(view.audio_recorder.stage, Stage::Idle)
                    && view.audio_recorder.retiring_monitor_output.is_none()
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "recording did not stop after monitor failure"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(path.exists(), "input take was lost after monitoring failed");
        editor.read_with(cx, |view, _| {
            assert!(view.error.as_ref().unwrap().contains("underrun"))
        });
        fixture.finish(cx);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn software_monitor_waits_for_priming_and_reports_terminal_states() {
        assert_eq!(
            monitor_action(Monitoring::Waiting, Some(MonitorStatus::Pending)),
            MonitorAction::Wait
        );
        assert_eq!(
            monitor_action(Monitoring::Waiting, Some(MonitorStatus::Priming)),
            MonitorAction::StartInput
        );
        assert_eq!(
            monitor_action(Monitoring::Active, Some(MonitorStatus::Running)),
            MonitorAction::Continue
        );
        assert_eq!(
            monitor_action(Monitoring::Active, Some(MonitorStatus::Underrun)),
            MonitorAction::Failed("input monitor underrun")
        );
        assert_eq!(
            monitor_action(Monitoring::Waiting, Some(MonitorStatus::SourceChanged)),
            MonitorAction::Failed("audio source changed")
        );
        assert_eq!(
            monitor_action(Monitoring::Waiting, None),
            MonitorAction::Failed("audio output is unavailable")
        );
    }

    #[gpui::test]
    fn software_monitor_toggle_is_explicit_and_locked_during_setup(cx: &mut TestAppContext) {
        let (project, track, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        editor.update(&mut visual, |view, _| {
            assert!(!view.audio_recorder.software_monitor);
            view.toggle_software_monitor();
            assert!(view.audio_recorder.software_monitor);
            let (_sender, result) = mpsc::sync_channel(1);
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
            view.toggle_software_monitor();
            assert!(view.audio_recorder.software_monitor);
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("finish audio recording")
            );
            view.audio_recorder.stage = Stage::Idle;
            view.audio_recorder.retiring_monitor_output = Some(2);
            assert!(view.audio_recorder.needs_poll());
            view.toggle_audio_recording();
            assert!(
                view.error
                    .as_ref()
                    .unwrap()
                    .contains("wait for software monitoring")
            );
            view.tick_audio_recording();
            assert!(!view.audio_recorder.needs_poll());
        });
    }

    #[gpui::test]
    fn audio_record_target_refuses_a_group_track(cx: &mut TestAppContext) {
        let (project, first, _, _) = audio_project();
        let mut editor = hex_project::Editor::new(project, 8).unwrap();
        editor
            .execute(hex_project::Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[first]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        let project = editor.project();
        let group = project
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap()
            .id();
        assert_eq!(
            Recorder::default().target_track(0, project, Some(group)),
            None
        );
        let (window, view) = editor_window(cx, project.clone(), Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("track-audio-arm-{}", group.value()).into_boxed_str());
        let position = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        view.read_with(&visual, |view, _| {
            assert!(
                !view
                    .audio_recorder
                    .is_audio_armed(view.project_generation, group)
            );
        });
    }

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
