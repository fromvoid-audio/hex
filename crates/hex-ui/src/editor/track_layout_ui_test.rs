use super::{
    EditorView,
    clip_drag_ui_test::editor_window,
    tracks::{TRACK_HEADER_WIDTH, TRACK_ROW_HEIGHT},
};
use gpui::{
    Bounds, Modifiers, MouseButton, MouseMoveEvent, Pixels, TestAppContext, VisualTestContext,
    point, px, size,
};
use hex_project::{Command, Created, Editor, Project, TrackId};
use std::sync::Arc;

fn fixture(
    cx: &mut TestAppContext,
    count: usize,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
    cx.update(super::bind_editor_keys);
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let tracks = (0..count)
        .map(|index| {
            let Created::Track(id) = model
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .unwrap()
            else {
                panic!("track");
            };
            id
        })
        .collect();
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1_280.0), px(800.0)));
    visual.run_until_parked();
    (visual, editor, tracks)
}

fn bounds(visual: &mut VisualTestContext, prefix: &str, track: TrackId) -> Bounds<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", track.value()).into_boxed_str(),
        ))
        .unwrap_or_else(|| panic!("missing {prefix}"))
}

fn drag(visual: &mut VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>) {
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn header_width_edges_align_every_timeline_layer_and_preserve_frame_coordinates(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let track = tracks[0];
    let original = bounds(&mut visual, "arrangement-track-header", track);
    let original_lane = bounds(&mut visual, "arrangement-lane", track);
    let viewport = editor.read_with(&visual, |view, _| {
        view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
    });
    let handle = bounds(&mut visual, "track-width-resize", track).center();
    drag(&mut visual, handle, point(handle.x - px(64.0), handle.y));
    let header = bounds(&mut visual, "arrangement-track-header", track);
    let lane = bounds(&mut visual, "arrangement-lane", track);
    assert_eq!(header.size.width, original.size.width + px(64.0));
    assert_eq!(lane.size.width, original_lane.size.width - px(64.0));
    assert_eq!(lane.origin.x, original_lane.origin.x);
    assert_eq!(lane.origin.x + lane.size.width, header.origin.x);
    for selector in [
        "arrangement-grid",
        "arrangement-overview-map",
        "timeline-ruler",
        "main-track-lane",
    ] {
        let layer = visual.debug_bounds(selector).expect(selector);
        assert!(
            (layer.size.width - lane.size.width).abs() <= px(1.0),
            "{selector}: {layer:?} != {lane:?}"
        );
    }
    assert_eq!(
        visual.debug_bounds("main-track-header").unwrap().size.width,
        header.size.width
    );
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .size
            .width,
        header.size.width
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.visible_viewport(view.state.as_ref().unwrap().timeline_frames),
            viewport
        );
        assert!((view.drag_lanes[&track].width - f32::from(lane.size.width)).abs() <= 1.0);
        assert!(view.last_enqueued_edit.is_none());
        assert!(!view.state.as_ref().unwrap().unsaved);
        assert!(!view.track_resize_active());
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let painted = view.drag_lanes[&track];
            view.place_edit_cursor(track, painted.left + painted.width / 2.0, true, window, cx)
        })
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.edit_cursor.unwrap().frame,
            viewport.start + viewport.length / 2
        )
    });
    // Both extremes use the existing workspace budget and retain minimum controls.
    let handle = bounds(&mut visual, "track-width-resize", track).center();
    drag(&mut visual, handle, point(px(0.0), handle.y));
    let body = visual.debug_bounds("arrangement-content").unwrap();
    let maximum = (f32::from(body.size.width)
        - (super::browser::MIN_WORKSPACE_WIDTH - TRACK_HEADER_WIDTH))
        .clamp(TRACK_HEADER_WIDTH, super::browser::DEFAULT_WIDTH);
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .width,
        px(maximum)
    );
    let handle = bounds(&mut visual, "track-width-resize", track).center();
    drag(&mut visual, handle, point(px(2_000.0), handle.y));
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .width,
        px(TRACK_HEADER_WIDTH)
    );
}

#[gpui::test]
fn bottom_edge_resizes_only_its_row_with_controls_and_lane_aligned(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let track = tracks[0];
    let before = bounds(&mut visual, "arrangement-track-header", track);
    let main = visual.debug_bounds("pinned-main-track").unwrap();
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, handle.y + px(80.0)));
    let header = bounds(&mut visual, "arrangement-track-header", track);
    let lane = bounds(&mut visual, "arrangement-lane", track);
    let next = bounds(&mut visual, "arrangement-track-header", tracks[1]);
    assert_eq!(header.size.height, before.size.height + px(80.0));
    assert_eq!(lane.origin.y, header.origin.y);
    assert_eq!(lane.size.height, header.size.height);
    assert_eq!(next.origin.y, header.origin.y + header.size.height);
    assert_eq!(next.size.height, px(TRACK_ROW_HEIGHT));
    assert_eq!(visual.debug_bounds("pinned-main-track").unwrap(), main);
    let controls = bounds(&mut visual, "track-controls", track);
    let handle = bounds(&mut visual, "track-height-resize", track);
    assert!(
        controls.origin.y + controls.size.height <= handle.origin.y,
        "resize edge must not cover controls: {controls:?} {handle:?}"
    );
    // The original menu control stays clickable and tracks the taller row.
    let sends = bounds(&mut visual, "track-return-sends", track);
    visual.simulate_click(sends.center(), Modifiers::default());
    visual.run_until_parked();
    let menu = bounds(&mut visual, "return-send-menu", track);
    assert!(menu.size.height > px(0.0));
    editor.update(&mut visual, |view, cx| {
        view.send_menu_track = None;
        cx.notify();
    });
    visual.run_until_parked();
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, px(0.0)));
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .height,
        px(TRACK_ROW_HEIGHT)
    );
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, px(2_000.0)));
    let body = visual.debug_bounds("arrangement-content").unwrap();
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .height,
        body.size.height.max(px(TRACK_ROW_HEIGHT))
    );
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none())
    });
}

#[gpui::test]
fn resize_capture_cancels_on_escape_lost_button_window_change_and_stale_revision(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let track = tracks[0];
    for mode in 0..5 {
        let before = bounds(&mut visual, "arrangement-track-header", track);
        let handle = bounds(&mut visual, "track-height-resize", track).center();
        let to = point(handle.x - px(100.0), handle.y + px(60.0));
        visual.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            bounds(&mut visual, "arrangement-track-header", track)
                .size
                .height,
            before.size.height + px(60.0)
        );
        match mode {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.simulate_event(MouseMoveEvent {
                position: to,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            2 => visual.simulate_resize(size(px(1_200.0), px(800.0))),
            3 => visual
                .update(|window, cx| editor.update(cx, |view, cx| view.search_browser(window, cx))),
            _ => {
                editor.update(&mut visual, |view, _| {
                    let state = view.state.as_ref().unwrap();
                    view.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: state.canonical_audio_source.clone(),
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        midi_preview: Arc::clone(&state.midi_preview),
                        gain_preview: state.gain_preview.clone(),
                        timeline_frames: state.timeline_frames,
                        can_undo: state.can_undo,
                        can_redo: state.can_redo,
                        revision: state.revision + 1,
                        unsaved: state.unsaved,
                        document_path: state.document_path.clone(),
                    }));
                });
                visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            }
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            bounds(&mut visual, "arrangement-track-header", track)
                .size
                .height,
            before.size.height,
            "cancel mode {mode}"
        );
        editor.read_with(&visual, |view, _| {
            assert!(!view.track_resize_active());
            assert!(view.last_enqueued_edit.is_none());
        });
    }
    // A no-op round trip and Escape during width preview preserve the prior width.
    let before = bounds(&mut visual, "arrangement-track-header", track)
        .size
        .width;
    let handle = bounds(&mut visual, "track-width-resize", track).center();
    visual.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(handle.x - px(40.0), handle.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(handle, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", track)
            .size
            .width,
        before
    );
}

#[gpui::test]
fn variable_rows_keep_scroll_anchor_and_reset_measurements_for_reused_project_ids(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 40);
    let track = tracks[0];
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, handle.y + px(80.0)));
    editor.update(&mut visual, |view, cx| {
        view.track_scroll
            .scroll_to_item(12, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let anchor = bounds(&mut visual, "arrangement-track-header", tracks[12]);
    let handle = bounds(&mut visual, "track-height-resize", tracks[12]).center();
    drag(&mut visual, handle, point(handle.x, handle.y + px(60.0)));
    let resized = bounds(&mut visual, "arrangement-track-header", tracks[12]);
    assert_eq!(resized.origin.y, anchor.origin.y);
    assert_eq!(resized.size.height, anchor.size.height + px(60.0));
    assert_eq!(
        bounds(&mut visual, "arrangement-lane", tracks[12])
            .size
            .height,
        resized.size.height
    );
    let next = bounds(&mut visual, "arrangement-track-header", tracks[13]);
    assert_eq!(next.origin.y, resized.origin.y + resized.size.height);
    visual.simulate_resize(size(px(1_200.0), px(800.0)));
    visual.run_until_parked();
    let resized_window = bounds(&mut visual, "arrangement-track-header", tracks[12]);
    assert_eq!(resized_window.origin.y, anchor.origin.y);
    editor.read_with(&visual, |view, _| {
        let total = tracks.iter().map(|id| view.track_height(*id)).sum::<f32>();
        let height = view.arrangement_bounds.unwrap().height;
        assert!(
            (f32::from(view.track_scroll.max_offset().height) - (total - height).max(0.0)).abs()
                <= 1.0,
            "viewport width changes must retain measured nonuniform scroll extent"
        );
    });
    editor.update(&mut visual, |view, cx| {
        view.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    let reset = bounds(&mut visual, "arrangement-track-header", track);
    assert_eq!(reset.size.height, px(TRACK_ROW_HEIGHT));
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .origin
            .y,
        reset.origin.y + reset.size.height
    );
}

#[gpui::test]
fn empty_project_main_header_width_handle_uses_current_window_limit(cx: &mut TestAppContext) {
    let (mut visual, editor, _) = fixture(cx, 0);
    let from = visual
        .debug_bounds("main-track-width-resize")
        .unwrap()
        .center();
    drag(&mut visual, from, point(from.x - px(64.0), from.y));
    assert_eq!(
        visual.debug_bounds("main-track-header").unwrap().size.width,
        px(TRACK_HEADER_WIDTH + 64.0)
    );
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let main = visual.debug_bounds("main-track-header").unwrap();
    let ruler = visual.debug_bounds("timeline-ruler").unwrap();
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    assert_eq!(ruler.size.width, lane.size.width);
    assert!(lane.size.width >= px(super::browser::MIN_WORKSPACE_WIDTH - TRACK_HEADER_WIDTH));
    assert!(main.size.width >= px(TRACK_HEADER_WIDTH));
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none())
    });
}
