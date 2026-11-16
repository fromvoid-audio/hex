//! Actual pointer routing and released-preview custody at the clip paint owner.
use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point};
use hex_project::{BeatTime, Command};

fn next_paint_event(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(std::time::Instant::now() < deadline, "paint terminal");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn paint_fixture(
    cx: &mut TestAppContext,
    stale: bool,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<super::super::EditorView>,
    hex_project::TrackId,
) {
    use gpui::AppContext;
    use hex_session::service::{Request, Service};
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48_000, 32, 0).unwrap();
    next_paint_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "paint".into(),
        }))
        .unwrap();
    let added = next_paint_event(&service);
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let displayed = next_paint_event(&service).state;
    if stale {
        service
            .submit(Request::Edit(Command::RenameTrack {
                track,
                name: "changed source".into(),
            }))
            .unwrap();
        assert!(next_paint_event(&service).error.is_none());
    }
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                super::super::EditorView::new(window, cx, service, Err("offline paint test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(displayed);
                view.viewport = Some(super::super::viewport::Viewport::fit(384_000));
                view.focus.focus(window);
                view.snap_grid = super::super::musical_time::SnapGrid::Quarter;
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, track)
}

#[gpui::test]
fn double_click_creates_a_clip_in_the_clicked_grid_cell(cx: &mut TestAppContext) {
    use super::super::musical_time::{SnapGrid, grid_lines};

    for (grid, fraction) in [
        (SnapGrid::Quarter, 0.0),
        (SnapGrid::Quarter, 0.25),
        (SnapGrid::Quarter, 0.75),
        (SnapGrid::Adaptive, 0.75),
    ] {
        let (handle, editor, track) = paint_fixture(cx, false);
        editor.update(cx, |view, cx| {
            view.snap_grid = grid;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let (left, right, viewport) = editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            let viewport = view.visible_viewport(state.timeline_frames);
            let lines = grid_lines(&state.project, viewport, f32::from(lane.size.width), grid);
            let cell = lines
                .windows(2)
                .find(|cell| cell[0].frame >= 144_000)
                .unwrap();
            (cell[0].frame, cell[1].frame, viewport)
        });
        let x = |frame| {
            lane.left()
                + lane.size.width * ((frame - viewport.start) as f32 / viewport.length as f32)
        };
        let at = point(x(left) + (x(right) - x(left)) * fraction, lane.center().y);
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.run_until_parked();
        let preview = visual.debug_bounds("clip-paint-preview").unwrap();
        assert!(
            f32::from(preview.left() - x(left)).abs() < 1.0,
            "clip preview must start at the clicked cell's left edge: {grid:?}, {fraction}"
        );
        assert!(f32::from(preview.right() - x(right)).abs() < 1.0);

        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            let event = next_paint_event(&view.service);
            assert!(event.error.is_none(), "{:?}", event.error);
            let Some(hex_project::Created::Clip(id)) = event.created else {
                panic!("clip creation terminal");
            };
            let clip = event.state.project.midi_clip(id).unwrap();
            let tempo = event.state.project.tempo_map();
            assert_eq!(tempo.beat_to_frame(clip.start()).unwrap(), left);
            let end = BeatTime::from_raw(clip.start().raw() + clip.length().raw());
            assert_eq!(tempo.beat_to_frame(end).unwrap(), right);
        });
    }
}

#[gpui::test]
fn double_press_selects_existing_clip_header_without_moving_it(cx: &mut TestAppContext) {
    for reverse in [false, true] {
        let (handle, editor, track) = paint_fixture(cx, false);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let at = point(
            lane.left() + lane.size.width * 0.1875,
            lane.top() + lane.size.height * 0.15,
        );
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(
            point(
                at.x + lane.size.width * if reverse { -0.0625 } else { 0.0625 },
                at.y,
            ),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.read_with(cx, |view, _| {
            let clip = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id();
            assert_eq!(view.selected_clip(), Some(clip));
            assert!(view.clip_paint_gesture.is_none());
            assert!(view.pointer_down.is_none());
            assert!(view.active_clip_drag.is_none());
            assert!(view.time_selection_gesture.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
    }
}

#[gpui::test]
fn released_paint_survives_repaints_and_escape_until_success_or_refusal(cx: &mut TestAppContext) {
    for stale in [false, true] {
        let (handle, editor, track) = paint_fixture(cx, stale);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let at = point(
            lane.left() + lane.size.width * 0.5,
            lane.top() + lane.size.height * if stale { 0.85 } else { 0.5 },
        );
        let end = point(at.x + lane.size.width * 0.0625, at.y);
        visual.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = visual.debug_bounds("clip-paint-preview").unwrap();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |view, _| next_paint_event(&view.service));
        assert_eq!(terminal.error.is_some(), stale);
        visual.simulate_keystrokes("escape");
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(
                visual.debug_bounds("clip-paint-preview"),
                Some(preview),
                "released display waits for its own terminal"
            );
        }
        editor.update(cx, |view, cx| {
            view.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-paint-preview").is_none());
        editor.update(cx, |view, _| {
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.clip_paint_gesture.is_none());
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .midi_clips()
                    .len(),
                if stale { 1 } else { 2 }
            );
            if !stale {
                let painted = view.state.as_ref().unwrap().project.clone();
                view.enqueue(hex_session::service::Request::Undo);
                let terminal = next_paint_event(&view.service);
                assert!(terminal.error.is_none());
                view.receive(terminal);
                assert_eq!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .midi_clips()
                        .len(),
                    1
                );
                view.enqueue(hex_session::service::Request::Redo);
                let terminal = next_paint_event(&view.service);
                assert!(terminal.error.is_none());
                view.receive(terminal);
                assert_eq!(view.state.as_ref().unwrap().project, painted);
            }
        });
    }
}

#[gpui::test]
fn audio_rectangle_double_press_selects_at_every_height(cx: &mut TestAppContext) {
    use super::super::clip_drag_ui_test::{audio_project, editor_window};
    for (fraction, reverse) in [(0.15, false), (0.5, true), (0.85, false)] {
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, std::sync::Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        let at = point(
            lane.left() + lane.size.width * 0.125,
            lane.top() + lane.size.height * fraction,
        );
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(
            point(
                at.x + lane.size.width * if reverse { -0.0625 } else { 0.0625 },
                at.y,
            ),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("clip-paint-preview").is_none(),
            "height {fraction}"
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(view.selected_clip(), Some(clip));
            assert!(!view.clip_paint_held());
            assert!(view.pointer_down.is_none());
            assert!(view.active_clip_drag.is_none());
            assert!(view.time_selection_gesture.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
        editor.update(cx, |view, cx| {
            view.cancel_clip_paint();
            cx.notify();
        });
    }
}

#[gpui::test]
fn closed_service_refusal_retires_unsubmitted_paint_without_editing(cx: &mut TestAppContext) {
    let (handle, editor, track) = paint_fixture(cx, false);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let at = point(
        lane.left() + lane.size.width * 0.5,
        lane.top() + lane.size.height * 0.5,
    );
    visual.simulate_event(MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        click_count: 2,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    editor.update(cx, |view, _| {
        assert!(view.clip_paint_held());
        view.service
            .shutdown(std::time::Duration::from_millis(250))
            .unwrap();
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.clip_paint_gesture.is_none());
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.error.as_ref().unwrap().contains("worker stopped"));
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .len(),
            1
        );
    });
    assert!(visual.debug_bounds("clip-paint-preview").is_none());
}
