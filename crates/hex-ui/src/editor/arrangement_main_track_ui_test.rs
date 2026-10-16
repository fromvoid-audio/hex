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

fn completed_event(state: &ViewState) -> Event {
    Event {
        state: Arc::new(ViewState {
            project: Arc::clone(&state.project),
            waveforms: state.waveforms.clone(),
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
        request_complete: true,
        computer_keyboard_complete: None,
        save_complete: false,
        open_complete: false,
        project_replaced: false,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
        device_preview: None,
        gesture_ack: None,
        gesture_protocol_ack: false,
        gesture_request: false,
        active_device_gesture: None,
    }
}

#[gpui::test]
fn return_section_is_bounded_and_pinned_above_main_at_minimum_size(cx: &mut TestAppContext) {
    let (project, _) = project_with_returns(16);
    let (handle, _) = editor_window(cx, project, Arc::default());
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
        .debug_bounds("clip-view-toggle-row")
        .expect("detail pane remains available");
    assert_eq!(returns.size.height, px(67.0));
    assert!(
        returns.origin.y >= content.origin.y + content.size.height - px(1.0),
        "returns={returns:?}, content={content:?}"
    );
    assert!(returns.origin.y + returns.size.height <= main.origin.y + px(1.0));
    assert!(main.origin.y + main.size.height <= detail.origin.y + px(1.0));
    assert!(content.size.height > px(1.0));
    assert!(visual.debug_bounds("return-track-row-1").is_some());
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
                project: Arc::new(project_with_tracks(count)),
                waveforms: Default::default(),
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
            .debug_bounds("clip-view-toggle-row")
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
        view.pending_time_range_move = Some(super::time_selection::PendingTimeRangeMove {
            source: range.clone(),
            range,
            project_scope: false,
            destination_first_index: 0,
            destination_last_index: track_ids.len() - 1,
            source_revision: state.revision,
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
        view.pending_time_range_move = Some(super::time_selection::PendingTimeRangeMove {
            source: range.clone(),
            range,
            project_scope: true,
            destination_first_index: 0,
            destination_last_index: track_ids.len() - 1,
            source_revision: state.revision,
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
