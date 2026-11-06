//! The global spectrum slot can display the native take recorder. The editor
//! owns project-scoped arm intent and the displayed recent list. The native
//! output confirms PCM and the dedicated worker owns the file operations.

use super::{
    EditorView,
    waveform::{button, toggle_button_with_style},
};
use crate::theme;
use gpui::{Context, Div, Stateful, div, prelude::*, px, rgb};
use hex_audio::sample_recording::{self, End, OutputCaptureId, Phase, Source, Take};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

const RECENT_LIMIT: usize = 128;

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

#[derive(Default)]
pub(super) struct Recorder {
    pub(super) visible: bool,
    generation: Option<u64>,
    revision: Option<u64>,
    armed: Vec<Arm>,
    job: Option<Job>,
    phase: Option<Phase>,
    recent: Vec<Row>,
    error: Option<String>,
}

struct Job {
    result: Receiver<std::io::Result<sample_recording::Takes>>,
    names: Vec<(Source, String)>,
    output: u64,
}

struct Row {
    name: String,
    take: Take,
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
        let armed = self.sample_recorder.generation == Some(self.project_generation)
            && self.sample_recorder.armed.contains(&arm);
        let (background, text) = super::track_gain::activator_colors(armed);
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
        .w(px(22.0))
        .h(px(18.0))
        .flex_shrink_0()
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            if view.opening_project || view.sample_source_name(arm.source()).is_none() {
                return;
            }
            view.sync_sample_arm_context();
            if let Some(index) = view
                .sample_recorder
                .armed
                .iter()
                .position(|entry| *entry == arm)
            {
                view.sample_recorder.armed.remove(index);
            } else {
                view.sample_recorder.armed.push(arm);
            }
            cx.notify();
        }))
    }

    fn sync_sample_arm_context(&mut self) -> bool {
        let generation = Some(self.project_generation);
        if self.sample_recorder.generation != generation {
            self.sample_recorder.generation = generation;
            self.sample_recorder.revision = None;
            self.sample_recorder.armed.clear();
            return true;
        }
        let Some(state) = &self.state else {
            return false;
        };
        if self.sample_recorder.revision == Some(state.revision) {
            return false;
        }
        self.sample_recorder.revision = Some(state.revision);
        let before = self.sample_recorder.armed.len();
        self.sample_recorder.armed.retain(|arm| match arm {
            Arm::Main => true,
            Arm::Output(OutputCaptureId::Track(id)) => state.project.track(*id).is_some(),
            Arm::Output(OutputCaptureId::Return(id)) => {
                state.project.return_tracks().iter().any(|r| r.id() == *id)
            }
        });
        self.sample_recorder.armed.len() != before
    }

    pub(super) fn sample_recorder_mode_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = if self.sample_recorder.visible {
            "spectrum"
        } else {
            "recorder"
        };
        button(
            label,
            "switch spectrum and recorder",
            ("sample-recorder-mode", 0),
        )
        .debug_selector(|| "sample-recorder-mode".into())
        .on_click(cx.listener(|view, _, _, cx| {
            view.sample_recorder.visible = !view.sample_recorder.visible;
            view.tick_project_spectrum();
            cx.notify();
        }))
    }

    fn start_sample_take(&mut self) -> Result<Job, String> {
        self.sync_sample_arm_context();
        if self.opening_project {
            return Err("wait for the project to finish opening".into());
        }
        let mut names = Vec::new();
        let mut outputs = Vec::new();
        let mut main = false;
        for arm in self.sample_recorder.armed.iter().copied() {
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
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("sample take storage is unavailable")?;
        let root = hex_preferences::theme_preferences_path(&home).with_file_name("sample-takes");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory = root.join(format!("take-{stamp}-{}", std::process::id()));
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
                let result = std::fs::create_dir_all(root)
                    .and_then(|()| sample_recording::write_takes(reader, &directory));
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
        if let Some(job) = &self.sample_recorder.job {
            if let Some(audio) = self
                .audio
                .as_ref()
                .filter(|audio| audio.control.output_identity() == Some(job.output))
            {
                audio.control.stop_sample_recording();
            }
            return;
        }
        self.sample_recorder.error = None;
        match self.start_sample_take() {
            Ok(job) => {
                self.sample_recorder.job = Some(job);
                self.sample_recorder.phase = Some(Phase::Preparing);
            }
            Err(error) => self.sample_recorder.error = Some(error),
        }
    }

    pub(super) fn tick_sample_recorder(&mut self) -> bool {
        let arms_changed = self.sync_sample_arm_context();
        let Some(job) = &self.sample_recorder.job else {
            return arms_changed;
        };
        let phase = self
            .audio
            .as_ref()
            .filter(|audio| audio.control.output_identity() == Some(job.output))
            .and_then(|audio| audio.control.sample_recording_phase())
            .unwrap_or(Phase::Finishing);
        let phase_changed = self.sample_recorder.phase.replace(phase) != Some(phase);
        let result = match job.result.try_recv() {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(TryRecvError::Disconnected) => Err("sample take writer exited".into()),
            Err(TryRecvError::Empty) => return arms_changed || phase_changed,
        };
        if let Some(audio) = self.audio.as_mut() {
            audio.control.reclaim_sample_recording();
        }
        let Some(job) = self.sample_recorder.job.take() else {
            return true;
        };
        self.sample_recorder.phase = None;
        match result {
            Ok(batch) => {
                for take in batch.takes {
                    let name = job
                        .names
                        .iter()
                        .find(|(source, _)| *source == take.source)
                        .map_or_else(|| "sample".into(), |(_, name)| name.clone());
                    self.sample_recorder.recent.insert(0, Row { name, take });
                }
                self.sample_recorder.recent.truncate(RECENT_LIMIT);
                self.sample_recorder.error = match batch.write_error {
                    Some(error) => Some(format!("sample take interrupted · {error}")),
                    None if batch.end == End::Stopped => None,
                    None => Some(format!(
                        "sample take interrupted · {}",
                        end_label(batch.end)
                    )),
                };
            }
            Err(error) => {
                self.sample_recorder.error = Some(format!("sample take writer · {error}"))
            }
        }
        true
    }

    pub(super) fn sample_recorder_panel(
        &self,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let phase = self.sample_recorder.phase;
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
                    .child(theme::text("sample recorder"))
                    .child(self.sample_recorder_mode_button(cx))
                    .child(
                        button(
                            if phase.is_some() { "stop" } else { "record" },
                            "start or stop sample recording",
                            ("sample-record", 0),
                        )
                        .debug_selector(|| "sample-record".into())
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.sample_record_or_stop();
                            cx.notify();
                        })),
                    )
                    .child(theme::text(format!(
                        "{label} · {} armed",
                        self.sample_recorder.armed.len()
                    ))),
            )
            .child(
                div()
                    .id("sample-take-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .children(
                        self.sample_recorder
                            .error
                            .as_ref()
                            .map(|error| div().child(theme::text(error))),
                    )
                    .children(self.sample_recorder.recent.iter().map(|row| {
                        let seconds = row.take.frames as f64 / f64::from(row.take.sample_rate);
                        div().child(theme::text(format!(
                            "{} · {seconds:.2} s",
                            hex_session::service::text::verbatim(&row.name)
                        )))
                    })),
            )
    }
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
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

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
            let point = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_click(point, Modifiers::default());
            visual.run_until_parked();
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
        editor.read_with(cx, |view, _| {
            assert!(!view.sample_recorder.visible);
            assert_eq!(view.sample_recorder.armed.len(), 2);
            assert!(view.sample_recorder.armed.contains(&Arm::Main));
            assert!(
                view.sample_recorder
                    .armed
                    .contains(&Arm::Output(OutputCaptureId::Track(track)))
            );
        });
    }
}
