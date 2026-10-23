use super::{
    EditorView,
    clip_drag_ui_test::editor_window,
    tracks::{TRACK_HEADER_WIDTH, TRACK_ROW_HEIGHT},
};
use gpui::{
    Bounds, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseMoveEvent, Pixels,
    TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{Command, Created, Editor, Project, TrackId};
use std::sync::Arc;

fn fixture(
    cx: &mut TestAppContext,
    count: usize,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
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
    let (visual, editor) = project_window(cx, model.project().clone());
    (visual, editor, tracks)
}

fn project_window(
    cx: &mut TestAppContext,
    project: Project,
) -> (VisualTestContext, gpui::Entity<EditorView>) {
    cx.update(super::bind_editor_keys);
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1_280.0), px(800.0)));
    visual.run_until_parked();
    (visual, editor)
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
        px(super::track_layout::HEADER_ONLY_HEIGHT)
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
                        comparison_audio: None,
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
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

#[gpui::test]
fn selected_lane_paint_tracks_virtualized_scrolling_and_compact_window(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 40);
    let first = bounds(&mut visual, "track-name", tracks[0]).center();
    visual.simulate_click(first, Modifiers::default());
    visual.run_until_parked();
    let tint = |track: TrackId| {
        Box::leak(format!("selected-track-lane-{}", track.value()).into_boxed_str()) as &'static str
    };
    assert!(!visual.rendered_filled_quads(tint(tracks[0])).is_empty());
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.scroll_to_item(20, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.rendered_filled_quads(tint(tracks[0])).is_empty());
    let next = bounds(&mut visual, "track-name", tracks[20]).center();
    visual.simulate_click(next, Modifiers::default());
    visual.run_until_parked();
    assert!(!visual.rendered_filled_quads(tint(tracks[20])).is_empty());
    visual.simulate_resize(size(px(880.0), px(620.0)));
    visual.run_until_parked();
    assert!(!visual.rendered_filled_quads(tint(tracks[20])).is_empty());
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.rendered_filled_quads(tint(tracks[0])).is_empty());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [tracks[20]])
    });
}

#[gpui::test]
fn option_resize_equalizes_all_ordinary_rows_and_escape_restores_each_height(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 3);
    for (track, delta) in [(tracks[0], 40.0), (tracks[1], 80.0)] {
        let handle = bounds(&mut visual, "track-height-resize", track).center();
        drag(&mut visual, handle, point(handle.x, handle.y + px(delta)));
    }
    let original = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.clone()
    });
    let old: Vec<_> = tracks
        .iter()
        .map(|track| {
            bounds(&mut visual, "arrangement-track-header", *track)
                .size
                .height
        })
        .collect();
    let main = visual.debug_bounds("pinned-main-track").unwrap();
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    for cancel in [true, false] {
        let handle = bounds(&mut visual, "track-height-resize", tracks[0]).center();
        let to = point(handle.x, handle.y + px(20.0));
        visual.simulate_mouse_down(handle, MouseButton::Left, alt);
        visual.simulate_mouse_move(to, MouseButton::Left, alt);
        visual.run_until_parked();
        let expected = old[0] + px(20.0);
        let mut previous_bottom = None;
        for track in &tracks {
            let header = bounds(&mut visual, "arrangement-track-header", *track);
            let lane = bounds(&mut visual, "arrangement-lane", *track);
            assert_eq!(header.size.height, expected);
            assert_eq!(lane.size.height, expected);
            assert_eq!(lane.top(), header.top());
            if let Some(bottom) = previous_bottom {
                assert_eq!(header.top(), bottom);
            }
            previous_bottom = Some(header.bottom());
        }
        if cancel {
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            for (track, height) in tracks.iter().zip(old.iter().copied()) {
                assert_eq!(
                    bounds(&mut visual, "arrangement-track-header", *track)
                        .size
                        .height,
                    height
                );
            }
        } else {
            visual.simulate_mouse_up(to, MouseButton::Left, alt);
            visual.run_until_parked();
            for track in &tracks {
                assert_eq!(
                    bounds(&mut visual, "arrangement-track-header", *track)
                        .size
                        .height,
                    expected
                );
            }
        }
        assert_eq!(visual.debug_bounds("pinned-main-track").unwrap(), main);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert_eq!(view.worker_requests_pending, 0);
        });
    }
}

#[gpui::test]
fn minimum_track_height_keeps_only_readable_title_and_resize_hit(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 1);
    let track = tracks[0];
    let handle = bounds(&mut visual, "track-height-resize", track).center();
    drag(&mut visual, handle, point(handle.x, px(0.0)));
    let header = bounds(&mut visual, "arrangement-track-header", track);
    let title = bounds(&mut visual, "track-name", track);
    let resize = bounds(&mut visual, "track-height-resize", track);
    assert_eq!(header.size.height, px(23.0));
    assert_eq!(title.size.height, px(18.0));
    assert!(title.top() >= header.top() && title.bottom() <= resize.top());
    for prefix in [
        "track-controls",
        "track-mute",
        "track-volume",
        "track-pan",
        "track-return-sends",
    ] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("{prefix}-{}", track.value()).into_boxed_str()
                ))
                .is_none(),
            "compact row must not retain hidden {prefix} hits"
        );
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.worker_requests_pending, 0)
    });
}

fn focus_workspace(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) {
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| view.focus.focus(window));
    });
}

fn assert_heights(visual: &mut VisualTestContext, tracks: &[TrackId], heights: &[f32]) {
    visual.run_until_parked();
    assert_eq!(tracks.len(), heights.len());
    for (&track, &height) in tracks.iter().zip(heights) {
        let header = bounds(visual, "arrangement-track-header", track);
        let lane = bounds(visual, "arrangement-lane", track);
        assert_eq!(
            header.size.height,
            px(height),
            "track {track:?}: {header:?}"
        );
        assert_eq!(lane.size.height, header.size.height);
        assert_eq!(lane.top(), header.top());
    }
}

fn install_layout_project(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    project: &Project,
) {
    editor.update(&mut *visual, |view, cx| {
        let old = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(project.clone()),
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: false,
            can_redo: false,
            revision: old.revision + 1,
            unsaved: false,
            document_path: None,
        }));
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn h_overview_roundtrip_preserves_distinct_heights_and_fixed_return_main_rows(
    cx: &mut TestAppContext,
) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for name in ["first", "second"] {
        model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap();
    }
    model
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap();
    let tracks: Vec<_> = model.project().tracks().iter().map(|t| t.id()).collect();
    let return_track = model.project().return_tracks()[0].id();
    let original = model.project().snapshot();
    let (mut visual, editor) = project_window(cx, model.project().clone());
    for (track, delta) in [(tracks[0], 40.0), (tracks[1], 80.0)] {
        let edge = bounds(&mut visual, "track-height-resize", track).center();
        drag(&mut visual, edge, point(edge.x, edge.y + px(delta)));
    }
    assert_heights(&mut visual, &tracks, &[84.0, 124.0]);
    focus_workspace(&mut visual, &editor);
    for key in ["cmd-h", "ctrl-h", "alt-h", "shift-h"] {
        visual.simulate_keystrokes(key);
        assert_heights(&mut visual, &tracks, &[84.0, 124.0]);
    }
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("h").unwrap(),
        is_held: true,
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("h").unwrap(),
    });
    assert_heights(&mut visual, &tracks, &[84.0, 124.0]);
    // The header is a real nested focus owner, not a global shortcut context.
    let name = bounds(&mut visual, "track-name", tracks[0]).center();
    visual.simulate_click(name, Modifiers::default());
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| assert!(view.track_header_focused(window)));
    });
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.0, 23.0]);
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("h").unwrap(),
        is_held: true,
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("h").unwrap(),
    });
    assert_heights(&mut visual, &tracks, &[23.0, 23.0]);
    assert_eq!(
        visual
            .debug_bounds(Box::leak(
                format!("return-track-header-{}", return_track.value()).into_boxed_str(),
            ))
            .expect("return header")
            .size
            .height,
        px(44.0)
    );
    assert_eq!(
        visual
            .debug_bounds("main-track-header")
            .unwrap()
            .size
            .height,
        px(44.0)
    );
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[84.0, 124.0]);
    visual.simulate_keystrokes("tab h");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.mode, super::EditorMode::Session)
    });
    visual.simulate_keystrokes("tab");
    assert_heights(&mut visual, &tracks, &[84.0, 124.0]);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn h_overview_single_and_option_resizes_are_temporary_and_reentry_starts_compact(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 3);
    for (track, delta) in [(tracks[0], 40.0), (tracks[1], 80.0)] {
        let edge = bounds(&mut visual, "track-height-resize", track).center();
        drag(&mut visual, edge, point(edge.x, edge.y + px(delta)));
    }
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.0; 3]);
    let edge = bounds(&mut visual, "track-height-resize", tracks[0]).center();
    drag(&mut visual, edge, point(edge.x, edge.y + px(30.0)));
    assert_heights(&mut visual, &tracks, &[53.0, 23.0, 23.0]);
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    let edge = bounds(&mut visual, "track-height-resize", tracks[1]).center();
    let to = point(edge.x, edge.y + px(40.0));
    visual.simulate_mouse_down(edge, MouseButton::Left, alt);
    visual.simulate_mouse_move(to, MouseButton::Left, alt);
    assert_heights(&mut visual, &tracks, &[63.0; 3]);
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, alt);
    assert_heights(&mut visual, &tracks, &[53.0, 23.0, 23.0]);
    let edge = bounds(&mut visual, "track-height-resize", tracks[1]).center();
    let to = point(edge.x, edge.y + px(40.0));
    visual.simulate_mouse_down(edge, MouseButton::Left, alt);
    visual.simulate_mouse_move(to, MouseButton::Left, alt);
    visual.simulate_mouse_up(to, MouseButton::Left, alt);
    assert_heights(&mut visual, &tracks, &[63.0; 3]);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[84.0, 124.0, 44.0]);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.0; 3]);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[84.0, 124.0, 44.0]);
    editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none())
    });
}

#[gpui::test]
fn h_overview_defers_to_text_numeric_modal_nested_focus_and_resize_capture(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (mut visual, editor, tracks) = fixture(cx, 1);
    let track = tracks[0];
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("cmd-f h");
    assert_heights(&mut visual, &tracks, &[44.0]);
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        });
    });
    let name = bounds(&mut visual, "track-name", track).center();
    visual.simulate_click(name, Modifiers::default());
    visual.simulate_keystrokes("cmd-r h");
    editor.read_with(&visual, |view, _| assert!(view.track_name.is_editing()));
    assert_heights(&mut visual, &tracks, &[44.0]);
    visual.simulate_keystrokes("escape");
    super::tempo_input_ui_test::begin(&mut visual);
    visual.simulate_keystrokes("h");
    editor.update(&mut visual, |view, cx| {
        assert!(view.project_tempo_editing(cx))
    });
    assert_heights(&mut visual, &tracks, &[44.0]);
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| view.midi_focus.focus(window));
    });
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[44.0]);
    focus_workspace(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        view.close_prompt_open = true;
        cx.notify();
    });
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[44.0]);
    editor.update(&mut visual, |view, cx| {
        view.close_prompt_open = false;
        cx.notify();
    });
    focus_workspace(&mut visual, &editor);
    let edge = bounds(&mut visual, "track-height-resize", track).center();
    let to = point(edge.x, edge.y + px(30.0));
    visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[74.0]);
    editor.read_with(&visual, |view, _| assert!(view.track_resize_active()));
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.0]);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[74.0]);
}

#[gpui::test]
fn h_enabled_keyboard_owns_note_69_and_release_while_pending_edits_block_overview(
    cx: &mut TestAppContext,
) {
    use super::midi_canvas::creation_tests::{fixture as midi_fixture, pump};
    use hex_session::service::Request;
    let (mut visual, editor, _environment, _) = midi_fixture(cx);
    let track = editor.read_with(&visual, |view, _| view.selected_track.unwrap());
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("m");
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("h").unwrap(),
        is_held: false,
    });
    let input = editor.read_with(&visual, |view, _| {
        assert!(view.computer_keyboard.enabled);
        let notes: Vec<_> = view.computer_keyboard.physical_notes().collect();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].pitch, notes[0].velocity), (69, 100));
        let input = view.computer_keyboard.input.as_ref().unwrap();
        assert_eq!((input.key_word(9).unwrap() & 255) - 1, 69);
        Arc::clone(input)
    });
    assert_heights(&mut visual, &[track], &[44.0]);
    let original_word = input.key_word(9);
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("h").unwrap(),
        is_held: true,
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.computer_keyboard.physical_notes().count(), 1)
    });
    assert_eq!(
        input.key_word(9),
        original_word,
        "held H must not retrigger"
    );
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-h").unwrap(),
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.computer_keyboard.physical_notes().count(), 0)
    });
    assert_eq!(input.key_word(9), Some(0));
    visual.simulate_keystrokes("m");
    editor.update(&mut visual, |view, _| {
        assert!(!view.computer_keyboard.enabled);
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "pending".into()
        })));
        assert!(view.edit_pending());
    });
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &[track], &[44.0]);
    pump(&editor, &mut visual);
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &[track], &[23.0]);
}

#[gpui::test]
fn h_overview_topology_prunes_both_height_maps_and_generation_restarts_normal_layout(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    for (track, delta) in [(tracks[0], 40.0), (tracks[1], 80.0)] {
        let edge = bounds(&mut visual, "track-height-resize", track).center();
        drag(&mut visual, edge, point(edge.x, edge.y + px(delta)));
    }
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    let edge = bounds(&mut visual, "track-height-resize", tracks[0]).center();
    drag(&mut visual, edge, point(edge.x, edge.y + px(17.0)));
    assert_heights(&mut visual, &tracks, &[40.0, 23.0]);
    let project = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.as_ref().clone()
    });
    let mut model = Editor::new(project, 8).unwrap();
    model.execute(Command::RemoveTrack(tracks[0])).unwrap();
    install_layout_project(&mut visual, &editor, model.project());
    assert_heights(&mut visual, &[tracks[1]], &[23.0]);
    assert!(model.undo());
    install_layout_project(&mut visual, &editor, model.project());
    // Reappearing stable IDs must not recover either removed height entry.
    assert_heights(&mut visual, &tracks, &[23.0, 23.0]);
    let Created::Track(new) = model
        .execute(Command::AddTrack { name: "new".into() })
        .unwrap()
    else {
        panic!("track")
    };
    install_layout_project(&mut visual, &editor, model.project());
    let tracks = [tracks[0], tracks[1], new];
    assert_heights(&mut visual, &tracks, &[23.0; 3]);
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[44.0, 124.0, 44.0]);
    visual.simulate_keystrokes("h");
    let edge = bounds(&mut visual, "track-height-resize", tracks[1]).center();
    drag(&mut visual, edge, point(edge.x, edge.y + px(37.0)));
    assert_heights(&mut visual, &tracks, &[23.0, 60.0, 23.0]);
    editor.update(&mut visual, |view, cx| {
        view.project_generation += 1;
        cx.notify();
    });
    assert_heights(&mut visual, &tracks, &[44.0; 3]);
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.0; 3]);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[44.0; 3]);
}
