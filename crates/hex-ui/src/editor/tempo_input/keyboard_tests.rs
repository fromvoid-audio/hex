//! Real focused key dispatch and canonical Service tempo edits.
use super::*;
use crate::editor::{bind_editor_keys, test_environment::Environment};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, size};
use hex_session::service::Service;
use std::{sync::Arc, time::Instant};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(Instant::now() < until, "real Service did not settle");
        std::thread::yield_now();
    }
}

fn fixture(cx: &mut TestAppContext) -> (VisualTestContext, Entity<EditorView>, Arc<Environment>) {
    cx.update(bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1280.), px(800.)));
    pump(&editor, &mut visual);
    (visual, editor, environment)
}

fn begin(visual: &mut VisualTestContext, selector: &'static str) {
    visual.run_until_parked();
    let bounds = visual.debug_bounds(selector).expect("rendered BPM field");
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn focused_project_arrows_adjust_local_draft_then_commit_exact_micro_bpm(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment) = fixture(cx);
    begin(&mut visual, "project-tempo-value");
    visual.simulate_keystrokes("up");
    editor.read_with(&visual, |view, cx| {
        let numeric = view.project_tempo_control.as_ref().unwrap().entity.read(cx);
        assert_eq!(
            numeric.display_text(),
            "121",
            "Up changes the focused BPM draft by one"
        );
        assert!(numeric.is_editing());
        assert!(view.last_enqueued_edit.is_none(), "arrow draft stays local");
    });
    visual.simulate_keystrokes("shift-down");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(
            view.project_tempo_control
                .as_ref()
                .unwrap()
                .entity
                .read(cx)
                .display_text(),
            "120.9",
            "Shift is fine even for a Command-fine opt-in field"
        );
    });
    visual.simulate_keystrokes("shift-up");
    visual.simulate_keystrokes("cmd-down cmd-down cmd-up down");
    visual.simulate_keystrokes("enter");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            119_900_000
        );
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            120_000_000
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Redo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            119_900_000
        )
    });
}

fn project_text(editor: &Entity<EditorView>, visual: &VisualTestContext) -> String {
    editor.read_with(visual, |view, cx| {
        view.project_tempo_control
            .as_ref()
            .unwrap()
            .entity
            .read(cx)
            .display_text()
            .to_owned()
    })
}

#[gpui::test]
fn project_steps_preserve_bounds_precision_cancel_invalid_and_composition(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment) = fixture(cx);
    begin(&mut visual, "project-tempo-value");
    for (text, keys, expected) in [
        ("1", "down cmd-down", "1"),
        ("1000", "up cmd-up", "1000"),
        ("120.123456", "up cmd-down", "121.023456"),
        ("120", "shift-up", "120.1"),
        ("120", "shift-down", "119.9"),
        ("120", "cmd-up", "120.1"),
    ] {
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(text);
        visual.simulate_keystrokes(keys);
        assert_eq!(project_text(&editor, &visual), expected);
    }
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("120.123456");
    for _ in 0..100 {
        visual.simulate_keystrokes("shift-up");
    }
    assert_eq!(project_text(&editor, &visual), "130.123456");
    for _ in 0..100 {
        visual.simulate_keystrokes("shift-down");
    }
    assert_eq!(project_text(&editor, &visual), "120.123456");
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("nan");
    visual.simulate_keystrokes("up cmd-down");
    assert_eq!(project_text(&editor, &visual), "nan");
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("120");
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let entity = view.project_tempo_control.as_ref().unwrap().entity.clone();
            entity.update(cx, |numeric, cx| {
                gpui::EntityInputHandler::replace_and_mark_text_in_range(
                    numeric,
                    Some(0..3),
                    "12",
                    None,
                    window,
                    cx,
                )
            });
        })
    });
    visual.simulate_keystrokes("up cmd-down");
    assert_eq!(
        project_text(&editor, &visual),
        "12",
        "composing text is not adjusted"
    );
    visual.simulate_keystrokes("escape");
    assert_eq!(project_text(&editor, &visual), "120");
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none());
        assert!(!view.state.as_ref().unwrap().can_undo);
    });
    // Returning to root focus keeps arrows out of the BPM owner.
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("up down cmd-up cmd-down");
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none())
    });
}

#[gpui::test]
fn sample_bpm_uses_same_arrow_policy_and_actual_canonical_history(cx: &mut TestAppContext) {
    let (mut visual, editor, environment) = fixture(cx);
    let path = environment.root().join("samples/tempo.wav");
    let frames = 96_000_u32;
    let n = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + n).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&n.to_le_bytes());
    for _ in 0..frames {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(path)))
    });
    pump(&editor, &mut visual);
    let clip = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let clip = view.state.as_ref().unwrap().project.tracks()[0].clips()[0].id();
        assert_eq!(
            view.selected_clip,
            Some(clip),
            "canonical import selects its initial clip"
        );
        clip
    });
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = true;
        let bounds = view.state.as_ref().unwrap().project.clip(clip).unwrap();
        let settings = hex_project::default_audio_warp(
            bounds,
            view.state.as_ref().unwrap().project.tempo_map(),
            hex_project::WarpMode::RePitch,
        )
        .unwrap();
        assert!(view.enqueue(Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings)
        })));
        cx.notify();
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |view, _| {
        format!("{:?}", view.state.as_ref().unwrap().project)
    });
    begin(&mut visual, "audio-sample-bpm-value");
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("120.123456");
    visual.simulate_keystrokes("up shift-up cmd-down down shift-down");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            format!("{:?}", view.state.as_ref().unwrap().project),
            before,
            "sample arrow draft is local"
        )
    });
    visual.simulate_keystrokes("enter");
    pump(&editor, &mut visual);
    let tempo = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .sample_bpm
            .unwrap()
            .micro_bpm()
    };
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(tempo(view), 120_023_456);
        assert!(matches!(view.last_enqueued_edit, Some(Command::SetAudioClipSampleBpm{clip: actual, ..}) if actual==clip));
    });
    let changed = editor.read_with(&visual, |view, _| {
        format!("{:?}", view.state.as_ref().unwrap().project)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    // Before its first explicit edit sample_bpm is None (tempo-map fallback),
    // not a fabricated Some(120). Undo restores the complete actual state.
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            format!("{:?}", view.state.as_ref().unwrap().project),
            before
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(tempo(view), 120_023_456);
        assert_eq!(
            format!("{:?}", view.state.as_ref().unwrap().project),
            changed
        );
    });
    begin(&mut visual, "audio-sample-bpm-value");
    visual.simulate_keystrokes("up cmd-up escape");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| assert_eq!(tempo(view), 120_023_456));
}
