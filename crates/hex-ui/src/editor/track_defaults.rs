//! New empty-track gain preference and admission-time capture. Persistence runs
//! on a background executor; a pending or failed save never changes the value
//! used by a creation request.
use super::EditorView;
use crate::theme;
use gpui::{AppContext, Context, Div, Entity, Subscription, div, prelude::*, px};
use hex_preferences::TrackDefaults;
use hex_project::{Command, TrackId};
use hex_session::service::Request;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::path::PathBuf;

struct Binding {
    control: Entity<NumericControl>,
    _subscription: Subscription,
}
pub(super) struct State {
    path: PathBuf,
    committed: TrackDefaults,
    loading: bool,
    saving: bool,
    writable: bool,
    error: Option<String>,
    binding: Option<Binding>,
}
impl State {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: TrackDefaults::default(),
            loading: true,
            saving: false,
            writable: true,
            error: None,
            binding: None,
        }
    }

    pub(super) fn captured_gain(&self) -> Result<f32, &'static str> {
        if self.loading {
            return Err("track defaults are loading · retry shortly");
        }
        if !self.writable {
            return Err("track defaults could not be loaded · open settings for details");
        }
        Ok(self.committed.gain())
    }
}

impl EditorView {
    pub(super) fn start_track_defaults_load(&mut self, cx: &mut Context<Self>) {
        let path = self.track_defaults.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load_track_defaults(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.track_defaults;
                state.loading = false;
                match result {
                    Ok(preferences) => state.committed = preferences,
                    Err(error) => {
                        state.writable = false;
                        state.error = Some(format!(
                            "could not load track defaults · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_track_default_gain(&mut self, gain: f64, cx: &mut Context<Self>) {
        let state = &mut self.track_defaults;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let Ok(preferences) = TrackDefaults::new(gain as f32) else {
            return;
        };
        if preferences == state.committed {
            return;
        }
        state.saving = true;
        state.error = None;
        let path = state.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save_track_defaults(&path, preferences)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.track_defaults;
                state.saving = false;
                match result {
                    Ok(()) => state.committed = preferences,
                    Err(error) => {
                        state.error = Some(format!(
                            "could not save track defaults · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn device_track_gain(
        &self,
        destination: hex_session::service::ArrangementDeviceDestination,
    ) -> Result<Option<f32>, &'static str> {
        match destination {
            hex_session::service::ArrangementDeviceDestination::NewTrack { .. } => {
                self.track_defaults.captured_gain().map(Some)
            }
            hex_session::service::ArrangementDeviceDestination::ExistingTrack { .. } => Ok(None),
        }
    }

    pub(super) fn insert_default_track(&mut self, before_track: Option<TrackId>) {
        let gain = match self.track_defaults.captured_gain() {
            Ok(gain) => gain,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        self.enqueue(Request::Edit(Command::InsertDefaultTrackWithGain {
            before_track,
            gain,
        }));
    }

    pub(super) fn track_defaults_row(&mut self, cx: &mut Context<Self>) -> Div {
        if self.track_defaults.binding.is_none() {
            let config = NumericSpec::new(
                0.0,
                2.0,
                1.0,
                NumericScale::LinearAmplitudeDecibels,
                1,
                Some("db"),
            )
            .and_then(|spec| {
                NumericControlConfig::new(
                    "new-empty-track-volume",
                    theme::text("new empty track volume · db"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    f64::from(self.track_defaults.committed.gain()),
                    theme::control_style(),
                )
            });
            if let Ok(config) = config {
                let control = cx.new(|cx| {
                    NumericControl::from_config(
                        cx,
                        config.with_presentation(NumericControlPresentation::FilledSlider),
                    )
                });
                let subscription =
                    cx.subscribe(&control, |view, _, event: &NumericControlEvent, cx| {
                        if event.phase == NumericEventPhase::Commit {
                            view.save_track_default_gain(event.value, cx);
                        }
                    });
                self.track_defaults.binding = Some(Binding {
                    control,
                    _subscription: subscription,
                });
            }
        }
        let state = &self.track_defaults;
        let mut row = div().flex().flex_col().gap(px(3.0));
        if let Some(binding) = &state.binding {
            binding.control.update(cx, |control, cx| {
                control.set_enabled(!state.loading && !state.saving && state.writable, cx);
                let _ = control.set_external_value(f64::from(state.committed.gain()), cx);
            });
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w(px(190.0))
                            .text_xs()
                            .child(theme::text("new empty track volume")),
                    )
                    .child(div().flex_1().children([binding.control.clone()])),
            );
        }
        let status = state.error.as_deref().unwrap_or(if state.loading {
            "loading track defaults…"
        } else if state.saving {
            "saving track defaults…"
        } else {
            ""
        });
        row.child(
            div()
                .min_h(px(14.0))
                .text_xs()
                .debug_selector(|| "track-defaults-status".into())
                .child(theme::text(status)),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use std::time::Duration;

    fn wait(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            visual.run_until_parked();
            if editor.read_with(&*visual, |view, _| {
                !view.track_defaults.loading
                    && !view.track_defaults.saving
                    && !view.audio_import_settings.is_loading()
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "track defaults worker timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[gpui::test]
    fn arrangement_batch_import_captures_committed_gain_without_affecting_other_destinations(
        cx: &mut TestAppContext,
    ) {
        use hex_session::import_batch::{AudioBatchDestination, ConsoleDestination};
        use hex_session::import_batch_decode::AudioBatchContext;
        let (handle, editor, _, _, _) = super::super::loop_selection_ui_test::fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        wait(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            let target = v.state.as_ref().unwrap().project.tracks()[0].id();
            let context = AudioBatchContext { project_generation: v.project_generation, revision: v.state.as_ref().unwrap().revision, active_gesture: false };
            let make = |destination| Request::audio_batch(vec![PathBuf::from("/sample.wav")], destination, context).unwrap().0;
            let destination = AudioBatchDestination::NewTracks { before_track: None, start: 42 };
            v.track_defaults.committed = TrackDefaults::new(0.5).unwrap();
            let legacy = v.capture_audio_import_policy(Request::Import(PathBuf::from("/legacy.wav"))).unwrap();
            let anchored = v.capture_audio_import_policy(Request::ImportToNewTrack { path: PathBuf::from("/anchored.wav"), before_track: Some(target), start: 123 }).unwrap();
            let captured = v.capture_audio_import_policy(make(destination)).unwrap();
            let console_new = AudioBatchDestination::Console(ConsoleDestination::NewTrack { before_track: Some(target), scene: hex_project::SessionSceneTarget::Insert { before: None } });
            let captured_console = v.capture_audio_import_policy(make(console_new)).unwrap();
            v.track_defaults.committed = TrackDefaults::new(0.25).unwrap();
            assert!(matches!(captured_console, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.5)));
            assert!(matches!(legacy, Request::ImportToNewTrackWithGain { before_track: None, start: 0, gain, .. } if gain == 0.5));
            assert!(matches!(anchored, Request::ImportToNewTrackWithGain { before_track: Some(id), start: 123, gain, .. } if id == target && gain == 0.5));
            v.track_defaults.saving = true;
            let during_save = v.capture_audio_import_policy(make(destination)).unwrap();
            assert!(matches!(during_save, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.25)));
            v.track_defaults.saving = false;
            assert!(matches!(captured, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.5)));
            v.track_defaults.loading = true;
            assert!(v.capture_audio_import_policy(make(destination)).unwrap_err().contains("loading"));
            assert!(v.capture_audio_import_policy(make(console_new)).unwrap_err().contains("loading"));
            let existing = v.capture_audio_import_policy(make(AudioBatchDestination::Track { track: target, start: 0 })).unwrap();
            assert!(matches!(existing, Request::ImportAudioBatch { options, .. } if options.new_track_gain.is_none()));
            let mut project = hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 2).unwrap();
            let hex_project::Created::Scene(scene) = project.execute(Command::AddScene { name: "scene".into() }).unwrap() else { panic!("scene") };
            let console = v.capture_audio_import_policy(make(AudioBatchDestination::Console(ConsoleDestination::Cell { track: target, scene }))).unwrap();
            assert!(matches!(console, Request::ImportAudioBatch { options, .. } if options.new_track_gain.is_none()));
            v.track_defaults.loading = false;
            v.track_defaults.writable = false;
            assert!(v.capture_audio_import_policy(make(destination)).unwrap_err().contains("open settings"));
            assert!(v.capture_audio_import_policy(make(console_new)).unwrap_err().contains("open settings"));
            assert!(v.capture_audio_import_policy(Request::Import(PathBuf::from("/legacy.wav"))).unwrap_err().contains("open settings"));
            assert!(matches!(v.capture_audio_import_policy(Request::ImportToTrack { path: PathBuf::from("/existing.wav"), track: target, start: 12 }).unwrap(), Request::ImportToTrack { track, start: 12, .. } if track == target));
        });
    }

    #[gpui::test]
    fn device_track_gain_captures_committed_value_and_preserves_existing_targets(
        cx: &mut TestAppContext,
    ) {
        use hex_session::service::ArrangementDeviceDestination;
        let (handle, editor, _, _, _) = super::super::loop_selection_ui_test::fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        wait(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            let new = ArrangementDeviceDestination::NewTrack { before_track: None };
            let existing = ArrangementDeviceDestination::ExistingTrack {
                track,
                instrument: None,
            };
            view.track_defaults.committed = TrackDefaults::new(0.5).unwrap();
            let captured = view.device_track_gain(new).unwrap();
            view.track_defaults.committed = TrackDefaults::new(0.25).unwrap();
            view.track_defaults.saving = true;
            assert_eq!(captured, Some(0.5));
            assert_eq!(view.device_track_gain(new).unwrap(), Some(0.25));
            view.track_defaults.loading = true;
            assert!(view.device_track_gain(new).unwrap_err().contains("loading"));
            assert_eq!(view.device_track_gain(existing).unwrap(), None);
            view.track_defaults.loading = false;
            view.track_defaults.writable = false;
            assert!(
                view.device_track_gain(new)
                    .unwrap_err()
                    .contains("open settings")
            );
            assert_eq!(view.device_track_gain(existing).unwrap(), None);
        });
    }

    #[gpui::test]
    fn settings_control_persists_gain_and_new_empty_tracks_capture_it(cx: &mut TestAppContext) {
        let (handle, editor, _, _, files) = super::super::loop_selection_ui_test::fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        wait(&editor, &mut visual);
        let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        editor.update(&mut visual, |v, _| v.insert_default_track(None));
        super::super::loop_selection_ui_test::pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert!((p.tracks().last().unwrap().gain() - 0.251_188_64).abs() < 1e-7);
            assert_eq!(p.tracks()[0].gain(), original.tracks()[0].gain());
        });
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        let value = visual
            .debug_bounds("new-empty-track-volume-value")
            .expect("settings gain control")
            .center();
        visual.simulate_mouse_down(value, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(value, MouseButton::Left, Modifiers::default());
        visual.simulate_input("-6");
        visual.simulate_keystrokes("enter");
        wait(&editor, &mut visual);
        let saved =
            hex_preferences::load_track_defaults(&files.root().join("config/track-defaults.json"))
                .unwrap();
        assert!((20.0 * saved.gain().log10() + 6.0).abs() < 1e-4);
        editor.update(&mut visual, |v, cx| {
            let path = v.track_defaults.path.clone();
            v.track_defaults = State::new(path);
            v.start_track_defaults_load(cx);
        });
        wait(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.track_defaults.committed, saved)
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.update(&mut visual, |v, _| v.insert_default_track(None));
        super::super::loop_selection_ui_test::pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert!((p.tracks().last().unwrap().gain() - saved.gain()).abs() < 1e-7);
            assert!((p.tracks()[2].gain() - 0.251_188_64).abs() < 1e-7);
        });
    }

    #[gpui::test]
    fn failed_save_retains_committed_gain_for_creation(cx: &mut TestAppContext) {
        let (handle, editor, _, _, files) = super::super::loop_selection_ui_test::fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        wait(&editor, &mut visual);
        let parent = files.root().join("not-a-directory");
        std::fs::write(&parent, b"keep").unwrap();
        editor.update(&mut visual, |v, cx| {
            v.track_defaults.path = parent.join("track-defaults.json");
            v.save_track_default_gain(0.5, cx);
        });
        wait(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.track_defaults.committed, TrackDefaults::default());
            assert!(v.track_defaults.error.is_some());
        });
        editor.update(&mut visual, |v, _| v.insert_default_track(None));
        super::super::loop_selection_ui_test::pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .last()
                    .unwrap()
                    .gain(),
                TrackDefaults::default().gain()
            )
        });
        assert_eq!(std::fs::read(parent).unwrap(), b"keep");
    }

    #[gpui::test]
    fn invalid_load_refuses_new_track_and_preserves_the_file(cx: &mut TestAppContext) {
        let (handle, editor, _, _, files) = super::super::loop_selection_ui_test::fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        wait(&editor, &mut visual);
        let path = files.root().join("config/bad-track-defaults.json");
        std::fs::write(&path, b"invalid").unwrap();
        editor.update(&mut visual, |v, cx| {
            v.track_defaults = State::new(path.clone());
            v.start_track_defaults_load(cx);
        });
        wait(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            let before = v.state.as_ref().unwrap().project.clone();
            v.last_enqueued_edit = None;
            v.insert_default_track(None);
            assert!(v.last_enqueued_edit.is_none());
            assert_eq!(v.state.as_ref().unwrap().project, before);
            assert!(
                v.error
                    .as_deref()
                    .is_some_and(|error| error.contains("open settings"))
            );
        });
        assert_eq!(std::fs::read(path).unwrap(), b"invalid");
    }
}
