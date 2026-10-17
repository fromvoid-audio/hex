use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{AudioEventEdit, Command};

#[gpui::test]
fn event_ruler_selection_grid_and_shortcuts_stay_inside_clip_editor(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-events-waveform")
        .expect("event canvas");
    assert!(bounds.size.width > px(100.0) && bounds.size.height > px(40.0));
    let down = point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.origin.y + px(10.0),
    );
    let up = point(bounds.origin.x + bounds.size.width * 0.75, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-event-time-selection").is_some());
    let cursor = editor.read_with(cx, |view, _| {
        let range = view.audio_event_editor.range.expect("selected time");
        assert!(range.0 < range.1);
        assert_eq!(view.audio_event_editor.grid, SnapGrid::Adaptive);
        view.audio_event_editor.cursor
    });
    visual.simulate_keystrokes("cmd-4");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.audio_event_editor.grid, SnapGrid::Off);
        assert_eq!(view.snap_grid, SnapGrid::Adaptive);
    });
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx,|view,_|assert!(matches!(view.last_enqueued_edit,Some(Command::EditAudioEvents{clip:id,operation:AudioEventEdit::SplitRange{start:_,end}}) if id==clip && end==cursor)));
}

#[gpui::test]
fn event_drag_preview_cancel_and_revision_change_never_edit_outer_clip(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = point(
        bounds.origin.x + bounds.size.width * 0.3,
        bounds.center().y + px(12.0),
    );
    let moved = point(down.x + bounds.size.width * 0.2, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.audio_event_editor.gesture.is_some())
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let changed = hex_session::service::ViewState {
            canonical_audio_source: state.canonical_audio_source.clone(),
            project: state.project.clone(),
            waveforms: state.waveforms.clone(),
            audio_clip_waveforms: state.audio_clip_waveforms.clone(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: state.can_undo,
            can_redo: state.can_redo,
            revision: state.revision + 1,
            unsaved: state.unsaved,
            document_path: state.document_path.clone(),
        };
        view.state = Some(std::sync::Arc::new(changed));
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}

#[gpui::test]
fn event_move_preview_and_release_use_snapped_inner_coordinates(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        let next = hex_session::service::ViewState {
            canonical_audio_source: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        };
        // TestPlatform has no worker/audio output; real input still goes through
        // the mounted editor and its normal command queue boundary.
        view.state = Some(std::sync::Arc::new(next));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let original = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(
        bounds.origin.x + bounds.size.width * 0.125,
        bounds.center().y + px(12.0),
    );
    let up = point(bounds.origin.x + bounds.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = visual.debug_bounds("audio-event-1").unwrap();
    assert!(preview.origin.x > original.origin.x + px(20.0));
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "motion is a local preview"
        )
    });
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    editor.read_with(cx,|view,_|assert!(matches!(view.last_enqueued_edit,Some(Command::EditAudioEvents{clip:id,operation:AudioEventEdit::Move{event:1,start}}) if id==clip && start==36000)));
}

#[gpui::test]
fn selecting_an_off_grid_event_without_dragging_never_moves_audio(cx: &mut TestAppContext) {
    let (handle, editor, track, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        canonical
            .execute(Command::MoveClip {
                clip,
                track,
                start: 17000,
            })
            .unwrap();
        view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            project: std::sync::Arc::new(canonical.project().clone()),
            waveforms: state.waveforms.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: state.midi_preview.clone(),
            gain_preview: state.gain_preview.clone(),
            timeline_frames: state.timeline_frames,
            can_undo: true,
            can_redo: false,
            revision: 2,
            unsaved: true,
            document_path: None,
        }));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(
        bounds.origin.x + bounds.size.width * 0.125,
        bounds.center().y + px(12.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "selection alone must not snap the event's off-grid start"
        )
    });
}
