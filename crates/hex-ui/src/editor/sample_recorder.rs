//! The global spectrum slot can display the native take recorder. The editor
//! owns project-scoped arm intent and the displayed recent list. The native
//! output confirms PCM and the dedicated worker owns the file operations.

use super::{EditorView, waveform::toggle_button_with_style};
use crate::theme;
use gpui::{Context, Div, Stateful, div, prelude::*, px, rgb};
use hex_audio::sample_recording::{self, End, OutputCaptureId, Phase, Source, Take};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

mod beat_guides;
mod excerpt;
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

#[derive(Default)]
pub(super) struct Recorder {
    pub(super) visible: bool,
    generation: Option<u64>,
    revision: Option<u64>,
    armed: Vec<Arm>,
    job: Option<Job>,
    phase: Option<Phase>,
    recent: Vec<Row>,
    waveform: Option<waveform::Waveform>,
    audition_job: bool,
    error: Option<String>,
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
        let armed = self.sample_recorder.generation == Some(self.project_generation)
            && self.sample_recorder.armed.contains(&arm);
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
                let result =
                    hex_session::service::sample_take_root(document.as_deref()).and_then(|root| {
                        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
                        let directory = root.join(name);
                        sample_recording::write_takes(reader, &directory)
                            .map_err(|error| take_write_error(&directory, &error))
                    });
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
        let preview_changed = self.tick_take_preview();
        let Some(job) = &self.sample_recorder.job else {
            return arms_changed || preview_changed;
        };
        let phase = self
            .audio
            .as_ref()
            .filter(|audio| audio.control.output_identity() == Some(job.output))
            .and_then(|audio| audio.control.sample_recording_phase())
            .unwrap_or(Phase::Finishing);
        let phase_changed = self.sample_recorder.phase.replace(phase) != Some(phase);
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err("sample take writer exited".into()),
            Err(TryRecvError::Empty) => return arms_changed || phase_changed || preview_changed,
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
        let waveform = self.take_waveform_panel(cx);
        let waveform_open = waveform.is_some();
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
                    .child(div().min_w_0().truncate().child(theme::text(format!(
                        "{label} · {} armed",
                        self.sample_recorder.armed.len()
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
                                self.sample_recorder
                                    .error
                                    .as_ref()
                                    .map(|error| div().child(theme::text(error))),
                            )
                            .children(self.sample_recorder.recent.iter().enumerate().map(
                                |(index, row)| {
                                    let seconds =
                                        row.take.frames as f64 / f64::from(row.take.sample_rate);
                                    let open = row.clone();
                                    div()
                                        .id(gpui::SharedString::from(
                                            row.take.path.to_string_lossy().into_owned(),
                                        ))
                                        .debug_selector(move || format!("recorded-take-{index}"))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.open_recorded_take(open.clone(), cx)
                                        }))
                                        .child(theme::text(format!(
                                            "{} · {seconds:.2} s",
                                            hex_session::service::text::verbatim(&row.name)
                                        )))
                                },
                            )),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
impl EditorView {
    pub(super) fn show_take_fixture(&mut self, take: Take) {
        self.sample_recorder.visible = true;
        self.sample_recorder.recent.push(Row {
            name: "captured audio".into(),
            take,
        });
    }
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
        let directory = Path::new("/Users/Example/Project/Sample-Takes/Take-ABC");
        let error = std::io::Error::other("No Space Left");
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Err(take_write_error(directory, &error)))
            .unwrap();
        editor.update(&mut visual, |view, _| {
            view.sample_recorder.job = Some(Job { result: receiver, names: Vec::new(), output: 0 });
            assert!(view.tick_sample_recorder());
            let message = theme::text(view.sample_recorder.error.as_deref().unwrap());
            assert_eq!(message.as_ref(), "sample take writer · no space left · check /Users/Example/Project/Sample-Takes/Take-ABC for recoverable take files");
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err("sample take storage is unavailable".into())).unwrap();
        editor.update(&mut visual, |view, _| {
            view.sample_recorder.job = Some(Job { result: receiver, names: Vec::new(), output: 0 });
            assert!(view.tick_sample_recorder());
            assert_eq!(view.sample_recorder.error.as_deref(), Some("sample take writer · sample take storage is unavailable"));
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        editor.update(&mut visual, |view, _| {
            view.sample_recorder.job = Some(Job {
                result: receiver,
                names: Vec::new(),
                output: 0,
            });
            assert!(view.tick_sample_recorder());
            assert_eq!(
                view.sample_recorder.error.as_deref(),
                Some("sample take writer · sample take writer exited")
            );
        });
    }

    #[gpui::test]
    fn recorder_header_fits_the_button_labels_and_hit_targets(cx: &mut TestAppContext) {
        #[cfg(target_os = "macos")]
        {
            cx.use_native_text_system().expect("native font shaping");
            cx.update(|cx| theme::load_fonts(cx).expect("bundled fonts"));
        }
        let (project, _, _, _) = audio_project();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        let check = |visual: &mut VisualTestContext, selector, label: &str| {
            let text = theme::text(label);
            let width = visual.update(|window, _| {
                window
                    .text_system()
                    .shape_line(
                        text.clone(),
                        window.rem_size() * 0.75,
                        &[gpui::TextRun {
                            len: text.len(),
                            font: gpui::font(theme::FONT_FAMILY),
                            color: rgb(0xffffff).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    )
                    .width
            });
            let bounds = visual.debug_bounds(selector).expect("painted control");
            assert!(bounds.size.width > width, "{label} must fit its button");
            bounds
        };
        for (width, height) in [(880.0, 540.0), (1440.0, 800.0)] {
            visual.simulate_resize(gpui::size(px(width), px(height)));
            visual.run_until_parked();
            let mode = check(&mut visual, "sample-recorder-mode", "recorder");
            visual.simulate_click(mode.center(), Modifiers::default());
            visual.run_until_parked();
            let panel = visual.debug_bounds("sample-recorder").unwrap();
            for (phase, label) in [(Some(Phase::Capturing), "stop"), (None, "record")] {
                editor.update(&mut visual, |view, cx| {
                    view.sample_recorder.phase = phase;
                    cx.notify();
                });
                visual.run_until_parked();
                let mode = check(&mut visual, "sample-recorder-mode", "spectrum");
                let record = check(&mut visual, "sample-record", label);
                assert!(panel.left() <= mode.left() && mode.right() < record.left());
                assert!(record.right() <= panel.right());
            }
            let record = visual.debug_bounds("sample-record").unwrap();
            visual.simulate_click(record.center(), Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.sample_recorder.error.as_deref(),
                    Some("arm at least one source for sampling")
                );
            });
            let mode = visual.debug_bounds("sample-recorder-mode").unwrap();
            visual.simulate_click(mode.center(), Modifiers::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds("project-spectrum").is_some());
        }
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
            assert!(!view.sample_recorder.armed.contains(&Arm::Main));
            assert!(
                view.sample_recorder
                    .armed
                    .contains(&Arm::Output(OutputCaptureId::Track(track)))
            );
        });
        visual.simulate_click(arm.center(), Modifiers::default());
        visual.run_until_parked();
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
