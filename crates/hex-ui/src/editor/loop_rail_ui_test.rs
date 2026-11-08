use super::{EditorView, Viewport};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, Editor, LoopRegion};
use hex_session::service::{Event, Request, Service};
use std::{sync::Arc, thread, time::Duration};

const TIMELINE_FRAMES: u64 = 384_000;

fn loop_editor(
    cx: &mut TestAppContext,
    initial: Option<LoopRegion>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    loop_editor_with_tracks(cx, initial, 1)
}

fn loop_editor_with_tracks(
    cx: &mut TestAppContext,
    initial: Option<LoopRegion>,
    tracks: usize,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    cx.update(super::bind_editor_keys);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    fn next(service: &Service) -> Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "next: hang guard expired"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    let mut event = next(&service);
    if tracks > 0 {
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "loop fixture".into(),
            }))
            .unwrap();
        let first = next(&service);
        let track = first.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(12).unwrap(),
            }))
            .unwrap();
        event = next(&service);
        for _ in 1..tracks {
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "other track".into(),
                }))
                .unwrap();
            event = next(&service);
        }
    }
    if let Some(region) = initial {
        service
            .submit(Request::Edit(Command::SetLoopRegion {
                start: region.start(),
                end: region.end(),
                enabled: region.enabled(),
            }))
            .unwrap();
        event = next(&service);
    }
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline loop rail test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(event.state);
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    editor.update(cx, |view, window_cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES,
        });
        window_cx.notify();
    });
    (handle, editor)
}

fn rail_point(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail bounds");
    point(
        rail.origin.x + rail.size.width * fraction,
        rail.origin.y + rail.size.height / 2.0,
    )
}

fn wait_for_loop(
    cx: &mut TestAppContext,
    handle: gpui::AnyWindowHandle,
    editor: &gpui::Entity<EditorView>,
    expected: Option<(u64, u64, bool)>,
) {
    for _ in 0..500 {
        cx.update_window(handle, |_, _, cx| {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
        })
        .expect("loop editor window");
        if editor.read_with(cx, |view, _| {
            let Some(state) = view.state.as_ref() else {
                return false;
            };
            state
                .project
                .loop_region()
                .map(|region| (region.start(), region.end(), region.enabled()))
                == expected
        }) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    let observed = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref();
        (
            state.and_then(|state| state.project.loop_region()),
            state.map(|state| state.revision),
            view.error.clone(),
            view.status.clone(),
            view.loop_rail_gesture.is_some(),
        )
    });
    panic!("loop command did not publish expected project revision: {observed:?}");
}

#[gpui::test]
fn loop_rail_paint_previews_then_commits_adaptive_grid_range_disabled(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let map = visual
        .debug_bounds("main-arrangement-map")
        .expect("bottom Main miniature");
    let ruler = visual
        .debug_bounds("timeline-ruler")
        .expect("ruler timeline");
    assert_eq!(rail.origin.x, map.origin.x);
    assert_eq!(rail.size.width, map.size.width);
    assert_eq!(rail.origin.x, ruler.origin.x);

    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
        assert!(view.loop_rail_gesture.is_some(), "drag owns the rail");
    });
    assert!(visual.debug_bounds("loop-rail-range").is_some());

    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 192_000, false)));
    let viewport = editor.read_with(cx, |view, _| view.viewport);
    assert_eq!(
        viewport,
        Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES
        })
    );
}

#[gpui::test]
fn command_drag_uses_free_frames_and_zoomed_panned_coordinates(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let viewport = Viewport {
        start: 48_000,
        length: 192_000,
    };
    editor.update(cx, |view, window_cx| {
        view.viewport = Some(viewport);
        window_cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let start = point(
        rail.origin.x + rail.size.width * 0.123,
        rail.origin.y + rail.size.height / 2.0,
    );
    let end = point(
        rail.origin.x + rail.size.width * 0.377,
        rail.origin.y + rail.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let expected = editor.read_with(cx, |view, _| {
        let lane = view.arrangement_bounds.expect("arrangement bounds");
        let left = lane.left;
        let width = lane.width - view.track_header_width();
        let state = view.state.as_ref().unwrap();
        let start_frame =
            super::seek::pointer_to_frame(f32::from(start.x), left, width, viewport).unwrap();
        let end_frame =
            super::seek::pointer_to_frame(f32::from(end.x), left, width, viewport).unwrap();
        assert_eq!(state.timeline_frames, TIMELINE_FRAMES);
        (start_frame, end_frame)
    });
    visual.simulate_mouse_down(start, MouseButton::Left, command);
    visual.simulate_mouse_move(end, MouseButton::Left, command);
    visual.simulate_mouse_up(end, MouseButton::Left, command);
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((expected.0, expected.1, false)));
    assert_ne!(
        expected.0 % 6_000,
        0,
        "fixture starts off the adaptive grid"
    );
    assert_ne!(expected.1 % 6_000, 0, "fixture ends off the adaptive grid");
}

#[gpui::test]
fn loop_rail_drag_continues_outside_rail_without_starting_lane_selection(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("arrangement-loop-rail")
        .expect("loop rail");
    let start = rail_point(&mut visual, 0.25);
    let outside = point(rail.origin.x + rail.size.width + px(24.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.loop_rail_gesture.is_some(),
            "captured loop gesture continues"
        );
        assert!(
            view.time_selection_gesture.is_none(),
            "lane selection stays idle"
        );
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 384_000, false)));
}

#[gpui::test]
fn loop_rail_command_click_activates_resizable_ends_preserve_enable_and_toolbar_disables(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = loop_editor(cx, Some(LoopRegion::new(48_000, 192_000, false).unwrap()));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    assert_eq!(
        visual.rendered_background("loop-rail-range"),
        Some(
            gpui::Hsla::from(gpui::rgb(crate::theme::color(
                crate::theme::PaletteRole::Muted
            )))
            .alpha(0.62)
            .into()
        )
    );
    let midpoint = rail_point(&mut visual, 0.25);
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(midpoint, MouseButton::Left, command);
    visual.simulate_mouse_up(midpoint, MouseButton::Left, command);
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((48_000, 192_000, true)));

    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("loop-rail-range"),
        Some(
            gpui::Hsla::from(gpui::rgb(crate::theme::color(
                crate::theme::PaletteRole::Text
            )))
            .alpha(0.94)
            .into()
        )
    );
    let start = rail_point(&mut visual, 0.125);
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeLeftRight),
        "loop endpoints expose a resize cursor"
    );
    let new_start = rail_point(&mut visual, 0.25);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(new_start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(new_start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 192_000, true)));

    let end = rail_point(&mut visual, 0.5);
    let new_end = rail_point(&mut visual, 0.75);
    visual.simulate_mouse_move(end, None, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(new_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(new_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 288_000, true)));

    let toggle = visual
        .debug_bounds("transport-loop-toggle")
        .expect("rendered loop toolbar control");
    visual.simulate_mouse_down(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(toggle.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    wait_for_loop(cx, handle, &editor, Some((96_000, 288_000, false)));
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background("loop-rail-range"),
        Some(
            gpui::Hsla::from(gpui::rgb(crate::theme::color(
                crate::theme::PaletteRole::Muted
            )))
            .alpha(0.62)
            .into()
        )
    );
}

#[gpui::test]
fn stale_loop_state_cancels_preview_without_overwriting_the_new_range(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, window_cx| {
        let old = view.state.as_ref().unwrap();
        let mut history = Editor::new((*old.project).clone(), 8).expect("project history");
        history
            .execute(Command::SetLoopRegion {
                start: 144_000,
                end: 240_000,
                enabled: true,
            })
            .expect("external loop edit");
        view.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(history.project().clone()),
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        window_cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.loop_rail_gesture.is_none(), "stale preview is retired");
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(LoopRegion::new(144_000, 240_000, true).unwrap()),
            "releasing the stale gesture must preserve the newer loop range"
        );
    });

    let start = rail_point(&mut visual, 0.7);
    let end = rail_point(&mut visual, 0.9);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, window_cx| {
        let old = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::clone(&old.project),
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        window_cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.loop_rail_gesture.is_none(),
            "revision change retires preview"
        );
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(LoopRegion::new(144_000, 240_000, true).unwrap())
        );
    });
}

#[gpui::test]
fn shift_click_extent_loop_bar_uses_exact_unquantized_bounds_without_transport_or_history(
    cx: &mut TestAppContext,
) {
    for (enabled, edge) in [(false, false), (true, false), (false, true), (true, true)] {
        let region = LoopRegion::new(12_345, 98_765, enabled).unwrap();
        let (handle, editor) = loop_editor_with_tracks(cx, Some(region), 2);
        editor.update(cx, |view, cx| {
            view.viewport = Some(Viewport {
                start: 30_000,
                length: 120_000,
            });
            let first = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.time_selection = Some(super::time_selection::TimeSelection {
                anchor_track: first,
                end_track: first,
                anchor_frame: 111,
                end_frame: 222,
            });
            view.time_selection_project_scope = false;
            view.last_enqueued_edit = None;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let bar = visual.debug_bounds("loop-rail-range").unwrap();
        let at = if edge {
            point(bar.right() - px(1.0), bar.center().y)
        } else {
            bar.center()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, shift);
        visual.simulate_mouse_up(at, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let range = view
                .normalized_time_selection()
                .expect("shift-click creates a project range");
            assert_eq!(
                (range.start(), range.end()),
                (region.start(), region.end()),
                "shift-click loop bar must select its captured canonical extent"
            );
            assert_eq!(
                range.tracks(),
                before
                    .project
                    .tracks()
                    .iter()
                    .map(|track| track.id())
                    .collect::<Vec<_>>()
            );
            assert!(view.time_selection_project_scope);
            assert_eq!(view.state.as_ref().unwrap().project, before.project);
            assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(
                (
                    view.state.as_ref().unwrap().can_undo,
                    view.state.as_ref().unwrap().can_redo
                ),
                (before.can_undo, before.can_redo)
            );
            assert!(view.last_enqueued_edit.is_none());
            assert!(
                view.pending_audio_commands.is_empty()
                    && view.prepared_seek.is_none()
                    && view.latched_seek_intent.is_none()
            );
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.loop_rail_gesture.is_none());
        });
    }
}

#[gpui::test]
fn shift_click_extent_keeps_shift_drag_steps_and_one_loop_history_edge(cx: &mut TestAppContext) {
    let original = LoopRegion::new(96_000, 144_000, true).unwrap();
    let (handle, editor) = loop_editor(cx, Some(original));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bar = visual.debug_bounds("loop-rail-range").unwrap();
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    let length = editor.read_with(cx, |view, _| {
        view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
            .length
    });
    let down = bar.center();
    // Move a little more than two loop lengths; Shift keeps whole-length steps.
    let to = point(
        down.x + rail.size.width * (105_600.0 / length as f32),
        down.y,
    );
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, shift);
    visual.simulate_mouse_move(to, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.time_selection.is_none(),
            "drag must not turn into a click selection"
        )
    });
    visual.simulate_mouse_up(to, MouseButton::Left, shift);
    wait_for_loop(cx, handle, &editor, Some((192_000, 240_000, true)));
    editor.read_with(cx, |view, _| assert!(view.time_selection.is_none()));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    wait_for_loop(cx, handle, &editor, Some((96_000, 144_000, true)));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    wait_for_loop(cx, handle, &editor, Some((192_000, 240_000, true)));
}

#[gpui::test]
fn shift_click_extent_refuses_empty_missing_stale_and_owned_context_without_side_effects(
    cx: &mut TestAppContext,
) {
    let region = LoopRegion::new(12_345, 98_765, false).unwrap();
    for (tracks, initial) in [(0, Some(region)), (1, None)] {
        let (handle, editor) = loop_editor_with_tracks(cx, initial, tracks);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let at = if initial.is_some() {
            visual.debug_bounds("loop-rail-range").unwrap().center()
        } else {
            rail_point(&mut visual, 0.5)
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, shift);
        visual.simulate_mouse_up(at, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.normalized_time_selection().is_none());
            assert_eq!(view.state.as_ref().unwrap().project, before.project);
            assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
            assert!(view.pending_audio_commands.is_empty());
            assert_eq!(view.worker_requests_pending, 0);
        });
    }
    let (handle, editor) = loop_editor_with_tracks(cx, Some(region), 2);
    VisualTestContext::from_window(handle, cx).run_until_parked();
    editor.update(cx, |view, _| {
        let state = view.state.as_ref().unwrap().clone();
        let first = state.project.tracks()[0].id();
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: first,
            end_track: first,
            anchor_frame: 111,
            end_frame: 222,
        });
        let before = (
            view.time_selection,
            view.time_selection_project_scope,
            view.selection_intent_epoch,
        );
        let generation = view.project_generation;
        for (start, end, captured_generation, captured_revision) in [
            (1, 1, generation, state.revision),
            (2, 1, generation, state.revision),
            (1, 2, generation + 1, state.revision),
            (1, 2, generation, state.revision + 1),
        ] {
            assert!(!view.select_project_time_range(
                start,
                end,
                captured_generation,
                captured_revision
            ));
            assert_eq!(
                (
                    view.time_selection,
                    view.time_selection_project_scope,
                    view.selection_intent_epoch
                ),
                before
            );
        }
        view.save_dialog_open = true;
        assert!(!view.select_project_time_range(1, 2, generation, state.revision));
        view.save_dialog_open = false;
        assert_eq!(
            (
                view.time_selection,
                view.time_selection_project_scope,
                view.selection_intent_epoch
            ),
            before
        );
        assert_eq!(view.state.as_ref().unwrap().project, state.project);
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn shift_click_extent_command_chord_preserves_the_existing_loop_toggle(cx: &mut TestAppContext) {
    let (handle, editor) = loop_editor(cx, Some(LoopRegion::new(96_000, 144_000, false).unwrap()));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let at = visual.debug_bounds("loop-rail-range").unwrap().center();
    let chord = Modifiers {
        shift: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(at, MouseButton::Left, chord);
    visual.simulate_mouse_up(at, MouseButton::Left, chord);
    wait_for_loop(cx, handle, &editor, Some((96_000, 144_000, true)));
    editor.read_with(cx, |view, _| assert!(view.time_selection.is_none()));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    wait_for_loop(cx, handle, &editor, Some((96_000, 144_000, false)));
}
