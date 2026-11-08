use super::clip_drag_ui_test::editor_window;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::{Command, Created, Editor, Project, ReturnId};
use hex_session::service::{Event, ViewState};
use std::sync::Arc;

fn project_with_tracks(count: usize) -> Project {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    for index in 0..count {
        let Created::Track(_) = editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track")
        else {
            panic!("track result")
        };
    }
    editor.project().clone()
}

fn project_with_returns(count: usize) -> (Project, Vec<ReturnId>) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack {
                name: format!("return {index}"),
            })
            .expect("add return")
        else {
            panic!("return result")
        };
        ids.push(id);
    }
    (editor.project().clone(), ids)
}

pub(super) fn completed_event(state: &ViewState) -> Event {
    Event {
        state: Arc::new(ViewState {
            canonical_audio_source: None,
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
            revision: state.revision.saturating_add(1),
            unsaved: state.unsaved,
            document_path: state.document_path.clone(),
        }),
        playback: None,
        status: None,
        error: None,
        recovery_failure: None,
        request_complete: true,
        sampler_load_complete: None,
        sampler_edit_complete: None,
        plugin_effect_complete: Vec::new(),
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
    }
}

#[gpui::test]
fn return_section_is_bounded_and_pinned_above_main_at_minimum_size(cx: &mut TestAppContext) {
    let (project, _) = project_with_returns(16);
    let mut model = Editor::new(project, 64).unwrap();
    for index in 0..40 {
        model
            .execute(Command::AddTrack {
                name: format!("regular {index}"),
            })
            .unwrap();
    }
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let returns = visual
        .debug_bounds("pinned-return-tracks")
        .expect("pinned return section");
    let main = visual
        .debug_bounds("pinned-main-track")
        .expect("pinned main track");
    let content = visual
        .debug_bounds("arrangement-content")
        .expect("regular tracks remain independently scrollable");
    let detail = visual
        .debug_bounds("clip-view")
        .expect("detail pane remains available");
    assert_eq!(returns.size.height, px(88.0));
    assert!(
        returns.origin.y >= content.origin.y + content.size.height - px(1.0),
        "returns={returns:?}, content={content:?}"
    );
    assert!(returns.origin.y + returns.size.height <= main.origin.y + px(1.0));
    assert!(main.origin.y + main.size.height <= detail.origin.y + px(1.0));
    assert!(content.size.height > px(1.0));
    assert!(visual.debug_bounds("return-track-row-1").is_some());
    editor.update(&mut visual, |view, cx| {
        view.track_scroll.set_offset(point(px(0.0), px(-500.0)));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("pinned-return-tracks").unwrap(),
        returns
    );
    assert_eq!(visual.debug_bounds("pinned-main-track").unwrap(), main);
}

#[gpui::test]
fn focusing_a_return_keeps_regular_selection_and_project_scope(cx: &mut TestAppContext) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("track")
    else {
        panic!("track result")
    };
    let Created::ReturnTrack(return_track) = editor
        .execute(Command::AddReturnTrack {
            name: "return a".into(),
        })
        .expect("return")
    else {
        panic!("return result")
    };
    let (handle, view) = editor_window(cx, editor.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    view.update(cx, |view, cx| {
        view.selected_track = Some(track);
        view.time_selection_project_scope = true;
        cx.notify();
    });
    visual.run_until_parked();
    let selector =
        Box::leak(format!("return-track-focus-{}", return_track.value()).into_boxed_str());
    let focus = visual.debug_bounds(selector).expect("return focus target");
    let point = point(focus.center().x, focus.center().y);
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.selected_track, Some(track));
        assert_eq!(view.focused_return, Some(return_track));
        assert!(view.time_selection_project_scope);
        assert!(view.device_panel_active);
        assert!(view.clip_view_open);
    });
    assert!(visual.debug_bounds("device-panel").is_some());
}

#[gpui::test]
fn main_track_stays_pinned_below_regular_tracks_at_minimum_size(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(cx, project_with_tracks(0), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));

    let mut pinned_top = None;
    for count in [0, 2, 40] {
        editor.update(cx, |view, cx| {
            view.state = Some(Arc::new(ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::new(project_with_tracks(count)),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview: Arc::default(),
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                timeline_frames: 384_000,
                can_undo: false,
                can_redo: false,
                revision: count as u64 + 1,
                unsaved: false,
                document_path: None,
            }));
            view.project_generation = view.project_generation.wrapping_add(1);
            cx.notify();
        });
        visual.run_until_parked();

        let row = visual
            .debug_bounds("pinned-main-track")
            .expect("pinned main row");
        let content = visual
            .debug_bounds("arrangement-content")
            .expect("scrollable regular track and append region");
        let detail = visual
            .debug_bounds("clip-view")
            .expect("shared detail panel remains below arrangement");
        assert_eq!(row.size.height, px(super::tracks::TRACK_ROW_HEIGHT));
        assert!(row.origin.y >= content.origin.y + content.size.height - px(1.0));
        assert!(row.origin.y + row.size.height <= detail.origin.y + px(1.0));
        assert!(visual.debug_bounds("arrangement-lane-main").is_none());
        let lane = visual
            .debug_bounds("main-track-lane")
            .expect("main has a timeline lane");
        let header = visual
            .debug_bounds("main-track-header")
            .expect("main has a mixer header");
        assert_eq!(header.size.width, px(super::tracks::TRACK_HEADER_WIDTH));
        assert_eq!(lane.size.width + header.size.width, row.size.width);
        assert!(visual.debug_bounds("main-track-grid").is_some());
        if let Some(previous) = pinned_top {
            assert_eq!(
                row.origin.y, previous,
                "track count must not move the pinned row"
            );
        }
        pinned_top = Some(row.origin.y);
    }
}

#[gpui::test]
fn main_gain_control_dispatches_canonical_master_command(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(cx, project_with_tracks(1), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let button = visual
        .debug_bounds("main-gain-up")
        .expect("main gain control");
    let button_point = point(button.center().x, button.center().y);
    visual.simulate_mouse_down(button_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button_point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::MasterGainBy { amount }) if (*amount - 0.1).abs() < f32::EPSILON
        ));
    });

    let lane = visual
        .debug_bounds("main-track-lane")
        .expect("main timeline lane");
    let click = point(lane.center().x, lane.center().y);
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.time_selection.is_some());
        assert!(view.edit_cursor.is_some());
        assert!(view.time_selection_project_scope);
        assert!(view.selected_track.is_none());
    });
    editor.update(cx, |view, cx| {
        view.mode = super::EditorMode::Session;
        view.last_enqueued_edit = None;
        cx.notify();
    });
    visual.run_until_parked();
    let session_gain = visual
        .debug_bounds("main-gain-up")
        .expect("session keeps its master controls");
    let session_click = point(session_gain.center().x, session_gain.center().y);
    visual.simulate_mouse_down(session_click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(session_click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::MasterGainBy { amount }) if (*amount - 0.1).abs() < f32::EPSILON
        ));
    });
}

#[gpui::test]
fn main_lane_drag_selects_all_tracks_and_copy_uses_project_scope(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (handle, editor) = editor_window(cx, project_with_tracks(3), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds("main-track-lane")
        .expect("main timeline lane");
    let start = point(
        lane.origin.x + lane.size.width * 0.2,
        lane.origin.y + lane.size.height * 0.7,
    );
    let end = point(lane.origin.x + lane.size.width * 0.55, start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let track_ids = editor.read_with(cx, |view, _| {
        let range = view.normalized_time_selection().expect("project selection");
        assert_eq!(range.tracks().len(), 3);
        assert_eq!(
            range.tracks(),
            view.state
                .as_ref()
                .expect("project state")
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>()
        );
        assert!(view.time_selection_project_scope);
        assert!(view.selected_track.is_none());
        range.tracks().to_vec()
    });
    for track in track_ids {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("edit-time-selection-{}", track.value()).into_boxed_str()
                ))
                .is_some()
        );
    }
    visual.simulate_keystrokes("cmd-c");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.last_time_request, Some(("copy", None)));
    });
}

#[gpui::test]
fn unsnapped_main_gestures_keep_main_geometry_after_resize_and_pan(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(cx, project_with_tracks(3), Arc::default());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        // Main and regular rows can differ by one pixel (or more after layout changes).
        for (left, width, start) in [(284.0, 1460.0, 0), (200.0, 680.0, 48_000)] {
            view.viewport = Some(super::viewport::Viewport {
                start,
                length: 192_000,
            });
            let viewport =
                view.visible_viewport(view.state.as_ref().expect("state").timeline_frames);
            let lane = super::drag::LaneBounds {
                left,
                width,
                top: 400.0,
                height: 30.0,
            };
            view.main_track_lane_bounds = Some(lane);
            view.main_track_lane_epoch = view.drag_layout_generation;
            for regular in view.drag_lanes.values_mut() {
                regular.left = left;
                regular.width = width - 1.0;
            }
            let x = left + width * 0.2;
            let y = lane.top + lane.height * 0.7;
            let expected = viewport.start + viewport.length / 5;
            assert!(view.begin_main_time_selection_gesture(x, y, true, false));
            view.update_time_selection_gesture(x, y, true);
            assert!(view.finish_time_selection_gesture());
            assert!(
                view.normalized_time_selection().is_none(),
                "stationary click became a range"
            );
            assert_eq!(view.edit_cursor.expect("cursor").frame, expected);

            assert!(view.begin_main_time_selection_gesture(x, y, true, false));
            assert!(view.update_time_selection_gesture(left + width * 0.6, y, true));
            assert!(view.finish_time_selection_gesture());
            let range = view.normalized_time_selection().expect("main range");
            assert_eq!(
                (range.start(), range.end()),
                (expected, viewport.start + viewport.length * 3 / 5)
            );
            assert_eq!(range.tracks().len(), 3);
            assert!(view.time_selection_project_scope);
            assert!(view.selected_track.is_none());

            // Grabbing the selected rectangle without moving must not shift its contents.
            let handle_y = lane.top + 2.0;
            assert!(view.begin_main_time_range_move_gesture(x, handle_y));
            assert!(view.update_time_range_move_gesture(x, handle_y, true));
            assert!(view.finish_time_range_move_gesture());
            assert!(view.last_enqueued_edit.is_none());

            assert!(view.begin_main_time_selection_gesture(x, y, true, false));
            // Inside Main's right edge but outside the narrower regular lane:
            // the timer must not spuriously scroll the viewport.
            view.update_time_selection_gesture(left + width - 0.5, y, true);
            let now = std::time::Instant::now() + std::time::Duration::from_millis(50);
            assert!(!view.tick_time_gesture_autoscroll(now));
            assert_eq!(view.visible_viewport(384_000), viewport);
            let selection = view.time_selection;
            view.main_track_lane_epoch = view.drag_layout_generation.wrapping_add(1);
            assert!(!view.update_time_selection_gesture(x, y, true));
            assert_eq!(
                view.time_selection, selection,
                "stale Main geometry must not use a regular row"
            );
            view.main_track_lane_epoch = view.drag_layout_generation;
            assert!(view.finish_time_selection_gesture());
        }
    });
}

#[gpui::test]
fn empty_project_main_lane_does_not_create_an_invalid_selection(cx: &mut TestAppContext) {
    let (handle, editor) = editor_window(cx, project_with_tracks(0), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds("main-track-lane")
        .expect("main timeline lane");
    let point = lane.center();
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.time_selection.is_none());
        assert!(view.edit_cursor.is_none());
        assert!(!view.time_selection_project_scope);
        assert!(view.selected_track.is_none());
    });
    let stale_track = project_with_tracks(1).tracks()[0].id();
    editor.update(cx, |view, _| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: stale_track,
            end_track: stale_track,
            anchor_frame: 0,
            end_frame: 48_000,
        });
        view.time_selection_project_scope = true;
        assert_eq!(view.time_selection_track_indices(), None);
    });
}

#[gpui::test]
fn moving_an_ordinary_selection_across_all_tracks_does_not_promote_project_scope(
    cx: &mut TestAppContext,
) {
    let project = project_with_tracks(3);
    let track_ids: Vec<_> = project.tracks().iter().map(|track| track.id()).collect();
    let (_handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().expect("view state");
        let range = hex_project::TimeRange::new(track_ids.clone(), 48_000, 96_000).expect("range");
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: track_ids[0],
            end_track: *track_ids.last().expect("tracks"),
            anchor_frame: range.start(),
            end_frame: range.end(),
        });
        view.edit_cursor = Some(super::edit_cursor::EditCursor {
            track: track_ids[0],
            frame: range.start(),
            generation: view.project_generation,
            pulse_color: 0,
            pulse_started: std::time::Instant::now(),
        });
        view.pending_time_range_move = Some(super::time_selection::PendingTimeRangeMove {
            source: range.clone(),
            range,
            project_scope: false,
            destination_first_index: 0,
            destination_last_index: track_ids.len() - 1,
            source_revision: state.revision,
            selection_intent_epoch: view.selection_intent_epoch.expect("selection epoch"),
            copy_modifier: false,
        });
        let event = completed_event(state);
        view.move_time_selection_complete(&event);
        assert!(!view.time_selection_project_scope);
        assert_eq!(view.selected_track, track_ids.last().copied());
    });
}

#[gpui::test]
fn moving_a_main_selection_preserves_project_scope_explicitly(cx: &mut TestAppContext) {
    let project = project_with_tracks(3);
    let track_ids: Vec<_> = project.tracks().iter().map(|track| track.id()).collect();
    let (_handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().expect("view state");
        let range = hex_project::TimeRange::new(track_ids.clone(), 48_000, 96_000).expect("range");
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: track_ids[0],
            end_track: *track_ids.last().expect("tracks"),
            anchor_frame: range.start(),
            end_frame: range.end(),
        });
        view.time_selection_project_scope = true;
        view.edit_cursor = Some(super::edit_cursor::EditCursor {
            track: track_ids[0],
            frame: range.start(),
            generation: view.project_generation,
            pulse_color: 0,
            pulse_started: std::time::Instant::now(),
        });
        view.pending_time_range_move = Some(super::time_selection::PendingTimeRangeMove {
            source: range.clone(),
            range,
            project_scope: true,
            destination_first_index: 0,
            destination_last_index: track_ids.len() - 1,
            source_revision: state.revision,
            selection_intent_epoch: view.selection_intent_epoch.expect("selection epoch"),
            copy_modifier: false,
        });
        let event = completed_event(state);
        view.move_time_selection_complete(&event);
        assert!(view.time_selection_project_scope);
        assert_eq!(
            view.time_selection
                .map(|selection| (selection.anchor_track, selection.end_track)),
            Some((track_ids[0], *track_ids.last().expect("tracks")))
        );
        assert_eq!(view.selected_track, None);
    });
}

#[gpui::test]
fn main_selection_handle_option_drag_copies_the_project_rectangle(cx: &mut TestAppContext) {
    let project = project_with_tracks(3);
    let track_ids: Vec<_> = project.tracks().iter().map(|track| track.id()).collect();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: track_ids[0],
            end_track: *track_ids.last().expect("tracks"),
            anchor_frame: 48_000,
            end_frame: 144_000,
        });
        view.time_selection_project_scope = true;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds("main-track-lane")
        .expect("main timeline lane");
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    let start = point(
        lane.origin.x + lane.size.width * 0.25,
        lane.origin.y + lane.size.height * 0.15,
    );
    let destination = point(lane.origin.x + lane.size.width * 0.7, start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, option);
    visual.simulate_mouse_move(destination, MouseButton::Left, option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let display = view
            .time_range_move_display()
            .expect("project copy preview");
        assert!(display.copy_modifier);
        assert_eq!(display.source.tracks(), track_ids);
        assert_eq!(display.destination_first_index, 0);
        assert_eq!(display.destination_last_index, 2);
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::CopyTimeRangeTo { range, track, .. })
                if range.tracks() == track_ids && *track == track_ids[0]
        ));
    });
}

#[gpui::test]
fn return_headers_share_track_geometry_without_redundant_section_chrome(cx: &mut TestAppContext) {
    for count in [0, 1, 2, 4, 16] {
        for mode in [
            hex_session::DocumentAudioMode::Resident,
            hex_session::DocumentAudioMode::VerifiedStreaming,
        ] {
            let (project, returns) = project_with_returns(count);
            let mut model = Editor::new(project, 8).unwrap();
            let Created::Track(track) = model
                .execute(Command::AddTrack {
                    name: "ordinary".into(),
                })
                .unwrap()
            else {
                panic!("ordinary track")
            };
            let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
            editor.update(cx, |view, cx| {
                view.document_audio_mode = mode;
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(880.0), px(540.0)));
            visual.run_until_parked();
            assert!(visual.debug_bounds("return-track-section-header").is_none());
            assert!(visual.debug_bounds("add-return-track").is_none());
            let section = visual.debug_bounds("pinned-return-tracks").unwrap();
            assert_eq!(
                section.size.height,
                px(count.min(2) as f32 * super::tracks::TRACK_ROW_HEIGHT)
            );
            let main = visual.debug_bounds("pinned-main-track").unwrap();
            assert!(section.bottom() <= main.top() + px(1.0));
            let ordinary = visual
                .debug_bounds(Box::leak(
                    format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
                ))
                .unwrap();
            if let Some(id) = returns.first() {
                let header = visual
                    .debug_bounds(Box::leak(
                        format!("return-track-header-{}", id.value()).into_boxed_str(),
                    ))
                    .unwrap();
                assert_eq!(header.size, ordinary.size);
                assert_eq!(header.right(), ordinary.right());
                if count == 1 {
                    // The minimum window clamps headers to the shared minimum.
                    // Give the shared width owner room for a real resize.
                    visual.simulate_resize(size(px(1280.0), px(800.0)));
                    visual.run_until_parked();
                    let initial_width = visual
                        .debug_bounds(Box::leak(
                            format!("return-track-header-{}", id.value()).into_boxed_str(),
                        ))
                        .unwrap()
                        .size
                        .width;
                    let handle = visual
                        .debug_bounds(Box::leak(
                            format!("track-width-resize-{}", track.value()).into_boxed_str(),
                        ))
                        .unwrap()
                        .center();
                    let to = point(handle.x - px(64.0), handle.y);
                    visual.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
                    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
                    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
                    visual.run_until_parked();
                    let ordinary = visual
                        .debug_bounds(Box::leak(
                            format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
                        ))
                        .unwrap();
                    let resized = visual
                        .debug_bounds(Box::leak(
                            format!("return-track-header-{}", id.value()).into_boxed_str(),
                        ))
                        .unwrap();
                    assert_eq!(resized.size.width, initial_width + px(64.0));
                    assert_eq!(resized.size, ordinary.size);
                    assert_eq!(resized.right(), ordinary.right());
                }
                let header = visual
                    .debug_bounds(Box::leak(
                        format!("return-track-header-{}", id.value()).into_boxed_str(),
                    ))
                    .unwrap();
                let gain_prefix = if mode == hex_session::DocumentAudioMode::Resident {
                    "return-volume"
                } else {
                    "return-gain-up"
                };
                for prefix in [
                    "return-track-focus",
                    gain_prefix,
                    "return-mute",
                    "return-remove",
                ] {
                    let control = visual
                        .debug_bounds(Box::leak(
                            format!("{prefix}-{}", id.value()).into_boxed_str(),
                        ))
                        .unwrap();
                    assert!(control.left() >= header.left() && control.right() <= header.right());
                    assert!(control.top() >= header.top() && control.bottom() <= header.bottom());
                }
            }
        }
    }
}

#[gpui::test]
fn clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize(
    cx: &mut TestAppContext,
) {
    use crate::editor::{
        loop_selection_ui_test::{fixture, pump},
        musical_time::SnapGrid,
        viewport::Viewport,
    };
    use hex_project::BeatTime;
    use hex_session::service::Request;
    for (offset, tail) in [(0, 480), (137, 480), (0, 48), (137, 48)] {
        let cx = &mut cx.new_app();
        let (handle, editor, audio, midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        let beat = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .frame_to_beat(offset)
                .unwrap()
        });
        for command in [
            editor.read_with(&visual, |v, _| Command::MoveClip {
                clip: audio,
                track: v.state.as_ref().unwrap().project.tracks()[0].id(),
                start: offset,
            }),
            editor.read_with(&visual, |v, _| Command::MoveMidiClip {
                clip: midi,
                track: v.state.as_ref().unwrap().project.tracks()[1].id(),
                start: beat,
            }),
        ] {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(command)))
            });
            pump(&editor, &mut visual);
        }
        let section_start = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .frame_to_beat(offset + 48_000 - tail)
                .unwrap()
        });
        editor.update(&mut visual, |v, _| {
            let revision = v.state.as_ref().unwrap().revision;
            assert!(v.enqueue(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision,
                command: Command::AddArrangementSection {
                    name: "aligned".into(),
                    color: hex_project::ElementColor::Cyan,
                    start: section_start,
                    end: BeatTime::from_raw(beat.raw() + (2 << 32)),
                },
            }));
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let p = &state.project;
            let c = p.clip(audio).unwrap();
            assert_eq!(c.start() + c.duration(), 48_000 + offset);
            let m = p.midi_clip(midi).unwrap();
            assert_eq!(
                p.tempo_map()
                    .beat_to_frame(BeatTime::from_raw(m.start().raw() + m.length().raw()))
                    .unwrap(),
                48_000 + offset
            );
        });
        // A short tail and a touching successor expose minimum-size/inset and
        // independent endpoint-rounding errors that a single wide clip misses.
        let split_beat = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .frame_to_beat(offset + 48_000 - tail)
                .unwrap()
        });
        for command in [
            Command::DuplicateClip(audio),
            Command::DuplicateMidiClip(midi),
            Command::SplitClip {
                clip: audio,
                frame: offset + 48_000 - tail,
            },
            Command::SplitMidiClip {
                clip: midi,
                beat: split_beat,
            },
        ] {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(command)))
            });
            pump(&editor, &mut visual);
        }
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision: v.state.as_ref().unwrap().revision,
                command: Command::AddArrangementSection {
                    name: "successor".into(),
                    color: hex_project::ElementColor::Cyan,
                    start: BeatTime::from_raw(beat.raw() + (2 << 32)),
                    end: BeatTime::from_raw(beat.raw() + (4 << 32)),
                },
            }));
        });
        pump(&editor, &mut visual);
        let (audio, midi, next_audio, next_midi, original) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let audio_clips = p.tracks()[0].clips();
            let midi_clips = p.tracks()[1].midi_clips();
            let frame = |c: &hex_project::MidiClip| p.tempo_map().beat_to_frame(c.start()).unwrap();
            (
                audio_clips
                    .iter()
                    .find(|c| c.start() == offset + 48_000 - tail)
                    .unwrap()
                    .id(),
                midi_clips
                    .iter()
                    .find(|c| frame(c) == offset + 48_000 - tail)
                    .unwrap()
                    .id(),
                audio_clips
                    .iter()
                    .find(|c| c.start() == offset + 48_000)
                    .unwrap()
                    .id(),
                midi_clips
                    .iter()
                    .find(|c| frame(c) == offset + 48_000)
                    .unwrap()
                    .id(),
                p.snapshot(),
            )
        });
        let audio_selector: &'static str =
            Box::leak(format!("clip-visual-{}", audio.value()).into_boxed_str());
        let midi_selector: &'static str =
            Box::leak(format!("midi-clip-visual-{}", midi.value()).into_boxed_str());
        let next_audio_selector: &'static str =
            Box::leak(format!("clip-visual-{}", next_audio.value()).into_boxed_str());
        let next_midi_selector: &'static str =
            Box::leak(format!("midi-clip-visual-{}", next_midi.value()).into_boxed_str());
        for (width, viewport) in [
            (
                1280.0,
                Viewport {
                    start: 0,
                    length: 64_000,
                },
            ),
            (
                1281.0,
                Viewport {
                    start: 6_000,
                    length: 64_000,
                },
            ),
            (
                1397.0,
                Viewport {
                    start: 12_345,
                    length: 96_000,
                },
            ),
        ] {
            visual.simulate_resize(size(px(width), px(800.0)));
            editor.update(&mut visual, |v, cx| {
                v.viewport = Some(viewport);
                v.snap_grid = SnapGrid::Quarter;
                cx.notify();
            });
            visual.run_until_parked();
            let lane = visual.debug_bounds("main-track-lane").unwrap();
            let grid = visual.debug_bounds("arrangement-grid-line-48000").unwrap();
            let expected = lane.left()
                + lane.size.width
                    * ((offset + 48_000 - viewport.start) as f32 / viewport.length as f32);
            let endpoint = visual
                .debug_bounds("arrangement-section-0")
                .unwrap()
                .right();
            let scale = visual.update(|window, _| window.scale_factor());
            let section_edge = visual
                .debug_bounds("arrangement-section-trailing-0")
                .unwrap();
            assert_eq!(section_edge.left(), endpoint);
            assert_eq!(f64::from(section_edge.scale(scale).size.width), 1.0);
            // GPUI's test window is 2x. Off-grid times may round to the nearest
            // physical pixel, but every consumer must choose the same pixel.
            assert!(
                (endpoint - expected).abs() <= px(0.25),
                "end={endpoint:?}, projection={expected:?}, viewport={viewport:?}, offset={offset}, tail={tail}"
            );
            if offset == 0 {
                assert_eq!(endpoint, grid.left());
                assert_eq!(section_edge.left(), grid.left());
            }
            for (prefix, id) in [("clip-trailing", audio), ("midi-clip-trailing", midi)] {
                let name = Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
                let edge = visual.debug_bounds(name).unwrap();
                assert_eq!(edge.left(), endpoint);
                assert_eq!(f64::from(edge.scale(scale).size.width), 1.0);
            }
            for selector in [audio_selector, midi_selector, "arrangement-section-0"] {
                let bounds = visual.debug_bounds(selector).unwrap();
                assert!(
                    (bounds.right() - endpoint).abs() < px(0.02),
                    "{selector}: end={:?} section-end={endpoint:?}, width={width}, viewport={viewport:?}, offset={offset}",
                    bounds.right()
                );
                if bounds.size.width > px(0.0) {
                    if selector == "arrangement-section-0" {
                        // Section fill is a shaped canvas, not one full rectangular
                        // scene quad. Its upper piece must retain the same endpoint.
                        let cut = crate::section_surface::cut(
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            crate::section_surface::CHAMFER,
                        );
                        let projected = bounds.scale(scale);
                        assert!(
                            visual
                                .rendered_filled_quads(selector)
                                .iter()
                                .any(|(paint, _)| (f64::from(paint.left())
                                    - f64::from(projected.left()))
                                .abs()
                                    < 0.1
                                    && (f64::from(paint.right()) - f64::from(projected.right()))
                                        .abs()
                                        < 0.1
                                    && (f64::from(paint.top()) - f64::from(projected.top())).abs()
                                        < 0.1
                                    && (f64::from(paint.size.height)
                                        - f64::from((bounds.size.height - px(cut)).scale(scale)))
                                    .abs()
                                        < 0.1),
                            "actual shaped section: {bounds:?}, offset={offset}, tail={tail}"
                        );
                    } else {
                        assert!(
                            visual.rendered_surface_geometry(selector).is_some(),
                            "actual painted surface: {selector}, bounds={bounds:?}, offset={offset}, tail={tail}"
                        );
                    }
                }
            }
            for selector in [
                next_audio_selector,
                next_midi_selector,
                "arrangement-section-1",
            ] {
                assert_eq!(
                    visual.debug_bounds(selector).unwrap().left(),
                    endpoint,
                    "touching items must share the same physical edge: {selector}"
                );
            }
            let audio_bounds = visual.debug_bounds(audio_selector).unwrap();
            let midi_bounds = visual.debug_bounds(midi_selector).unwrap();
            assert_eq!(
                audio_bounds.left(),
                midi_bounds.left(),
                "short audio/MIDI starts"
            );
            assert_eq!(
                audio_bounds.left(),
                visual.debug_bounds("arrangement-section-0").unwrap().left()
            );
            let projected_start = lane.left()
                + lane.size.width
                    * ((offset + 48_000 - tail - viewport.start) as f32 / viewport.length as f32);
            assert!((audio_bounds.left() - projected_start).abs() <= px(0.25));
            if tail == 480 {
                let header = visual
                    .debug_bounds(Box::leak(
                        format!("clip-name-{}", audio.value()).into_boxed_str(),
                    ))
                    .unwrap();
                visual.simulate_click(
                    point(audio_bounds.center().x, header.center().y),
                    Modifiers::default(),
                );
                visual.run_until_parked();
                assert_eq!(
                    visual.debug_bounds(audio_selector),
                    Some(audio_bounds),
                    "selection cannot move the clip boundary"
                );
                editor.read_with(&visual, |v, _| assert!(v.clip_is_selected(audio)));
            }
            if offset == 0 && width == 1280.0 {
                for (selector, clip) in [(audio_selector, audio), (midi_selector, midi)] {
                    let bounds = visual.debug_bounds(selector).unwrap();
                    let move_bounds = visual
                        .debug_bounds(Box::leak(
                            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
                        ))
                        .unwrap();
                    assert!(
                        move_bounds.size.width >= px(0.5),
                        "narrow clip must retain a visible move body: clip={bounds:?}, body={move_bounds:?}"
                    );
                    let down = point(bounds.center().x, bounds.top() + px(4.0));
                    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
                    visual.simulate_mouse_move(
                        point(down.x + px(12.0), down.y),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    visual.run_until_parked();
                    editor.read_with(&visual, |v, _| {
                        assert_eq!(
                            v.active_clip_drag.as_ref().map(|drag| drag.clip),
                            Some(clip)
                        );
                        assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
                    });
                    visual.simulate_keystrokes("escape");
                    visual.simulate_mouse_up(
                        point(down.x + px(12.0), down.y),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    pump(&editor, &mut visual);
                    assert_eq!(visual.debug_bounds(selector), Some(bounds));
                }
            }
            editor.read_with(&visual, |v, _| {
                for track in v.state.as_ref().unwrap().project.tracks() {
                    let measured = v.drag_lanes.get(&track.id()).unwrap();
                    assert_eq!(measured.left, f32::from(lane.left()));
                    assert_eq!(measured.width, f32::from(lane.size.width));
                }
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original)
            });
        }
    }
}
