use crate::editor::{
    clip_drag_ui_test::{pump_move as pump, upper_rail_owned_fixture},
    musical_time::SnapGrid,
    viewport::Viewport,
};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::Command;

#[gpui::test]
fn actual_cmd_f_uses_arranger_range_before_browser_search_and_one_undo(cx: &mut TestAppContext) {
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    let (track, clip, before) = editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 0,
            length: 96_000,
        });
        v.snap_grid = SnapGrid::Sixteenth;
        cx.notify();
        let p = &v.state.as_ref().unwrap().project;
        (
            p.tracks()[0].id(),
            p.tracks()[0].clips()[0].id(),
            p.snapshot(),
        )
    });
    visual.update(|window, cx| {
        window.activate_window();
        editor.read(cx).focus.focus(window);
    });
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let from = point(
        lane.left() + lane.size.width / 12.0,
        lane.top() + lane.size.height * 0.6,
    );
    let to = point(lane.left() + px(3.0), from.y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.time_selection_gesture.is_some(),
            "clip body must begin a range; trim={} clipdrag={} from={from:?} lane={lane:?}",
            v.active_trim_drag.is_some(),
            v.active_clip_drag.is_some()
        )
    });
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-f");
    editor.read_with(&visual, |v, _| {
        assert!(
            v.time_selection_gesture.is_some(),
            "held range retains input custody"
        );
        assert!(v.last_enqueued_edit.is_none());
    });
    visual.update(|window, cx| {
        assert!(
            editor.read(cx).focus.is_focused(window),
            "held gesture must not open browser search"
        )
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let range = editor.read_with(&visual, |v, _| v.normalized_time_selection().unwrap());
    assert_eq!(range.start(), 0);
    assert!(range.end() > 4_000 && range.end() < 12_000);
    visual.simulate_keystrokes("cmd-f");
    editor.read_with(&visual, |v, _| {
        assert!(matches!(&v.last_enqueued_edit, Some(Command::CreateArrangementFades { range: queued }) if queued == &range), "Command-F must dispatch the canonical fade intent");
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    pump(&editor, &mut visual);
    let accepted = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .retained_fade()
                .fade_in(),
            range.duration()
        );
        assert_eq!(v.normalized_time_selection(), Some(range));
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted)
    });
}

#[gpui::test]
fn cmd_f_respects_focused_context_and_explicit_search_intent(cx: &mut TestAppContext) {
    use crate::editor::{EditorMode, SearchBrowser, time_selection::TimeSelection};
    for context in [
        "header",
        "browser",
        "midi",
        "automation",
        "explicit-search",
        "session",
    ] {
        let cx = &mut cx.new_app();
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1280.0), px(800.0)));
        let midi = if context == "midi" {
            let track = editor.read_with(&visual, |v, _| {
                v.state.as_ref().unwrap().project.tracks()[1].id()
            });
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(hex_session::service::Request::Edit(
                    Command::PlaceMidiClip {
                        track,
                        start: hex_project::BeatTime::ZERO,
                        length: hex_project::BeatTime::ONE,
                    }
                )))
            });
            pump(&editor, &mut visual);
            Some(editor.read_with(&visual, |v, _| {
                (
                    track,
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .midi_clips()[0]
                        .id(),
                )
            }))
        } else {
            None
        };
        let before = editor.update(&mut visual, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 0,
                end_frame: 6_000,
            });
            v.last_enqueued_edit = None;
            if let Some((track, clip)) = midi {
                v.selected_track = Some(track);
                v.selected_clip = Some(clip);
                v.clip_view_open = true;
                v.device_panel_active = false;
            }
            if context == "automation" {
                v.automation_view.enabled = true;
            }
            if context == "session" {
                v.mode = EditorMode::Session;
            }
            cx.notify();
            v.state.as_ref().unwrap().project.snapshot()
        });
        visual.update(|window, cx| {
            window.activate_window();
            editor.update(cx, |v, cx| match context {
                "header" => v.focus_track_header_after_navigation(
                    v.state.as_ref().unwrap().project.tracks()[0].id(),
                    window,
                ),
                "browser" => v.search_browser(window, cx),
                "midi" => v.midi_focus.focus(window),
                _ => v.focus.focus(window),
            });
        });
        visual.run_until_parked();
        if context == "explicit-search" {
            visual.update(|window, cx| window.dispatch_action(Box::new(SearchBrowser), cx));
            visual.run_until_parked();
        } else {
            visual.simulate_keystrokes("cmd-f");
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            if context == "header" {
                assert!(matches!(
                    v.last_enqueued_edit,
                    Some(Command::CreateArrangementFades { .. })
                ));
            } else {
                assert!(v.last_enqueued_edit.is_none(), "context {context}");
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            }
        });
        if context != "header" {
            visual.update(|window, cx| {
                assert!(
                    editor
                        .read(cx)
                        .browser
                        .search
                        .read(cx)
                        .focus_handle()
                        .is_focused(window),
                    "search must acquire input focus; context={context}"
                )
            });
            visual.simulate_input("filter");
            visual.run_until_parked();
            editor.read_with(&visual, |v, cx| {
                assert_eq!(
                    v.browser.search.read(cx).text(),
                    "filter",
                    "context={context}"
                )
            });
        } else {
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
        }
    }
}
