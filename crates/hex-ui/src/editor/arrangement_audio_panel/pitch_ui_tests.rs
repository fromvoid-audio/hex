//! Pitch uses the shared dial/input and ordinary clip-edit receipt.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
#[gpui::test]
fn pitch_is_a_dial_and_cancel_keeps_canonical_audio_unchanged(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (handle, editor, _, _) = super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let knob = visual
        .debug_bounds("audio-clip-pitch-knob")
        .expect("pitch must use a shared knob");
    let sidebar = visual.debug_bounds("arrangement-audio-controls").unwrap();
    assert!(
        knob.top() >= sidebar.top() && knob.bottom() <= sidebar.bottom(),
        "pitch must be initially reachable: {knob:?} within {sidebar:?}"
    );
    let gain = visual.debug_bounds("audio-clip-gain").unwrap();
    assert!(
        gain.left() >= knob.right()
            && gain.right() <= sidebar.right()
            && gain.bottom() <= sidebar.bottom(),
        "pitch and gain must both remain reachable: {gain:?}, {knob:?}, {sidebar:?}"
    );
    let value = visual
        .debug_bounds("audio-clip-pitch-value")
        .expect("pitch keeps editable cents readout");
    assert!(knob.size.width >= px(20.) && knob.size.height >= px(20.));
    assert!(value.size.width > px(0.));
    let down = knob.center();
    let up = down + point(px(0.), px(-16.));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, cx| {
        assert_ne!(
            v.audio_clip_controls.numbers[&Field::Pitch]
                .entity
                .read(cx)
                .value(),
            0.
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
        assert!(!v.audio_clip_controls.has_pending());
    });
}

#[gpui::test]
fn pitch_release_keeps_its_draft_until_accepted_or_refused_ack(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    for accepted in [true, false] {
        let (handle, editor, _, clip) =
            super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().clone());
        let down = visual
            .debug_bounds("audio-clip-pitch-knob")
            .unwrap()
            .center();
        let up = down + point(px(0.), px(-16.));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let (command, final_value) = editor.read_with(cx, |v,cx| {
            assert_eq!(v.worker_requests_pending,1);
            assert!(v.audio_clip_controls.has_pending());
            let value=v.audio_clip_controls.numbers[&Field::Pitch].entity.read(cx).value();
            assert_ne!(value,0.);
            assert!(matches!(v.last_enqueued_edit,Some(Command::SetAudioClipPitch {clip:id,cents}) if id==clip && f64::from(cents)==value));
            (v.last_enqueued_edit.clone().unwrap(),value)
        });
        for _ in 0..2 {
            let mut event =
                super::super::super::arrangement_main_track_ui_test::completed_event(&before);
            event.request_complete = false;
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
            editor.read_with(cx, |v, cx| {
                assert!(v.audio_clip_controls.has_pending());
                assert_eq!(
                    v.audio_clip_controls.numbers[&Field::Pitch]
                        .entity
                        .read(cx)
                        .value(),
                    final_value
                );
                assert_eq!(v.state.as_ref().unwrap().project, before.project);
            });
        }
        let mut terminal =
            super::super::super::arrangement_main_track_ui_test::completed_event(&before);
        if accepted {
            let mut model = hex_project::Editor::new((*before.project).clone(), 8).unwrap();
            model.execute(command).unwrap();
            let state = std::sync::Arc::get_mut(&mut terminal.state).unwrap();
            state.project = std::sync::Arc::new(model.project().clone());
            state.can_undo = true;
        } else {
            terminal.error = Some("fixture pitch refusal".into());
        }
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(cx, |v, cx| {
            assert!(!v.audio_clip_controls.has_pending());
            let control = v.audio_clip_controls.numbers[&Field::Pitch].entity.read(cx);
            assert_eq!(control.value(), if accepted { final_value } else { 0. });
            assert_eq!(
                control.display_text(),
                format!("{:+.0} ct", control.value())
            );
        });
    }
}
fn pitch(project: &hex_project::Project, clip: ClipId) -> i16 {
    project
        .clip(clip)
        .unwrap()
        .audio_edit()
        .map_or(0, |e| e.domain.pitch_cents)
}
fn enter(visual: &mut VisualTestContext, value: &str) {
    let at = visual
        .debug_bounds("audio-clip-pitch-value")
        .unwrap()
        .center();
    visual.simulate_click(at, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input(value);
    visual.simulate_keystrokes("enter");
}
#[gpui::test]
fn pitch_knob_real_worker_history_entry_reset_group_scope_and_save_reopen(cx: &mut TestAppContext) {
    use super::super::super::clip_drag_ui_test::{pump_move as pump, upper_rail_owned_fixture};
    use hex_session::service::Request;
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (environment, handle, editor) = upper_rail_owned_fixture(cx);
    let clip = editor.read_with(cx, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find_map(|t| t.clips().first().map(|c| c.id()))
            .unwrap()
    });
    editor.update(cx, |v, cx| {
        v.clip_view_open = true;
        v.selected_clip = Some(clip);
        v.selected_track = v
            .state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.clips().iter().any(|c| c.id() == clip))
            .map(|t| t.id());
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds("audio-clip-pitch-knob")
        .unwrap()
        .center();
    let up = down + point(px(0.), px(-12.));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let dragged = editor.read_with(cx, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        pitch(&v.state.as_ref().unwrap().project, clip)
    });
    assert_ne!(dragged, 0);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), dragged)
    });
    enter(&mut visual, "137 ct");
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 137)
    });
    visual.simulate_keystrokes("up");
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 237)
    });
    visual.simulate_keystrokes("shift-up");
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 238)
    });
    let at = visual
        .debug_bounds("audio-clip-pitch-knob")
        .unwrap()
        .center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        click_count: 2,
        ..Default::default()
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 0)
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 238)
    });
    enter(&mut visual, "-123 ct");
    pump(&editor, &mut visual);
    let save = environment.root().join("pitch-knob.hex");
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::SaveNew(save.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetAudioClipPitch { clip, cents: 0 })))
    });
    pump(&editor, &mut visual);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Open(save))));
    pump(&editor, &mut visual);
    let (track, asset, sample_rate) = editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(pitch(p, clip), -123);
        (
            p.tracks()
                .iter()
                .find(|t| t.clips().iter().any(|c| c.id() == clip))
                .unwrap()
                .id(),
            p.clip(clip).unwrap().asset(),
            u64::from(p.sample_rate()),
        )
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceClip {
            track,
            asset,
            start: sample_rate * 4,
            duration: sample_rate,
            source_offset: 0,
            source_length: sample_rate,
            looping: false
        })))
    });
    pump(&editor, &mut visual);
    let second = editor.read_with(cx, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.id() == track)
            .unwrap()
            .clips()
            .iter()
            .find(|c| c.id() != clip)
            .unwrap()
            .id()
    });
    editor.update(cx, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetAudioClipPitch {
            clip: second,
            cents: 700
        })))
    });
    pump(&editor, &mut visual);
    editor.update(cx, |v, cx| {
        v.clip_view_open = true;
        v.viewport = Some(super::super::super::viewport::Viewport::fit(
            v.state.as_ref().unwrap().timeline_frames,
        ));
        cx.notify();
    });
    visual.run_until_parked();
    for (id, modifiers) in [
        (clip, Modifiers::default()),
        (
            second,
            Modifiers {
                shift: true,
                ..Default::default()
            },
        ),
    ] {
        let selector: &'static str =
            Box::leak(format!("clip-name-{}", id.value()).into_boxed_str());
        let at = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(at, modifiers);
        visual.run_until_parked();
    }
    let before_group = editor.read_with(cx, |v, _| {
        assert_eq!(v.selected_clip_ids(), vec![clip, second]);
        v.state.as_ref().unwrap().project.snapshot()
    });
    enter(&mut visual, "800 ct");
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(pitch(p, clip), -23);
        assert_eq!(pitch(p, second), 800);
    });
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_group)
    });
}

#[gpui::test]
fn pitch_source_replacement_cancels_held_draft_and_repitch_keeps_transpose_unavailable(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (handle, editor, _, clip) =
        super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let down = visual
        .debug_bounds("audio-clip-pitch-knob")
        .unwrap()
        .center();
    let up = down + point(px(0.), px(-10.));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, cx| {
        assert!(v.last_enqueued_edit.is_none() && !v.audio_clip_controls.has_pending());
        assert_eq!(
            v.audio_clip_controls.numbers[&Field::Pitch]
                .entity
                .read(cx)
                .value(),
            0.
        );
    });
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().clone());
    let mut model = hex_project::Editor::new((*before.project).clone(), 8).unwrap();
    let settings = hex_project::default_audio_warp(
        model.project().clip(clip).unwrap(),
        model.project().tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    model
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let mut event = super::super::super::arrangement_main_track_ui_test::completed_event(&before);
    let state = std::sync::Arc::get_mut(&mut event.state).unwrap();
    state.project = std::sync::Arc::new(model.project().clone());
    state.revision += 1;
    editor.update(cx, |v, cx| {
        v.state = Some(event.state);
        cx.notify();
    });
    visual.run_until_parked();
    let at = visual
        .debug_bounds("audio-clip-pitch-knob")
        .unwrap()
        .center();
    visual.simulate_mouse_move(at, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow)
    );
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        at + point(px(0.), px(-10.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, cx| {
        assert!(v.last_enqueued_edit.is_none() && !v.audio_clip_controls.has_pending());
        assert_eq!(pitch(&v.state.as_ref().unwrap().project, clip), 0);
        assert_eq!(
            v.audio_clip_controls.numbers[&Field::Pitch]
                .entity
                .read(cx)
                .value(),
            0.
        );
    });
}
