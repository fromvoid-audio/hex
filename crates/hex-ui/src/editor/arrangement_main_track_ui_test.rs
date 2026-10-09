use super::clip_drag_ui_test::editor_window;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::ViewState;
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
                midi_preview: Arc::default(),
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
        assert!(view.time_selection.is_none());
        assert!(view.edit_cursor.is_none());
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
