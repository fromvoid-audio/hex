//! Actual clip-focused keys and audio brace input across real worker terminals.
use super::*;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::BeatTime;
use std::time::Instant;
pub(in crate::editor) fn hold(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Some(event) = editor.update(&mut *visual, |v, _| v.service.try_event().ok()) {
            return event;
        }
        assert!(Instant::now() < deadline, "real worker terminal");
        std::thread::yield_now();
    }
}
fn range(visual: &mut VisualTestContext, selector: &'static str, a: f32, b: f32) {
    let bounds = visual.debug_bounds(selector).unwrap();
    let down = point(
        bounds.left() + bounds.size.width * a,
        bounds.top() + px(10.0),
    );
    let up = point(bounds.left() + bounds.size.width * b, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn clip_editor_midi_w_and_l_set_source_selection_and_keep_global_loop_isolated(
    cx: &mut TestAppContext,
) {
    for console in [false, true] {
        check_midi_loop_keys(cx, console);
    }
}
fn check_midi_loop_keys(cx: &mut TestAppContext, console: bool) {
    let (mut visual, editor, environment, mut clip) = midi_canvas::creation_tests::fixture(cx);
    if console {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddScene {
                name: "scene".into()
            })));
        });
        midi_canvas::creation_tests::pump(&editor, &mut visual);
        let scene = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.scenes()[0].id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                    source: clip,
                    scene
                }))
            );
        });
        midi_canvas::creation_tests::pump(&editor, &mut visual);
        clip = editor.update(&mut visual, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            let Some(hex_project::SessionCellContent::Midi(source)) =
                v.state.as_ref().unwrap().project.session_cell(track, scene)
            else {
                panic!("midi cell")
            };
            let id = source.id();
            v.mode = EditorMode::Session;
            v.selected_cell = Some((track, scene));
            v.selected_clip = Some(id);
            cx.notify();
            id
        });
        visual.run_until_parked();
    }
    range(&mut visual, "midi-source-ruler", 0.75, 0.25);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let selection = editor.read_with(&visual, |v, _| v.midi_time_selection_bounds().unwrap());
    assert_eq!(selection, (BeatTime::ONE.raw(), BeatTime::ONE.raw() * 3));
    for (key, enabled) in [("w", true), ("l", false)] {
        visual.simulate_keystrokes(key);
        midi_canvas::creation_tests::pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let source = p.midi_source(clip).unwrap();
            assert_eq!(
                (
                    source.loop_start().raw(),
                    source.loop_end().raw(),
                    source.looping()
                ),
                (selection.0, selection.1, enabled)
            );
            assert!(p.loop_region().is_none());
            assert_eq!(v.midi_time_selection_bounds(), Some(selection));
            if !console {
                assert_eq!(
                    p.midi_clip(clip).unwrap().length(),
                    BeatTime::from_quarters(4).unwrap()
                );
            }
        });
    }
    visual.simulate_keystrokes("cmd-z");
    midi_canvas::creation_tests::pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .looping()
        )
    });
    visual.simulate_keystrokes("cmd-z");
    midi_canvas::creation_tests::pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    midi_canvas::creation_tests::pump(&editor, &mut visual);
    let reopened = midi_ruler::roundtrip(&editor, &mut visual, &environment);
    assert_eq!(
        reopened.midi_source(clip).unwrap().loop_start().raw(),
        selection.0
    );
    assert!(reopened.midi_source(clip).unwrap().looping());
}
#[gpui::test]
fn clip_editor_audio_w_l_and_brace_keep_full_placement_and_released_paint(cx: &mut TestAppContext) {
    let (files, handle, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    let clip = editor.update(&mut visual, |v, cx| {
        let t = &v.state.as_ref().unwrap().project.tracks()[0];
        let clip = t.clips()[0].id();
        v.selected_track = Some(t.id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.audio_event_focus.focus(window)));
    visual.run_until_parked();
    // A legacy source loop remains that same canonical loop when no range is
    // selected; toggling must not capture its already-repeated output as a view.
    for (key, enabled) in [("w", true), ("l", false)] {
        visual.simulate_keystrokes(key);
        clip_drag_ui_test::pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let c = v.state.as_ref().unwrap().project.clip(clip).unwrap();
            assert_eq!(c.looping(), enabled);
            assert!(c.audio_edit().is_none());
        });
    }
    if !editor.read_with(&visual, |v, _| v.audio_loop_grid().is_off()) {
        visual.simulate_keystrokes("cmd-4");
    }
    range(&mut visual, "audio-events-waveform", 0.75, 0.25);
    let (before, selected) = editor.read_with(&visual, |v, _| {
        (
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .clone(),
            v.audio_loop_time_selection().unwrap(),
        )
    });
    visual.simulate_keystrokes("w");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let c = p.clip(clip).unwrap();
        let r = c.audio_event_root();
        assert_eq!(
            (c.start(), c.duration()),
            (before.start(), before.duration())
        );
        assert_eq!((r.view_start, r.view_end), selected);
        assert_eq!(r.domain, before.audio_event_root().domain);
        assert_eq!(
            r.placement.unwrap().mode,
            hex_project::AudioViewMode::Repeat
        );
        assert!(p.loop_region().is_none());
        assert_eq!(v.audio_loop_time_selection(), Some(selected));
    });
    assert_eq!(
        visual
            .debug_bounds("audio-loop-end-marker")
            .unwrap()
            .right(),
        visual.debug_bounds("audio-loop-end").unwrap().right(),
        "painted end boundary matches its canonical edge"
    );
    let edge = visual.debug_bounds("audio-loop-start").unwrap();
    let bar = visual.debug_bounds("audio-loop-ruler").unwrap();
    let down = edge.center();
    let up = point(down.x + bar.size.width * 0.25, down.y);
    let free = Modifiers::default();
    visual.simulate_mouse_down(down, MouseButton::Left, free);
    visual.simulate_mouse_move(up, MouseButton::Left, free);
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-loop-start").unwrap();
    assert!(held.left() > edge.left() + px(5.0));
    visual.simulate_mouse_up(up, MouseButton::Left, free);
    let event = hold(&editor, &mut visual);
    assert!(
        event.request_complete && event.error.is_none(),
        "{:?}",
        event.error
    );
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("audio-loop-start").unwrap(),
            held,
            "released loop marker remains painted until its exact terminal"
        );
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("audio-loop-start").unwrap(),
        held,
        "acknowledged loop edits preserve the source ruler's coordinate system"
    );
    editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.clip(clip).unwrap();
        let r = c.audio_event_root();
        assert!(r.view_start > selected.0);
        assert_eq!(r.view_end, selected.1);
        assert_eq!(
            (c.start(), c.duration()),
            (before.start(), before.duration())
        );
    });
    // Expanding the opposite marker stays inside the same retained-source view.
    let end = visual.debug_bounds("audio-loop-end").unwrap().center();
    let bar = visual.debug_bounds("audio-loop-ruler").unwrap();
    let expanded = point(bar.left() + bar.size.width * 0.9, end.y);
    visual.simulate_mouse_down(end, MouseButton::Left, free);
    visual.simulate_mouse_move(expanded, MouseButton::Left, free);
    visual.simulate_mouse_up(expanded, MouseButton::Left, free);
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root()
                .view_end
                > selected.1
        )
    });
    visual.simulate_keystrokes("cmd-z");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root()
                .view_end,
            selected.1
        )
    });
    visual.simulate_keystrokes("cmd-z");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    visual.simulate_keystrokes("l");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.clip(clip).unwrap();
        assert_eq!(
            c.audio_event_root().placement.unwrap().mode,
            hex_project::AudioViewMode::Once
        );
        assert!(v.state.as_ref().unwrap().project.loop_region().is_none());
    });

    for stale in [false, true] {
        let before_cancel =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let marker = visual.debug_bounds("audio-loop-end").unwrap();
        let at = marker.center();
        let moved = point(at.x - px(25.0), at.y);
        visual.simulate_mouse_down(at, MouseButton::Left, free);
        visual.simulate_mouse_move(moved, MouseButton::Left, free);
        visual.run_until_parked();
        if stale {
            editor.update(&mut visual, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            });
            visual.run_until_parked();
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(moved, MouseButton::Left, free);
        clip_drag_ui_test::pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_cancel)
        });
    }
    let reopened = midi_ruler::roundtrip(&editor, &mut visual, &files);
    assert_eq!(
        reopened.snapshot(),
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot())
    );
    let snapshot = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let source_tab = visual.debug_bounds("audio-source-tab").unwrap();
    visual.simulate_click(source_tab.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("w l");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            snapshot,
            "source inspector keys never change the global transport loop"
        )
    });
}
