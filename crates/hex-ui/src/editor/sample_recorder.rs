//! The global spectrum slot can display the native take recorder. The editor
//! owns project-scoped arm intent and the displayed recent list. The native
//! output confirms PCM and the dedicated worker owns the file operations.

use super::{EditorView, waveform::toggle_button_with_style};
use crate::theme;
use gpui::{Context, Div, Stateful, div, prelude::*, px, rgb};
use hex_audio::{
    rolling_recorder::{self, RollingHistory, RollingSource},
    sample_recording::{self, End, OutputCaptureId, Phase, Source, Take},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod beat_guides;
mod excerpt;
#[cfg(feature = "ui-test-support")]
pub(super) mod layout_fixture;
mod selection;
mod waveform;

// A single take from every supported source must remain reachable.
const RECENT_LIMIT: usize = hex_engine::MAX_OUTPUT_CAPTURES + 1;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Arm {
    Output(OutputCaptureId),
    Main,
}
impl Arm {
    fn source(self) -> Source {
        match self {
            Self::Output(OutputCaptureId::Track(id)) => Source::Track(id.value()),
            Self::Output(OutputCaptureId::Return(id)) => Source::Return(id),
            Self::Main => Source::Main,
        }
    }
    fn identity(self) -> (&'static str, u64) {
        match self.source() {
            Source::Track(id) => ("sample-arm-track", id),
            Source::Return(id) => ("sample-arm-return", id.value()),
            Source::Main => ("sample-arm-main", 0),
        }
    }
}

struct RollingCollector {
    output: u64,
    generation: u64,
    source: RollingSource,
    history: Arc<Mutex<RollingHistory>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for RollingCollector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct KeepJob {
    result: Receiver<Result<Take, String>>,
    name: String,
}

#[derive(Default)]
pub(super) struct Recorder {
    pub(super) visible: bool,
    generation: Option<u64>,
    revision: Option<u64>,
    armed: Vec<Arm>,
    job: Option<Job>,
    phase: Option<Phase>,
    rolling: Option<RollingCollector>,
    rolling_source: Option<RollingSource>,
    keep_job: Option<KeepJob>,
    recent: Vec<Row>,
    waveform: Option<waveform::Waveform>,
    audition_job: bool,
    error: Option<String>,
}

impl Recorder {
    pub(super) fn is_armed(&self, generation: u64, arm: Arm) -> bool {
        self.generation == Some(generation) && self.armed.contains(&arm)
    }
}

struct Job {
    result: Receiver<Result<sample_recording::Takes, String>>,
    names: Vec<(Source, String)>,
    output: u64,
}

#[derive(Clone)]
struct Row {
    name: String,
    take: Take,
}

// The waveform adapter uses an icon-width hit target. Recorder labels
// need their own text-sized surfaces, including their horizontal padding.
fn button(label: &'static str, tooltip: &'static str, id: (&'static str, u64)) -> Stateful<Div> {
    super::waveform::button(label, tooltip, id)
        .w_auto()
        .px_1()
        .whitespace_nowrap()
        .debug_selector(move || id.0.into())
}

impl EditorView {
    fn sample_source_name(&self, source: Source) -> Option<&str> {
        let project = &self.state.as_ref()?.project;
        match source {
            Source::Track(id) => project
                .tracks()
                .iter()
                .find(|track| track.id().value() == id)
                .map(hex_project::Track::name),
            Source::Return(id) => project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .map(hex_project::ReturnTrack::name),
            Source::Main => Some("main"),
        }
    }

    pub(super) fn sample_arm_button(&self, arm: Arm, cx: &mut Context<Self>) -> Stateful<Div> {
        let armed = self.device_panel.sample_recorder.generation == Some(self.project_generation)
            && self.device_panel.sample_recorder.armed.contains(&arm);
        let (background, text) = if armed {
            let background = theme::color(theme::PaletteRole::Recording);
            (background, theme::color(theme::PaletteRole::Background))
        } else {
            super::track_gain::activator_colors(false)
        };
        let (key, id) = arm.identity();
        toggle_button_with_style(
            "◉",
            if armed {
                "disarm for sampling"
            } else {
                "arm for sampling"
            },
            (key, id),
            theme::action_style().with_state_colors(background, text),
        )
        .debug_selector(move || format!("{key}-{id}"))
        .w(px(16.0))
        .h(px(14.0))
        .text_size(px(10.0))
        .border_1()
        .border_color(rgb(theme::control_style().border))
        .flex_shrink_0()
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            if view.persistence.opening_project || view.sample_source_name(arm.source()).is_none() {
                return;
            }
            view.sync_sample_arm_context();
            if let Some(index) = view
                .device_panel
                .sample_recorder
                .armed
                .iter()
                .position(|entry| *entry == arm)
            {
                view.device_panel.sample_recorder.armed.remove(index);
            } else {
                view.device_panel.sample_recorder.armed.push(arm);
            }
            cx.notify();
        }))
    }

    fn sync_sample_arm_context(&mut self) -> bool {
        let generation = Some(self.project_generation);
        if self.device_panel.sample_recorder.generation != generation {
            self.device_panel.sample_recorder.generation = generation;
            self.device_panel.sample_recorder.revision = None;
            self.device_panel.sample_recorder.armed.clear();
            return true;
        }
        let Some(state) = &self.state else {
            return false;
        };
        if self.device_panel.sample_recorder.revision == Some(state.revision) {
            return false;
        }
        self.device_panel.sample_recorder.revision = Some(state.revision);
        let before = self.device_panel.sample_recorder.armed.len();
        self.device_panel
            .sample_recorder
            .armed
            .retain(|arm| match arm {
                Arm::Main => true,
                Arm::Output(OutputCaptureId::Track(id)) => state.project.track(*id).is_some(),
                Arm::Output(OutputCaptureId::Return(id)) => {
                    state.project.return_tracks().iter().any(|r| r.id() == *id)
                }
            });
        self.device_panel.sample_recorder.armed.len() != before
    }

    pub(super) fn sample_recorder_mode_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = if self.device_panel.sample_recorder.visible {
            "spectrum"
        } else {
            "recorder"
        };
        button(
            label,
            "switch spectrum and recorder",
            ("sample-recorder-mode", 0),
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.device_panel.sample_recorder.visible = !view.device_panel.sample_recorder.visible;
            view.tick_project_spectrum(cx);
            cx.notify();
        }))
    }

    fn start_sample_take(&mut self) -> Result<Job, String> {
        self.sync_sample_arm_context();
        if self.persistence.opening_project {
            return Err("wait for the project to finish opening".into());
        }
        let mut names = Vec::new();
        let mut outputs = Vec::new();
        let mut main = false;
        for arm in self.device_panel.sample_recorder.armed.iter().copied() {
            let source = arm.source();
            let Some(name) = self.sample_source_name(source) else {
                continue;
            };
            names.push((source, name.to_owned()));
            match arm {
                Arm::Output(output) => outputs.push(output),
                Arm::Main => main = true,
            }
        }
        if names.is_empty() {
            return Err("arm at least one source for sampling".into());
        }
        let document = self
            .state
            .as_ref()
            .and_then(|state| state.document_path.clone());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let name = format!("take-{stamp}-{}", std::process::id());
        let audio = self.audio.as_mut().ok_or("audio output is unavailable")?;
        let output = audio
            .control
            .output_identity()
            .ok_or("audio output is unavailable")?;
        let reader = audio
            .control
            .start_sample_recording(self.project_generation, &outputs, main)
            .map_err(|e| e.to_string())?;
        let (complete, result) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("hex-sample-takes".into())
            .spawn(move || {
                let result = write_sample_take(
                    hex_session::service::sample_take_root(document.as_deref()),
                    &name,
                    |directory| sample_recording::write_takes(reader, directory),
                );
                let _ = complete.send(result);
            })
            .map_err(|e| e.to_string())?;
        Ok(Job {
            result,
            names,
            output,
        })
    }

    fn sample_record_or_stop(&mut self) {
        if let Some(job) = &self.device_panel.sample_recorder.job {
            if let Some(audio) = self
                .audio
                .as_ref()
                .filter(|audio| audio.control.output_identity() == Some(job.output))
            {
                audio.control.stop_sample_recording();
            }
            return;
        }
        self.device_panel.sample_recorder.error = None;
        match self.start_sample_take() {
            Ok(job) => {
                self.device_panel.sample_recorder.job = Some(job);
                self.device_panel.sample_recorder.phase = Some(Phase::Preparing);
            }
            Err(error) => self.device_panel.sample_recorder.error = Some(error),
        }
    }

    fn sync_rolling_recorder(&mut self) -> bool {
        let generation = self.project_generation;
        let current_output = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity());
        let mut changed = false;

        // Drop stale collector if audio output disappeared or changed identity.
        if let Some(collector) = &self.device_panel.sample_recorder.rolling
            && Some(collector.output) != current_output
        {
            self.device_panel.sample_recorder.rolling = None;
            changed = true;
        }

        // Start always-on 60s rolling RAM recorder automatically whenever audio output is active.
        if self.device_panel.sample_recorder.rolling.is_none()
            && let Some(output) = current_output
            && let Some(audio) = self.audio.as_mut()
        {
            let source = self
                .device_panel
                .sample_recorder
                .rolling_source
                .unwrap_or(RollingSource::Main);
            if let Ok(mut reader) = audio.control.start_rolling_recording(generation, source) {
                let rate = reader.sample_rate();
                if let Ok(history) =
                    RollingHistory::new(rate, rolling_recorder::ROLLING_WINDOW_SECONDS)
                {
                    let history = Arc::new(Mutex::new(history));
                    let stop = Arc::new(AtomicBool::new(false));
                    let thread_history = Arc::clone(&history);
                    let thread_stop = Arc::clone(&stop);
                    if let Ok(thread) = std::thread::Builder::new()
                        .name("hex-rolling-audio".into())
                        .spawn(move || {
                            while !thread_stop.load(Ordering::Acquire) {
                                if let Ok(mut guard) = thread_history.lock() {
                                    guard.drain(&mut reader);
                                }
                                if reader.is_disconnected() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(15));
                            }
                        })
                    {
                        self.device_panel.sample_recorder.rolling = Some(RollingCollector {
                            output,
                            generation,
                            source,
                            history,
                            stop,
                            thread: Some(thread),
                        });
                        self.device_panel.sample_recorder.rolling_source = Some(source);
                        changed = true;
                    } else {
                        audio.control.stop_rolling_recording();
                    }
                } else {
                    audio.control.stop_rolling_recording();
                }
            }
        }

        // Reset rolling source cleanly on project replacement or deleted isolated track.
        let desired_source = match self.device_panel.sample_recorder.rolling_source {
            Some(RollingSource::Track(track_id)) => {
                let exists = self.state.as_ref().is_some_and(|state| {
                    state
                        .project
                        .tracks()
                        .iter()
                        .any(|track| track.id().value() == track_id)
                });
                if exists {
                    RollingSource::Track(track_id)
                } else {
                    RollingSource::Main
                }
            }
            _ => RollingSource::Main,
        };
        if let Some(collector) = self.device_panel.sample_recorder.rolling.as_mut()
            && (collector.generation != generation || collector.source != desired_source)
        {
            let next_source = if collector.generation != generation {
                RollingSource::Main
            } else {
                desired_source
            };
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio.control.set_rolling_source(generation, next_source);
            }
            if let Ok(mut guard) = collector.history.lock() {
                guard.clear();
            }
            collector.generation = generation;
            collector.source = next_source;
            self.device_panel.sample_recorder.rolling_source = Some(next_source);
            changed = true;
        }
        changed
    }

    pub(super) fn toggle_rolling_isolation(&mut self) {
        let Some(selected) = self.selected_track else {
            self.device_panel.sample_recorder.error =
                Some("select a track to isolate for rolling capture".into());
            return;
        };
        let current = self
            .device_panel
            .sample_recorder
            .rolling_source
            .unwrap_or(RollingSource::Main);
        let next = if current == RollingSource::Track(selected.value()) {
            RollingSource::Main
        } else {
            RollingSource::Track(selected.value())
        };
        self.device_panel.sample_recorder.rolling_source = Some(next);
        self.device_panel.sample_recorder.error = None;
        if let Some(collector) = self.device_panel.sample_recorder.rolling.as_mut() {
            if let Some(audio) = self.audio.as_mut() {
                let _ = audio
                    .control
                    .set_rolling_source(self.project_generation, next);
            }
            if let Ok(mut guard) = collector.history.lock() {
                guard.clear();
            }
            collector.source = next;
            collector.generation = self.project_generation;
        }
    }

    pub(super) fn keep_rolling_capture(&mut self) {
        if self.persistence.opening_project {
            self.device_panel.sample_recorder.error =
                Some("wait for the project to finish opening".into());
            return;
        }
        if self.device_panel.sample_recorder.keep_job.is_some() {
            return;
        }
        let Some(collector) = &self.device_panel.sample_recorder.rolling else {
            self.device_panel.sample_recorder.visible = true;
            self.device_panel.sample_recorder.error =
                Some("rolling audio recorder is waiting for active audio output".into());
            return;
        };
        let snapshot = match collector.history.lock() {
            Ok(guard) => guard.snapshot(),
            Err(_) => {
                self.device_panel.sample_recorder.error =
                    Some("rolling audio buffer is unavailable".into());
                return;
            }
        };
        if snapshot.frames.is_empty() {
            self.device_panel.sample_recorder.visible = true;
            self.device_panel.sample_recorder.error = Some("rolling audio buffer is empty".into());
            return;
        }
        let label = match snapshot.source {
            RollingSource::Main => "rolling · main".to_owned(),
            RollingSource::Track(id) => self.sample_source_name(Source::Track(id)).map_or_else(
                || format!("rolling · track {id}"),
                |name| format!("rolling · {name}"),
            ),
        };
        let document = self
            .state
            .as_ref()
            .and_then(|state| state.document_path.clone());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir_name = format!("rolling-{stamp}-{}", std::process::id());
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("hex-rolling-keep".into())
            .spawn(move || {
                let result = (|| {
                    let root = hex_session::service::sample_take_root(document.as_deref())?;
                    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                    let dir = root.join(&dir_name);
                    snapshot
                        .write_wav(&dir)
                        .map_err(|e| take_write_error(&dir, &e))
                })();
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.device_panel.sample_recorder.visible = true;
                self.device_panel.sample_recorder.error = None;
                self.device_panel.sample_recorder.keep_job = Some(KeepJob {
                    result: rx,
                    name: label,
                });
            }
            Err(error) => {
                self.device_panel.sample_recorder.error = Some(error.to_string());
            }
        }
    }

    fn tick_rolling_keep_job(&mut self) -> bool {
        let Some(job) = &self.device_panel.sample_recorder.keep_job else {
            return false;
        };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err("rolling audio writer exited".into()),
            Err(TryRecvError::Empty) => return false,
        };
        let Some(job) = self.device_panel.sample_recorder.keep_job.take() else {
            return true;
        };
        match result {
            Ok(take) => {
                self.device_panel.sample_recorder.recent.insert(
                    0,
                    Row {
                        name: job.name,
                        take,
                    },
                );
                self.device_panel
                    .sample_recorder
                    .recent
                    .truncate(RECENT_LIMIT);
                self.device_panel.sample_recorder.error = None;
            }
            Err(error) => {
                self.device_panel.sample_recorder.error =
                    Some(format!("rolling keep failed · {error}"));
            }
        }
        true
    }

    fn rolling_status_label(&self) -> String {
        let source = self
            .device_panel
            .sample_recorder
            .rolling_source
            .unwrap_or(RollingSource::Main);
        let source_text = match source {
            RollingSource::Main => "main".to_owned(),
            RollingSource::Track(id) => self
                .sample_source_name(Source::Track(id))
                .map_or_else(|| format!("track {id}"), str::to_owned),
        };
        let secs = self
            .device_panel
            .sample_recorder
            .rolling
            .as_ref()
            .and_then(|c| c.history.lock().ok().map(|h| h.buffered_seconds()))
            .unwrap_or(0.0);
        format!("60s ram: {source_text} ({secs:.1}s)")
    }

    pub(super) fn tick_sample_recorder(&mut self) -> bool {
        let arms_changed = self.sync_sample_arm_context();
        let rolling_changed = self.sync_rolling_recorder();
        let keep_changed = self.tick_rolling_keep_job();
        let preview_changed = self.tick_take_preview();
        let Some(job) = &self.device_panel.sample_recorder.job else {
            return arms_changed || rolling_changed || keep_changed || preview_changed;
        };
        let phase = self
            .audio
            .as_ref()
            .filter(|audio| audio.control.output_identity() == Some(job.output))
            .and_then(|audio| audio.control.sample_recording_phase())
            .unwrap_or(Phase::Finishing);
        let phase_changed = self.device_panel.sample_recorder.phase.replace(phase) != Some(phase);
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err("sample take writer exited".into()),
            Err(TryRecvError::Empty) => {
                return arms_changed
                    || rolling_changed
                    || keep_changed
                    || phase_changed
                    || preview_changed;
            }
        };
        if let Some(audio) = self.audio.as_mut() {
            audio.control.reclaim_sample_recording();
        }
        let Some(job) = self.device_panel.sample_recorder.job.take() else {
            return true;
        };
        self.device_panel.sample_recorder.phase = None;
        match result {
            Ok(batch) => {
                for take in batch.takes {
                    let name = job
                        .names
                        .iter()
                        .find(|(source, _)| *source == take.source)
                        .map_or_else(|| "sample".into(), |(_, name)| name.clone());
                    self.device_panel
                        .sample_recorder
                        .recent
                        .insert(0, Row { name, take });
                }
                self.device_panel
                    .sample_recorder
                    .recent
                    .truncate(RECENT_LIMIT);
                self.device_panel.sample_recorder.error = match batch.write_error {
                    Some(error) => Some(format!("sample take interrupted · {error}")),
                    None if batch.end == End::Stopped => None,
                    None => Some(format!(
                        "sample take interrupted · {}",
                        end_label(batch.end)
                    )),
                };
            }
            Err(error) => {
                self.device_panel.sample_recorder.error =
                    Some(format!("sample take writer · {error}"))
            }
        }
        true
    }

    pub(super) fn sample_recorder_panel(
        &self,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let waveform = self.take_waveform_panel(cx);
        let waveform_open = waveform.is_some();
        let phase = self.device_panel.sample_recorder.phase;
        let label = match phase {
            Some(Phase::Preparing) => "preparing",
            Some(Phase::Capturing) => "recording",
            Some(Phase::Finishing) => "finishing",
            None => "ready",
        };
        div()
            .id("sample-recorder")
            .debug_selector(|| "sample-recorder".into())
            .h(px(height.max(90.0)))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .text_xs()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .child(
                div()
                    .h(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(theme::text("sample recorder")),
                    )
                    .child(self.sample_recorder_mode_button(cx))
                    .child(
                        button(
                            if phase.is_some() { "stop" } else { "record" },
                            "start or stop sample recording",
                            ("sample-record", 0),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.sample_record_or_stop();
                            cx.notify();
                        })),
                    )
                    .child(
                        button(
                            if matches!(
                                self.device_panel.sample_recorder.rolling_source,
                                Some(RollingSource::Track(_))
                            ) {
                                "isolated"
                            } else {
                                "isolate"
                            },
                            "isolate selected track for 60s ram recorder (cmd-shift-i)",
                            ("rolling-isolate", 0),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.toggle_rolling_isolation();
                            cx.notify();
                        })),
                    )
                    .child(
                        button(
                            "keep 60s",
                            "keep latest 60s rolling ram audio capture (cmd-shift-k)",
                            ("rolling-keep", 0),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.keep_rolling_capture();
                            cx.notify();
                        })),
                    )
                    .child(div().min_w_0().truncate().child(theme::text(format!(
                        "{label} · {} armed · {}",
                        self.device_panel.sample_recorder.armed.len(),
                        self.rolling_status_label()
                    )))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_h_0()
                    .children(waveform)
                    .child(
                        div()
                            .id("sample-take-list")
                            .when(waveform_open, |list| list.w(px(168.0)).flex_shrink_0())
                            .when(!waveform_open, |list| list.flex_1())
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_1()
                            .children(
                                self.device_panel
                                    .sample_recorder
                                    .error
                                    .as_ref()
                                    .map(|error| div().child(theme::text(error))),
                            )
                            .children(
                                self.device_panel
                                    .sample_recorder
                                    .recent
                                    .iter()
                                    .enumerate()
                                    .map(|(index, row)| {
                                        let seconds = row.take.frames as f64
                                            / f64::from(row.take.sample_rate);
                                        let open = row.clone();
                                        div()
                                            .id(gpui::SharedString::from(
                                                row.take.path.to_string_lossy().into_owned(),
                                            ))
                                            .debug_selector(move || {
                                                format!("recorded-take-{index}")
                                            })
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                view.open_recorded_take(open.clone(), cx)
                                            }))
                                            .child(theme::text(format!(
                                                "{} · {seconds:.2} s",
                                                hex_session::service::text::verbatim(&row.name)
                                            )))
                                    }),
                            ),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
impl EditorView {
    pub(super) fn show_take_fixture(&mut self, take: Take) {
        self.device_panel.sample_recorder.visible = true;
        self.device_panel.sample_recorder.recent.push(Row {
            name: "captured audio".into(),
            take,
        });
    }
}

fn write_sample_take(
    root: Result<PathBuf, String>,
    name: &str,
    write: impl FnOnce(&Path) -> std::io::Result<sample_recording::Takes>,
) -> Result<sample_recording::Takes, String> {
    let root = root?;
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let directory = root.join(name);
    write(&directory).map_err(|error| take_write_error(&directory, &error))
}

fn take_write_error(directory: &Path, error: &std::io::Error) -> String {
    format!(
        "{error} · check {} for recoverable take files",
        hex_session::service::text::verbatim(directory.display())
    )
}

fn end_label(end: End) -> &'static str {
    match end {
        End::Stopped => "stopped",
        End::OutputFailed => "audio output changed or failed",
        End::SourceChanged => "project source changed",
        End::Backpressure => "take writer could not keep up",
        End::InvalidBlock => "audio was unavailable",
        End::UnsupportedOutput => "unsupported output mode",
        End::StorageFailed => "storage unavailable",
        End::TransportChanged => "transport changed",
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    #[gpui::test]
    fn failed_take_reports_the_case_sensitive_recovery_location(cx: &mut TestAppContext) {
        let (project, _, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        struct TestRoot(PathBuf);
        impl Drop for TestRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = std::env::temp_dir().join(format!(
            "Hex-Recovery-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _cleanup = TestRoot(root.clone());
        let directory = root.join("Take-ABC");
        let result = write_sample_take(Ok(root.clone()), "Take-ABC", |path| {
            std::fs::create_dir(path)?;
            std::fs::write(path.join("Partial.WAV"), b"recoverable bytes")?;
            Err(std::io::Error::other("No Space Left"))
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).unwrap();
        editor.update(&mut visual, |view, _| {
            view.device_panel.sample_recorder.job = Some(Job {
                result: receiver,
                names: Vec::new(),
                output: 0,
            });
            assert!(view.tick_sample_recorder());
            let message = theme::text(view.device_panel.sample_recorder.error.as_deref().unwrap());
            assert_eq!(
                message.as_ref(),
                format!(
                    "sample take writer · no space left · check {} for recoverable take files",
                    directory.display()
                )
            );
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(write_sample_take(
                Err("sample take storage is unavailable".into()),
                "Take-ABC",
                |_| panic!("writer must not run without a root"),
            ))
            .unwrap();
        editor.update(&mut visual, |view, _| {
            view.device_panel.sample_recorder.job = Some(Job {
                result: receiver,
                names: Vec::new(),
                output: 0,
            });
            assert!(view.tick_sample_recorder());
            assert_eq!(
                view.device_panel.sample_recorder.error.as_deref(),
                Some("sample take writer · sample take storage is unavailable")
            );
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(sample_recording::Takes {
                takes: vec![Take {
                    source: Source::Main,
                    path: directory.join("main.wav"),
                    frames: 64,
                    sample_rate: 48_000,
                    timing: std::sync::Arc::from([]),
                }],
                end: End::Stopped,
                write_error: None,
            }))
            .unwrap();
        editor.update(&mut visual, |view, _| {
            view.device_panel.sample_recorder.job = Some(Job {
                result: receiver,
                names: vec![(Source::Main, "main".into())],
                output: 0,
            });
            assert!(view.tick_sample_recorder());
            assert!(view.device_panel.sample_recorder.error.is_none());
            assert_eq!(view.device_panel.sample_recorder.recent.len(), 1);
            assert_eq!(
                view.device_panel.sample_recorder.recent[0].take.path,
                directory.join("main.wav")
            );
        });
        assert_eq!(
            std::fs::read(directory.join("Partial.WAV")).unwrap(),
            b"recoverable bytes"
        );
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        editor.update(&mut visual, |view, _| {
            view.device_panel.sample_recorder.job = Some(Job {
                result: receiver,
                names: Vec::new(),
                output: 0,
            });
            assert!(view.tick_sample_recorder());
            assert_eq!(
                view.device_panel.sample_recorder.error.as_deref(),
                Some("sample take writer · sample take writer exited")
            );
        });
    }

    #[gpui::test]
    fn sampler_mode_switch_preserves_independent_arrangement_and_main_arms(
        cx: &mut TestAppContext,
    ) {
        let (project, _, _, _) = audio_project();
        let track = project.tracks()[0].id();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let track_arm = Box::leak(format!("sample-arm-track-{}", track.value()).into_boxed_str());
        for selector in [track_arm, "sample-arm-main-0"] {
            let bounds = visual.debug_bounds(selector).unwrap();
            assert_eq!(bounds.size.width, px(16.0));
            assert_eq!(bounds.size.height, px(14.0));
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.simulate_mouse_move(gpui::point(px(0.0), px(0.0)), None, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(
                visual.rendered_background(selector),
                Some(rgb(theme::color(theme::PaletteRole::Recording)).into())
            );
        }
        let switch = visual
            .debug_bounds("sample-recorder-mode")
            .unwrap()
            .center();
        visual.simulate_click(switch, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("sample-recorder").is_some());
        let switch = visual
            .debug_bounds("sample-recorder-mode")
            .unwrap()
            .center();
        visual.simulate_click(switch, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("project-spectrum").is_some());
        // Main's Console arm is the same project-scoped intent, now in the
        // pinned mixer column rather than the legacy footer.
        editor.update(&mut visual, |view, cx| {
            view.mode = crate::editor::EditorMode::Session;
            cx.notify();
        });
        visual.run_until_parked();
        let arm = visual.debug_bounds("sample-arm-main-0").unwrap();
        let column = visual.debug_bounds("console-main-mixer-column").unwrap();
        assert!(column.contains(&arm.center()));
        assert_eq!(
            visual.rendered_background("sample-arm-main-0"),
            Some(rgb(theme::color(theme::PaletteRole::Recording)).into())
        );
        visual.simulate_click(arm.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(!view.device_panel.sample_recorder.armed.contains(&Arm::Main));
            assert!(
                view.device_panel
                    .sample_recorder
                    .armed
                    .contains(&Arm::Output(OutputCaptureId::Track(track)))
            );
        });
        visual.simulate_click(arm.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(!view.device_panel.sample_recorder.visible);
            assert_eq!(view.device_panel.sample_recorder.armed.len(), 2);
            assert!(view.device_panel.sample_recorder.armed.contains(&Arm::Main));
            assert!(
                view.device_panel
                    .sample_recorder
                    .armed
                    .contains(&Arm::Output(OutputCaptureId::Track(track)))
            );
        });
    }

    #[gpui::test]
    fn rolling_recorder_hotkey_isolates_selected_track_without_solo_and_keeps_capture(
        cx: &mut gpui::TestAppContext,
    ) {
        let (mut visual, editor, _env, _) = crate::editor::midi_canvas::creation_tests::fixture(cx);
        visual.run_until_parked();
        let track = editor.read_with(&visual, |view, _| view.selected_track.unwrap());

        // Press Cmd+Shift+I to isolate the selected track for the rolling recorder.
        // Ordinary solo/mute and track selection must remain completely unchanged.
        visual.simulate_keystrokes("cmd-shift-i");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.device_panel.sample_recorder.rolling_source,
                Some(RollingSource::Track(track.value()))
            );
            assert_eq!(view.selected_track, Some(track));
            let tr = view.state.as_ref().unwrap().project.track(track).unwrap();
            assert!(!tr.soloed());
            assert!(!tr.muted());
        });

        // Pressing Cmd+Shift+I again returns to Main mix.
        visual.simulate_keystrokes("cmd-shift-i");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.device_panel.sample_recorder.rolling_source,
                Some(RollingSource::Main)
            );
        });

        // Install a synthetic populated RollingHistory collector and press Cmd+Shift+K to keep it.
        let mut history = RollingHistory::new(48_000, 1).unwrap();
        history.push_frames(&[[0.25, -0.25]; 128]);
        let shared_history = Arc::new(Mutex::new(history));

        editor.update(&mut visual, |view, cx| {
            view.device_panel.sample_recorder.rolling = Some(RollingCollector {
                output: 99,
                generation: view.project_generation,
                source: RollingSource::Main,
                history: Arc::clone(&shared_history),
                stop: Arc::new(AtomicBool::new(true)),
                thread: None,
            });
            cx.notify();
        });
        visual.simulate_keystrokes("cmd-shift-k");
        visual.run_until_parked();

        // Wait for the background WAV keep writer to publish into sample_recorder.recent.
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let done = editor.update(&mut visual, |view, _| {
                view.tick_rolling_keep_job();
                view.device_panel.sample_recorder.keep_job.is_none()
            });
            if done {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "rolling keep timeout");
            std::thread::sleep(Duration::from_millis(2));
        }
        editor.read_with(&visual, |view, _| {
            assert!(view.device_panel.sample_recorder.visible);
            assert!(
                view.device_panel.sample_recorder.error.is_none(),
                "unexpected error: {:?}",
                view.device_panel.sample_recorder.error
            );
            assert_eq!(view.device_panel.sample_recorder.recent.len(), 1);
            assert_eq!(view.device_panel.sample_recorder.recent[0].take.frames, 128);
            assert!(
                view.device_panel.sample_recorder.recent[0]
                    .take
                    .path
                    .exists()
            );
            // Rolling RAM buffer remains intact after keeping.
            assert_eq!(shared_history.lock().unwrap().buffered_frames(), 128);
        });
    }
}
