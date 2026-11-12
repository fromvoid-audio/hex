//! Typed header focus and one bounded text draft over canonical name/tag edits.
//! Existing draft barriers, cancellation and open finalization own both text domains.
use super::{
    CancelTrackName, CommitTrackName, EditorView, RemoveHeaderTracks, RenameTrack, browser_input,
};
use crate::theme;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, KeyBinding, Subscription, Window, div,
    prelude::*,
};
use hex_project::{Command, Project, ReturnId, ReturnTrack, Track, TrackId};
use hex_session::service::Request;
use hex_widgets::TextInput;
use std::collections::BTreeMap;

/// Stable naming owners; Return identities never pass through ordinary track APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Target {
    Track(TrackId),
    Return(ReturnId),
}
impl Target {
    pub(super) fn name(self, project: &Project) -> Option<&str> {
        match self {
            Self::Track(id) => project.track(id).map(Track::name),
            Self::Return(id) => project
                .return_tracks()
                .iter()
                .find(|track| track.id() == id)
                .map(ReturnTrack::name),
        }
    }
    fn name_editing_supported(self, mode: super::EditorMode) -> bool {
        match mode {
            super::EditorMode::Arrangement => true,
            super::EditorMode::Session => matches!(self, Self::Return(_)),
        }
    }
    fn rename(self, name: String) -> Command {
        match self {
            Self::Track(track) => Command::RenameTrack { track, name },
            Self::Return(return_track) => Command::RenameReturnTrack { return_track, name },
        }
    }
}

#[derive(Default)]
pub(super) struct TrackNameState {
    focuses: BTreeMap<Target, FocusHandle>,
    edit: Option<NameEdit>,
    scope: Option<(u64, u64)>,
}
struct NameEdit {
    target: Target,
    generation: u64,
    revision: u64,
    original: String,
    tags: Option<Vec<hex_project::TrackTag>>,
    input: Entity<TextInput>,
    _blur: Subscription,
}
impl NameEdit {
    fn matches(&self, project: &Project) -> bool {
        match &self.tags {
            Some(tags) => {
                matches!(self.target, Target::Track(id) if project.track(id).is_some_and(|track| track.tags() == tags.as_slice()))
            }
            None => self.target.name(project) == Some(self.original.as_str()),
        }
    }
}
pub(super) fn parse_tags(text: &str) -> Result<Vec<hex_project::TrackTag>, hex_project::Invalid> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut tags = Vec::new();
    for value in text.split(',') {
        if tags.len() == hex_project::MAX_TRACK_TAGS {
            return Err(hex_project::Invalid("a track supports at most 16 tags"));
        }
        tags.push(hex_project::TrackTag::new(value)?);
    }
    Ok(tags)
}
impl TrackNameState {
    pub(super) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }
    pub(super) fn pending_return(&self, generation: u64) -> Option<ReturnId> {
        let edit = self
            .edit
            .as_ref()
            .filter(|edit| edit.generation == generation)?;
        match edit.target {
            Target::Return(id) => Some(id),
            Target::Track(_) => None,
        }
    }
}
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "cmd-r",
            RenameTrack,
            Some("hex_track_header && !hex_text_editing"),
        ),
        KeyBinding::new(
            "cmd-r",
            RenameTrack,
            Some("hex_return_header && !hex_text_editing"),
        ),
        KeyBinding::new(
            "backspace",
            RemoveHeaderTracks,
            Some("hex_track_header && !hex_text_editing"),
        ),
        KeyBinding::new(
            "x",
            RemoveHeaderTracks,
            Some("hex_track_header && !hex_text_editing && !hex_text_input"),
        ),
        // Forward-delete owns the same header deletion; it must not fall
        // through to the editor's clip deletion.
        KeyBinding::new(
            "delete",
            RemoveHeaderTracks,
            Some("hex_track_header && !hex_text_editing"),
        ),
        KeyBinding::new("enter", CommitTrackName, Some("hex_track_name")),
        KeyBinding::new("escape", CancelTrackName, Some("hex_track_name")),
    ]);
}
impl EditorView {
    /// A selected track header in either view owns keyboard track deletion.
    pub(super) fn remove_header_tracks(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(Target::Track(focused)) = self.focused_header_target(window) else {
            return false;
        };
        let tracks = self.selected_track_ids();
        if tracks.is_empty() || !tracks.contains(&focused) {
            return true;
        }
        if self.selection_delete_blocked() {
            return true;
        }
        if self.edit_pending()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.track_resize_active()
            || self.track_reorder_active()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
        {
            self.error = Some("finish the current edit before removing tracks".into());
            return true;
        }
        if super::device_panel::finalize_controls(self, cx)
            && self.enqueue_device_barrier(Request::Edit(Command::RemoveTracks(tracks)))
        {
            self.duplicate_intent.cancel_chain();
        }
        true
    }

    pub(super) fn track_header_focused(&self, window: &Window) -> bool {
        self.mode == super::EditorMode::Arrangement
            && self.track_name.focuses.iter().any(|(target, focus)| {
                matches!(target, Target::Track(_)) && focus.is_focused(window)
            })
    }
    pub(super) fn focused_header_target(&self, window: &Window) -> Option<Target> {
        self.track_name
            .focuses
            .iter()
            .find_map(|(target, focus)| focus.is_focused(window).then_some(*target))
    }
    pub(super) fn focus_track_header_after_navigation(&self, track: TrackId, window: &mut Window) {
        self.track_name
            .focuses
            .get(&Target::Track(track))
            .unwrap_or(&self.focus)
            .focus(window);
    }
    pub(super) fn reconcile_track_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &self.track_name.edit {
            super::browser_input::restyle(&edit.input, cx);
        }
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
                    .retain(|target, _| target.name(&state.project).is_some());
            }
            if old_focus.is_some_and(|id| !self.track_name.focuses.contains_key(&id)) {
                self.focus.focus(window);
            }
            self.track_name.scope = current;
        }
        if let Some(edit) = &self.track_name.edit
            && ((edit.tags.is_none() && !edit.target.name_editing_supported(self.mode))
                || edit.generation != self.project_generation
                || !self.state.as_ref().is_some_and(|state| {
                    state.revision == edit.revision && edit.matches(&state.project)
                }))
        {
            let focused = edit.input.read(cx).focus_handle().is_focused(window);
            self.track_name.edit = None;
            if focused {
                self.focus.focus(window);
            }
        }
        // An inline tag input can lose actual window focus before GPUI delivers
        // its deferred blur notification. Retire only this current tag draft;
        // no hidden edit may keep the shared header/loop guards active.
        if self.track_name.edit.as_ref().is_some_and(|edit| {
            edit.tags.is_some() && !edit.input.read(cx).focus_handle().is_focused(window)
        }) {
            self.commit_track_name(false, cx);
        }
    }
    pub(super) fn begin_track_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .track_name
            .focuses
            .iter()
            .find_map(|(target, focus)| focus.is_focused(window).then_some(*target))
        else {
            return;
        };
        self.begin_name_target(target, window, cx);
    }
    /// A delayed menu answer cannot retarget a newer document or canonical edit.
    pub(super) fn begin_captured_name(
        &mut self,
        target: Target,
        generation: u64,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == revision && target.name(&state.project).is_some()
            })
        {
            self.begin_name_target(target, window, cx);
        }
    }
    pub(super) fn begin_captured_track_tags(
        &mut self,
        track: TrackId,
        generation: u64,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation == self.project_generation
            && self.state.as_ref().is_some_and(|state| {
                state.revision == revision && state.project.track(track).is_some()
            })
        {
            self.begin_header_text(Target::Track(track), true, window, cx);
        }
    }
    fn begin_name_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_header_text(target, false, window, cx);
    }
    fn begin_header_text(
        &mut self,
        target: Target,
        tags: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tags && !self.track_view_change_available(cx) {
            self.error = Some("finish or cancel the current edit before editing tags".into());
            return;
        }
        if (!tags && !target.name_editing_supported(self.mode))
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
        if matches!(target, Target::Return(_))
            && self.mode == super::EditorMode::Session
            && self.track_gain_controls.has_pending_draft(cx)
        {
            self.error = Some("finish the current numeric edit before renaming the return".into());
            cx.notify();
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        let original_tags = if tags {
            let Target::Track(track) = target else { return };
            let Some(track) = state.project.track(track) else {
                return;
            };
            Some(track.tags().to_vec())
        } else {
            None
        };
        let Some(original) = original_tags
            .as_ref()
            .map(|tags| {
                tags.iter()
                    .map(|tag| tag.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .or_else(|| target.name(&state.project).map(str::to_owned))
        else {
            return;
        };
        let revision = state.revision;
        // This existing name-input mode preserves case/Unicode and selects all.
        let input = cx.new(|cx| {
            if tags {
                browser_input::tags(original.clone(), cx)
            } else {
                browser_input::name(original.clone(), cx)
            }
        });
        let focus = input.read(cx).focus_handle().clone();
        let input_id = input.entity_id();
        let blur = cx.on_blur(&focus, window, move |view, _, cx| {
            // A late notification from a retired input cannot commit its successor.
            if view
                .track_name
                .edit
                .as_ref()
                .is_some_and(|edit| edit.input.entity_id() == input_id)
            {
                view.commit_track_name(false, cx);
            }
        });
        self.track_name.edit = Some(NameEdit {
            target,
            generation: self.project_generation,
            revision,
            original,
            tags: original_tags,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
    }
    fn commit_track_name(&mut self, keep_invalid: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.track_name.edit.take() else {
            return;
        };
        let name = edit.input.read(cx).text().to_owned();
        if edit.generation == self.project_generation
            && !self.edit_pending()
            && self.state.as_ref().is_some_and(|state| {
                state.revision == edit.revision && edit.matches(&state.project)
            })
            && name != edit.original
        {
            if edit.tags.is_some() {
                match (edit.target, parse_tags(&name)) {
                    (Target::Track(track), Ok(tags)) => {
                        self.enqueue_device_barrier(Request::Edit(Command::SetTrackTags {
                            track,
                            tags,
                        }));
                    }
                    (_, Err(error)) => {
                        self.error = Some(error.to_string().into());
                        if keep_invalid {
                            self.track_name.edit = Some(edit);
                        }
                    }
                    _ => {}
                }
            } else if name.trim().is_empty() {
                self.error = Some("track name must not be empty".into());
            } else {
                self.enqueue(Request::Edit(edit.target.rename(name)));
            }
        }
        cx.notify();
    }
    /// Match the existing Enter/blur commit law, but keep a refused draft
    /// visible when a project replacement is requested.
    pub(super) fn finalize_track_name_for_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = &self.track_name.edit else {
            return true;
        };
        let name = edit.input.read(cx).text();
        if self.edit_pending()
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || edit.generation != self.project_generation
            || !self.state.as_ref().is_some_and(|state| {
                state.revision == edit.revision && edit.matches(&state.project)
            })
            || if edit.tags.is_some() {
                parse_tags(name).is_err()
            } else {
                name.trim().is_empty()
                    || name.len() > hex_project::MAX_NAME_BYTES
                    || name.chars().any(char::is_control)
            }
        {
            self.error =
                Some("finish or cancel the current name edit before opening a project".into());
            return false;
        }
        self.finish_track_name(true, window, cx);
        true
    }
    pub(super) fn finish_track_name(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let track = self.track_name.edit.as_ref().map(|edit| edit.target);
        if commit {
            self.commit_track_name(true, cx);
        } else {
            self.track_name.edit = None;
        }
        if let Some(edit) = &self.track_name.edit {
            edit.input.read(cx).focus_handle().focus(window);
            cx.notify();
            return;
        }
        track
            .and_then(|id| self.track_name.focuses.get(&id))
            .unwrap_or(&self.focus)
            .focus(window);
        cx.notify();
    }
    pub(super) fn track_tags_input(&self, track: TrackId) -> Option<Entity<TextInput>> {
        self.track_name
            .edit
            .as_ref()
            .filter(|edit| edit.target == Target::Track(track) && edit.tags.is_some())
            .map(|edit| edit.input.clone())
    }
    /// Existing header geometry and tooltip remain owned by tracks.rs.
    pub(super) fn track_header_focus(
        &mut self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.header_name_focus(Target::Track(track), cx)
    }

    /// Allocate one stable header focus owner, including before its Console
    /// page is painted during keyboard navigation.
    pub(super) fn header_name_focus(
        &mut self,
        target: Target,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.track_name
            .focuses
            .entry(target)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
    /// One inline label/input renderer for both stable naming owners. Row geometry
    /// and ordinary header focus remain with their existing callers.
    pub(super) fn track_name_label(
        &mut self,
        target: Target,
        name: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus = self.header_name_focus(target, cx);
        let click_focus = focus.clone();
        let (element_id, input_selector): (gpui::ElementId, &str) = match target {
            Target::Track(id) => (("track-name-focus", id.value()).into(), "track-name-input"),
            Target::Return(id) => (
                ("return-name-focus", id.value()).into(),
                "return-name-input",
            ),
        };
        let content = if let Some(edit) = &self.track_name.edit
            && edit.target == target
            && edit.tags.is_none()
        {
            div()
                .debug_selector(move || input_selector.into())
                .child(edit.input.clone())
        } else {
            div().child(theme::text(name))
        };
        div()
            .id(element_id)
            .min_w_0()
            .flex_1()
            .when(matches!(target, Target::Return(_)), |label| {
                label
                    .track_focus(&focus)
                    .key_context(if self.mode == super::EditorMode::Session {
                        "hex_return_header hex_console_return_header"
                    } else {
                        "hex_return_header"
                    })
            })
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if view
                        .track_name
                        .edit
                        .as_ref()
                        .is_some_and(|edit| edit.target == target)
                    {
                        return;
                    }
                    match target {
                        Target::Track(id) => {
                            view.select_track_with_modifiers(id, event.modifiers(), window)
                        }
                        Target::Return(id) => {
                            if !view.focus_return_details(id, window, cx) {
                                return;
                            }
                        }
                    }
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
        let deadline = crate::test_wait::hang_deadline();
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
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            let input = view.track_name.edit.as_ref().unwrap().input.read(cx);
            assert_eq!(
                input.text(),
                "Verse S 🎹",
                "display policy must preserve the submitted name"
            );
            assert_eq!(
                input.painted_text(),
                Some("verse s 🎹"),
                "editing and committed display must use the same lowercase policy"
            );
        });
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
            assert_eq!(
                view.track_name.edit.as_ref().unwrap().target,
                Target::Track(target)
            )
        });
        visual.simulate_keystrokes("cmd-r");
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.track_name.edit.as_ref().unwrap().target,
                Target::Track(target)
            )
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

#[cfg(all(test, feature = "ui-test-support"))]
mod delete_tests;
