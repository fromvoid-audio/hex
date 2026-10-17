use super::*;
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
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

#[gpui::test]
fn copying_after_an_outer_cut_uses_only_the_visible_inner_event(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
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
        let hex_project::Created::Clip(right) = canonical
            .execute(Command::SplitClip { clip, frame: 48000 })
            .unwrap()
        else {
            panic!("fixture")
        };
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
        view.selected_clip = Some(right);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.center().y + px(12.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-c");
    editor.read_with(cx, |view, _| {
        let clipboard = view.audio_event_editor.clipboard.as_ref().unwrap();
        assert_eq!(clipboard.length, 48000);
        assert_eq!(clipboard.events[0].source_start, 48000);
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn closing_events_releases_focus_and_gesture(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(body.center().x, body.center().y + px(12.0));
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    let toggle = visual.debug_bounds("clip-view-toggle").unwrap().center();
    visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(!view.clip_view_open);
            assert!(
                !view.audio_event_focus.is_focused(window),
                "closed events cannot retain shortcut focus"
            );
            assert!(view.focus.is_focused(window));
            assert!(view.audio_event_editor.gesture.is_none());
        })
    })
    .unwrap();
}

#[gpui::test]
fn cropped_event_drag_range_move_and_nudge_stay_inside_visible_container(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let state = view.state.as_ref().unwrap();
        let mut canonical = hex_project::Editor::new((*state.project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 48000 },
            })
            .unwrap();
        let hex_project::Created::Clip(middle) = canonical
            .execute(Command::SplitClip { clip, frame: 24000 })
            .unwrap()
        else {
            panic!("split")
        };
        canonical
            .execute(Command::SplitClip {
                clip: middle,
                frame: 72000,
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
        view.selected_clip = Some(middle);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    editor.update(cx, |view, _| {
        view.audio_event_editor.grid = SnapGrid::Off;
        let (_, root, _) = view.event_context().unwrap();
        let events = view
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(view.selected_clip.unwrap())
            .unwrap()
            .editable_audio_events()
            .unwrap();
        let first = *events
            .events
            .iter()
            .find(|e| e.start == root.view_start)
            .unwrap();
        let last = *events
            .events
            .iter()
            .find(|e| e.end == root.view_end)
            .unwrap();
        let left = f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width);
        for (kind, anchor, origin, x) in [
            (Kind::Move(first), 30000, left + width * 0.125, left),
            (Kind::Move(last), 60000, left + width * 0.75, left + width),
            (
                Kind::RangeMove {
                    start: root.view_start,
                    end: root.view_end,
                },
                48000,
                left + width * 0.5,
                left,
            ),
        ] {
            view.audio_event_editor.gesture = Some(Gesture {
                anchor,
                pointer_x: origin,
                origin_x: origin,
                moved: false,
                kind,
                operation: None,
            });
            view.event_update_pointer(x, false);
            let operation = view
                .audio_event_editor
                .gesture
                .as_ref()
                .unwrap()
                .operation
                .as_ref()
                .unwrap();
            let (start, span) = match operation {
                AudioEventEdit::Move { event, start } => (
                    *start,
                    events
                        .events
                        .iter()
                        .find(|e| e.id == *event)
                        .unwrap()
                        .length(),
                ),
                AudioEventEdit::MoveRange {
                    start,
                    end,
                    destination,
                } => (*destination, end - start),
                _ => panic!("move"),
            };
            assert!(
                start >= root.view_start && start + span <= root.view_end,
                "move escaped visible crop: {operation:?}"
            );
        }
        view.audio_event_editor.gesture = None;
        view.audio_event_editor.selected = Some(last.id);
        view.event_nudge(true, false, false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Move { start: 48000, .. },
                    ..
                })
            ),
            "nudge escaped crop: {:?}",
            view.last_enqueued_edit
        );
    });
}

#[test]
fn moved_and_pasted_waveform_positions_read_event_composed_pcm() {
    use std::{collections::BTreeMap, sync::Arc};
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "wave".into(),
            path: "wave.wav".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 20 },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: 40,
            },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Paste {
                at: 0,
                length: 10,
                events: vec![AudioEvent {
                    id: 0,
                    start: 0,
                    end: 10,
                    source_start: 70,
                }],
            },
        })
        .unwrap();
    let frames: Vec<_> = (0..100).map(|i| [i as f32 / 100.0; 2]).collect();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48000, frames.clone()).unwrap(),
    )]);
    let prepared = hex_engine::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
    )
    .unwrap();
    let pcm = prepared.arrangement().audio_clip_views()[0].audio.frames();
    let events = editor.project().clip(clip).unwrap().audio_events();
    for event in &events.events {
        let position = view::prepared_waveform_position(*event, *event, event.start).unwrap();
        assert_eq!(pcm[position as usize], frames[event.source_start as usize]);
    }
    let original = *events.events.iter().find(|e| e.id == 1).unwrap();
    let preview = AudioEvent {
        start: 60,
        end: 80,
        ..original
    };
    let position = view::prepared_waveform_position(preview, original, preview.start).unwrap();
    assert_eq!(position, 40);
    assert_eq!(pcm[position as usize], frames[0]);
    assert_ne!(
        pcm[original.source_start as usize], frames[0],
        "reading raw source_start in composed PCM paints pasted content instead"
    );
}

fn replace_event_project(view: &mut EditorView, project: hex_project::Project, revision: u64) {
    let state = view.state.as_ref().unwrap();
    view.state = Some(std::sync::Arc::new(hex_session::service::ViewState {
        canonical_audio_source: None,
        project: std::sync::Arc::new(project),
        waveforms: state.waveforms.clone(),
        audio_clip_waveforms: Default::default(),
        midi_preview: state.midi_preview.clone(),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: true,
        can_redo: false,
        revision,
        unsaved: true,
        document_path: None,
    }));
}

#[gpui::test]
fn outer_trim_clamps_stale_cursor_range_and_hidden_selection(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 2);
        cx.notify();
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.audio_event_editor.range = Some((0, 24000));
        view.audio_event_editor.cursor = 0;
        view.audio_event_editor.selected = Some(1);
        view.event_copy(false);
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        let frame = canonical.project().clip(clip).unwrap().start() + 48000;
        canonical
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame,
            })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 3);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        let (_, root, _) = view.event_context().unwrap();
        assert_eq!(root.view_start, 48000);
        assert_eq!(
            view.audio_event_editor.cursor, root.view_start,
            "cursor must follow crop bounds"
        );
        assert!(
            view.audio_event_editor.range.is_none(),
            "hidden time range must be cleared"
        );
        assert!(
            view.audio_event_editor.selected.is_none(),
            "hidden event must be deselected"
        );
        view.event_delete();
        assert!(
            view.last_enqueued_edit.is_none(),
            "delete must not edit an old hidden selection"
        );
        view.event_paste(false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Paste { at: 48000, .. },
                    ..
                })
            ),
            "paste must use visible cursor"
        );
    });
}

#[gpui::test]
fn duplicate_and_clipboard_paste_refuse_beyond_visible_child(cx: &mut TestAppContext) {
    let (handle, editor, _, clip) = super::super::clip_view_ui_test::audio_detail_fixture(cx);
    editor.update(cx, |view, cx| {
        let mut canonical =
            hex_project::Editor::new((*view.state.as_ref().unwrap().project).clone(), 8).unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 24000 },
            })
            .unwrap();
        canonical
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Delete {
                    start: 60000,
                    end: 96000,
                },
            })
            .unwrap();
        canonical
            .execute(Command::SplitClip { clip, frame: 48000 })
            .unwrap();
        replace_event_project(view, canonical.project().clone(), 2);
        cx.notify();
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.audio_event_editor.range = Some((24000, 48000));
        view.event_copy(false);
        view.event_duplicate();
        assert!(
            view.last_enqueued_edit.is_none(),
            "duplicate at crop end must not overwrite hidden audio"
        );
        assert!(view.error.is_some(), "refusal must be visible");
        view.audio_event_editor.cursor = 48000;
        for insert in [false, true] {
            view.error = None;
            view.event_paste(insert);
            assert!(
                view.last_enqueued_edit.is_none(),
                "clipboard cannot write past crop"
            );
            assert!(view.error.is_some());
        }
        view.audio_event_editor.cursor = 0;
        view.error = None;
        view.event_paste(true);
        assert!(
            view.last_enqueued_edit.is_none(),
            "insert must not shift a tail into hidden root space"
        );
        assert!(view.error.is_some());
        view.audio_event_editor.cursor = 24000;
        view.event_paste(false);
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::Paste {
                        at: 24000,
                        length: 24000,
                        ..
                    },
                    ..
                })
            ),
            "a fitting visible paste must still work"
        );
    });
}
