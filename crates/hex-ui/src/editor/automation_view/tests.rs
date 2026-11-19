//! Real rendered keys and lane edits through the ordinary worker/receipt owner.
use super::*;
use crate::editor::loop_selection_ui_test::{click, fixture, pump};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
fn selector(prefix: &str, id: u64) -> &'static str {
    Box::leak(format!("{prefix}-{id}").into_boxed_str())
}

fn toggle_toolbar(visual: &mut VisualTestContext) {
    let button = visual.debug_bounds("automation-view-toggle").unwrap();
    visual.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn toolbar_roundtrip_fades_audio_and_midi_without_activation_selection_or_history_edits(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, midi, false);
    click(&mut visual, audio, true);
    editor.update(&mut visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        v.time_selection = Some(crate::editor::time_selection::TimeSelection {
            anchor_track: Some(p.tracks()[0].id()),
            end_track: Some(p.tracks()[1].id()),
            anchor_frame: 12_000,
            end_frame: 36_000,
        });
    });
    let before = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.state.as_ref().unwrap().revision,
            v.selected_clip_ids(),
        )
    });
    let fills = [audio, midi].map(|clip| {
        visual
            .rendered_background(selector("clip-name", clip.value()))
            .unwrap()
    });
    toggle_toolbar(&mut visual);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.automation_view.enabled);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.0);
        assert_eq!(v.state.as_ref().unwrap().revision, before.1);
        assert_eq!(v.selected_clip_ids(), before.2);
        assert_eq!(v.worker_requests_pending, 0);
    });
    for (i, clip) in [audio, midi].into_iter().enumerate() {
        assert_eq!(
            visual.rendered_background(selector("clip-name", clip.value())),
            Some(fills[i].opacity(0.22))
        );
    }
    for key in [
        "0",
        "c",
        "cmd-d",
        "e",
        "b",
        "cmd-b",
        "r",
        "g",
        "cmd-x",
        "cmd-v",
        "cmd-shift-v",
        "right",
        "left",
        "shift-right",
        "shift-left",
        "cmd-shift-m",
        "cmd-r",
    ] {
        visual.simulate_keystrokes(key);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before.0,
                "{key}"
            );
            assert!(v.clip_name_edit.is_none(), "{key}");
        });
    }
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.track_header_focus(track, cx).focus(window);
        });
    });
    visual.run_until_parked();
    for key in ["right", "shift-right", "cmd-shift-m"] {
        visual.simulate_keystrokes(key);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before.0,
                "header {key}"
            );
        });
    }
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    toggle_toolbar(&mut visual);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.automation_view.enabled);
        assert_eq!(v.selected_clip_ids(), before.2);
        assert_eq!(v.state.as_ref().unwrap().revision, before.1);
    });
    for (i, clip) in [audio, midi].into_iter().enumerate() {
        assert_eq!(
            visual.rendered_background(selector("clip-name", clip.value())),
            Some(fills[i])
        );
    }
}

#[gpui::test]
fn foreground_mode_blocks_single_clip_nudge_and_trim_with_root_or_header_focus(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for clip in [audio, midi] {
        click(&mut visual, clip, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        toggle_toolbar(&mut visual);
        for header in [false, true] {
            if header {
                visual.update(|window, cx| {
                    editor.update(cx, |v, cx| {
                        v.track_header_focus(v.selected_track.unwrap(), cx)
                            .focus(window);
                    });
                });
            }
            visual.run_until_parked();
            for key in ["right", "left", "shift-right", "shift-left"] {
                visual.simulate_keystrokes(key);
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    assert_eq!(
                        v.state.as_ref().unwrap().project.snapshot(),
                        before,
                        "{clip:?} header={header} {key}"
                    );
                });
            }
        }
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert!(v.track_name.is_editing()));
        visual.simulate_keystrokes("a");
        editor.read_with(&visual, |v, _| assert!(v.automation_view.enabled));
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        toggle_toolbar(&mut visual);
    }
}

#[gpui::test]
fn foreground_curve_owns_pointer_and_released_marker_until_exact_worker_terminal(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    toggle_toolbar(&mut visual);
    visual.run_until_parked();
    let background_selection =
        editor.read_with(&visual, |v, _| (v.selected_track, v.selected_clip_ids()));
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[1].id()
    });
    // Foreground editing requires an explicit parameter selection; A assigns none.
    editor.update(&mut visual, |v, cx| {
        v.gain_lane.track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let plot = visual
        .debug_bounds(selector("arrangement-automation-plot", track.value()))
        .unwrap();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = point(
        plot.left() + plot.size.width * 0.25,
        plot.top() + plot.size.height * 0.4,
    );
    let target = point(down.x, plot.top() + plot.size.height * 0.7);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let authored = editor.read_with(&visual, |v, _| {
        assert!(
            v.active_clip_drag.is_none()
                && v.active_trim_drag.is_none()
                && v.time_selection_gesture.is_none()
        );
        assert_eq!(v.selected_track, background_selection.0);
        assert_eq!(
            v.selected_clip_ids(),
            background_selection.1,
            "foreground automation must retain background clip selection"
        );
        v.gain_lane.draft.unwrap().point
    });
    let marker =
        Box::leak(format!("gain-point-{}-{}", track.value(), authored.beat.raw()).into_boxed_str());
    let held = visual.debug_bounds(marker).unwrap();
    assert!((held.center().y - target.y).abs() < px(1.0));
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(marker), Some(held));
        editor.read_with(&visual, |v, _| {
            assert!(v.gain_lane.draft.is_none() && v.gain_lane.pending.is_some());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.gain_lane.pending.is_none());
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.track(track).unwrap().gain_envelope(), &[authored]);
        assert_eq!(p.snapshot().tracks[0].content, before.tracks[0].content);
    });
    assert_eq!(visual.debug_bounds(marker), Some(held));
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    toggle_toolbar(&mut visual);
    toggle_toolbar(&mut visual);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert_eq!(
            state.project.track(track).unwrap().gain_envelope(),
            &[authored]
        );
        assert_eq!(state.revision, revision);
        assert_eq!(v.selected_track, background_selection.0);
        assert_eq!(v.selected_clip_ids(), background_selection.1);
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot().tracks,
            before.tracks
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    let node = visual.debug_bounds(marker).unwrap().center();
    visual.simulate_mouse_down(node, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(node, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.worker_requests_pending, 0,
            "selecting an unchanged point creates no history edit"
        )
    });
    visual.simulate_keystrokes("backspace");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert!(p.track(track).unwrap().gain_envelope().is_empty());
        assert!(
            p.clip(audio).is_some(),
            "delete targets automation, not retained clips"
        );
    });
}

#[gpui::test]
fn automation_key_preserves_typing_midi_and_held_edits(cx: &mut TestAppContext) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    for key in ["alt-a", "cmd-a"] {
        visual.simulate_keystrokes(key);
    }
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: true,
    });
    editor.read_with(&visual, |v, _| assert!(!v.automation_view.enabled));
    visual
        .update(|window, cx| editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window)));
    visual.simulate_keystrokes("a shift-a");
    visual.simulate_input("a");
    visual.run_until_parked();
    editor.read_with(&visual, |v, cx| {
        assert!(!v.automation_view.enabled);
        assert!(v.browser.search.read(cx).text().contains('a'));
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    visual.simulate_keystrokes("a shift-a");
    editor.read_with(&visual, |v, _| assert!(!v.automation_view.enabled));
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    editor.update(&mut visual, |v, _| v.computer_keyboard.enabled = true);
    visual.simulate_keystrokes("a");
    editor.read_with(&visual, |v, _| assert!(!v.automation_view.enabled));
    visual.simulate_keystrokes("shift-a");
    editor.read_with(&visual, |v, _| assert!(v.automation_view.enabled));
    editor.update(&mut visual, |v, _| v.computer_keyboard.enabled = false);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for enabled in [false, true, false] {
        visual.simulate_keystrokes("a");
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.automation_view.enabled, enabled);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.track_header_focus(track, cx).focus(window);
        });
    });
    visual.simulate_keystrokes("a");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.automation_view.enabled));
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, cx| {
        v.gain_lane.track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let plot = visual
        .debug_bounds(selector("arrangement-automation-plot", track.value()))
        .unwrap();
    visual.simulate_mouse_down(plot.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("a");
    editor.read_with(&visual, |v, _| {
        assert!(
            v.automation_view.enabled && v.gain_lane.draft.is_some(),
            "held edit keeps its view"
        );
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(plot.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.gain_lane.draft.is_none() && v.gain_lane.pending.is_none());
        assert_eq!(v.worker_requests_pending, 0);
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain_envelope()
                .is_empty()
        );
    });
}

fn next_event(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> hex_session::service::Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Some(event) = editor.update(&mut *visual, |v, _| v.service.try_event().ok()) {
            return event;
        }
        assert!(std::time::Instant::now() < until, "worker terminal");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
#[gpui::test]
fn earlier_completion_cannot_retire_released_gain_preview_and_own_refusal_does(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    toggle_toolbar(&mut visual);
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, cx| {
        v.gain_lane.track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let plot = visual
        .debug_bounds(selector("arrangement-automation-plot", track.value()))
        .unwrap();
    let at = point(plot.center().x, plot.top() + plot.size.height * 0.25);
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    let draft = editor.read_with(&visual, |v, _| v.gain_lane.draft.unwrap().point);
    // Inject an independently admitted worker edit after the draft starts. The
    // UI rejects new edits here; this models a stale canonical snapshot so the
    // FIFO receipt and refusal paths remain covered despite that admission rule.
    editor.update(&mut visual, |v, _| {
        v.service
            .submit(hex_session::service::Request::Edit(
                hex_project::Command::RenameTrack {
                    track,
                    name: "renamed".into(),
                },
            ))
            .unwrap();
        v.worker_requests_pending += 1;
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    let marker =
        Box::leak(format!("gain-point-{}-{}", track.value(), draft.beat.raw()).into_boxed_str());
    visual.run_until_parked();
    let held = visual.debug_bounds(marker).unwrap();
    let earlier = next_event(&editor, &mut visual);
    assert!(earlier.error.is_none());
    editor.update(&mut visual, |v, cx| {
        v.receive(earlier);
        cx.notify();
    });
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(marker), Some(held));
        editor.read_with(&visual, |v, _| assert!(v.gain_lane.pending.is_some()));
    }
    let refusal = next_event(&editor, &mut visual);
    assert!(
        refusal
            .error
            .as_ref()
            .is_some_and(|e| e.contains("project changed"))
    );
    editor.update(&mut visual, |v, cx| {
        v.receive(refusal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(marker).is_none());
    editor.read_with(&visual, |v, _| {
        assert!(v.gain_lane.pending.is_none());
        let track = v.state.as_ref().unwrap().project.track(track).unwrap();
        assert_eq!(track.name(), "renamed");
        assert!(track.gain_envelope().is_empty());
    });
}

#[gpui::test]
fn a_and_toolbar_share_global_layers_without_assigning_parameters(cx: &mut TestAppContext) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // One selected clip, one unselected track: the view belongs to the arranger.
    click(&mut visual, audio, false);
    let before = editor.read_with(&visual, |view, _| {
        (
            view.state.as_ref().unwrap().project.snapshot(),
            view.selected_clip_ids(),
        )
    });
    let fills = [audio, midi].map(|clip| {
        visual
            .rendered_background(selector("clip-name", clip.value()))
            .unwrap()
    });
    let inactive = visual
        .rendered_background("automation-view-toggle")
        .unwrap();
    visual.simulate_keystrokes("a");
    visual.run_until_parked();
    let active = visual
        .rendered_background("automation-view-toggle")
        .unwrap();
    assert_ne!(
        active, inactive,
        "automation mode must have a latched surface indicator"
    );
    editor.read_with(&visual, |view, _| {
        assert!(view.automation_view.enabled);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before.0);
        assert_eq!(view.selected_clip_ids(), before.1);
        assert_eq!(view.worker_requests_pending, 0);
    });
    for (index, clip) in [audio, midi].into_iter().enumerate() {
        assert_eq!(
            visual.rendered_background(selector("clip-name", clip.value())),
            Some(fills[index].opacity(0.22))
        );
    }
    let tracks = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>()
    });
    for track in &tracks {
        assert!(
            visual
                .debug_bounds(selector("arrangement-automation-plot", track.value()))
                .is_none(),
            "entering automation view never assigns a parameter"
        );
    }
    // A toolbar change must immediately be the state A observes on the next key.
    toggle_toolbar(&mut visual);
    visual.simulate_event(gpui::MouseMoveEvent {
        position: point(px(0.0), px(0.0)),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("automation-view-toggle"),
        Some(inactive)
    );
    for track in &tracks {
        assert!(
            visual
                .debug_bounds(selector("arrangement-automation-plot", track.value()))
                .is_none()
        );
    }
    visual.simulate_keystrokes("a");
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("automation-view-toggle"),
        Some(active)
    );
    visual.simulate_keystrokes("a");
    visual.run_until_parked();
    for (index, clip) in [audio, midi].into_iter().enumerate() {
        assert_eq!(
            visual.rendered_background(selector("clip-name", clip.value())),
            Some(fills[index])
        );
    }
    editor.read_with(&visual, |view, _| {
        assert!(!view.automation_view.enabled);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before.0);
        assert_eq!(view.selected_clip_ids(), before.1);
    });
    visual.simulate_keystrokes("0");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(!view.automation_view.enabled);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.clip_active(audio), Some(false));
        assert_eq!(project.clip_active(midi), Some(true));
    });
}
