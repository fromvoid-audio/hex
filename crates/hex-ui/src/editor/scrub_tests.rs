use super::*;
use gpui::{point, px, size};

fn context() -> ScrubContext {
    ScrubContext {
        generation: 7,
        revision: 11,
        viewport: Viewport {
            start: 100,
            length: 1_000,
        },
        bounds: Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(100.0), px(24.0)),
        },
    }
}

#[test]
fn latest_target_replaces_moves_and_release_survives_an_in_flight_destination() {
    let mut gesture = ScrubGesture {
        context: context(),
        latest: Some(100),
        last_issued: None,
        released: false,
    };
    gesture.admitted(100);
    for frame in 101..1_000 {
        gesture.target(frame, false);
    }
    gesture.target(1_100, true);
    assert_eq!(gesture.latest, Some(1_100));
    assert_eq!(gesture.last_issued, Some(100));
    assert!(gesture.released);
    gesture.admitted(1_100);
    assert_eq!(gesture.latest, None);
    gesture.target(1_100, true);
    assert_eq!(
        gesture.latest, None,
        "the final already-admitted target needs no duplicate request"
    );
}

#[cfg(feature = "ui-test-support")]
mod ui {
    use super::*;
    use crate::editor::{PendingPlaybackAction, clip_drag_ui_test};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use hex_engine::{PlaybackAction, PlaybackCommand};
    use std::sync::Arc;

    fn editor(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        cx.update(crate::editor::bind_editor_keys);
        let (handle, editor) = clip_drag_ui_test::editor_window(
            cx,
            hex_project::Project::new(48_000).unwrap(),
            Arc::default(),
        );
        editor.update(cx, |view, cx| {
            // Keep a genuine transport acknowledgment gate closed while
            // testing pointer coalescing in the offline UI fixture.
            view.pending_seek_ticket = Some(1);
            cx.notify();
        });
        (handle, editor)
    }

    fn ruler(visual: &mut VisualTestContext) -> Bounds<Pixels> {
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        visual.debug_bounds("timeline-ruler").expect("ruler")
    }

    fn pointer(bounds: Bounds<Pixels>, fraction: f32) -> gpui::Point<Pixels> {
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.origin.y + bounds.size.height / 2.0,
        )
    }

    #[gpui::test]
    fn actual_ruler_down_move_up_retains_only_final_target_until_seek_gate_opens(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor) = editor(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let bounds = ruler(&mut visual);
        visual.simulate_mouse_down(
            pointer(bounds, 0.1),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.read_with(cx, |view, _| assert!(view.scrub.gesture.is_some()));
        for fraction in [0.2, 0.7, 0.4] {
            visual.simulate_mouse_move(
                pointer(bounds, fraction),
                MouseButton::Left,
                Modifiers::default(),
            );
        }
        visual.simulate_mouse_up(
            pointer(bounds, 0.8),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.update(cx, |view, _| {
            let gesture = view.scrub.gesture.unwrap();
            assert!(gesture.released);
            let expected = crate::editor::seek::pointer_to_frame(
                f32::from(pointer(bounds, 0.8).x),
                f32::from(bounds.origin.x),
                f32::from(bounds.size.width),
                gesture.context.viewport,
            )
            .unwrap();
            assert_eq!(gesture.latest, Some(expected));
            assert!(
                !view.tick_scrub(),
                "in-flight seek blocks the next admission"
            );
            assert!(
                !view.lose_scrub_pointer(),
                "hover after release must retain its final target"
            );
            assert_eq!(view.scrub.gesture.unwrap().latest, Some(expected));
            assert!(
                view.pending_audio_commands.is_empty(),
                "pointer moves do not enqueue commands"
            );
            assert_eq!(
                view.offline_cursor, 0,
                "the displayed cursor does not fake a transport acknowledgment"
            );
            view.pending_seek_ticket = None;
            for _ in 0..crate::editor::UI_REQUEST_CAPACITY {
                view.pending_audio_commands
                    .push_back(PendingPlaybackAction {
                        action: PlaybackAction::Transport(PlaybackCommand::Pause),
                        generation: view.project_generation,
                        revision: None,
                        intent_token: None,
                    });
            }
            assert!(
                !view.tick_scrub(),
                "queue pressure retains the final destination"
            );
            assert_eq!(view.scrub.gesture.unwrap().latest, Some(expected));
            view.pending_audio_commands.clear();
            assert!(
                view.tick_scrub(),
                "one unsupported offline seek reports its existing error"
            );
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains("audio output"))
            );
            assert!(view.scrub.gesture.is_none());
            assert!(
                !view.tick_scrub(),
                "the refusal cannot repeat every timer tick"
            );
        });
    }

    #[gpui::test]
    fn stale_viewport_revision_and_geometry_cancel_future_scrub_targets(cx: &mut TestAppContext) {
        let (handle, editor) = editor(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let bounds = ruler(&mut visual);
        for change in 0..3 {
            editor.update(cx, |view, _| {
                let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
                view.update_scrub_geometry(bounds);
                assert!(view.begin_scrub(f32::from(pointer(bounds, 0.2).x), viewport, bounds));
                match change {
                    0 => {
                        view.viewport = Some(Viewport {
                            start: 1,
                            length: viewport.length / 2,
                        })
                    }
                    1 => view.project_generation += 1,
                    _ => view.update_scrub_geometry(Bounds {
                        origin: point(bounds.origin.x + px(1.0), bounds.origin.y),
                        ..bounds
                    }),
                }
                view.tick_scrub();
                assert!(view.scrub.gesture.is_none());
            });
        }
        editor.update(cx, |view, _| {
            view.update_scrub_geometry(bounds);
            let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
            assert!(view.begin_scrub(f32::from(pointer(bounds, 0.3).x), viewport, bounds));
            let state = view.state.as_ref().unwrap();
            view.state = Some(Arc::new(hex_session::service::ViewState {
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
                revision: state.revision + 1,
                unsaved: state.unsaved,
                document_path: state.document_path.clone(),
            }));
            view.tick_scrub();
            assert!(view.scrub.gesture.is_none());
        });
    }

    #[gpui::test]
    fn escape_stop_and_focus_loss_cancel_future_targets_without_clearing_admitted_seek(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor) = editor(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let bounds = ruler(&mut visual);
        visual.simulate_mouse_down(
            pointer(bounds, 0.3),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_keystrokes("escape");
        editor.read_with(cx, |view, _| assert!(view.scrub.gesture.is_none()));
        visual.simulate_mouse_up(
            pointer(bounds, 0.3),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.update(cx, |view, cx| {
            view.pending_seek_ticket = Some(1);
            cx.notify();
        });
        visual.simulate_mouse_down(
            pointer(bounds, 0.4),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.update(|window, cx| {
            editor.update(cx, |view, _| view.midi_focus.focus(window));
        });
        // GPUI publishes focus-path changes during its next draw phase.
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.scrub.gesture.is_none());
            assert_eq!(
                view.pending_seek_ticket,
                Some(1),
                "focus loss preserves an admitted seek's acknowledgment"
            );
        });
        editor.update(cx, |view, _| {
            let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
            assert!(view.begin_scrub(f32::from(pointer(bounds, 0.5).x), viewport, bounds));
            view.stop();
            assert!(view.scrub.gesture.is_none());
            assert_eq!(
                view.pending_seek_ticket, None,
                "Stop owns the existing cancellation lifecycle"
            );
        });
    }
}
