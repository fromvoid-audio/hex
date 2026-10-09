use super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::Command;
use hex_session::service::Event;
use std::sync::Arc;

#[gpui::test]
fn clicked_audio_cursor_is_visible_and_command_e_uses_it_offline(cx: &mut TestAppContext) {
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
            view.time_selection_gesture.is_none(),
            "a clip hit remains available to clip selection and drag handling"
        );
    });
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let frame = editor.read_with(cx, |view, _| {
        let cursor = view.edit_cursor.expect("click places cursor");
        assert_eq!(cursor.track, source);
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
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitClip { clip: id, frame: position }) if id == clip && position == frame));
    });
}

#[gpui::test]
fn command_click_is_free_and_command_e_routes_midi_split(cx: &mut TestAppContext) {
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
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitMidiClip { clip: id, beat: position }) if id == clip && position == beat));
    });
}

#[gpui::test]
fn empty_position_and_missing_cursor_do_not_dispatch_a_split(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-e");
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
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.edit_cursor.expect("cursor on empty track").track,
            target
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
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let selection = view.normalized_time_selection().expect("time selection");
        assert_eq!(selection.tracks(), ids);
        assert!(selection.end() > selection.start());
        assert_eq!(view.selected_track, Some(ids[3]));
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
    visual.simulate_mouse_up(reselect_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let range = view.normalized_time_selection().expect("replacement range");
        assert_eq!(range.tracks(), &ids[2..]);
        assert_eq!(view.selected_track, Some(ids[3]));
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
        assert_eq!(
            view.time_selection_paste_target(),
            Some((ids[0], selection.start()))
        );
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
        anchor_track: source,
        end_track: source,
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
        assert!(view.time_range_move_display().is_none());
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
            anchor_track: source,
            end_track: source,
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
            anchor_track: first_target,
            end_track: second_source,
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
            anchor_track: source,
            end_track: source,
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
        request_complete: true,
        save_complete: false,
        open_complete: false,
        project_replaced: false,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
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
                anchor_track: source,
                end_track: source,
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
