use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};

pub(super) fn begin(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    let bounds = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
}

#[gpui::test]
fn project_tempo_layout_is_first_compact_and_keeps_neighbors_at_supported_widths(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Default::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        editor.update(cx, |view, cx| {
            view.mode = mode;
            cx.notify();
        });
        for width in [880.0, 1280.0] {
            visual.simulate_resize(size(px(width), px(540.0)));
            visual.run_until_parked();
            let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
            let tempo = visual.debug_bounds("project-tempo-field").unwrap();
            let tuning = visual.debug_bounds("project tuning · cmd+alt+t").unwrap();
            let play = visual
                .debug_bounds(if mode == EditorMode::Arrangement {
                    "play or pause · space · right-click for history playback"
                } else {
                    "play or pause · space"
                })
                .unwrap();
            let output = visual.debug_bounds("audio output status").unwrap();
            assert!(tempo.left() >= toolbar.left() + px(crate::theme::TITLEBAR_LEADING_INSET));
            assert!(tempo.size.width <= px(82.0));
            assert!(tempo.right() <= tuning.origin.x && tuning.right() < play.origin.x);
            assert!(play.right() <= toolbar.right());
            assert!(output.right() <= toolbar.right());
        }
    }
}

#[gpui::test]
fn project_tempo_stale_revision_and_project_switch_retire_drafts(cx: &mut TestAppContext) {
    cx.update(bind_editor_keys);
    for project_switch in [false, true] {
        let project = hex_project::Project::new(48_000).unwrap();
        let (handle, editor) =
            clip_drag_ui_test::editor_window(cx, project.clone(), Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        begin(&mut visual);
        visual.simulate_input("123.456789");
        editor.update(cx, |view, cx| {
            let mut model = hex_project::Editor::new(project.clone(), 8).unwrap();
            model
                .execute(hex_project::Command::SetTempoPoint {
                    beat: hex_project::BeatTime::ZERO,
                    tempo: hex_project::Tempo::new(90_000_000).unwrap(),
                })
                .unwrap();
            let old = view.state.as_ref().unwrap();
            view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: std::sync::Arc::new(model.project().clone()),
                waveforms: old.waveforms.clone(),
                source_overviews: old.source_overviews.clone(),
                audio_clip_waveforms: old.audio_clip_waveforms.clone(),
                midi_preview: old.midi_preview.clone(),
                gain_preview: old.gain_preview.clone(),
                timeline_frames: old.timeline_frames,
                can_undo: true,
                can_redo: false,
                revision: old.revision + u64::from(!project_switch),
                unsaved: true,
                document_path: None,
            }));
            if project_switch {
                view.project_generation += 1;
            }
            cx.notify();
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("enter");
        editor.update(cx, |view, _| {
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tempo_map()
                    .tempo_at(hex_project::BeatTime::ZERO)
                    .micro_bpm(),
                90_000_000
            );
        });
    }
}

#[gpui::test]
fn project_tempo_cursor_movement_cannot_retarget_an_active_draft(cx: &mut TestAppContext) {
    cx.update(bind_editor_keys);
    let mut model =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    model
        .execute(hex_project::Command::SetTempoPoint {
            beat: hex_project::BeatTime::from_quarters(2).unwrap(),
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let (handle, editor) =
        clip_drag_ui_test::editor_window(cx, model.project().clone(), Default::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    begin(&mut visual);
    visual.simulate_input("91.234567");
    editor.update(cx, |view, cx| {
        view.offline_cursor = 96_000;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    let command = editor.read_with(cx, |view, _| view.last_enqueued_edit.clone().unwrap());
    model.execute(command).unwrap();
    assert_eq!(
        model.project().tempo_map().points().len(),
        1,
        "a global edit must replace the tempo map, not leave a later tempo point"
    );
    for quarter in [0, 2, 8] {
        assert_eq!(
            model
                .project()
                .tempo_map()
                .tempo_at(hex_project::BeatTime::from_quarters(quarter).unwrap())
                .micro_bpm(),
            91_234_567
        );
    }
}

#[gpui::test]
fn project_tempo_direct_input_validation_cancel_focus_and_canonical_commit(
    cx: &mut TestAppContext,
) {
    cx.update(bind_editor_keys);
    let project = hex_project::Project::new(48_000).unwrap();
    let (handle, editor) =
        clip_drag_ui_test::editor_window(cx, project.clone(), Default::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("project-tempo-value")
        .expect("direct top-left project tempo input");
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    for invalid in ["", "nan", "inf", "0.999999", "1000.000001", "-12", "abc"] {
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_keystrokes("backspace");
        visual.simulate_input(invalid);
        visual.simulate_keystrokes("enter");
        editor.read_with(cx, |view, _| {
            assert!(
                view.last_enqueued_edit.is_none(),
                "invalid {invalid:?} must not enqueue"
            );
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tempo_map()
                    .tempo_at(hex_project::BeatTime::ZERO)
                    .micro_bpm(),
                120_000_000
            );
        });
    }
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("60");
    visual.simulate_keystrokes("m space z x c v l s");
    editor.read_with(cx, |view, _| {
        assert!(
            !view.computer_keyboard.enabled,
            "text entry must not toggle keyboard"
        )
    });
    visual.simulate_keystrokes("escape");
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "cancel leaves canonical tempo alone"
        )
    });
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("123.456789");
    editor.read_with(cx, |view, _| {
        assert!(view.last_enqueued_edit.is_none(), "draft stays local")
    });
    visual.simulate_keystrokes("enter");
    let command = editor.read_with(cx, |view, _| {
        view.last_enqueued_edit
            .clone()
            .expect("canonical tempo command")
    });
    assert!(
        matches!(command, hex_project::Command::SetProjectTempo { tempo } if tempo.micro_bpm() == 123_456_789)
    );
    let mut model = hex_project::Editor::new(project.clone(), 8).unwrap();
    model.execute(command).unwrap();
    let beat = hex_project::BeatTime::from_quarters(4).unwrap();
    assert_ne!(
        model.project().tempo_map().beat_to_frame(beat).unwrap(),
        project.tempo_map().beat_to_frame(beat).unwrap()
    );
    let changed = model.project().clone();
    assert!(model.undo());
    assert_eq!(model.project().tempo_map(), project.tempo_map());
    assert!(model.redo());
    assert_eq!(model.project().tempo_map(), changed.tempo_map());
}

#[gpui::test]
fn project_tempo_blur_commits_valid_text_and_invalid_blur_releases_editor_shortcuts(
    cx: &mut TestAppContext,
) {
    cx.update(bind_editor_keys);
    for (text, expected) in [("130.25", Some(130_250_000)), ("", None), ("nan", None)] {
        let (handle, editor) = clip_drag_ui_test::editor_window(
            cx,
            hex_project::Project::new(48_000).unwrap(),
            Default::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        begin(&mut visual);
        visual.simulate_keystrokes("backspace");
        visual.simulate_input(text);
        visual.run_until_parked();
        visual.update(|_, cx| {
            editor.update(cx, |view, cx| {
                assert!(
                    view.project_tempo_editing(cx),
                    "draft must actually own text focus before blur"
                )
            })
        });
        visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            match expected {
                Some(micro) => assert!(matches!(view.last_enqueued_edit, Some(hex_project::Command::SetProjectTempo { tempo }) if tempo.micro_bpm() == micro)),
                None => assert!(view.last_enqueued_edit.is_none()),
            }
        });
        if expected.is_none() {
            visual.simulate_keystrokes("m");
            editor.read_with(cx, |view, _| {
                assert!(
                    view.computer_keyboard.enabled,
                    "invalid unfocused tempo draft must not suppress editor shortcuts"
                )
            });
        }
    }
}

#[gpui::test]
fn playback_vector_controls_follow_actual_transport_status(cx: &mut TestAppContext) {
    use crate::editor::device_panel::tests::eq_ui_tests::{graph_layout_tests, held_pointer_tests};
    use crate::editor::midi_canvas::creation_tests::fixture;
    use std::time::Instant;

    let _guard = graph_layout_tests::status_fixture_guard();
    let (mut visual, editor, _environment, _) = fixture(cx);
    let (mut output, mut backend) =
        held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    visual.run_until_parked();
    let play = visual
        .debug_bounds("play or pause · space · right-click for history playback")
        .unwrap();
    let stop = visual.debug_bounds("stop · escape").unwrap();
    assert!(
        !visual
            .rendered_monochrome_colors("transport-play-icon")
            .is_empty()
    );
    assert!(
        !visual
            .rendered_monochrome_colors("transport-stop-icon")
            .is_empty()
    );
    assert!(visual.debug_bounds("transport-pause-icon").is_none());

    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        visual.simulate_mouse_move(gpui::point(px(2.0), px(500.0)), None, Modifiers::default());
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let style = crate::theme::action_style();
        let normal = gpui::Hsla::from(gpui::rgb(style.text));
        assert!(
            visual
                .rendered_monochrome_colors("transport-play-icon")
                .contains(&normal)
        );
        visual.simulate_mouse_move(
            gpui::point(play.origin.x + px(1.0), play.origin.y + px(1.0)),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let hover = gpui::Hsla::from(gpui::rgb(style.hover_text.unwrap_or(style.text)));
        assert!(
            visual
                .rendered_monochrome_colors("transport-play-icon")
                .contains(&hover)
        );
    }

    for (button, playing) in [(play, true), (stop, false)] {
        visual.simulate_click(button.center(), Modifiers::default());
        let deadline = crate::test_wait::hang_deadline();
        loop {
            held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            if editor.read_with(cx, |view, _| {
                view.audio_status().is_some_and(|status| {
                    status.playing.load(std::sync::atomic::Ordering::Acquire) == playing
                })
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "transport command did not reach the callback"
            );
        }
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let (shown, hidden) = if playing {
            ("transport-pause-icon", "transport-play-icon")
        } else {
            ("transport-play-icon", "transport-pause-icon")
        };
        assert!(!visual.rendered_monochrome_colors(shown).is_empty());
        assert!(visual.debug_bounds(hidden).is_none());
        assert_eq!(
            visual.debug_bounds("play or pause · space · right-click for history playback"),
            Some(play)
        );
        assert_eq!(visual.debug_bounds("stop · escape"), Some(stop));
    }
}

#[gpui::test]
fn transport_edit_vectors_keep_history_commands_and_theme_colors(cx: &mut TestAppContext) {
    use crate::editor::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, _) = fixture(cx);
    visual.run_until_parked();
    let split = visual
        .debug_bounds("split selection edges or edit cursor · e")
        .unwrap();
    let undo = visual.debug_bounds("undo · cmd+z").unwrap();
    let redo = visual.debug_bounds("redo · cmd+shift+z").unwrap();
    for selector in [
        "transport-split-icon",
        "transport-undo-icon",
        "transport-redo-icon",
    ] {
        assert!(!visual.rendered_monochrome_colors(selector).is_empty());
    }
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        visual.simulate_mouse_move(gpui::point(px(2.0), px(500.0)), None, Modifiers::default());
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let style = crate::theme::action_style();
        assert!(
            visual
                .rendered_monochrome_colors("transport-split-icon")
                .contains(&gpui::Hsla::from(gpui::rgb(style.text)))
        );
        visual.simulate_mouse_move(
            gpui::point(split.origin.x + px(1.0), split.origin.y + px(1.0)),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(
            visual
                .rendered_monochrome_colors("transport-split-icon")
                .contains(&gpui::Hsla::from(gpui::rgb(
                    style.hover_text.unwrap_or(style.text)
                )))
        );
    }
    let count = editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len());
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddMidiTrack {
                name: "history test".into()
            }
        )))
    });
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len()),
        count + 1
    );
    visual.simulate_click(undo.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len()),
        count
    );
    visual.simulate_click(redo.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len()),
        count + 1
    );
    assert_eq!(visual.debug_bounds("undo · cmd+z"), Some(undo));
    assert_eq!(visual.debug_bounds("redo · cmd+shift+z"), Some(redo));
    assert_eq!(
        visual.debug_bounds("split selection edges or edit cursor · e"),
        Some(split)
    );
}

#[gpui::test]
fn transport_creation_vectors_preserve_track_and_clip_commands(cx: &mut TestAppContext) {
    use crate::editor::midi_canvas::creation_tests::{fixture, pump};
    let (mut visual, editor, _environment, _) = fixture(cx);
    editor.update(cx, |view, cx| {
        view.offline_cursor = 96_000;
        cx.notify();
    });
    visual.run_until_parked();
    let add = visual.debug_bounds("add track").unwrap();
    let midi = visual
        .debug_bounds("add four-beat midi clip · cmd+shift+m")
        .unwrap();
    let remove = visual.debug_bounds("remove selected track").unwrap();
    for selector in [
        "transport-add-track-icon",
        "transport-add-midi-icon",
        "transport-remove-track-icon",
    ] {
        assert!(!visual.rendered_monochrome_colors(selector).is_empty());
    }
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            crate::theme::apply_preferences(self.0);
        }
    }
    let original = crate::theme::preferences();
    let _restore = RestoreTheme(original);
    for (background, text) in [(0x20, 0xe0), (0xe0, 0x20)] {
        let mut preferences = original;
        preferences.background = hex_preferences::RgbColor::new(background, background, background);
        preferences.text = hex_preferences::RgbColor::new(text, text, text);
        crate::theme::apply_preferences(preferences);
        visual.simulate_mouse_move(gpui::point(px(2.0), px(500.0)), None, Modifiers::default());
        editor.update(cx, |_, cx| cx.notify());
        visual.run_until_parked();
        let style = crate::theme::action_style();
        assert!(
            visual
                .rendered_monochrome_colors("transport-add-track-icon")
                .contains(&gpui::Hsla::from(gpui::rgb(style.text)))
        );
        visual.simulate_mouse_move(
            gpui::point(add.origin.x + px(1.0), add.origin.y + px(1.0)),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(
            visual
                .rendered_monochrome_colors("transport-add-track-icon")
                .contains(&gpui::Hsla::from(gpui::rgb(
                    style.hover_text.unwrap_or(style.text)
                )))
        );
    }
    let (track, tracks, clips) = editor.read_with(cx, |view, _| {
        let track = view.selected_track.unwrap();
        (
            track,
            view.state().unwrap().project.tracks().len(),
            view.state()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .len(),
        )
    });
    visual.simulate_click(midi.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view
            .state()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len()),
        clips + 1
    );
    visual.simulate_click(add.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len()),
        tracks + 1
    );
    editor.update(cx, |view, cx| {
        view.selected_track = view
            .state()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|candidate| candidate.id() != track)
            .map(|candidate| candidate.id());
        cx.notify();
    });
    visual.simulate_click(remove.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(cx, |view, _| view.state().unwrap().project.tracks().len()),
        tracks
    );
    assert_eq!(visual.debug_bounds("add track"), Some(add));
    assert_eq!(visual.debug_bounds("remove selected track"), Some(remove));
    assert_eq!(
        visual.debug_bounds("add four-beat midi clip · cmd+shift+m"),
        Some(midi)
    );
}
