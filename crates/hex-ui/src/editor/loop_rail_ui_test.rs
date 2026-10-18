use super::{EditorView, Viewport, clip_drag_ui_test};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{Command, Editor, LoopRegion};
use std::{sync::Arc, thread, time::Duration};

const TIMELINE_FRAMES: u64 = 384_000;

fn loop_editor(
    cx: &mut TestAppContext,
    initial: Option<LoopRegion>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    cx.update(super::bind_editor_keys);
    let (mut project, _, _, _) = clip_drag_ui_test::audio_project();
    if let Some(region) = initial {
        let mut history = Editor::new(project, 8).expect("project history");
        history
            .execute(Command::SetLoopRegion {
                start: region.start(),
                end: region.end(),
                enabled: region.enabled(),
            })
            .expect("valid initial loop");
        project = history.project().clone();
    }
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, window_cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES,
        });
        window_cx.notify();
    });
    (handle, editor)
}

fn rail_point(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail bounds");
    point(
        rail.origin.x + rail.size.width * fraction,
        rail.origin.y + rail.size.height / 2.0,
    )
}

fn wait_for_loop(
    cx: &mut TestAppContext,
    handle: gpui::AnyWindowHandle,
    editor: &gpui::Entity<EditorView>,
    expected: Option<(u64, u64, bool)>,
) {
    for _ in 0..500 {
        cx.update_window(handle, |_, _, cx| {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
        })
        .expect("loop editor window");
        if editor.read_with(cx, |view, _| {
            let Some(state) = view.state.as_ref() else {
                return false;
            };
            state
                .project
                .loop_region()
                .map(|region| (region.start(), region.end(), region.enabled()))
                == expected
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    let observed = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref();
        (
            state.and_then(|state| state.project.loop_region()),
            state.map(|state| state.revision),
            view.error.clone(),
            view.status.clone(),
            view.loop_rail_gesture.is_some(),
        )
    });
    panic!("loop command did not publish expected project revision: {observed:?}");
}

#[gpui::test]
fn loop_rail_paint_previews_then_commits_adaptive_grid_range_disabled(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let overview = visual
        .debug_bounds("arrangement-overview-map")
        .expect("overview timeline");
    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("ruler timeline");
    assert_eq!(rail.origin.x, overview.origin.x);
    assert_eq!(rail.size.width, overview.size.width);
    assert_eq!(rail.origin.x, ruler.origin.x);

    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
        assert!(view.loop_rail_gesture.is_some(), "drag owns the rail");
    });
    assert!(visual.debug_bounds("loop-rail-range").is_some());

    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 192_000, false)));
    let viewport = editor.read_with(cx, |view, _| view.viewport);
    assert_eq!(
        viewport,
        Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES
        })
    );
}

#[gpui::test]
fn command_drag_uses_free_frames_and_zoomed_panned_coordinates(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let viewport = Viewport {
        start: 48_000,
        length: 192_000,
    };
    editor.update(cx, |view, window_cx| {
        view.viewport = Some(viewport);
        window_cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let start = point(
        rail.origin.x + rail.size.width * 0.123,
        rail.origin.y + rail.size.height / 2.0,
    );
    let end = point(
        rail.origin.x + rail.size.width * 0.377,
        rail.origin.y + rail.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let expected = editor.read_with(cx, |view, _| {
        let lane = view.arrangement_bounds.expect("arrangement bounds");
        let left = lane.left;
        let width = lane.width - super::tracks::TRACK_HEADER_WIDTH;
        let state = view.state.as_ref().unwrap();
        let start_frame =
            super::seek::pointer_to_frame(f32::from(start.x), left, width, viewport).unwrap();
        let end_frame =
            super::seek::pointer_to_frame(f32::from(end.x), left, width, viewport).unwrap();
        assert_eq!(state.timeline_frames, TIMELINE_FRAMES);
        (start_frame, end_frame)
    });
    visual.simulate_mouse_down(start, MouseButton::Left, command);
    visual.simulate_mouse_move(end, MouseButton::Left, command);
    visual.simulate_mouse_up(end, MouseButton::Left, command);
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((expected.0, expected.1, false)));
    assert_ne!(
        expected.0 % 6_000,
        0,
        "fixture starts off the adaptive grid"
    );
    assert_ne!(expected.1 % 6_000, 0, "fixture ends off the adaptive grid");
}

#[gpui::test]
fn loop_rail_drag_continues_outside_rail_without_starting_lane_selection(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let start = rail_point(&mut visual, 0.25);
    let outside = point(rail.origin.x + rail.size.width + px(24.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.loop_rail_gesture.is_some(),
            "captured loop gesture continues"
        );
        assert!(
            view.time_selection_gesture.is_none(),
            "lane selection stays idle"
        );
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 384_000, false)));
}

#[gpui::test]
fn loop_rail_click_activates_resizable_ends_preserve_enable_and_toolbar_disables(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = loop_editor(cx, Some(LoopRegion::new(48_000, 192_000, false).unwrap()));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let midpoint = rail_point(&mut visual, 0.25);
    visual.simulate_mouse_down(midpoint, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(midpoint, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((48_000, 192_000, true)));

    let start = rail_point(&mut visual, 0.125);
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeLeftRight),
        "loop endpoints expose a resize cursor"
    );
    let new_start = rail_point(&mut visual, 0.25);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(new_start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(new_start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 192_000, true)));

    let end = rail_point(&mut visual, 0.5);
    let new_end = rail_point(&mut visual, 0.75);
    visual.simulate_mouse_move(end, None, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(new_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(new_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 288_000, true)));

    let toggle = visual
        .debug_bounds("toggle loop on/off · l sets selection · cmd+l sets audio clip")
        .expect("rendered loop toolbar control");
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 288_000, false)));
    assert!(visual.debug_bounds("loop-rail-range").is_some());
}

#[gpui::test]
fn stale_loop_state_cancels_preview_without_overwriting_the_new_range(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, window_cx| {
        let old = view.state.as_ref().unwrap();
        let mut history = Editor::new((*old.project).clone(), 8).expect("project history");
        history
            .execute(Command::SetLoopRegion {
                start: 144_000,
                end: 240_000,
                enabled: true,
            })
            .expect("external loop edit");
        view.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            project: Arc::new(history.project().clone()),
            waveforms: old.waveforms.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        window_cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.loop_rail_gesture.is_none(), "stale preview is retired");
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(LoopRegion::new(144_000, 240_000, true).unwrap()),
            "releasing the stale gesture must preserve the newer loop range"
        );
    });

    let start = rail_point(&mut visual, 0.7);
    let end = rail_point(&mut visual, 0.9);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, window_cx| {
        let old = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        window_cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.loop_rail_gesture.is_none(),
            "revision change retires preview"
        );
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(LoopRegion::new(144_000, 240_000, true).unwrap())
        );
    });
}
