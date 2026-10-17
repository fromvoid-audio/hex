//! Track-header focus and view-only name drafts over canonical RenameTrack.
use super::{CancelTrackName, CommitTrackName, EditorView, RenameTrack, browser_input::*};
use crate::theme;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, KeyBinding, Subscription, Window, div,
    prelude::*,
};
use hex_project::{Command, Track, TrackId};
use hex_session::service::Request;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct TrackNameState {
    focuses: BTreeMap<TrackId, FocusHandle>,
    edit: Option<NameEdit>,
    scope: Option<(u64, u64)>,
}
struct NameEdit {
    track: TrackId,
    generation: u64,
    revision: u64,
    original: String,
    input: Entity<BrowserSearch>,
    _blur: Subscription,
}
impl TrackNameState {
    pub(super) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }
}
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "cmd-r",
            RenameTrack,
            Some("hex_track_header && !hex_text_editing"),
        ),
        KeyBinding::new("enter", CommitTrackName, Some("hex_track_name")),
        KeyBinding::new("escape", CancelTrackName, Some("hex_track_name")),
        KeyBinding::new("backspace", BrowserBackspace, Some("hex_track_name")),
        KeyBinding::new("delete", BrowserDelete, Some("hex_track_name")),
        KeyBinding::new("left", BrowserLeft, Some("hex_track_name")),
        KeyBinding::new("right", BrowserRight, Some("hex_track_name")),
        KeyBinding::new("shift-left", BrowserSelectLeft, Some("hex_track_name")),
        KeyBinding::new("shift-right", BrowserSelectRight, Some("hex_track_name")),
        KeyBinding::new("cmd-a", BrowserSelectAll, Some("hex_track_name")),
        KeyBinding::new("home", BrowserHome, Some("hex_track_name")),
        KeyBinding::new("cmd-left", BrowserHome, Some("hex_track_name")),
        KeyBinding::new("end", BrowserEnd, Some("hex_track_name")),
        KeyBinding::new("cmd-right", BrowserEnd, Some("hex_track_name")),
        KeyBinding::new("cmd-v", BrowserPaste, Some("hex_track_name")),
        KeyBinding::new("cmd-c", BrowserCopy, Some("hex_track_name")),
        KeyBinding::new("cmd-x", BrowserCut, Some("hex_track_name")),
    ]);
}
impl EditorView {
    pub(super) fn track_header_focused(&self, window: &Window) -> bool {
        self.mode == super::EditorMode::Arrangement
            && self
                .track_name
                .focuses
                .values()
                .any(|focus| focus.is_focused(window))
    }
    pub(super) fn focus_track_header_after_navigation(&self, track: TrackId, window: &mut Window) {
        if self.track_header_focused(window) {
            self.track_name
                .focuses
                .get(&track)
                .unwrap_or(&self.focus)
                .focus(window);
        }
    }
    pub(super) fn reconcile_track_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .state
            .as_ref()
            .map(|state| (self.project_generation, state.revision));
        if self.track_name.scope != current {
            let old_focus = self
                .track_name
                .focuses
                .iter()
                .find(|(_, f)| f.is_focused(window))
                .map(|(id, _)| *id);
            let generation_changed = self
                .track_name
                .scope
                .is_some_and(|old| current.is_none_or(|new| old.0 != new.0));
            if generation_changed {
                self.track_name.focuses.clear();
            } else if let Some(state) = &self.state {
                self.track_name
                    .focuses
                    .retain(|id, _| state.project.track(*id).is_some());
            }
            if old_focus.is_some_and(|id| !self.track_name.focuses.contains_key(&id)) {
                self.focus.focus(window);
            }
            self.track_name.scope = current;
        }
        if let Some(edit) = &self.track_name.edit
            && (self.mode != super::EditorMode::Arrangement
                || edit.generation != self.project_generation
                || !self.state.as_ref().is_some_and(|state| {
                    state.revision == edit.revision
                        && state
                            .project
                            .track(edit.track)
                            .is_some_and(|t| t.name() == edit.original)
                }))
        {
            let focused = edit.input.read(cx).focus_handle().is_focused(window);
            self.track_name.edit = None;
            if focused {
                self.focus.focus(window);
            }
        }
    }
    pub(super) fn begin_track_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != super::EditorMode::Arrangement
            || self.edit_pending()
            || self.track_name.edit.is_some()
            || self.clip_name_edit.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
        {
            return;
        }
        let Some(track) = self
            .track_name
            .focuses
            .iter()
            .find(|(_, focus)| focus.is_focused(window))
            .map(|(id, _)| *id)
        else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        let Some(source) = state.project.track(track) else {
            return;
        };
        let original = source.name().to_owned();
        let revision = state.revision;
        // This existing name-input mode preserves case/Unicode and selects all.
        let input = cx.new(|cx| BrowserSearch::for_clip_name(original.clone(), cx));
        let focus = input.read(cx).focus_handle().clone();
        let blur = cx.on_blur(&focus, window, |view, _, cx| view.commit_track_name(cx));
        self.track_name.edit = Some(NameEdit {
            track,
            generation: self.project_generation,
            revision,
            original,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
    }
    fn commit_track_name(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.track_name.edit.take() else {
            return;
        };
        let name = edit.input.read(cx).content.clone();
        if edit.generation == self.project_generation
            && !self.edit_pending()
            && self.state.as_ref().is_some_and(|state| {
                state.revision == edit.revision
                    && state
                        .project
                        .track(edit.track)
                        .is_some_and(|t| t.name() == edit.original)
            })
            && name != edit.original
        {
            if name.trim().is_empty() {
                self.error = Some("track name must not be empty".into());
            } else {
                self.enqueue(Request::Edit(Command::RenameTrack {
                    track: edit.track,
                    name,
                }));
            }
        }
        cx.notify();
    }
    pub(super) fn finish_track_name(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let track = self.track_name.edit.as_ref().map(|edit| edit.track);
        if commit {
            self.commit_track_name(cx);
        } else {
            self.track_name.edit = None;
        }
        track
            .and_then(|id| self.track_name.focuses.get(&id))
            .unwrap_or(&self.focus)
            .focus(window);
        cx.notify();
    }
    /// Existing header geometry and tooltip remain owned by tracks.rs.
    pub(super) fn track_header_focus(
        &mut self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.track_name
            .focuses
            .entry(track)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub(super) fn track_name_label(
        &mut self,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = track.id();
        let click_focus = self.track_header_focus(id, cx);
        let content = if let Some(edit) = &self.track_name.edit
            && edit.track == id
        {
            div()
                .debug_selector(|| "track-name-input".into())
                .child(edit.input.clone())
        } else {
            div().child(theme::text(track.name()))
        };
        div()
            .min_w_0()
            .flex_1()
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if view
                        .track_name
                        .edit
                        .as_ref()
                        .is_some_and(|edit| edit.track == id)
                    {
                        return;
                    }
                    view.select_track_with_modifiers(id, event.modifiers(), window);
                    click_focus.focus(window);
                    cx.notify();
                }),
            )
            .child(content)
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use std::sync::Arc;
    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual.debug_bounds(selector).expect("visible header");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker response missing"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn persisted(command: Command, track: TrackId, expected: &str) {
        use hex_session::service::Service;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hex-track-name-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("renamed.hex");
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "audio source".into(),
            }))
            .unwrap();
        let before = next(&service);
        assert_eq!(before.state.project.tracks()[0].id(), track);
        service.submit(Request::Edit(command)).unwrap();
        assert_eq!(
            next(&service).state.project.track(track).unwrap().name(),
            expected
        );
        service.submit(Request::Undo).unwrap();
        assert_eq!(
            next(&service).state.project.track(track).unwrap().name(),
            "audio source"
        );
        service.submit(Request::Redo).unwrap();
        assert_eq!(
            next(&service).state.project.track(track).unwrap().name(),
            expected
        );
        service.submit(Request::SaveNew(path.clone())).unwrap();
        next(&service);
        service.submit(Request::Open(path)).unwrap();
        assert_eq!(
            next(&service).state.project.track(track).unwrap().name(),
            expected
        );
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[gpui::test]
    fn focused_track_header_renames_inline_enter_is_one_persisted_canonical_edit(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, _, _) = audio_project();
        let original = project.clone();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        // The empty header background also owns track focus, not just its name.
        let header = visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let blank = gpui::point(
            header.right() - gpui::px(12.0),
            header.top() + gpui::px(1.0),
        );
        visual.simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-name-input").is_some());
        visual.simulate_input("Verse S 🎹");
        visual.simulate_keystrokes("enter");
        let command = editor.read_with(&visual,|view,_| {
            assert!(view.track_name.edit.is_none());
            assert!(matches!(&view.last_enqueued_edit,Some(Command::RenameTrack {track:id,name}) if *id==track && name=="Verse S 🎹"));
            view.last_enqueued_edit.clone().unwrap()
        });
        let mut canonical = hex_project::Editor::new(original.clone(), 8).unwrap();
        canonical.execute(command.clone()).unwrap();
        assert_eq!(
            canonical.project().track(track).unwrap().name(),
            "Verse S 🎹"
        );
        assert!(canonical.undo());
        assert_eq!(canonical.project(), &original);
        assert!(!canonical.undo());
        assert!(canonical.redo());
        persisted(command, track, "Verse S 🎹");
    }
    #[gpui::test]
    fn header_escape_cancels_and_clip_focus_keeps_existing_clip_rename(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(
            &mut visual,
            Box::leak(format!("track-name-{}", track.value()).into_boxed_str()),
        );
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        visual.simulate_input("discard");
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |view, _| {
            assert!(view.track_name.edit.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
        click(
            &mut visual,
            Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str()),
        );
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.track_name.edit.is_none());
            assert!(view.clip_name_edit.is_some());
            assert!(view.last_enqueued_edit.is_none());
        });
    }
    #[gpui::test]
    fn header_navigation_follows_actual_focus_and_text_focus_cannot_start_rename(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(
            &mut visual,
            Box::leak(format!("track-name-{}", track.value()).into_boxed_str()),
        );
        visual.simulate_keystrokes("down cmd-r");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.track_name.edit.as_ref().unwrap().track, target)
        });
        visual.simulate_keystrokes("cmd-r");
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.track_name.edit.as_ref().unwrap().track, target)
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
        });
        visual.simulate_keystrokes("cmd-r");
        editor.read_with(&visual, |view, _| {
            assert!(view.track_name.edit.is_none());
            assert!(view.clip_name_edit.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
    }
}
