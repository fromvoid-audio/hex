//! New-track gain preference and admission-time capture. Persistence runs
//! on a background executor; a pending or failed save never changes the value
//! used by a creation request. Empty track display follows its own ordered
//! Service reply while project and audio preparation retain canonical custody.
use super::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
    track_proposals::{Anchor, TrackProposal},
};
use crate::theme;
use gpui::{AppContext, Context, Div, Entity, Stateful, Subscription, div, prelude::*, px, rgb};
use hex_preferences::TrackDefaults;
use hex_project::{Command, TrackId};
use hex_session::service::{Event, Request};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericControlPresentation, NumericEventPhase, NumericScale, NumericSpec,
};
use std::path::PathBuf;

struct Binding {
    control: Entity<NumericControl>,
    _subscription: Subscription,
}

pub(super) struct PendingEmptyTrack {
    id: u64,
    before_track: Option<TrackId>,
    gain: f32,
    receipt: Receipt,
}

impl PendingEmptyTrack {
    fn key(&self) -> (Option<TrackId>, u32) {
        (self.before_track, self.gain.to_bits())
    }
}

fn empty_track_key(request: &Request) -> Option<(Option<TrackId>, u32)> {
    match request {
        Request::Edit(Command::InsertDefaultTrackWithGain { before_track, gain }) => {
            Some((*before_track, gain.to_bits()))
        }
        _ => None,
    }
}

pub(super) struct State {
    path: Option<PathBuf>,
    committed: TrackDefaults,
    loading: bool,
    saving: bool,
    writable: bool,
    error: Option<String>,
    binding: Option<Binding>,
}
impl State {
    pub(super) fn new(path: Option<PathBuf>) -> Self {
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

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_state(&mut self, gain: f32, loading: bool, writable: bool) {
        self.committed = TrackDefaults::new(gain).unwrap();
        self.loading = loading;
        self.writable = writable;
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
        let Some(path) = self.settings.track_defaults.path.clone() else {
            self.settings.track_defaults.loading = false;
            self.settings.track_defaults.writable = false;
            self.settings.track_defaults.error =
                Some("track defaults unavailable · home directory is unset".into());
            return;
        };
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
                let state = &mut view.settings.track_defaults;
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
        let state = &mut self.settings.track_defaults;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let Ok(preferences) = TrackDefaults::new(gain as f32) else {
            return;
        };
        if preferences == state.committed {
            return;
        }
        let Some(path) = state.path.clone() else {
            state.writable = false;
            state.error = Some("track defaults unavailable · home directory is unset".into());
            return;
        };
        state.saving = true;
        state.error = None;
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
                let state = &mut view.settings.track_defaults;
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
                self.settings.track_defaults.captured_gain().map(Some)
            }
            hex_session::service::ArrangementDeviceDestination::ExistingTrack { .. } => Ok(None),
        }
    }

    pub(super) fn insert_default_track(&mut self, before_track: Option<TrackId>) {
        let gain = match self.settings.track_defaults.captured_gain() {
            Ok(gain) => gain,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let show_preview = self.state.as_ref().is_some_and(|state| {
            state.project.tracks().len() + self.pending_empty_tracks.len() < hex_project::MAX_TRACKS
                && before_track.is_none_or(|id| state.project.track(id).is_some())
        });
        let preview = self
            .next_empty_track_id
            .checked_add(1)
            .filter(|_| show_preview)
            .map(|next| {
                let id = self.next_empty_track_id;
                self.next_empty_track_id = next;
                self.pending_empty_tracks.push(PendingEmptyTrack {
                    id,
                    before_track,
                    gain,
                    receipt: Receipt::new(self.project_generation),
                });
                id
            });
        let accepted = self.enqueue(Request::Edit(Command::InsertDefaultTrackWithGain {
            before_track,
            gain,
        }));
        if let Some(id) = preview {
            if accepted {
                self.reveal_session_track_proposal(before_track);
            } else {
                self.retire_empty_track(id);
            }
        }
    }

    pub(super) fn pending_empty_track_for_request(&self, request: &Request) -> Option<u64> {
        let key = empty_track_key(request)?;
        self.pending_empty_tracks
            .iter()
            .find(|pending| pending.receipt.is_queued() && pending.key() == key)
            .map(|pending| pending.id)
    }

    pub(super) fn empty_track_submitted(&mut self, id: u64) {
        if let Some(pending) = self.pending_empty_tracks.iter_mut().find(|p| p.id == id) {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn retire_empty_track(&mut self, id: u64) -> bool {
        let Some(index) = self.pending_empty_tracks.iter().position(|p| p.id == id) else {
            return false;
        };
        self.pending_empty_tracks.remove(index);
        true
    }

    pub(super) fn retire_abandoned_empty_track(&mut self) -> bool {
        if !self
            .pending_empty_tracks
            .iter()
            .any(|p| p.receipt.is_queued())
        {
            return false;
        }
        let mut queued: Vec<_> = self.queued_requests().filter_map(empty_track_key).collect();
        let before = self.pending_empty_tracks.len();
        self.pending_empty_tracks.retain(|pending| {
            if !pending.receipt.is_queued() {
                return true;
            }
            let Some(index) = queued.iter().position(|key| *key == pending.key()) else {
                return false;
            };
            queued.swap_remove(index);
            true
        });
        self.pending_empty_tracks.len() != before
    }

    pub(super) fn observe_empty_track(&mut self, event: &Event) {
        if event.project_replaced {
            self.pending_empty_tracks.clear();
            return;
        }
        let mut terminal = None;
        for pending in &mut self.pending_empty_tracks {
            match pending
                .receipt
                .observe(self.project_generation, false, event.request_complete)
            {
                Disposition::Waiting => (),
                Disposition::Terminal => terminal = Some(pending.id),
                Disposition::Invalidated => {
                    self.pending_empty_tracks.clear();
                    return;
                }
            }
        }
        if let Some(id) = terminal {
            if event.error.is_none()
                && matches!(event.created, Some(hex_project::Created::Track(_)))
                && self.mode == EditorMode::Session
            {
                self.retain_session_track_proposal_page(TrackProposal::Empty(id));
            }
            self.retire_empty_track(id);
        }
    }

    pub(super) fn track_proposal_anchors(&self) -> Vec<Anchor> {
        self.proposed_plugin_track()
            .into_iter()
            .chain(
                self.pending_empty_tracks
                    .iter()
                    .map(|pending| (pending.before_track, TrackProposal::Empty(pending.id))),
            )
            .collect()
    }

    fn empty_track_header(&self, id: u64) -> Div {
        let ahead = self
            .pending_empty_tracks
            .iter()
            .take_while(|pending| pending.id != id)
            .count();
        let name = self
            .state
            .as_ref()
            .map(|state| state.project.next_default_track_name(ahead));
        div()
            .flex()
            .h_full()
            .min_w_0()
            .items_center()
            .px_2()
            .border_l_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_xs()
            .child(
                div()
                    .truncate()
                    .child(theme::text(name.unwrap_or_default())),
            )
    }

    pub(super) fn proposed_arrangement_empty_track(&self, id: u64) -> Div {
        div()
            .debug_selector(|| "arrangement-pending-empty-track".into())
            .w_full()
            .h(px(super::tracks::TRACK_ROW_HEIGHT))
            .flex()
            .justify_end()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .children([self.empty_track_header(id).w(px(self.track_header_width()))])
    }

    pub(super) fn proposed_session_empty_track(&self, id: u64, width: f32) -> Stateful<Div> {
        self.empty_track_header(id)
            .id(("session-pending-empty-track", id))
            .debug_selector(|| "session-pending-empty-track".into())
            .w(px(width))
            .flex_shrink_0()
    }

    pub(super) fn track_defaults_row(&mut self, cx: &mut Context<Self>) -> Div {
        if self.settings.track_defaults.binding.is_none() {
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
                    "new-track-volume",
                    theme::text("new track volume · db"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    f64::from(self.settings.track_defaults.committed.gain()),
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
                self.settings.track_defaults.binding = Some(Binding {
                    control,
                    _subscription: subscription,
                });
            }
        }
        let state = &self.settings.track_defaults;
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
                            .child(theme::text("new track volume")),
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
                !view.settings.track_defaults.loading
                    && !view.settings.track_defaults.saving
                    && !view.settings.audio_import.is_loading()
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
            v.settings.track_defaults.committed = TrackDefaults::new(0.5).unwrap();
            let legacy = v.capture_audio_import_policy(Request::Import(PathBuf::from("/legacy.wav"))).unwrap();
            let anchored = v.capture_audio_import_policy(Request::ImportToNewTrack { path: PathBuf::from("/anchored.wav"), before_track: Some(target), start: 123 }).unwrap();
            let captured = v.capture_audio_import_policy(make(destination)).unwrap();
            let console_new = AudioBatchDestination::Console(ConsoleDestination::NewTrack { before_track: Some(target), scene: hex_project::SessionSceneTarget::Insert { before: None } });
            let captured_console = v.capture_audio_import_policy(make(console_new)).unwrap();
            v.settings.track_defaults.committed = TrackDefaults::new(0.25).unwrap();
            assert!(matches!(captured_console, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.5)));
            assert!(matches!(legacy, Request::ImportToNewTrackWithGain { before_track: None, start: 0, gain, .. } if gain == 0.5));
            assert!(matches!(anchored, Request::ImportToNewTrackWithGain { before_track: Some(id), start: 123, gain, .. } if id == target && gain == 0.5));
            v.settings.track_defaults.saving = true;
            let during_save = v.capture_audio_import_policy(make(destination)).unwrap();
            assert!(matches!(during_save, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.25)));
            v.settings.track_defaults.saving = false;
            assert!(matches!(captured, Request::ImportAudioBatch { options, .. } if options.new_track_gain == Some(0.5)));
            v.settings.track_defaults.loading = true;
            assert!(v.capture_audio_import_policy(make(destination)).unwrap_err().contains("loading"));
            assert!(v.capture_audio_import_policy(make(console_new)).unwrap_err().contains("loading"));
            let existing = v.capture_audio_import_policy(make(AudioBatchDestination::Track { track: target, start: 0 })).unwrap();
            assert!(matches!(existing, Request::ImportAudioBatch { options, .. } if options.new_track_gain.is_none()));
            let mut project = hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 2).unwrap();
            let hex_project::Created::Scene(scene) = project.execute(Command::AddScene { name: "scene".into() }).unwrap() else { panic!("scene") };
            let console = v.capture_audio_import_policy(make(AudioBatchDestination::Console(ConsoleDestination::Cell { track: target, scene }))).unwrap();
            assert!(matches!(console, Request::ImportAudioBatch { options, .. } if options.new_track_gain.is_none()));
            v.settings.track_defaults.loading = false;
            v.settings.track_defaults.writable = false;
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
            view.settings.track_defaults.committed = TrackDefaults::new(0.5).unwrap();
            let captured = view.device_track_gain(new).unwrap();
            view.settings.track_defaults.committed = TrackDefaults::new(0.25).unwrap();
            view.settings.track_defaults.saving = true;
            assert_eq!(captured, Some(0.5));
            assert_eq!(view.device_track_gain(new).unwrap(), Some(0.25));
            view.settings.track_defaults.loading = true;
            assert!(view.device_track_gain(new).unwrap_err().contains("loading"));
            assert_eq!(view.device_track_gain(existing).unwrap(), None);
            view.settings.track_defaults.loading = false;
            view.settings.track_defaults.writable = false;
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
            .debug_bounds("new-track-volume-value")
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
            let path = v.settings.track_defaults.path.clone();
            v.settings.track_defaults = State::new(path);
            v.start_track_defaults_load(cx);
        });
        wait(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.settings.track_defaults.committed, saved)
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
            v.settings.track_defaults.path = Some(parent.join("track-defaults.json"));
            v.save_track_default_gain(0.5, cx);
        });
        wait(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.settings.track_defaults.committed,
                TrackDefaults::default()
            );
            assert!(v.settings.track_defaults.error.is_some());
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
            v.settings.track_defaults = State::new(Some(path.clone()));
            v.start_track_defaults_load(cx);
        });
        wait(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            let before = v.state.as_ref().unwrap().project.clone();
            v.test_probes.last_enqueued_edit = None;
            v.insert_default_track(None);
            assert!(v.test_probes.last_enqueued_edit.is_none());
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
