use super::{EditorMode, clip_drag_ui_test};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
use hex_project::Command;
use hex_session::{CachedPeak, CachedWaveform, service::ViewState};
use std::{collections::BTreeMap, sync::Arc};

#[gpui::test]
fn midi_clip_view_toggle_preserves_selection_and_moves_focus_out_of_hidden_roll(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, track, _, clip) = clip_drag_ui_test::midi_project();
    let note = project
        .midi_source(clip)
        .and_then(|source| source.notes().first())
        .expect("fixture note")
        .id();
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, window_cx| {
        editor.update(window_cx, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.midi_selection.set_single(note);
            view.midi_marquee = Some(([1.0, 1.0], [2.0, 2.0], false));
            view.midi_focus.focus(window);
            view.clip_view_open = true;
            cx.notify();
        });
    })
    .expect("focus midi editor");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-source-controls").is_some());
    assert!(visual.debug_bounds("midi-note-roll").is_some());
    let open_panel = visual.debug_bounds("clip-view").expect("open detail panel");
    assert!(f32::from(open_panel.size.height) >= 228.0);

    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("clip view toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.clip_view_open, "toggle click should hide detail")
    });
    let collapsed_panel = visual
        .debug_bounds("clip-view")
        .expect("collapsed detail panel");
    assert!(f32::from(collapsed_panel.size.height) < f32::from(open_panel.size.height));
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_clip, Some(clip));
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(!view.clip_view_open);
        assert!(view.midi_marquee.is_none());
    });
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, _| assert!(!view.midi_focus.is_focused(window)));
    })
    .expect("read editor focus");

    visual.simulate_keystrokes("right");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(!matches!(
            view.last_enqueued_edit,
            Some(Command::UpdateMidiNote { .. } | Command::UpdateMidiNotes { .. })
        ));
    });

    let toggle = visual
        .debug_bounds("clip-view-toggle")
        .expect("collapsed toggle");
    let toggle_point = point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(toggle_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let reopened_panel = visual
        .debug_bounds("clip-view")
        .expect("reopened detail panel");
    assert_eq!(reopened_panel.size.height, open_panel.size.height);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.selected_clip, Some(clip));
        assert_eq!(view.midi_selection.primary(), Some(note));
        assert!(view.clip_view_open);
    });
}

fn audio_detail_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<super::EditorView>,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let (project, track, _, clip) = clip_drag_ui_test::audio_project();
    let asset = project.clip(clip).expect("clip").asset();
    let peaks = Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            vec![
                [
                    CachedPeak {
                        min: -0.6,
                        max: 0.7
                    },
                    CachedPeak {
                        min: -0.4,
                        max: 0.5
                    },
                ];
                64
            ],
        )
        .expect("cached peaks"),
    );
    let (handle, editor) = clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
    editor.update(cx, |view, window_cx| {
        let mut waveforms = BTreeMap::new();
        waveforms.insert(asset, peaks);
        view.state = Some(Arc::new(ViewState {
            project: Arc::new(project),
            waveforms,
            midi_preview: Arc::new(Default::default()),
            timeline_frames: 384_000,
            can_undo: false,
            can_redo: false,
            revision: 1,
            unsaved: false,
            document_path: None,
        }));
        view.mode = EditorMode::Arrangement;
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        window_cx.notify();
    });
    (handle, editor, track, clip)
}

#[gpui::test]
fn selected_arrangement_audio_shows_left_controls_ruler_and_stereo_waveform(
    cx: &mut TestAppContext,
) {
    let (handle, _, _, _) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let controls = visual
        .debug_bounds("arrangement-audio-controls")
        .expect("audio controls column");
    let waveform = visual
        .debug_bounds("arrangement-audio-waveform")
        .expect("detailed waveform panel");
    let ruler = visual
        .debug_bounds("arrangement-audio-time-ruler")
        .expect("source time ruler");
    let play_start = visual
        .debug_bounds("audio-clip-playstart-value")
        .expect("actual play-start value");
    assert!(f32::from(controls.size.width) > 0.0);
    assert!(f32::from(waveform.size.width) > 0.0);
    assert!(f32::from(waveform.size.height) > 0.0);
    assert!(f32::from(ruler.size.width) > 0.0);
    assert!(f32::from(play_start.size.width) > 0.0);
    assert!(visual.debug_bounds("source-start-marker").is_some());
    assert!(visual.debug_bounds("play-start-marker").is_some());
}

#[gpui::test]
fn audio_detail_dispatches_canonical_edits_and_rejects_stale_or_pending_clicks(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, clip) = audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual
        .debug_bounds("audio-clip-position-1")
        .expect("move clip control");
    let point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::MoveClip { clip: actual, track: actual_track, start })
                if actual == clip && actual_track == track && start == 48
        ));
    });

    // The callback still owns the rendered revision. A newer project revision
    // must invalidate its captured control instead of overwriting that edit.
    editor.update(cx, |view, _| {
        view.last_enqueued_edit = None;
        let old = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
    });
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));

    editor.update(cx, |view, _| {
        let old = view.state.as_ref().expect("state");
        view.state = Some(Arc::new(ViewState {
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            midi_preview: Arc::clone(&old.midi_preview),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        view.worker_requests_pending = 1;
    });
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
}
