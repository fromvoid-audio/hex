//! Default-input Arrangement recording. GPUI holds only job handles and captured
//! identities; native streams, file writing, alignment and project edits retain
//! their existing audio and Session owners.
use super::{EditorMode, EditorView};
use hex_audio::{
    input_capture::{
        CoordinatedJob, CoordinatedOutcome, InputCapture, InputConfig, spawn_coordinated_take,
    },
    input_devices::input_devices_on_control,
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
}
impl Recorder {
    pub(super) fn blocks_edits(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }
    pub(super) fn label(&self) -> &'static str {
        match self.stage {
            Stage::Idle => {
                "record selected track from default audio input · use hardware monitoring"
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
}

fn open_default_take(
    reader: Reader,
    rate: u32,
    document: Option<PathBuf>,
    cancel: &AtomicBool,
) -> Result<CoordinatedJob, String> {
    let inputs = input_devices_on_control().map_err(|error| error.to_string())?;
    let id = inputs
        .default()
        .ok_or("no default audio input is available")?
        .clone();
    let device = inputs.resolve(&id).map_err(|error| error.to_string())?;
    let channels = [2, 1]
        .into_iter()
        .find(|channels| {
            device
                .formats
                .iter()
                .any(|format| format.supports_recording(rate, *channels))
        })
        .ok_or("default audio input does not support f32 at the project sample rate")?;
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
            Stage::Capturing { job, stopping, .. } => {
                if !*stopping {
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
        let (sender, result) = mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("hex-record-input-setup".into())
            .spawn(move || {
                let job = open_default_take(reader, rate, document, &worker_cancel);
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
        self.status = "preparing default audio input · use hardware monitoring".into();
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
            Stage::Capturing { job, stopping, .. } => {
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
                        "canceling audio recording"
                    } else {
                        "preparing default audio input · use hardware monitoring"
                    }
                    .into();
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
                    self.status = "recording default audio input · click to stop".into();
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
        if self.audio_batch.is_some() || self.device_gesture.snapshot().is_some() {
            self.error = Some(
                format!(
                    "audio recording could not be committed · take retained at {}",
                    hex_session::service::text::verbatim(take.path.display())
                )
                .into(),
            );
            return;
        }
        let (request, control) =
            match Request::recorded_audio_take(
                take.path.to_owned(),
                binding.track,
                take.timeline_start,
                binding.context,
            ) {
                Ok(request) => request,
                Err(error) => {
                    self.error = Some(format!(
                    "audio recording could not be committed · {error} · take retained at {}",
                    hex_session::service::text::verbatim(take.path.display())
                ).into());
                    return;
                }
            };
        let retained_path = take.path.to_owned();
        if self.enqueue_recorded_audio(request) {
            self.audio_batch = Some(control);
            self.audio_batch_progress = None;
            self.audio_recorder.pending_commit_path = Some(retained_path);
            self.status = "committing recorded audio".into();
        } else {
            self.error = Some(
                format!(
                    "audio recording could not be queued · take retained at {}",
                    hex_session::service::text::verbatim(take.path.display())
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
    use gpui::{TestAppContext, VisualTestContext};

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
                    .contains("audio recording failed")
            );
            assert!(view.audio_batch.is_none());
            assert!(view.audio_recorder.pending_commit_path.is_none());
        });
        assert_eq!(std::fs::read(&path).unwrap(), b"retained");
        std::fs::remove_file(path).unwrap();
    }
}
