use super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, Created, Editor};
use hex_session::service::Event;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[gpui::test]
fn double_stop_starts_at_zero_and_preserves_each_arrangement_selection(cx: &mut TestAppContext) {
    use gpui::{MouseDownEvent, MouseUpEvent};
    use hex_engine::PlaybackCommand::PlayFrom;

    cx.update(super::bind_editor_keys);
    let (project, source, target, clip) = audio_project();
    let mut project = Editor::new(project, 8).unwrap();
    project
        .execute(Command::MoveClip {
            clip,
            track: source,
            start: 60_000,
        })
        .unwrap();
    project
        .execute(Command::SetLoopRegion {
            start: 96_000,
            end: 144_000,
            enabled: true,
        })
        .unwrap();

    for start_source in ["cursor", "range", "clip"] {
        let (handle, editor) = editor_window(cx, project.project().clone(), Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&target]);
        let cursor_at = point(px(lane.left + lane.width * 0.4), px(lane.top + 20.0));
        visual.simulate_click(cursor_at, Modifiers::default());
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                if start_source == "range" {
                    view.time_selection = Some(super::time_selection::TimeSelection {
                        anchor_track: Some(target),
                        end_track: Some(target),
                        anchor_frame: 80_000,
                        end_frame: 65_000,
                    });
                } else if start_source == "clip" {
                    view.select_clip_header(source, clip, Modifiers::default(), window, cx);
                }
                window.focus(&view.focus);
                cx.notify();
            });
        });
        let (placed, cursor, range, clips) = editor.read_with(&visual, |view, _| {
            let frame = view.playback_start_frame();
            match start_source {
                "range" => assert_eq!(frame, 65_000),
                "clip" => assert_eq!(frame, 60_000),
                _ => assert_eq!(frame, view.edit_cursor.unwrap().frame),
            }
            assert_ne!(frame, 0);
            (
                frame,
                view.edit_cursor.map(|cursor| (cursor.track, cursor.frame)),
                view.time_selection,
                view.selected_clip_ids(),
            )
        });

        let stop = visual.debug_bounds("transport-stop-icon").unwrap().center();
        let modifiers = Modifiers::default();
        visual.simulate_click(stop, modifiers);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.status, "stopped");
            assert!(!view.play_intent.desired_playing(true));
        });
        visual.simulate_keystrokes("space");
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.last_transport_command, Some(PlayFrom(placed)));
        });

        visual.simulate_click(stop, modifiers);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                placed,
                "the first click is a single stop"
            );
        });
        visual.simulate_event(MouseDownEvent {
            position: stop,
            button: MouseButton::Left,
            modifiers,
            click_count: 2,
            first_mouse: false,
        });
        visual.simulate_event(MouseUpEvent {
            position: stop,
            button: MouseButton::Left,
            modifiers,
            click_count: 2,
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.status, "stopped · play start at beginning");
        });
        visual.simulate_keystrokes("space");
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.last_transport_command,
                Some(PlayFrom(0)),
                "{start_source}"
            );
            assert_eq!(
                view.edit_cursor.map(|cursor| (cursor.track, cursor.frame)),
                cursor
            );
            assert_eq!(view.time_selection, range);
            assert_eq!(view.selected_clip_ids(), clips);
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .loop_region()
                    .unwrap()
                    .enabled()
            );
        });
        let zero = visual
            .debug_bounds("transport-start-line")
            .expect("reset play start");
        assert!((f32::from(zero.left()) - lane.left).abs() <= 1.0);
        assert!(visual.debug_bounds("transport-start-top").is_some());
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("edit-cursor-line-{}", target.value()).into_boxed_str()
                ))
                .is_none()
        );
    }
}

#[gpui::test]
fn two_effective_escape_stops_reset_the_start_until_the_next_placement(cx: &mut TestAppContext) {
    use hex_engine::PlaybackCommand::PlayFrom;

    cx.update(super::bind_editor_keys);
    let (project, _, track, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&track]);
    let at = |fraction| point(px(lane.left + lane.width * fraction), px(lane.top + 20.0));
    visual.simulate_click(at(0.4), Modifiers::default());
    visual.run_until_parked();
    let placed = editor.read_with(&visual, |view, _| view.edit_cursor.unwrap().frame);
    let marker = visual.debug_bounds("time-marker-top-start").unwrap();
    visual.simulate_click(marker.center(), Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.valid_locked_play_start().unwrap().frame, placed);
    });

    visual.simulate_keystrokes("escape space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.last_transport_command, Some(PlayFrom(placed)));
    });
    visual.simulate_keystrokes("escape");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.playback_start_frame(),
            placed,
            "Play disarms the previous Escape"
        );
    });
    visual.simulate_keystrokes("escape space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.last_transport_command, Some(PlayFrom(0)));
        assert_eq!(view.valid_locked_play_start().unwrap().frame, placed);
    });
    assert!(visual.debug_bounds("transport-start-top").is_some());
    assert!(visual.debug_bounds("locked-play-start-top").is_none());

    visual.simulate_click(at(0.7), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let new_start = view.edit_cursor.unwrap().frame;
        assert!(new_start > placed);
        assert_eq!(view.playback_start_frame(), new_start);
    });
    assert!(visual.debug_bounds("transport-start-top").is_none());
}

#[gpui::test]
fn empty_arrangement_space_matches_a_scrolled_zoomed_lane_and_keeps_selection(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, source, last, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let viewport = super::viewport::Viewport {
        start: 48_000,
        length: 128_000,
    };
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(viewport);
        cx.notify();
    });
    visual.run_until_parked();

    let (lane, tail, main) = editor.read_with(&visual, |view, _| {
        (
            view.drag_lanes[&source],
            view.painted_track_drop_lane(last).unwrap(),
            view.main_track_lane_bounds.unwrap(),
        )
    });
    let body = visual.debug_bounds("arrangement-content").unwrap();
    let last_bottom = px(tail.top + tail.height);
    assert!(body.bottom() > last_bottom);
    assert!(body.bottom() <= px(main.top));
    let tail_y = (last_bottom + body.bottom()) / 2.0;
    let x = px(lane.left + lane.width * 0.677);
    let lane_click = point(x, px(lane.top + lane.height * 0.5));
    let empty_click = point(x, tail_y);

    let mut frames = Vec::new();
    for free in [false, true] {
        let modifiers = Modifiers {
            platform: free,
            ..Modifiers::default()
        };
        visual.simulate_click(lane_click, modifiers);
        visual.run_until_parked();
        let lane_frame = editor.read_with(&visual, |view, _| {
            let cursor = view.edit_cursor.unwrap();
            assert_eq!(cursor.track, Some(source));
            cursor.frame
        });
        editor.update(&mut visual, |view, cx| {
            // A pre-existing clip/range would ordinarily supply Play's start.
            view.selected_clip = Some(clip);
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(source),
                end_track: Some(last),
                anchor_frame: 32_000,
                end_frame: 42_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let selected_tracks = editor.read_with(&visual, |view, _| view.selected_track_ids());
        visual.simulate_click(empty_click, modifiers);
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let cursor = view.project_point_cursor().expect("empty timeline click");
            assert_eq!(cursor.frame, lane_frame);
            assert_eq!(view.selected_track, Some(source));
            assert_eq!(view.selected_track_ids(), selected_tracks);
            assert_eq!(view.selected_clip, Some(clip));
            assert!(view.time_selection.is_none());
            assert_eq!(view.time_selection_paste_target(), None);
            assert_eq!(view.playback_start_frame(), lane_frame);
        });
        let line = visual.debug_bounds("project-edit-cursor-line").unwrap();
        let expected_x = lane.left + lane.width * viewport.fraction(lane_frame).unwrap();
        assert!((f32::from(line.left()) - expected_x).abs() <= 1.0);
        frames.push(lane_frame);
    }
    assert_ne!(
        frames[0], frames[1],
        "Command placement keeps the off-grid frame"
    );
    let placed = frames[1];
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.last_transport_command, None)
    });
    visual.simulate_keystrokes("space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(placed))
        );
    });

    let other = point(px(lane.left + lane.width * 0.31), tail_y);
    let assert_still_placed = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, _| {
            assert_eq!(
                view.project_point_cursor().map(|cursor| cursor.frame),
                Some(placed)
            );
        });
    };
    visual.simulate_mouse_down(empty_click, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(empty_click, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
    visual.simulate_click(other, Modifiers::default());
    visual.run_until_parked();
    assert_still_placed(&visual);
    visual.simulate_keystrokes("escape");

    for opening in [true, false] {
        editor.update(&mut visual, |view, _| {
            view.opening_project = opening;
            view.close_prompt_open = !opening;
        });
        visual.simulate_click(other, Modifiers::default());
        assert_still_placed(&visual);
    }
    editor.update(&mut visual, |view, _| {
        view.close_prompt_open = false;
        view.mode = super::EditorMode::Session;
    });
    visual.simulate_click(other, Modifiers::default());
    assert_still_placed(&visual);
    editor.update(&mut visual, |view, cx| {
        view.mode = super::EditorMode::Arrangement;
        view.suppress_cursor_click = true;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(other, Modifiers::default());
    assert_still_placed(&visual);
    editor.read_with(&visual, |view, _| assert!(!view.suppress_cursor_click));

    visual.simulate_click(other, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_ne!(view.project_point_cursor().unwrap().frame, placed);
    });
}

#[gpui::test]
fn empty_project_click_places_a_visible_play_start_without_tracks(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let project = hex_project::Project::new(48_000).unwrap();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("arrangement-content").unwrap();
    let frame = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let bounds = view.arrangement_bounds.unwrap();
        let width = bounds.width - view.track_header_width();
        let x = bounds.left + width * 0.357;
        let viewport = view.visible_viewport(state.timeline_frames);
        let frame = super::seek::pointer_to_frame(x, bounds.left, width, viewport).unwrap();
        (px(x), frame)
    });
    visual.simulate_click(
        point(frame.0, body.center().y),
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let cursor = view.project_point_cursor().expect("point on empty project");
        assert_eq!(cursor.frame, frame.1);
        assert!(cursor.frame > 0);
        assert_eq!(view.selected_track, None);
        assert!(view.time_selection.is_none());
        assert_eq!(view.playback_start_frame(), frame.1);
    });
    assert!(visual.debug_bounds("project-edit-cursor-line").is_some());
    visual.simulate_keystrokes("space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(frame.1))
        );
    });
}

#[gpui::test]
fn clicked_audio_cursor_is_visible_and_e_uses_it_offline(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, target, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("lane");
    let click = point(
        lane.origin.x + lane.size.width * 0.125,
        lane.origin.y + px(22.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.time_selection_gesture.is_some(),
            "the clip body behaves as an empty lane for point selection"
        );
    });
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let frame = editor.read_with(cx, |view, _| {
        let cursor = view.edit_cursor.expect("click places cursor");
        assert_eq!(cursor.track, Some(source));
        assert!(cursor.frame > 0 && cursor.frame < 96_000);
        assert!(view.normalized_time_selection().is_none());
        assert_eq!(
            view.cursor_position(),
            0,
            "edit click does not seek the audio playhead"
        );
        cursor.frame
    });
    let line = visual
        .debug_bounds(Box::leak(
            format!("edit-cursor-line-{}", source.value()).into_boxed_str(),
        ))
        .expect("rendered line");
    assert_eq!(line.origin.y, lane.origin.y);
    assert_eq!(line.size.height, lane.size.height);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("edit-cursor-line-{}", target.value()).into_boxed_str(),
            ))
            .is_none()
    );
    assert!(f32::from(line.size.width) <= 1.0);
    assert!(
        (f32::from(line.origin.x - lane.origin.x) / f32::from(lane.size.width)
            - frame as f32 / 384_000.0)
            .abs()
            < 0.002
    );
    editor.update(cx, |view, cx| {
        view.viewport = Some(super::viewport::Viewport {
            start: 10_000,
            length: 128_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let moved = visual
        .debug_bounds(Box::leak(
            format!("edit-cursor-line-{}", source.value()).into_boxed_str(),
        ))
        .expect("line follows viewport");
    let extent = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("track lane");
    let expected = (frame - 10_000) as f32 / 128_000.0;
    assert!(
        (f32::from(moved.origin.x - extent.origin.x) / f32::from(extent.size.width) - expected)
            .abs()
            < 0.002
    );
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitClip { clip: id, frame: position }) if id == clip && position == frame));
    });
}

#[gpui::test]
fn command_click_is_free_and_e_routes_midi_split(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("lane");
    let click = point(
        lane.origin.x + lane.size.width * 0.17,
        lane.origin.y + px(22.0),
    );
    let modifiers = Modifiers {
        platform: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(click, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(click, MouseButton::Left, modifiers);
    visual.run_until_parked();
    let beat = editor.read_with(cx, |view, _| {
        let frame = view.edit_cursor.expect("free click cursor").frame;
        let geometry = view
            .drag_lanes
            .get(&source)
            .expect("actual timeline geometry");
        let expected = ((f32::from(click.x) - geometry.left) as f64 / f64::from(geometry.width)
            * 384_000.0)
            .round() as u64;
        assert_eq!(frame, expected, "free placement keeps pointer precision");
        assert_ne!(
            frame % 24_000,
            0,
            "command click must remain off the quarter grid"
        );
        view.state
            .as_ref()
            .expect("state")
            .project
            .tempo_map()
            .frame_to_beat(frame)
            .expect("beat")
    });
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitMidiClip { clip: id, beat: position }) if id == clip && position == beat));
    });
}

#[gpui::test]
fn main_cursor_keeps_track_overlays_but_section_lane_gap_refuses_e(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (base, audio_track, midi_track, _) = audio_project();
    let mut project_editor = Editor::new(base, 8).expect("project editor");
    let Created::Clip(midi_clip) = project_editor
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).expect("midi length"),
        })
        .expect("midi clip")
    else {
        panic!("midi clip result")
    };
    project_editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");
    let (handle, editor) = editor_window(cx, project_editor.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let main_lane = visual.debug_bounds("main-track-lane").expect("main lane");
    // Check Main's lower body first, then the section title rail separately.
    let click = point(
        main_lane.origin.x + main_lane.size.width * 0.1,
        main_lane.origin.y + main_lane.size.height * 0.7,
    );
    let unsnapped = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(click, MouseButton::Left, unsnapped);
    visual.simulate_mouse_up(click, MouseButton::Left, unsnapped);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let cursor = view.edit_cursor.expect("main cursor");
        assert!(view.time_selection_project_scope);
        assert!(view.normalized_time_selection().is_none());
        assert!(cursor.frame > 0 && cursor.frame < 96_000);
        assert!(view.selected_track.is_none());
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("edit-cursor-line-{}", audio_track.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("edit-cursor-line-{}", midi_track.value()).into_boxed_str()
            ))
            .is_some()
    );

    let before = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    assert!(visual.debug_bounds("section-edit-cursor").is_none());
    // The lower body owns Main's all-track cursor/range. Section cursor and
    // gap-split refusal belong to the header after the shared range split.
    let header = point(
        click.x,
        main_lane.top()
            + px(super::time_selection::header_hit_height(f32::from(main_lane.size.height)) * 0.5),
    );
    visual.simulate_mouse_down(header, MouseButton::Left, unsnapped);
    visual.simulate_mouse_up(header, MouseButton::Left, unsnapped);
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-edit-cursor").is_some());
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "a gap must not fall through to song-wide clip splitting"
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(
            view.error
                .as_ref()
                .is_some_and(|e| e.contains("strictly inside a section"))
        );
    });
}

#[gpui::test]
fn empty_position_and_missing_cursor_do_not_dispatch_a_split(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty target lane");
    let click = point(
        lane.origin.x + lane.size.width / 2.0,
        lane.origin.y + px(22.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.edit_cursor.expect("cursor on empty track").track,
            Some(target)
        );
        assert!(view.last_enqueued_edit.is_none());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("inside a clip"))
        );
    });
}

#[gpui::test]
fn empty_lane_drag_selects_time_and_contiguous_tracks_in_either_direction(cx: &mut TestAppContext) {
    let mut model =
        hex_project::Editor::new(hex_project::Project::new(48_000).expect("project"), 8)
            .expect("editor");
    let mut ids = Vec::new();
    for index in 0..4 {
        let hex_project::Created::Track(id) = model
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track")
        else {
            panic!("track result");
        };
        ids.push(id);
    }
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = |visual: &mut VisualTestContext, id: hex_project::TrackId| {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", id.value()).into_boxed_str(),
            ))
            .expect("visible track lane")
    };
    let first_lane = lane(&mut visual, ids[0]);
    let last_lane = lane(&mut visual, ids[3]);
    let start = point(
        first_lane.origin.x + first_lane.size.width * 0.2,
        first_lane.origin.y + first_lane.size.height / 2.0,
    );
    let end = point(
        start.x + first_lane.size.width * 0.35,
        last_lane.origin.y + last_lane.size.height / 2.0,
    );
    cx.update_window(handle, |_, window, window_cx| {
        editor.update(window_cx, |view, _| view.midi_focus.focus(window));
    })
    .expect("focus prior editor");
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.update_window(handle, |_, window, window_cx| {
        editor.read_with(window_cx, |view, _| assert!(view.focus.is_focused(window)));
    })
    .expect("range drag restores arrangement focus");
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert_eq!(view.selected_track_ids(), ids));
    for id in &ids {
        let selector = Box::leak(format!("selected-track-lane-{}", id.value()).into_boxed_str());
        assert!(
            visual.rendered_background(selector).is_some(),
            "crossed lane highlights before release"
        );
    }
    let second = lane(&mut visual, ids[1]);
    let shrink = point(end.x, second.center().y);
    visual.simulate_mouse_move(shrink, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track_ids(), ids[..2])
    });
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let selection = view.normalized_time_selection().expect("time selection");
        assert_eq!(selection.tracks(), ids);
        assert!(selection.end() > selection.start());
        assert_eq!(view.selected_track, Some(ids[3]));
        assert_eq!(
            view.selected_track_ids(),
            ids,
            "painted time span selects every crossed track"
        );
        assert!(view.selected_clip.is_none());
        assert_eq!(
            view.time_selection_paste_target(),
            Some((ids[0], selection.start())),
            "paste origin is independent of endpoint focus"
        );
    });
    let shading = visual
        .debug_bounds(Box::leak(
            format!("edit-time-selection-{}", ids[2].value()).into_boxed_str(),
        ))
        .expect("selected track receives steady range fill");
    let selected_lane = lane(&mut visual, ids[2]);
    let handlebar = visual
        .debug_bounds(Box::leak(
            format!("time-selection-handle-{}", ids[2].value()).into_boxed_str(),
        ))
        .expect("selection handle exists for hover");
    assert_eq!(handlebar.origin.y, selected_lane.origin.y);
    assert!(
        (f32::from(handlebar.size.height) - f32::from(selected_lane.size.height) / 3.0).abs() < 1.0,
        "handle {:?}, lane {:?}",
        handlebar.size.height,
        selected_lane.size.height
    );
    assert_eq!(shading.origin.y, selected_lane.origin.y);
    assert_eq!(shading.size.height, selected_lane.size.height);
    assert!(shading.size.width > px(1.0));

    let reselect_start = point(
        selected_lane.origin.x + selected_lane.size.width * 0.35,
        selected_lane.origin.y + selected_lane.size.height * 0.75,
    );
    let reselect_end = point(
        last_lane.origin.x + last_lane.size.width * 0.45,
        last_lane.origin.y + last_lane.size.height * 0.75,
    );
    visual.simulate_mouse_down(reselect_start, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.time_range_move_gesture.is_none(),
            "below the handle must not move the old area"
        );
        assert!(
            view.time_selection_gesture.is_some(),
            "selection can restart inside itself"
        );
    });
    visual.simulate_mouse_move(reselect_end, MouseButton::Left, Modifiers::default());
    let across_anchor = point(reselect_end.x, first_lane.center().y);
    visual.simulate_mouse_move(across_anchor, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_track_ids(), ids[..3])
    });
    visual.simulate_mouse_move(reselect_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(reselect_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let range = view.normalized_time_selection().expect("replacement range");
        assert_eq!(range.tracks(), &ids[2..]);
        assert_eq!(view.selected_track, Some(ids[3]));
        assert_eq!(
            view.selected_track_ids(),
            ids[2..],
            "repainting shrinks explicit track membership"
        );
        assert!(
            view.last_enqueued_edit.is_none(),
            "reselection does not edit project content"
        );
    });

    let reverse_start = point(
        last_lane.origin.x + last_lane.size.width * 0.8,
        last_lane.origin.y + last_lane.size.height / 2.0,
    );
    let reverse_end = point(
        first_lane.origin.x + first_lane.size.width * 0.4,
        first_lane.origin.y + first_lane.size.height / 2.0,
    );
    visual.simulate_mouse_down(reverse_start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(reverse_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(reverse_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let selection = view.normalized_time_selection().expect("reverse selection");
        assert_eq!(selection.tracks(), ids);
        assert!(selection.start() < selection.end());
        assert_eq!(view.selected_track, Some(ids[0]));
        assert_eq!(view.selected_track_ids(), ids);
        assert_eq!(
            view.time_selection_paste_target(),
            Some((ids[0], selection.start()))
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.cancel_time_selection_gesture());
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.normalized_time_selection().is_none());
        assert_eq!(
            view.selected_track_ids(),
            ids,
            "clearing time does not discard explicit track selection"
        );
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            model.project()
        );
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn selected_time_range_drag_shows_and_dispatches_a_contiguous_move(cx: &mut TestAppContext) {
    let mut model =
        hex_project::Editor::new(hex_project::Project::new(48_000).expect("project"), 8)
            .expect("editor");
    let mut ids = Vec::new();
    for index in 0..3 {
        let hex_project::Created::Track(id) = model
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track")
        else {
            panic!("track result");
        };
        ids.push(id);
    }
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = |visual: &mut VisualTestContext, id: hex_project::TrackId| {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", id.value()).into_boxed_str(),
            ))
            .expect("visible track lane")
    };
    let source = lane(&mut visual, ids[0]);
    let target = lane(&mut visual, ids[1]);
    let source_start = point(
        source.origin.x + source.size.width * 0.2,
        source.origin.y + source.size.height / 2.0,
    );
    let source_end = point(
        source.origin.x + source.size.width * 0.4,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(source_start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(source_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(source_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.normalized_time_selection()
                .expect("selected range")
                .tracks(),
            &ids[0..1]
        );
    });

    let grab = point(
        source.origin.x + source.size.width * 0.3,
        source.origin.y + source.size.height * 0.25,
    );
    let destination = point(
        target.origin.x + target.size.width * 0.5,
        target.origin.y + target.size.height / 2.0,
    );
    editor.update(cx, |view, _| {
        assert!(view.begin_time_range_move_gesture(f32::from(grab.x), f32::from(grab.y)));
        assert!(view.update_time_range_move_gesture(
            f32::from(destination.x),
            f32::from(destination.y),
            false
        ));
        let generation = view.project_generation;
        view.project_generation = generation.wrapping_add(1);
        assert!(view.finish_time_range_move_gesture());
        assert!(
            view.last_enqueued_edit.is_none(),
            "stale generation must not submit a move"
        );
        assert!(view.pending_time_range_move.is_none());
        view.project_generation = generation;
    });
    visual.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = visual
        .debug_bounds(Box::leak(
            format!("time-selection-move-preview-{}", ids[1].value()).into_boxed_str(),
        ))
        .expect("move preview on destination row");
    assert_eq!(preview.origin.y, target.origin.y);
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::MoveTimeRange { track, .. }) if track == ids[1]
        ));
        assert!(
            view.normalized_time_selection().is_some(),
            "release must not also collapse the area to a click"
        );
    });
}

#[gpui::test]
fn option_drag_copies_time_range_and_keeps_source_preview_visible(cx: &mut TestAppContext) {
    let (project, source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: Some(source),
            end_track: Some(source),
            anchor_frame: 0,
            end_frame: 48_000,
        });
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("destination lane");
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    let down = point(
        source_lane.origin.x + source_lane.size.width * 0.04,
        source_lane.origin.y + source_lane.size.height * 0.2,
    );
    let destination = point(
        source_lane.origin.x + source_lane.size.width * 0.55,
        target_lane.origin.y + target_lane.size.height * 0.2,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, option);
    visual.simulate_mouse_move(destination, MouseButton::Left, option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let display = view.time_range_move_display().expect("copy preview");
        assert!(display.copy_modifier);
        assert!(view.time_range_move_gesture.is_some());
    });
    visual
        .debug_bounds(Box::leak(
            format!(
                "time-move-audio-fragment-{}-{}",
                target_track.value(),
                clip.value()
            )
            .into_boxed_str(),
        ))
        .expect("destination copy preview");
    visual
        .debug_bounds(Box::leak(
            format!("clip-visual-{}", clip.value()).into_boxed_str(),
        ))
        .expect("source remains visible while copying");

    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .time_range_move_display()
                .expect("still active preview")
                .copy_modifier
        );
    });
    visual.simulate_modifiers_change(option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let display = view.time_range_move_display().expect("copy preview");
        assert!(display.copy_modifier);
        assert_eq!(display.source.tracks(), &[source]);
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::CopyTimeRangeTo { range, track, .. })
                if range.tracks() == [source] && *track == target_track
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn audio_time_move_preview_masks_overlaps_and_restores_when_cancelled(cx: &mut TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    let selection = super::time_selection::TimeSelection {
        anchor_track: Some(source),
        end_track: Some(source),
        anchor_frame: 0,
        end_frame: 48_000,
    };
    editor.update(cx, |view, cx| {
        view.time_selection = Some(selection);
        view.selected_track = Some(source);
        cx.notify();
    });
    visual.run_until_parked();
    let down = point(
        lane.origin.x + lane.size.width * 0.025,
        lane.origin.y + lane.size.height * 0.2,
    );
    let (target_x, destination_end) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project state");
        let geometry = view.drag_lanes.get(&source).expect("measured lane");
        let viewport = view.visible_viewport(state.timeline_frames);
        let pointer_frame = super::seek::pointer_to_frame(
            f32::from(down.x),
            geometry.left,
            geometry.width,
            viewport,
        )
        .expect("pointer frame");
        let grab_frame = view
            .snap_grid
            .with_platform_modifier(false)
            .snap_frame(&state.project, pointer_frame, viewport, geometry.width)
            .expect("snapped grab frame");
        let offset = grab_frame;
        let destination_start = 24_000;
        let pointer_target = destination_start + offset;
        let x = geometry.left
            + (pointer_target - viewport.start) as f32 / viewport.length as f32 * geometry.width;
        (x, destination_start + 48_000)
    });
    editor.update(cx, |view, cx| {
        assert!(view.begin_time_range_move_gesture(f32::from(down.x), f32::from(down.y)));
        assert!(view.update_time_range_move_gesture(target_x, f32::from(down.y), false));
        let display = view.time_range_move_display().expect("live move display");
        assert_eq!(display.source.tracks(), &[source]);
        assert_eq!(display.source.start(), 0);
        assert_eq!(display.source.end(), 48_000);
        assert_eq!(display.destination_start, 24_000);
        assert_eq!(display.destination_end, destination_end);
        cx.notify();
    });
    visual.run_until_parked();
    let source_tail = visual
        .debug_bounds(Box::leak(
            format!(
                "time-move-audio-residual-{}-{destination_end}",
                source.value()
            )
            .into_boxed_str(),
        ))
        .expect("the clip tail after the union of source and destination remains visible");
    let destination = visual
        .debug_bounds(Box::leak(
            format!(
                "time-move-audio-fragment-{}-{}",
                source.value(),
                clip.value()
            )
            .into_boxed_str(),
        ))
        .expect("the selected waveform fragment appears at its destination");
    assert!(destination.origin.x > lane.origin.x);
    assert!(source_tail.origin.x > destination.origin.x);

    editor.update(cx, |view, cx| {
        assert!(view.update_time_range_move_gesture(target_x, -100.0, true));
        assert!(
            view.time_range_move_display().is_some(),
            "moving outside the lane retains the last valid preview"
        );
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        assert!(view.cancel_time_range_move_gesture());
        assert!(view.time_range_move_display().is_none());
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn captured_time_selection_projects_outside_pointer_and_release_position(cx: &mut TestAppContext) {
    let (project, _source, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty target lane");
    let start = point(
        lane.origin.x + lane.size.width * 0.25,
        lane.origin.y + lane.size.height * 0.75,
    );
    let outside_left_above = point(px(-80.0), px(-300.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside_left_above, None, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.time_selection_gesture.is_some(),
            "capture persists without button state"
        );
        let selection = view.time_selection.expect("live selection");
        assert_eq!(
            selection.end_track,
            view.visible_lane_near_y(-300.0).map(|(track, _)| track),
            "vertical target clamps to nearest fresh lane"
        );
        assert_eq!(
            selection.end_frame, 0,
            "left x clamps to the visible frame start"
        );
    });
    let release = point(lane.origin.x + lane.size.width + px(250.0), px(1_000.0));
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.time_selection_gesture.is_none());
        let selection = view.time_selection.expect("completed selection");
        assert_eq!(
            selection.end_track,
            Some(target),
            "release below tracks clamps to last lane"
        );
        let viewport = view.visible_viewport(384_000);
        assert_eq!(selection.end_frame, viewport.end().expect("viewport end"));
    });
}

#[gpui::test]
fn command_modifier_reprojects_captured_selection_without_pointer_motion(cx: &mut TestAppContext) {
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty lane");
    let down = point(
        lane.origin.x + lane.size.width * 0.1,
        lane.origin.y + lane.size.height * 0.75,
    );
    let moved = point(lane.origin.x + lane.size.width * 0.12345, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let (raw_frame, snapped_frame) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project");
        let geometry = view.drag_lanes.get(&target).expect("fresh lane");
        let viewport = view.visible_viewport(state.timeline_frames);
        let raw = super::seek::pointer_to_frame(
            f32::from(moved.x),
            geometry.left,
            geometry.width,
            viewport,
        )
        .expect("raw frame");
        let snapped = view
            .snap_grid
            .with_platform_modifier(false)
            .snap_frame(&state.project, raw, viewport, geometry.width)
            .expect("snapped frame");
        assert_ne!(raw, snapped, "fixture exercises the snapping difference");
        assert_eq!(view.time_selection.expect("selection").end_frame, snapped);
        (raw, snapped)
    });
    assert_ne!(raw_frame, snapped_frame);
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.time_selection_gesture
                .as_ref()
                .expect("gesture")
                .free_modifier
        );
        assert_eq!(
            view.time_selection.expect("selection").end_frame,
            raw_frame,
            "Command updates snapping at the stored pointer"
        );
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn captured_selection_edge_scrolls_while_pointer_is_stationary(cx: &mut TestAppContext) {
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty target lane");
    let start = point(
        lane.origin.x + lane.size.width * 0.25,
        lane.origin.y + lane.size.height * 0.75,
    );
    editor.update(cx, |view, _| {
        view.viewport = Some(super::viewport::Viewport {
            start: 0,
            length: 100_000,
        });
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let outside_right = point(lane.origin.x + lane.size.width + px(20.0), start.y);
    visual.simulate_mouse_move(
        outside_right,
        MouseButton::Left,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    let base = Instant::now();
    editor.update(cx, |view, _| {
        let gesture = view
            .time_selection_gesture
            .as_mut()
            .expect("captured selection");
        gesture.last_tick = base;
    });
    let before = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        assert!(view.tick_time_gesture_autoscroll(base + Duration::from_millis(50)));
    });
    let first = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        let _ = view.update_time_selection_gesture(
            f32::from(outside_right.x),
            f32::from(outside_right.y),
            true,
        );
        assert!(view.tick_time_gesture_autoscroll(base + Duration::from_millis(100)));
    });
    editor.read_with(cx, |view, _| {
        let after = view.visible_viewport(384_000);
        assert!(
            first.start > before.start && after.start > first.start,
            "interleaved pointer updates do not starve stationary right-edge panning"
        );
        assert_eq!(
            view.time_selection
                .expect("reprojected selection")
                .end_frame,
            after.end().expect("viewport end")
        );
        let gesture = view
            .time_selection_gesture
            .as_ref()
            .expect("gesture remains active");
        assert!(
            gesture.free_modifier,
            "Command/free-snap state survives pointer ticks"
        );
    });
    let outside_left = point(lane.origin.x - px(20.0), start.y);
    editor.update(cx, |view, _| {
        view.viewport = Some(super::viewport::Viewport {
            start: 100_000,
            length: 100_000,
        });
        let _ = view.update_time_selection_gesture(
            f32::from(outside_left.x),
            f32::from(outside_left.y),
            true,
        );
        assert!(view.tick_time_gesture_autoscroll(base + Duration::from_millis(150)));
    });
    editor.read_with(cx, |view, _| {
        assert!(view.visible_viewport(384_000).start < 100_000);
    });
    editor.update(cx, |view, _| {
        view.viewport = Some(super::viewport::Viewport {
            start: view.viewport_horizon(384_000) - 100_000,
            length: 100_000,
        });
        let _ = view.update_time_selection_gesture(
            f32::from(outside_right.x),
            f32::from(outside_right.y),
            true,
        );
    });
    let right_limit = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        assert!(!view.tick_time_gesture_autoscroll(base + Duration::from_millis(200)));
        assert_eq!(view.visible_viewport(384_000), right_limit);
    });
    editor.update(cx, |view, _| {
        view.viewport = Some(super::viewport::Viewport {
            start: 0,
            length: 100_000,
        });
        let _ = view.update_time_selection_gesture(
            f32::from(outside_left.x),
            f32::from(outside_left.y),
            true,
        );
    });
    let left_limit = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        assert!(!view.tick_time_gesture_autoscroll(base + Duration::from_millis(250)));
        assert_eq!(view.visible_viewport(384_000), left_limit);
    });
    visual.simulate_mouse_up(outside_right, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn completed_time_selection_survives_idle_ticks_and_remains_rendered(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for (project, source, target, _) in [audio_project(), midi_project()] {
        for project_scope in [false, true] {
            let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let lane = visual
                .debug_bounds(if project_scope {
                    "main-track-lane"
                } else {
                    Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str())
                })
                .expect("selection lane");
            let source_lane = visual
                .debug_bounds(Box::leak(
                    format!("arrangement-lane-{}", source.value()).into_boxed_str(),
                ))
                .expect("audio or midi lane");
            let start = point(
                lane.origin.x + lane.size.width * 0.02,
                lane.origin.y + lane.size.height * 0.75,
            );
            let end = point(
                lane.origin.x + lane.size.width * 0.3,
                if project_scope {
                    start.y
                } else {
                    source_lane.origin.y + source_lane.size.height * 0.75
                },
            );
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let selection = editor.read_with(cx, |view, _| {
                assert!(view.time_selection_gesture.is_none());
                assert_eq!(view.time_selection_project_scope, project_scope);
                view.time_selection.expect("completed range")
            });
            let viewport = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
            // The real controller pumps both ticks even after pointer capture ends.
            editor.update(cx, |view, cx| {
                for delay in [20, 100, 1_000] {
                    let now = Instant::now() + Duration::from_millis(delay);
                    assert!(!view.tick_time_gesture_autoscroll(now));
                    view.tick_edit_cursor(now);
                    assert_eq!(view.time_selection, Some(selection));
                    assert_eq!(view.time_selection_project_scope, project_scope);
                    assert_eq!(view.visible_viewport(384_000), viewport);
                }
                assert!(
                    view.last_enqueued_edit.is_none(),
                    "selection does not edit history"
                );
                cx.notify();
            });
            visual.run_until_parked();
            for track in [source, target] {
                assert!(
                    visual
                        .debug_bounds(Box::leak(
                            format!("edit-time-selection-{}", track.value()).into_boxed_str(),
                        ))
                        .is_some(),
                    "completed range stays painted over occupied and empty lanes"
                );
            }
            if project_scope {
                assert!(visual.debug_bounds("main-project-time-selection").is_some());
            }
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(
                    view.normalized_time_selection().is_none(),
                    "escape still clears the range"
                );
            });
        }
    }
}

#[gpui::test]
fn stale_or_canceled_time_gesture_cannot_autoscroll(cx: &mut TestAppContext) {
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty target lane");
    let start = point(
        lane.origin.x + lane.size.width * 0.25,
        lane.origin.y + lane.size.height * 0.75,
    );
    editor.update(cx, |view, _| {
        view.viewport = Some(super::viewport::Viewport {
            start: 100_000,
            length: 100_000,
        });
        assert!(view.begin_time_selection_gesture(
            f32::from(start.x),
            f32::from(start.y),
            true,
            false
        ));
        assert!(view.update_time_selection_gesture(
            f32::from(lane.origin.x + lane.size.width + px(10.0)),
            f32::from(start.y),
            true
        ));
        let gesture = view.time_selection_gesture.as_mut().expect("gesture");
        gesture.revision = gesture.revision.wrapping_add(1);
    });
    let before_stale_tick = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        assert!(!view.tick_time_gesture_autoscroll(Instant::now() + Duration::from_secs(1)));
        assert!(view.time_selection_gesture.is_none());
        assert_eq!(view.visible_viewport(384_000), before_stale_tick);
    });
    editor.update(cx, |view, _| {
        assert!(view.begin_time_selection_gesture(
            f32::from(start.x),
            f32::from(start.y),
            true,
            false
        ));
        assert!(view.update_time_selection_gesture(
            f32::from(lane.origin.x + lane.size.width + px(10.0)),
            f32::from(start.y),
            true
        ));
        assert!(view.cancel_time_selection_gesture());
    });
    let before_canceled_tick = editor.read_with(cx, |view, _| view.visible_viewport(384_000));
    editor.update(cx, |view, _| {
        assert!(!view.tick_time_gesture_autoscroll(Instant::now() + Duration::from_secs(1)));
        assert_eq!(view.visible_viewport(384_000), before_canceled_tick);
    });
}

#[gpui::test]
fn midi_time_move_preview_translates_cached_notes_to_the_destination(cx: &mut TestAppContext) {
    let (project, source, target, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("prepared midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("destination lane");
    editor.update(cx, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: Some(source),
            end_track: Some(source),
            anchor_frame: 24_000,
            end_frame: 72_000,
        });
        view.selected_track = Some(source);
        cx.notify();
    });
    visual.run_until_parked();
    let down = point(
        source_lane.origin.x + source_lane.size.width * 0.1,
        source_lane.origin.y + source_lane.size.height * 0.2,
    );
    let destination = point(
        target_lane.origin.x + target_lane.size.width * 0.55,
        target_lane.origin.y + target_lane.size.height * 0.2,
    );
    editor.update(cx, |view, cx| {
        assert!(view.begin_time_range_move_gesture(f32::from(down.x), f32::from(down.y)));
        assert!(view.update_time_range_move_gesture(
            f32::from(destination.x),
            f32::from(destination.y),
            false
        ));
        assert_eq!(
            view.time_range_move_display()
                .expect("midi move preview")
                .source
                .tracks(),
            &[source]
        );
        cx.notify();
    });
    visual.run_until_parked();
    let moved = visual
        .debug_bounds(Box::leak(
            format!("time-move-midi-fragment-{}", clip.value()).into_boxed_str(),
        ))
        .expect("cached note fragment appears on its destination row");
    assert_eq!(moved.origin.y, target_lane.origin.y);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("time-move-midi-residual-0-{}", clip.value()).into_boxed_str(),
            ))
            .is_some(),
        "the source clip tail remains visible after the selected time is hidden"
    );
}

#[gpui::test]
fn multi_row_move_preview_maps_each_source_row_to_its_destination_offset(cx: &mut TestAppContext) {
    let (project, first_source, first_target, _) = audio_project();
    let mut model = hex_project::Editor::new(project, 8).expect("editor");
    let hex_project::Created::Imported {
        track: second_source,
        clip: second_clip,
        ..
    } = model
        .execute(Command::ImportAudioTrack {
            name: "second source".into(),
            path: "second-missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .expect("second source import")
    else {
        panic!("expected imported second source");
    };
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", first_target.value()).into_boxed_str(),
        ))
        .expect("first selected source row");
    let destination_lanes = [first_source, first_target].map(|track| {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("destination row")
    });
    let source_tail_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", second_source.value()).into_boxed_str(),
        ))
        .expect("second selected source row");
    editor.update(cx, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: Some(first_target),
            end_track: Some(second_source),
            anchor_frame: 0,
            end_frame: 48_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let down = point(
        target_lane.origin.x + target_lane.size.width * 0.1,
        target_lane.origin.y + target_lane.size.height * 0.2,
    );
    let destination = point(
        destination_lanes[0].origin.x + destination_lanes[0].size.width * 0.6,
        destination_lanes[0].origin.y + destination_lanes[0].size.height * 0.2,
    );
    editor.update(cx, |view, _| {
        let grab = point(
            target_lane.origin.x + target_lane.size.width * 0.1,
            target_lane.origin.y + target_lane.size.height * 0.2,
        );
        assert!(view.begin_time_range_move_gesture(f32::from(grab.x), f32::from(grab.y)));
        assert!(view.update_time_range_move_gesture(f32::from(grab.x), -1_000.0, false));
        assert_eq!(
            view.time_range_move_display()
                .expect("range clamps at top")
                .destination_first_index,
            0
        );
        assert!(view.update_time_range_move_gesture(
            f32::from(target_lane.origin.x + target_lane.size.width * 0.25),
            10_000.0,
            false
        ));
        assert_eq!(
            view.time_range_move_display()
                .expect("range clamps at bottom")
                .destination_first_index,
            1
        );
        assert!(view.cancel_time_range_move_gesture());
    });
    editor.update(cx, |view, cx| {
        assert!(view.begin_time_range_move_gesture(f32::from(down.x), f32::from(down.y)));
        assert!(view.update_time_range_move_gesture(
            f32::from(destination.x),
            f32::from(destination.y),
            false
        ));
        let display = view.time_range_move_display().expect("two-row move");
        assert_eq!(display.source.tracks(), &[first_target, second_source]);
        assert_eq!(
            display.destination_last_index - display.destination_first_index,
            1
        );
        cx.notify();
    });
    visual.run_until_parked();
    let moved = visual
        .debug_bounds(Box::leak(
            format!(
                "time-move-audio-fragment-{}-{}",
                first_target.value(),
                second_clip.value()
            )
            .into_boxed_str(),
        ))
        .expect("second source row appears on the corresponding second destination row");
    assert_eq!(moved.origin.y, destination_lanes[1].origin.y);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("time-move-audio-residual-{}-48000", second_source.value())
                    .into_boxed_str(),
            ))
            .is_some(),
        "the second source row keeps its outside tail"
    );
    assert_eq!(source_tail_lane.size.height, moved.size.height);
}

#[gpui::test]
fn pending_time_move_preview_survives_until_worker_failure_then_restores_source(
    cx: &mut TestAppContext,
) {
    let (project, source, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("destination lane");
    editor.update(cx, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: Some(source),
            end_track: Some(source),
            anchor_frame: 0,
            end_frame: 48_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let down = point(
        source_lane.origin.x + source_lane.size.width * 0.04,
        source_lane.origin.y + source_lane.size.height * 0.2,
    );
    let destination = point(
        target_lane.origin.x + target_lane.size.width * 0.5,
        target_lane.origin.y + target_lane.size.height * 0.2,
    );
    editor.update(cx, |view, _| {
        assert!(view.begin_time_range_move_gesture(f32::from(down.x), f32::from(down.y)));
        assert!(view.update_time_range_move_gesture(
            f32::from(destination.x),
            f32::from(destination.y),
            false
        ));
        assert!(view.finish_time_range_move_gesture());
        assert!(view.pending_time_range_move.is_some());
        assert!(view.time_range_move_display().is_some());
    });
    let failed = editor.read_with(cx, |view, _| Event {
        state: Arc::clone(view.state.as_ref().expect("current project state")),
        playback: None,
        status: None,
        error: Some("test rejection".to_owned()),
        recovery_failure: None,
        request_complete: true,
        sampler_load_complete: None,
        sampler_edit_complete: None,
        plugin_effect_complete: Vec::new(),
        plugin_parameter_observation: false,
        plugin_parameters: None,
        computer_keyboard_complete: None,
        midi_take_complete: None,
        device_automation_take_complete: None,
        tuning_audition_complete: None,
        live_expression_complete: None,
        bounce_complete: None,
        audio_batch_complete: None,
        audio_batch_failure: None,
        import_truncation: None,
        save_complete: false,
        save_correlation: None,
        document_format_upgrade_required: false,
        save_confirmation_required: false,
        open_complete: false,
        project_replaced: false,
        stopped_playback_restore_complete: false,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
        device_preview: None,
        tuning_preview: None,
        tuning_gesture_ack: None,
        tuning_gesture_request: false,
        active_tuning_gesture: None,
        tuning_library: None,
        gesture_ack: None,
        gesture_protocol_ack: false,
        gesture_request: false,
        active_device_gesture: None,
    });
    editor.update(cx, |view, cx| {
        view.move_time_selection_complete(&failed);
        assert!(view.pending_time_range_move.is_none());
        assert!(view.time_range_move_display().is_none());
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn selection_handle_and_reselect_zones_own_gestures_over_selected_clips(cx: &mut TestAppContext) {
    for moving in [true, false] {
        let (project, source, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", source.value()).into_boxed_str(),
            ))
            .expect("lane");
        let click = point(
            lane.origin.x + lane.size.width * 0.125,
            lane.origin.y + lane.size.height * 0.75,
        );
        visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
        editor.update(cx, |view, cx| {
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(source),
                end_track: Some(source),
                anchor_frame: 0,
                end_frame: 96_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let top = point(click.x, lane.origin.y + lane.size.height * 0.2);
        visual.simulate_mouse_move(top, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::OpenHand)
        );
        let start = if moving { top } else { click };
        let end = point(lane.origin.x + lane.size.width * 0.2, start.y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert_eq!(view.time_range_move_gesture.is_some(), moving);
            assert_eq!(view.time_selection_gesture.is_some(), !moving);
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(
                view.active_clip_drag.is_none(),
                "selected zones must not drag a single clip"
            )
        });
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            if moving {
                assert!(matches!(
                    view.last_enqueued_edit,
                    Some(Command::MoveTimeRange { .. })
                ));
            } else {
                assert!(view.last_enqueued_edit.is_none());
                let range = view
                    .normalized_time_selection()
                    .expect("new selection over clip");
                assert!(range.start() > 0);
                assert!(range.end() < 96_000);
            }
        });
    }
}

#[gpui::test]
fn ripple_shortcuts_route_to_insert_and_respect_editor_focus(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            window.focus(&view.focus);
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(target),
                end_track: Some(source),
                anchor_frame: 96_000,
                end_frame: 48_000,
            });
        })
    });
    visual.simulate_keystrokes("cmd-shift-c");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.last_time_request, Some(("copy", None)))
    });
    visual.simulate_keystrokes("cmd-shift-v");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_time_request,
            Some(("insert", Some((source, 48_000))))
        )
    });
    visual.simulate_keystrokes("cmd-v");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_time_request,
            Some(("paste", Some((source, 48_000))))
        )
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.last_time_request = None;
            window.focus(&view.midi_focus);
        })
    });
    visual.simulate_keystrokes("cmd-shift-v");
    editor.read_with(cx, |view, _| assert!(view.last_time_request.is_none()));
}

#[gpui::test]
fn space_restarts_from_selection_then_edit_cursor_and_respects_focus(cx: &mut TestAppContext) {
    for enabled in [false, true] {
        assert_space_restart_policy(&mut cx.new_app(), enabled);
    }
}

fn assert_space_restart_policy(cx: &mut TestAppContext, enabled: bool) {
    cx.update(super::bind_editor_keys);
    let (project, source, target, _) = audio_project();
    let mut model = Editor::new(project, 8).unwrap();
    model
        .execute(Command::SetLoopRegion {
            start: 32_000,
            end: 64_000,
            enabled,
        })
        .unwrap();
    let project = model.project().clone();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            window.focus(&view.focus);
            view.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(source),
                frame: 73_000,
                generation: view.project_generation,
                pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
                pulse_started: std::time::Instant::now(),
            });
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(target),
                end_track: Some(source),
                anchor_frame: 52_000,
                end_frame: 28_000,
            });
        })
    });

    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(28_000)),
            "reverse time selection supplies its normalized start, even outside an enabled loop"
        );
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(source),
                end_track: Some(source),
                anchor_frame: 41_000,
                end_frame: 41_000,
            });
            view.last_transport_command = None;
            window.focus(&view.focus);
        })
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(73_000)),
            "a point selection falls back to the edit cursor, even outside an enabled loop"
        );
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.edit_cursor = None;
            view.time_selection = None;
            view.last_transport_command = None;
            window.focus(&view.focus);
        })
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(
                if enabled { 32_000 } else { 0 }
            )),
            "with no selection or cursor, an enabled loop start or project beginning"
        );
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            let token = view.play_intent.reserve_token().expect("play intent token");
            view.play_intent.admit(token, true);
            view.last_transport_command = None;
            window.focus(&view.focus);
        })
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::Pause),
            "Space pauses when the latest pending transport intent is Play"
        );
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            let token = view
                .play_intent
                .reserve_token()
                .expect("pause intent token");
            view.play_intent.admit(token, false);
            view.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(source),
                frame: 73_000,
                generation: view.project_generation,
                pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
                pulse_started: std::time::Instant::now(),
            });
            view.last_transport_command = None;
            window.focus(&view.focus);
        })
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(73_000)),
            "Space after Pause uses the edit cursor rather than the paused playhead"
        );
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(hex_project::LoopRegion::new(32_000, 64_000, enabled).unwrap())
        );
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.last_transport_command = None;
            window.focus(&view.midi_focus);
        })
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(73_000)),
            "Space reaches transport even if a retained focus is no longer mounted"
        );
    });
}

#[gpui::test]
fn space_starts_from_the_focused_piano_roll_without_repeating(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = super::midi_canvas::creation_tests::fixture(cx);
    assert!(visual.debug_bounds("midi-note-roll").is_some());
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
        is_held: true,
    });
    editor.read_with(cx, |view, _| assert!(view.last_transport_command.is_none()));
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(0))
        );
    });
}

#[gpui::test]
fn stateful_space_queues_play_from_without_a_seek_worker(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            window.focus(&view.focus);
            // Native resident metadata deliberately carries no graph handle.
            // Its stateful flag formerly forced Space through the seek worker.
            view.audible_playback = Some(super::PlaybackCandidate {
                worker_requires_preroll: Some(true),
                observation: None,
                audio_revision: 1,
                project_generation: view.project_generation,
                state: Arc::clone(view.state.as_ref().unwrap()),
                prepared_revision: None,
                seek_request_id: None,
                device_preview_identity: None,
                tuning_preview_identity: None,
            });
            view.time_selection = None;
            view.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(source),
                frame: 48_000_000,
                generation: view.project_generation,
                pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
                pulse_started: Instant::now(),
            });
        });
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(48_000_000))
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
}

#[gpui::test]
fn sampler_space_preserves_edit_cursor_time_selection_and_pause(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let mut model = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = model
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("midi track")
    };
    let (state, source_slot) =
        hex_engine::native_device_resources::sampler_import_template_on_control(4).unwrap();
    model
        .execute(Command::ImportDeviceAssetIntoNewInstrument {
            track,
            placement: hex_project::InstrumentLoadPlacement::First,
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 4,
            state,
            source_slot,
        })
        .unwrap();
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for selection in [false, true] {
        visual.update(|window, cx| {
            editor.update(cx, |view, _| {
                view.edit_cursor = Some(super::edit_cursor::EditCursor {
                    track: Some(track),
                    frame: 73_000,
                    generation: view.project_generation,
                    pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
                    pulse_started: Instant::now(),
                });
                view.time_selection = selection.then_some(super::time_selection::TimeSelection {
                    anchor_track: Some(track),
                    end_track: Some(track),
                    anchor_frame: 52_000,
                    end_frame: 28_000,
                });
                view.last_transport_command = None;
                window.focus(&view.focus);
            })
        });
        visual.simulate_keystrokes("space");
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::playback::PlaybackCommand::PlayFrom(
                    if selection { 28_000 } else { 73_000 }
                )),
                "sampler uses the ordinary selected start, selection={selection}"
            );
        });
    }
    editor.update(cx, |view, _| {
        let token = view.play_intent.reserve_token().unwrap();
        view.play_intent.admit(token, true);
        view.last_transport_command = None;
    });
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::Pause),
            "sampler uses ordinary pause without resetting the cursor"
        );
        assert_eq!(view.edit_cursor.unwrap().frame, 73_000);
    });
}

#[gpui::test]
fn time_selection_binary_dot_follows_drag_corner_and_true_visible_endpoint(
    cx: &mut TestAppContext,
) {
    for main in [false, true] {
        for tempo in [120_000_000, 333_123_456] {
            let mut model = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
            let mut ids = Vec::new();
            for index in 0..3 {
                let Created::Track(id) = model
                    .execute(Command::AddTrack {
                        name: format!("track {index}"),
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                ids.push(id);
            }
            model
                .execute(Command::SetProjectTempo {
                    tempo: hex_project::Tempo::new(tempo).unwrap(),
                })
                .unwrap();
            let before = model.project().clone();
            let frame = |beat| {
                before
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_quarters(beat).unwrap())
                    .unwrap()
            };
            let viewport = super::viewport::Viewport {
                start: 0,
                length: frame(8),
            };
            let (handle, editor) = editor_window(cx, before.clone(), Arc::default());
            let mut visual = VisualTestContext::from_window(handle, cx);
            editor.update(cx, |v, cx| {
                v.snap_grid = super::musical_time::SnapGrid::Quarter;
                v.viewport = Some(viewport);
                cx.notify();
            });
            visual.run_until_parked();
            let lane_selector = if main {
                "main-track-lane"
            } else {
                Box::leak(format!("arrangement-lane-{}", ids[1].value()).into_boxed_str())
            };
            let selection_selector = if main {
                "main-project-time-selection"
            } else {
                Box::leak(format!("edit-time-selection-{}", ids[1].value()).into_boxed_str())
            };
            for (start, end, binary) in [(2, 4, true), (4, 2, true), (2, 5, false)] {
                let lane = visual.debug_bounds(lane_selector).unwrap();
                let at = |beat| {
                    point(
                        lane.left()
                            + lane.size.width * (frame(beat) as f32 / viewport.length as f32),
                        lane.top() + lane.size.height * 0.75,
                    )
                };
                visual.simulate_mouse_down(at(start), MouseButton::Left, Modifiers::default());
                visual.simulate_mouse_move(at(end), MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                let selection = visual.debug_bounds(selection_selector).unwrap();
                let marker = visual.debug_bounds("time-selection-binary-length-marker");
                assert_eq!(
                    marker.is_some(),
                    binary,
                    "main={main}, tempo={tempo}, {start}..{end}"
                );
                if let Some(marker) = marker {
                    // Absolute child edges snap to the physical pixel grid.
                    let pixel = visual.update(|window, _| 1.0 / window.scale_factor().max(1.0));
                    assert_eq!(marker.size, gpui::size(px(5.0), px(5.0)));
                    assert!((f32::from(marker.top() - selection.top()) - 2.0).abs() <= pixel);
                    let gap = if end > start {
                        marker.right() - selection.right()
                    } else {
                        marker.left() - selection.left()
                    };
                    assert!(
                        f32::from(gap).abs() <= pixel,
                        "marker follows dragged corner: main={main}, tempo={tempo}, {start}..{end}, marker={marker:?}, selection={selection:?}, gap={gap:?}"
                    );
                    assert_eq!(
                        visual.rendered_background("time-selection-binary-length-marker"),
                        Some(gpui::Background::from(gpui::rgb(crate::theme::color(
                            crate::theme::PaletteRole::Background
                        ))))
                    );
                }
                if binary && end > start {
                    // Reverse and cross a nonbinary length without releasing capture.
                    visual.simulate_mouse_move(at(1), MouseButton::Left, Modifiers::default());
                    visual.run_until_parked();
                    let marker = visual
                        .debug_bounds("time-selection-binary-length-marker")
                        .unwrap();
                    let selection = visual.debug_bounds(selection_selector).unwrap();
                    let pixel = visual.update(|window, _| 1.0 / window.scale_factor().max(1.0));
                    assert!(f32::from(marker.left() - selection.left()).abs() <= pixel);
                    visual.simulate_mouse_move(at(5), MouseButton::Left, Modifiers::default());
                    visual.run_until_parked();
                    assert!(
                        visual
                            .debug_bounds("time-selection-binary-length-marker")
                            .is_none()
                    );
                    visual.simulate_mouse_move(at(end), MouseButton::Left, Modifiers::default());
                    visual.run_until_parked();
                }
                visual.simulate_mouse_up(at(end), MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                assert_eq!(
                    visual
                        .debug_bounds("time-selection-binary-length-marker")
                        .is_some(),
                    binary
                );
                if binary {
                    editor.update(cx, |v, cx| {
                        v.viewport = Some(if end > start {
                            super::viewport::Viewport {
                                start: 0,
                                length: frame(3),
                            }
                        } else {
                            super::viewport::Viewport {
                                start: frame(3),
                                length: frame(8) - frame(3),
                            }
                        });
                        cx.notify();
                    });
                    visual.run_until_parked();
                    assert!(visual.debug_bounds(selection_selector).is_some());
                    assert!(
                        visual
                            .debug_bounds("time-selection-binary-length-marker")
                            .is_none(),
                        "a clipped viewport edge is not the true selection endpoint"
                    );
                    editor.update(cx, |v, cx| {
                        v.viewport = Some(viewport);
                        cx.notify();
                    });
                    visual.run_until_parked();
                }
                editor.read_with(cx, |v, _| {
                    assert_eq!(*v.state.as_ref().unwrap().project, before);
                    assert!(v.last_enqueued_edit.is_none());
                });
            }
        }
    }
}

#[gpui::test]
fn time_selection_markers_split_live_above_loop_and_follow_visible_endpoints(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, _, target, _) = audio_project();
    let original = project.clone();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane_id = Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_id).unwrap();
    let start = point(lane.left() + lane.size.width * 0.25, lane.center().y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("time-marker-top-start")
            .unwrap()
            .size
            .width,
        px(10.0)
    );
    assert!(visual.debug_bounds("time-marker-top-end").is_none());
    for fraction in [0.6, 0.1, 0.6] {
        let pointer = point(lane.left() + lane.size.width * fraction, lane.center().y);
        visual.simulate_mouse_move(pointer, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let (first, last, viewport) = editor.read_with(cx, |view, _| {
            assert!(
                view.time_selection_gesture.is_some(),
                "paint updates before release"
            );
            let selection = view.time_selection.unwrap();
            (
                selection.anchor_frame.min(selection.end_frame),
                selection.anchor_frame.max(selection.end_frame),
                view.visible_viewport(view.state.as_ref().unwrap().timeline_frames),
            )
        });
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let bottom = visual.debug_bounds("edit-cursor-layer").unwrap().bottom();
        for (name, frame, leading) in [("start", first, true), ("end", last, false)] {
            let expected = lane.left() + lane.size.width * viewport.fraction(frame).unwrap();
            for edge in ["top", "bottom"] {
                let id = Box::leak(format!("time-marker-{edge}-{name}").into_boxed_str());
                let bounds = visual.debug_bounds(id).unwrap();
                let tip = if leading {
                    bounds.right()
                } else {
                    bounds.left()
                };
                assert!((f32::from(tip - expected)).abs() < 1.0);
                assert_eq!(bounds.size.width, px(5.0));
                if edge == "top" {
                    assert!(bounds.bottom() <= rail.top());
                    assert!(
                        rail.top() - bounds.bottom() <= px(1.0),
                        "upper tips sit immediately above the loop row"
                    );
                } else {
                    assert_eq!(bounds.bottom(), bottom);
                }
            }
        }
    }
    let end = point(lane.left() + lane.size.width * 0.6, lane.center().y);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("time-marker-top-end").is_some());
    // Zebra colors are actually painted, with a visible distinction between adjacent bands.
    let bands = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        super::musical_time::adaptive_grid_bands_for_grid(
            &state.project,
            view.visible_viewport(state.timeline_frames),
            f32::from(lane.size.width),
            view.snap_grid,
        )
    });
    let mut colors = Vec::new();
    for band in bands.iter().take(2) {
        let id = Box::leak(format!("arrangement-grid-band-{}", band.start_frame).into_boxed_str());
        let color = visual.rendered_background(id).unwrap();
        assert_eq!(
            color,
            gpui::rgb(crate::theme::time_grid_band_color(band.alternate)).into()
        );
        colors.push(color);
    }
    assert_eq!(colors.len(), 2);
    assert_ne!(colors[0], colors[1]);
    editor.update(cx, |view, cx| {
        let selection = view.time_selection.unwrap();
        let first = selection.anchor_frame.min(selection.end_frame);
        let last = selection.anchor_frame.max(selection.end_frame);
        view.viewport = Some(super::viewport::Viewport {
            start: (first + last) / 2,
            length: last - first,
        });
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("time-marker-top-start").is_none());
    assert!(visual.debug_bounds("time-marker-top-end").is_some());
    assert!(visual.debug_bounds("time-marker-bottom-end").is_some());
    // Escape rejoins the halves at the original point; restore its viewport first.
    editor.update(cx, |view, cx| {
        view.viewport = None;
        cx.notify();
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds("time-marker-top-start")
            .unwrap()
            .size
            .width,
        px(10.0)
    );
    assert!(visual.debug_bounds("time-marker-top-end").is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original)
    });
}

#[gpui::test]
fn e_splits_both_time_selection_edges_in_scope_with_one_undo(cx: &mut TestAppContext) {
    use super::{
        loop_selection_ui_test::{fixture, pump},
        musical_time::SnapGrid,
        viewport::Viewport,
    };
    use hex_session::service::Request;
    for reversed in [false, true] {
        let cx = &mut cx.new_app();
        let (handle, editor, _audio, midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let (audio_track, midi_track) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (p.tracks()[0].id(), p.tracks()[1].id())
        });
        for command in [
            Command::MoveMidiClip {
                clip: midi,
                track: midi_track,
                start: BeatTime::ZERO,
            },
            Command::AddMidiTrack {
                name: "outside split scope".into(),
            },
        ] {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(command)))
            });
            pump(&editor, &mut visual);
        }
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[2].id();
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(2 << 32),
            })));
        });
        pump(&editor, &mut visual);
        let (before, outside) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (p.snapshot(), p.tracks()[2].clone())
        });
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start: 0,
                length: 96_000,
            });
            v.snap_grid = SnapGrid::Off;
            v.last_enqueued_edit = None;
            cx.notify();
        });
        visual.update(|window, cx| {
            window.activate_window();
            editor.read(cx).focus.focus(window);
        });
        visual.run_until_parked();
        let lane = |visual: &mut VisualTestContext, track: hex_project::TrackId| {
            visual
                .debug_bounds(Box::leak(
                    format!("arrangement-lane-{}", track.value()).into_boxed_str(),
                ))
                .unwrap()
        };
        let a = lane(&mut visual, audio_track);
        let b = lane(&mut visual, midi_track);
        let start = point(
            a.left() + a.size.width * 0.125,
            a.top() + a.size.height * 0.6,
        );
        let end = point(
            b.left() + b.size.width * 0.375,
            b.top() + b.size.height * 0.6,
        );
        let (down, up) = if reversed { (end, start) } else { (start, end) };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("e");
        editor.read_with(&visual, |v, _| {
            assert!(
                v.last_enqueued_edit.is_none(),
                "held range keeps input custody"
            )
        });
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let (selection, range) = editor.read_with(&visual, |v, _| {
            let range = v
                .normalized_time_selection()
                .expect("actual completed range");
            assert_eq!(range.tracks(), [audio_track, midi_track]);
            assert!(range.start() > 0 && range.end() < 48_000);
            assert!(!v.time_selection_project_scope);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            (v.time_selection, range)
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window))
        });
        visual.simulate_keystrokes("e");
        visual.simulate_input("e");
        editor.read_with(&visual, |v, cx| {
            assert!(v.last_enqueued_edit.is_none());
            assert!(v.browser.search.read(cx).text().contains('e'));
        });
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        editor.update(&mut visual, |v, cx| {
            v.computer_keyboard.enabled = true;
            cx.notify();
        });
        visual.simulate_keystrokes("e");
        editor.read_with(&visual, |v, _| {
            assert!(v.last_enqueued_edit.is_none(), "computer midi owns e")
        });
        editor.update(&mut visual, |v, cx| {
            v.computer_keyboard.enabled = false;
            v.theme_settings.open = true;
            cx.notify();
        });
        visual.simulate_keystrokes("e");
        editor.read_with(&visual, |v, _| {
            assert!(v.last_enqueued_edit.is_none(), "modal owns input")
        });
        editor.update(&mut visual, |v, cx| {
            v.theme_settings.open = false;
            cx.notify();
        });
        visual.run_until_parked();
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        if reversed {
            visual.update(|window, cx| {
                editor.update(cx, |v, cx| {
                    v.track_header_focus(audio_track, cx).focus(window);
                })
            });
            visual.run_until_parked();
        }
        visual.simulate_keystrokes("cmd-e");
        editor.read_with(&visual, |v, _| {
            assert!(
                v.last_enqueued_edit.is_none(),
                "existing cmd-e remains unbound"
            )
        });
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                assert!(
                    v.focus.is_focused(window) || v.track_header_focused(window),
                    "arranger focus restored"
                );
                assert!(
                    v.arrangement_view_change_available(cx),
                    "split admission restored"
                );
                assert!(
                    v.normalized_time_selection().is_some(),
                    "range preserved through guards"
                );
            })
        });
        visual.simulate_keystrokes("e");
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let expected = vec![
                (0, range.start()),
                (range.start(), range.end()),
                (range.end(), 48_000),
            ];
            let audio: Vec<_> = p.tracks()[0]
                .clips()
                .iter()
                .map(|c| (c.start(), c.start() + c.duration()))
                .collect();
            assert_eq!(
                audio, expected,
                "e must split both edges of every scoped clip; reversed={reversed}"
            );
            let midi: Vec<_> = p.tracks()[1]
                .midi_clips()
                .iter()
                .map(|c| {
                    (
                        p.tempo_map().beat_to_frame(c.start()).unwrap(),
                        p.tempo_map()
                            .beat_to_frame(BeatTime::from_raw(c.start().raw() + c.length().raw()))
                            .unwrap(),
                    )
                })
                .collect();
            assert_eq!(midi, expected);
            assert_eq!(p.tracks()[2], outside, "unselected track remains intact");
            assert_eq!(v.time_selection, selection);
            p.snapshot()
        });
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("edit-time-selection-{}", audio_track.value()).into_boxed_str()
                ))
                .is_some()
        );
        let mut restored = before.clone();
        restored.next_id = after.next_id; // Undo retains the canonical ID high-water mark.
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                restored,
                "one undo restores every cut"
            );
            assert_eq!(v.time_selection, selection);
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
            assert_eq!(v.time_selection, selection);
        });
    }
}

#[gpui::test]
fn split_does_not_escape_a_held_tempo_control(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, track, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let a = point(
        lane.left() + lane.size.width * 0.0625,
        lane.top() + lane.size.height * 0.6,
    );
    let b = point(lane.left() + lane.size.width * 0.1875, a.y);
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(b, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(b, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let tempo = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(tempo.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("e");
    editor.read_with(&visual, |v, _| {
        assert!(
            v.last_enqueued_edit.is_none(),
            "held tempo must retain input custody"
        )
    });
    visual.simulate_mouse_up(tempo.center(), MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn transport_split_vector_dispatches_the_edit_cursor_command(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("arrangement-lane-{}", source.value()).into_boxed_str());
    let lane = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(
        point(
            lane.origin.x + lane.size.width * 0.125,
            lane.origin.y + px(22.0),
        ),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let frame = editor.read_with(cx, |view, _| view.edit_cursor.unwrap().frame);
    let split = visual
        .debug_bounds("split selection edges or edit cursor · e")
        .unwrap();
    assert!(
        !visual
            .rendered_monochrome_colors("transport-split-icon")
            .is_empty()
    );
    visual.simulate_click(split.center(), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitClip { clip: id, frame: position }) if id == clip && position == frame));
    });
}

#[gpui::test]
fn selected_clips_move_both_triangles_to_the_earliest_play_start(cx: &mut TestAppContext) {
    use hex_engine::playback::PlaybackCommand::PlayFrom;
    cx.update(super::bind_editor_keys);
    let (project, source, target, audio) = audio_project();
    let mut model = Editor::new(project, 8).unwrap();
    model
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    // Place the clip after the tempo edit, which preserves musical onsets.
    model
        .execute(Command::MoveClip {
            clip: audio,
            track: source,
            start: 60_000,
        })
        .unwrap();
    let Created::Clip(midi) = model
        .execute(Command::PlaceMidiClip {
            track: target,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    let midi_frame = model
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::ONE)
        .unwrap();
    assert!(midi_frame < 60_000 && midi_frame != BeatTime::ONE.raw());
    let project = model.project();
    let midi_preview =
        hex_engine::midi_clip_preview(project, project.midi_clip(midi).unwrap()).unwrap();
    let preview = hex_engine::MidiArrangementPreview::from_clips(vec![midi_preview]).unwrap();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let play = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            editor.update(cx, |view, _| {
                view.last_transport_command = None;
                window.focus(&view.focus);
            })
        });
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        editor.read_with(visual, |view, _| view.last_transport_command)
    };
    let markers = |visual: &mut VisualTestContext, frame, locked| {
        let viewport = editor.read_with(visual, |view, _| {
            view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
        });
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let expected = rail.left() + rail.size.width * viewport.fraction(frame).unwrap();
        let selectors = if locked {
            ["locked-play-start-top", "locked-play-start-bottom"]
        } else {
            ["time-marker-top-start", "time-marker-bottom-start"]
        };
        for selector in selectors {
            let bounds = visual.debug_bounds(selector).unwrap();
            assert_eq!(bounds.size.width, px(10.0));
            assert!((f32::from(bounds.center().x - expected)).abs() < 1.0);
        }
    };

    visual.update(|_, cx| {
        editor.update(cx, |view, _| {
            view.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(source),
                frame: 73_000,
                generation: view.project_generation,
                pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
                pulse_started: Instant::now(),
            });
        })
    });
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(73_000)),
        "no clip selection plays from the visible cursor"
    );
    markers(&mut visual, 73_000, false);

    super::loop_selection_ui_test::click(&mut visual, audio, false);
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(60_000)),
        "one selected clip plays from its start"
    );
    markers(&mut visual, 60_000, false);

    let triangle = visual
        .debug_bounds("time-marker-top-start")
        .unwrap()
        .center();
    visual.simulate_click(triangle, Modifiers::default());
    visual.run_until_parked();
    markers(&mut visual, 60_000, true);

    super::loop_selection_ui_test::click(&mut visual, midi, true);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_clip_ids().len(), 2);
    });
    assert_eq!(play(&mut visual), Some(PlayFrom(60_000)));
    markers(&mut visual, 60_000, true);
    assert!(visual.debug_bounds("time-marker-top-start").is_none());

    let triangle = visual
        .debug_bounds("locked-play-start-bottom")
        .unwrap()
        .center();
    visual.simulate_click(triangle, Modifiers::default());
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(midi_frame)),
        "the earliest start wins across tracks, converting MIDI beats through the tempo map"
    );
    markers(&mut visual, midi_frame, false);

    visual.update(|_, cx| {
        editor.update(cx, |view, _| {
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: Some(source),
                end_track: Some(source),
                anchor_frame: 90_000,
                end_frame: 80_000,
            });
        })
    });
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(80_000)),
        "a time selection keeps its normalized start over selected clips"
    );

    visual.update(|_, cx| {
        editor.update(cx, |view, _| {
            view.time_selection = None;
            view.edit_cursor_in_selected_clip = view.selected_clip;
        })
    });
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(73_000)),
        "a cursor placed inside the selected clip stays visible and keeps its frame"
    );
    markers(&mut visual, 73_000, false);

    visual.update(|_, cx| {
        editor.update(cx, |view, _| {
            view.edit_cursor_in_selected_clip = None;
            view.selected_clip = None;
            view.reconcile_clip_selection();
        })
    });
    assert_eq!(
        play(&mut visual),
        Some(PlayFrom(73_000)),
        "clearing the clip selection restores the cursor and its play start"
    );
    markers(&mut visual, 73_000, false);

    editor.update(&mut visual, |view, _| view.edit_cursor = None);
    super::loop_selection_ui_test::click(&mut visual, midi, false);
    assert_eq!(play(&mut visual), Some(PlayFrom(midi_frame)));
    markers(&mut visual, midi_frame, false);

    super::loop_selection_ui_test::click(&mut visual, audio, false);
    markers(&mut visual, 60_000, false);
    model
        .execute(Command::MoveClip {
            clip: audio,
            track: source,
            start: 84_000,
        })
        .unwrap();
    editor.update(&mut visual, |view, cx| {
        let before = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(super::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(model.project().clone()),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::clone(&before.midi_preview),
            gain_preview: before.gain_preview.clone(),
            timeline_frames: before.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: before.revision + 1,
            unsaved: true,
            document_path: None,
        }));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(play(&mut visual), Some(PlayFrom(84_000)));
    markers(&mut visual, 84_000, false);
}

#[gpui::test]
fn cursor_inside_a_clip_keeps_it_in_the_clip_editor_without_selecting_it(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_cursor_keeps_clip_detail(&mut cx.new_app(), midi);
    }
}

fn assert_cursor_keeps_clip_detail(cx: &mut TestAppContext, midi: bool) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, clip) = if midi {
        midi_project()
    } else {
        audio_project()
    };
    let preview = if midi {
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                .expect("midi preview");
        Arc::new(preview)
    } else {
        Arc::default()
    };
    let (handle, editor) = editor_window(cx, project, preview);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("lane");
    let click_at = |visual: &mut VisualTestContext, fraction: f32| {
        let at = point(
            lane.origin.x + lane.size.width * fraction,
            lane.origin.y + px(22.0),
        );
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };

    // 0.17 of the 384 000-frame view lies inside both fixtures' clips.
    click_at(&mut visual, 0.17);
    let cursor = editor.read_with(&visual, |view, _| {
        assert!(view.selected_clip.is_none(), "no whole-clip selection");
        assert_eq!(view.detail_clip_id(), Some(clip), "midi={midi}");
        assert!(!view.device_panel_active, "the clip editor stays shown");
        view.edit_cursor.expect("cursor").frame
    });
    assert!(visual.debug_bounds("edit-cursor-layer").is_some());

    visual.simulate_keystrokes("delete space");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(
            project.clip(clip).is_some() || project.midi_clip(clip).is_some(),
            "delete does not treat the editor subject as a selected clip"
        );
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(cursor)),
            "play starts from the visible cursor, not the clip start"
        );
    });

    click_at(&mut visual, 0.75);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.detail_clip_id(), None, "empty lane space retires it");
    });
}

#[gpui::test]
fn either_cursor_triangle_locks_play_start_across_selection_and_unlocks(cx: &mut TestAppContext) {
    use super::time_selection::TimeSelection;
    use hex_engine::playback::PlaybackCommand;
    cx.update(super::bind_editor_keys);
    let (project, track, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let point = point(lane.left() + lane.size.width * 0.125, lane.center().y);
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    let start = editor.read_with(&visual, |v, _| v.edit_cursor.unwrap().frame);
    let top = visual.debug_bounds("time-marker-top-start").unwrap();
    editor.update(&mut visual, |v, _| v.close_prompt_open = true);
    visual.simulate_click(top.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.valid_locked_play_start().is_none())
    });
    editor.update(&mut visual, |v, _| v.close_prompt_open = false);
    visual.simulate_click(top.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.valid_locked_play_start().unwrap().frame, start);
        assert_eq!(v.playback_start_frame(), start);
        assert!(v.last_transport_command.is_none(), "locking never seeks");
    });
    assert!(visual.debug_bounds("locked-play-start-top").is_some());
    assert!(visual.debug_bounds("locked-play-start-bottom").is_some());
    let rest: gpui::Background =
        gpui::rgb(crate::theme::locked_time_position_marker_color(0.0)).into();
    let peak: gpui::Background =
        gpui::rgb(crate::theme::locked_time_position_marker_color(1.0)).into();
    let paint = |visual: &mut VisualTestContext, selector| {
        visual.rendered_path_triangles(selector)[0].color
    };
    let start_pulse = editor.read_with(&visual, |v, _| v.locked_play_start.unwrap().pulse_started);
    assert_eq!(paint(&mut visual, "locked-play-start-top"), rest);
    assert_eq!(paint(&mut visual, "locked-play-start-bottom"), rest);
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = Some(clip);
        v.time_selection = Some(TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: start + 10_000,
            end_frame: start + 20_000,
        });
        assert!(v.tick_edit_cursor(start_pulse + std::time::Duration::from_millis(1_200)));
        cx.notify();
    });
    visual.run_until_parked();
    assert_ne!(rest, peak);
    assert_eq!(paint(&mut visual, "locked-play-start-top"), peak);
    assert_eq!(paint(&mut visual, "locked-play-start-bottom"), peak);
    editor.update(&mut visual, |v, cx| {
        assert!(v.tick_edit_cursor(start_pulse + std::time::Duration::from_millis(2_400)));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(paint(&mut visual, "locked-play-start-top"), rest);
    editor.read_with(&visual, |v, _| assert_eq!(v.playback_start_frame(), start));
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("space");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.last_transport_command,
            Some(PlaybackCommand::PlayFrom(start))
        )
    });
    let bottom = visual.debug_bounds("locked-play-start-bottom").unwrap();
    visual.simulate_click(bottom.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.valid_locked_play_start().is_none());
        assert_ne!(v.playback_start_frame(), start);
        assert_eq!(
            v.last_transport_command,
            Some(PlaybackCommand::PlayFrom(start)),
            "unlocking must not seek active playback"
        );
    });
}

#[gpui::test]
fn locked_point_moves_on_click_but_not_range_or_loop_and_invalidates_on_replacement(
    cx: &mut TestAppContext,
) {
    use hex_session::service::Request;
    let (handle, editor, _, _, _environment) = super::loop_selection_ui_test::fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let at = |fraction| point(lane.left() + lane.size.width * fraction, lane.center().y);
    visual.simulate_click(at(0.125), Modifiers::default());
    visual.run_until_parked();
    let bottom = visual.debug_bounds("time-marker-bottom-start").unwrap();
    visual.simulate_click(bottom.center(), Modifiers::default());
    visual.run_until_parked();
    let original = editor.read_with(&visual, |v, _| v.valid_locked_play_start().unwrap().frame);
    visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.6), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at(0.6), MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.normalized_time_selection().is_some());
        assert_eq!(v.playback_start_frame(), original);
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 12_000,
            end: 24_000,
            enabled: true
        })));
    });
    super::loop_selection_ui_test::pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.playback_start_frame(), original)
    });
    visual.simulate_click(at(0.75), Modifiers::default());
    visual.run_until_parked();
    let moved = editor.read_with(&visual, |v, _| {
        let moved = v.edit_cursor.unwrap().frame;
        assert_ne!(moved, original);
        assert_eq!(v.valid_locked_play_start().unwrap().frame, moved);
        assert_eq!(v.playback_start_frame(), moved);
        moved
    });
    let top = visual.debug_bounds("locked-play-start-top").unwrap();
    visual.simulate_click(top.center(), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.valid_locked_play_start().is_none())
    });
    visual.run_until_parked();
    let top = visual.debug_bounds("time-marker-top-start").unwrap();
    visual.simulate_click(top.center(), Modifiers::default());
    editor.update(&mut visual, |v, _| {
        assert_eq!(v.valid_locked_play_start().unwrap().frame, moved);
        v.project_generation += 1;
        assert!(v.valid_locked_play_start().is_none());
        v.tick_edit_cursor(Instant::now());
        assert!(v.locked_play_start.is_none());
    });
}
