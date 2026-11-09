//! Selection-driven, in-place clip label editing. Drafts never mutate the project.
use super::{CancelClipName, CommitClipName, EditorMode, EditorView, RenameClip, browser_input};
use crate::theme;
use gpui::{
    App, AppContext, Context, Entity, KeyBinding, Subscription, Window, div, prelude::*, px, rgb,
    rgba,
};
use hex_project::{ClipId, Command, SessionCellContent};
use hex_session::service::Request;
use hex_widgets::TextInput;

/// Shared title geometry; time-selection handles keep their own independent zones.
pub(super) const CLIP_NAME_HEIGHT: f32 = 14.0;

pub(super) struct NameEdit {
    clip: ClipId,
    generation: u64,
    revision: u64,
    original: String,
    input: Entity<TextInput>,
    _blur: Subscription,
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-r", RenameClip, Some("hex_editor && !hex_text_editing")),
        KeyBinding::new("enter", CommitClipName, Some("hex_clip_name")),
        KeyBinding::new("escape", CancelClipName, Some("hex_clip_name")),
    ]);
}
impl EditorView {
    pub(super) fn reconcile_clip_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &self.clip_name_edit {
            super::browser_input::restyle(&edit.input, cx);
        }
        if let Some(edit) = &self.clip_name_edit
            && (edit.generation != self.project_generation
                || self.clip_name_target() != Some(edit.clip)
                || !self.state.as_ref().is_some_and(|s| {
                    s.revision == edit.revision
                        && s.project.clip_name(edit.clip) == Some(edit.original.as_str())
                }))
        {
            let focused = edit.input.read(cx).focus_handle().is_focused(window);
            self.clip_name_edit = None;
            if focused {
                self.focus.focus(window);
            }
        }
    }

    fn clip_name_target(&self) -> Option<ClipId> {
        if self.mode == EditorMode::Session {
            let (track, scene) = self.selected_cell?;
            match self.state.as_ref()?.project.session_cell(track, scene)? {
                SessionCellContent::Audio(c) => Some(c.id()),
                SessionCellContent::Midi(c) => Some(c.id()),
                SessionCellContent::Stop => None,
            }
        } else {
            self.selected_clip
        }
    }
    pub(super) fn begin_clip_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_pending()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.time_range_move_gesture.is_some()
            || self.clip_paint_gesture.is_some()
            || self.clip_name_edit.is_some()
            || self.track_name.is_editing()
        {
            return;
        }
        let Some(clip) = self.clip_name_target() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(name) = state.project.clip_name(clip) else {
            return;
        };
        let original = name.to_owned();
        let revision = state.revision;
        let input = cx.new(|cx| browser_input::name(original.clone(), cx));
        let focus = input.read(cx).focus_handle().clone();
        let blur = cx.on_blur(&focus, window, |view, _, cx| {
            // Preserve the newly clicked control's focus.
            view.commit_clip_name(cx);
        });
        self.clip_name_edit = Some(NameEdit {
            clip,
            generation: self.project_generation,
            revision,
            original,
            input,
            _blur: blur,
        });
        focus.focus(window);
        cx.notify();
    }
    fn commit_clip_name(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.clip_name_edit.take() else {
            return;
        };
        let name = edit.input.read(cx).text().to_owned();
        if edit.generation == self.project_generation
            && self.state.as_ref().is_some_and(|s| {
                s.revision == edit.revision
                    && s.project.clip_name(edit.clip) == Some(edit.original.as_str())
            })
            && !self.edit_pending()
            && name != edit.original
        {
            if name.trim().is_empty() {
                self.error = Some("clip name must not be empty".into());
            } else {
                self.enqueue(Request::Edit(Command::RenameClip {
                    clip: edit.clip,
                    name,
                }));
            }
        }
        cx.notify();
    }
    /// Match the existing Enter/blur commit law, but keep a refused draft
    /// visible when a project replacement is requested.
    pub(super) fn finalize_clip_name_for_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = &self.clip_name_edit else {
            return true;
        };
        let name = edit.input.read(cx).text();
        if self.edit_pending()
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || edit.generation != self.project_generation
            || !self.state.as_ref().is_some_and(|state| {
                state.revision == edit.revision
                    && state.project.clip_name(edit.clip) == Some(edit.original.as_str())
            })
            || name.trim().is_empty()
            || name.len() > hex_project::MAX_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            self.error =
                Some("finish or cancel the current name edit before opening a project".into());
            return false;
        }
        self.finish_clip_name(true, window, cx);
        true
    }
    pub(super) fn finish_clip_name(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if commit {
            self.commit_clip_name(cx);
        } else {
            self.clip_name_edit = None;
        }
        self.focus.focus(window);
        cx.notify();
    }
    pub(super) fn clip_has_alias(&self, clip: ClipId) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.project.clip_alias(clip).is_some())
    }

    pub(super) fn clip_alias_indicator(
        &self,
        clip: ClipId,
        fill: u32,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.clip_has_alias(clip) {
            return None;
        }
        let ink = rgb(theme::readable_text(fill, 0x000000));
        Some(
            div()
                .id(("clip-alias", clip.value()))
                .debug_selector(move || format!("clip-alias-{}", clip.value()))
                .absolute()
                .right(px(2.0))
                .top(px(2.0))
                .w(px(10.0))
                .h(px(10.0))
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(3.0))
                        .w(px(7.0))
                        .h(px(6.0))
                        .border_1()
                        .border_color(ink)
                        .rounded(px(1.0)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(3.0))
                        .top(px(0.0))
                        .w(px(7.0))
                        .h(px(6.0))
                        .border_1()
                        .border_color(ink)
                        .rounded(px(1.0)),
                )
                .tooltip(|_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: "alias clip · shared content".into(),
                    })
                    .into()
                }),
        )
    }

    pub(super) fn clip_name_label(&self, clip: ClipId, name: &str) -> gpui::AnyElement {
        if let Some(edit) = &self.clip_name_edit
            && edit.clip == clip
            && edit.generation == self.project_generation
        {
            return div()
                .id(("clip-name-editor", clip.value()))
                .flex_1()
                .min_w_0()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(edit.input.clone())
                .into_any_element();
        }
        clip_name_text(name)
            .debug_selector(move || format!("clip-name-label-{}", clip.value()))
            .into_any_element()
    }
    pub(super) fn clip_name_header(
        &self,
        track: hex_project::TrackId,
        clip: ClipId,
        name: &str,
        fill: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.clip_name_strip(track, clip, name, fill, true, false, false, cx)
    }
    /// The canonical frame paints the same header fill under this content.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn clip_name_header_on_backdrop(
        &self,
        track: hex_project::TrackId,
        clip: ClipId,
        name: &str,
        fill: u32,
        notched: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.clip_name_strip(track, clip, name, fill, true, true, notched, cx)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn clip_name_preview_header(
        &self,
        track: hex_project::TrackId,
        clip: ClipId,
        name: &str,
        fill: u32,
        on_backdrop: bool,
        notched: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.clip_name_strip(track, clip, name, fill, false, on_backdrop, notched, cx)
    }
    pub(super) fn clip_name_drag_header(
        &self,
        clip: ClipId,
        name: &str,
        fill: u32,
        on_backdrop: bool,
    ) -> impl IntoElement + use<> {
        passive_name_header(name, fill, on_backdrop)
            .id(("clip-drag-name", clip.value()))
            .debug_selector(move || format!("clip-drag-name-{}", clip.value()))
    }

    #[allow(clippy::too_many_arguments)]
    fn clip_name_strip(
        &self,
        track: hex_project::TrackId,
        clip: ClipId,
        name: &str,
        fill: u32,
        interactive: bool,
        on_backdrop: bool,
        notched: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        clip_name_surface(fill)
            .when(on_backdrop, |header| header.bg(gpui::transparent_black()))
            .id(("clip-name", clip.value()))
            .debug_selector(move || format!("clip-name-{}", clip.value()))
            .when(interactive, |header| {
                header.cursor_pointer().on_click(cx.listener(
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        if view
                            .clip_name_edit
                            .as_ref()
                            .is_some_and(|edit| edit.clip == clip)
                        {
                            cx.stop_propagation();
                            return;
                        }
                        if view.clip_name_edit.is_some() {
                            view.finish_clip_name(true, window, cx);
                        }
                        view.select_clip_header(track, clip, event.modifiers(), window, cx);
                        view.focus.focus(window);
                        cx.notify();
                    },
                ))
            })
            .when(notched, |header| header.pr(px(6.0)))
            .when(self.clip_has_alias(clip), |header| {
                header.pr(px(if notched { 20.0 } else { 15.0 }))
            })
            .child(self.clip_name_label(clip, name))
            .children(
                self.clip_alias_indicator(clip, fill)
                    .map(|indicator| indicator.when(notched, |indicator| indicator.right(px(7.0)))),
            )
    }
}

/// A passive caption without an authored identity; clip and sample callers add
/// only their own diagnostic IDs. It never mounts a live rename input.
pub(super) fn passive_name_header(name: &str, fill: u32, on_backdrop: bool) -> gpui::Div {
    clip_name_surface(fill)
        .when(on_backdrop, |header| {
            header.bg(gpui::transparent_black()).pr(px(6.0))
        })
        .child(clip_name_text(name))
}

fn clip_name_surface(fill: u32) -> gpui::Div {
    let text = theme::readable_text(fill, theme::color(theme::PaletteRole::Text));
    div()
        .absolute()
        .top(px(-1.0))
        .left(px(-1.0))
        .right(px(-1.0))
        .h(px(CLIP_NAME_HEIGHT))
        .flex()
        .items_center()
        .px_1()
        .overflow_hidden()
        .text_xs()
        .line_height(px(14.0))
        .bg(rgb(fill))
        .border_b_1()
        .border_color(rgba((text << 8) | 0x70))
        .text_color(rgb(text))
}

// A passive drag clone must never mount the live rename input a second time.
fn clip_name_text(name: &str) -> gpui::Div {
    div().min_w_0().flex_1().truncate().child(theme::text(name))
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use std::sync::Arc;

    #[gpui::test]
    fn click_selects_header_cmd_r_edits_in_place_and_escape_cancels(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.read_with(cx, |v, cx| v.browser.search.read(cx).focus(window))
        });
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
        let header = visual.debug_bounds(selector).expect("visible name strip");
        visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| assert_eq!(v.selected_clip, Some(clip)));
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        let input = visual
            .debug_bounds("clip-name-input")
            .expect("in-place input");
        assert!(header.intersects(&input));
        visual.simulate_input("Verse S 🎹");
        visual.simulate_keystrokes("space l backspace");
        editor.read_with(cx, |v, cx| {
            assert_eq!(
                v.clip_name_edit.as_ref().unwrap().input.read(cx).text(),
                "Verse S 🎹 "
            );
            assert!(
                !v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .soloed()
            );
            assert!(v.last_enqueued_edit.is_none());
            assert!(v.last_transport_command.is_none());
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(v.clip_name_edit.is_none());
            assert_eq!(
                v.state.as_ref().unwrap().project.clip_name(clip),
                Some("clip")
            );
            assert!(v.last_enqueued_edit.is_none());
        });
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        visual.simulate_input("Chorus");
        visual.simulate_keystrokes("enter");
        editor.read_with(cx, |v, _| assert!(matches!(&v.last_enqueued_edit, Some(Command::RenameClip {clip: id, name}) if *id==clip && name=="Chorus")));
    }

    #[gpui::test]
    fn midi_and_session_sources_show_inline_drafts_without_launching(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, _, clip) = crate::editor::clip_drag_ui_test::midi_project();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.set_selected_clip(track, clip, window, cx);
                v.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-name-input").is_some());
        visual.simulate_keystrokes("escape");
        // Both source types use their existing Session label slot.
        let mut canonical = hex_project::Editor::new(project, 8).unwrap();
        let hex_project::Created::Scene(midi_scene) = canonical
            .execute(Command::AddScene {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        canonical
            .execute(Command::CopyArrangementMidiToSession {
                source: clip,
                scene: midi_scene,
            })
            .unwrap();
        let hex_project::Created::Imported {
            clip: audio,
            track: audio_track,
            ..
        } = canonical
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "missing.wav".into(),
                sample_rate: 48_000,
                frames: 48_000,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        let hex_project::Created::Scene(audio_scene) = canonical
            .execute(Command::AddScene {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        canonical
            .execute(Command::CopyArrangementAudioClipToSession {
                source: audio,
                scene: audio_scene,
            })
            .unwrap();
        // Each installed snapshot gets a new revision, as real project updates do,
        // so the Session projection rebuilds and includes the imported track.
        for (revision, (track, scene)) in
            (2..).zip([(track, midi_scene), (audio_track, audio_scene)])
        {
            visual.update(|window, cx| {
                editor.update(cx, |v, cx| {
                    v.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::new(canonical.project().clone()),
                        waveforms: Default::default(),
                        source_overviews: Default::default(),
                        audio_clip_waveforms: Default::default(),
                        midi_preview: Arc::default(),
                        gain_preview: Ok(Arc::default()),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision,
                        unsaved: false,
                        document_path: None,
                    }));
                    v.mode = EditorMode::Session;
                    v.selected_cell = Some((track, scene));
                    v.focus.focus(window);
                    cx.notify();
                })
            });
            visual.run_until_parked();
            visual.simulate_keystrokes("cmd-r");
            visual.run_until_parked();
            let input = visual
                .debug_bounds("clip-name-input")
                .expect("session inline editor");
            visual.simulate_mouse_down(input.center(), MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(input.center(), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(cx, |v, _| {
                assert!(v.clip_name_edit.is_some());
                assert!(v.last_transport_command.is_none());
            });
            visual.simulate_keystrokes("escape");
        }
    }

    #[gpui::test]
    fn stale_rename_never_changes_a_new_generation(cx: &mut TestAppContext) {
        cx.update(crate::editor::bind_editor_keys);
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.set_selected_clip(track, clip, window, cx);
                v.focus.focus(window);
            })
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        visual.simulate_input("stale");
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.project_generation += 1;
                v.finish_clip_name(true, window, cx);
                assert!(v.last_enqueued_edit.is_none());
            })
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_header_color_tests.rs"]
mod color_tests;
