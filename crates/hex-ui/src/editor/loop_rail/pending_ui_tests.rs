//! Real pointer scenes across a held, genuine Service response.
use super::*;
use crate::editor::test_environment::Environment;
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext, point, size};
use hex_project::Command;
use hex_session::service::{Event, Request, Service};
use std::{sync::Arc, time::Instant};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let ready = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.state.is_some() && !v.edit_pending()
        });
        visual.run_until_parked();
        if ready {
            return;
        }
        assert!(Instant::now() < deadline, "actual Service completion");
        std::thread::yield_now();
    }
}
fn fixture(
    cx: &mut TestAppContext,
    old: LoopRegion,
) -> (Arc<Environment>, Entity<EditorView>, VisualTestContext) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 4_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1400.), px(800.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "source".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 0,
            length: 384_000,
        });
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: old.start(),
            end: old.end(),
            enabled: old.enabled()
        })));
        cx.notify();
    });
    pump(&editor, &mut visual);
    (environment, editor, visual)
}
fn rail_point(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    point(rail.left() + rail.size.width * fraction, rail.center().y)
}
fn hold_real_response(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Some(event) = editor.update(&mut *visual, |v, _| v.service.try_event().ok()) {
            return event;
        }
        assert!(Instant::now() < deadline, "real worker response");
        std::thread::yield_now();
    }
}
fn painted_range(visual: &mut VisualTestContext) -> Bounds<gpui::Pixels> {
    let bounds = visual
        .debug_bounds("loop-rail-range")
        .expect("rendered loop geometry");
    assert!(
        !visual
            .rendered_quad_paint_layers("loop-rail-range", false)
            .is_empty(),
        "completed scene must contain the actual filled range quad"
    );
    bounds
}
fn assert_loop_paint(visual: &mut VisualTestContext, enabled: Option<bool>, message: &str) {
    let expected_fill: Option<gpui::Background> = enabled.map(|enabled| {
        let (role, opacity) = if enabled {
            (theme::PaletteRole::Text, 0.94)
        } else {
            (theme::PaletteRole::Muted, 0.62)
        };
        gpui::Hsla::from(rgb(theme::color(role)))
            .alpha(opacity)
            .into()
    });
    assert_eq!(
        visual.rendered_background("loop-rail-range"),
        expected_fill,
        "{message}"
    );
    if enabled.is_some() {
        let _ = painted_range(visual);
    } else {
        assert!(
            visual.debug_bounds("loop-rail-range").is_none(),
            "{message}"
        );
        assert!(
            visual.debug_bounds("loop-guide-start").is_none(),
            "{message}"
        );
        assert!(visual.debug_bounds("loop-guide-end").is_none(), "{message}");
    }
    let button_role = if enabled == Some(true) {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Muted
    };
    assert!(
        visual
            .rendered_monochrome_colors("transport-loop-toggle")
            .contains(&rgb(theme::color(button_role)).into()),
        "{message}"
    );
}
fn painted_guides(visual: &mut VisualTestContext) -> [Bounds<gpui::Pixels>; 2] {
    ["loop-guide-start", "loop-guide-end"].map(|name| {
        let bounds = visual.debug_bounds(name).expect("rendered loop guide");
        assert!(!visual.rendered_quad_paint_layers(name, false).is_empty());
        bounds
    })
}

#[gpui::test]
fn toggles_display_the_requested_state_while_the_real_response_is_held(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, false).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let bounds = painted_range(&mut visual);
    for (index, enabled) in [true, false, true, false, true].into_iter().enumerate() {
        match index {
            0 | 1 => {
                let button = visual.debug_bounds("transport-loop-toggle").unwrap();
                visual.simulate_click(button.center(), Modifiers::default());
            }
            2 => visual.simulate_keystrokes("cmd-l"),
            3 => visual.simulate_click(
                bounds.center(),
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            ),
            _ => visual.simulate_keystrokes("l"),
        }
        let response = hold_real_response(&editor, &mut visual);
        assert!(response.request_complete && response.error.is_none());
        let expected = LoopRegion::new(old.start(), old.end(), enabled).unwrap();
        assert_eq!(response.state.project.loop_region(), Some(expected));
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            assert_ne!(state.project.loop_region(), Some(expected));
        });
        let message = if index == 0 {
            "held toggle must repaint the loop range"
        } else {
            "held toggle and response must repaint both loop controls"
        };
        assert_loop_paint(&mut visual, Some(enabled), message);
        install(&editor, &mut visual, response);
        assert_loop_paint(&mut visual, Some(enabled), "installed toggle repaint");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.loop_region(),
                Some(expected)
            );
            assert!(v.pending_loop.is_none());
        });
    }

    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    assert_loop_paint(&mut visual, Some(false), "undo toggle repaint");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.loop_region(), Some(old));
    });

    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_loop_paint(&mut visual, Some(true), "redo toggle repaint");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.loop_region(),
            Some(LoopRegion::new(old.start(), old.end(), true).unwrap())
        );
    });
}

#[gpui::test]
fn stale_toolbar_toggle_refusal_preserves_clear_history_and_repaints(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, false).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let original_range = painted_range(&mut visual);
    let old_snapshot =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());

    // Change the Service's canonical loop without delivering its successful
    // response, leaving the view stale for the actual toolbar toggle.
    editor.update(&mut visual, |v, _| {
        v.service
            .submit(Request::Edit(Command::ClearLoopRegion))
            .unwrap();
    });
    let cleared = hold_real_response(&editor, &mut visual);
    assert!(cleared.request_complete && cleared.error.is_none());
    assert_eq!(cleared.state.project.loop_region(), None);
    assert!(cleared.state.can_undo);

    let button = visual.debug_bounds("transport-loop-toggle").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    let refused = hold_real_response(&editor, &mut visual);
    assert!(refused.request_complete && refused.error.is_some());
    assert_eq!(
        refused.state.project.snapshot(),
        cleared.state.project.snapshot()
    );
    assert_eq!(refused.state.can_undo, cleared.state.can_undo);
    assert_eq!(refused.state.can_redo, cleared.state.can_redo);
    assert_eq!(refused.state.revision, cleared.state.revision);
    assert_eq!(
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
        old_snapshot,
        "the held failure must not install canonical state early"
    );

    visual.run_until_parked();
    assert_loop_paint(&mut visual, Some(true), "held refused toggle repaint");

    // The earlier clear terminal is used as the history oracle; install the
    // genuine failed toggle terminal, which carries that same canonical state.
    install(&editor, &mut visual, refused);
    assert_loop_paint(&mut visual, None, "failed toggle installed repaint");
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_loop.is_none());
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.loop_region(), None);
        assert_eq!(v.state.as_ref().unwrap().can_undo, cleared.state.can_undo);
        assert_eq!(v.state.as_ref().unwrap().can_redo, cleared.state.can_redo);
    });

    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    assert_eq!(painted_range(&mut visual), original_range);
    assert_loop_paint(
        &mut visual,
        Some(false),
        "undo failed toggle history repaint",
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.loop_region(), Some(old));
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_loop_paint(&mut visual, None, "redo failed toggle history repaint");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.loop_region(), None);
    });
}

#[gpui::test]
fn replacement_loop_stays_painted_until_its_real_response_installs(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, true).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let old_paint = painted_range(&mut visual);
    let old_guides = painted_guides(&mut visual);
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    for (guide, frame) in old_guides.iter().zip([24_000.0, 48_000.0]) {
        let expected = rail.left() + rail.size.width * (frame / 384_000.0);
        assert!((f32::from(guide.left() - expected)).abs() < 1.0);
    }
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 0,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let zoomed = painted_guides(&mut visual);
    for (guide, frame) in zoomed.iter().zip([24_000.0, 48_000.0]) {
        let expected = rail.left() + rail.size.width * (frame / 192_000.0);
        assert!((f32::from(guide.left() - expected)).abs() < 1.0);
    }
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 0,
            length: 384_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = painted_range(&mut visual);
    let held_guides = painted_guides(&mut visual);
    assert!((f32::from(held_guides[0].left() - held.left())).abs() < 1.0);
    assert!((f32::from(held_guides[1].left() - held.right())).abs() < 1.0);
    assert_ne!(held, old_paint);
    let expected = editor.read_with(&visual, |v, _| {
        v.loop_rail_gesture.unwrap().preview.unwrap()
    });
    assert_eq!(expected, LoopRegion::new(96_000, 192_000, false).unwrap());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    // Delay UI delivery of a real worker Event, without faking acceptance or state.
    let response = hold_real_response(&editor, &mut visual);
    assert!(
        response.request_complete && response.error.is_none(),
        "{:?}",
        response.error
    );
    assert_eq!(response.state.project.loop_region(), Some(expected));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.run_until_parked();
    assert_eq!(
        painted_range(&mut visual),
        held,
        "accepted replacement must not flash the old canonical bar before its actual response"
    );
    assert_eq!(painted_guides(&mut visual), held_guides);
    editor.update(&mut visual, |v, cx| {
        v.receive(response);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(painted_range(&mut visual), held);
    assert_eq!(painted_guides(&mut visual), held_guides);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.loop_region(),
            Some(expected)
        );
        assert!(v.loop_rail_gesture.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    assert_eq!(painted_range(&mut visual), old_paint);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_eq!(painted_range(&mut visual), held);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ClearLoopRegion)))
    });
    pump(&editor, &mut visual);
    assert!(visual.debug_bounds("loop-guide-start").is_none());
    assert!(visual.debug_bounds("loop-guide-end").is_none());
}

fn install(editor: &Entity<EditorView>, visual: &mut VisualTestContext, event: Event) {
    editor.update(&mut *visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
}
fn begin_paint(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> Bounds<gpui::Pixels> {
    let start = rail_point(visual, 0.25);
    let end = rail_point(visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |v, _| assert!(v.loop_rail_gesture.is_some()));
    painted_range(visual)
}
#[gpui::test]
fn earlier_success_and_refusal_ack_do_not_retire_loop_replacement(cx: &mut TestAppContext) {
    for fail in [false, true] {
        let old = LoopRegion::new(24_000, 48_000, true).unwrap();
        let (_environment, editor, mut visual) = fixture(cx, old);
        let held = begin_paint(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::MasterGain {
                gain: if fail { f32::NAN } else { 0.5 }
            })))
        });
        let end = rail_point(&mut visual, 0.5);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let earlier = hold_real_response(&editor, &mut visual);
        let own = hold_real_response(&editor, &mut visual);
        assert_eq!(earlier.error.is_some(), fail);
        assert_eq!(earlier.state.project.loop_region(), Some(old));
        assert!(own.error.is_none(), "{:?}", own.error);
        let before_own = earlier.state.project.snapshot();
        install(&editor, &mut visual, earlier);
        assert_eq!(painted_range(&mut visual), held);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.pending_loop.unwrap().remaining, Some(1))
        });
        install(&editor, &mut visual, own);
        assert_eq!(painted_range(&mut visual), held);
        editor.read_with(&visual, |v, _| assert!(v.pending_loop.is_none()));
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_own)
        });
    }
}
#[gpui::test]
fn deferred_loop_is_stamped_only_when_the_real_request_is_submitted(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, true).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let held = begin_paint(&editor, &mut visual);
    // Existing UI backpressure queue; both entries will execute on real Service.
    editor.update(&mut visual, |v, _| {
        v.pending_requests
            .push_back(Request::Edit(Command::MasterGain { gain: 0.75 }))
    });
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.pending_requests.len(), 2);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_loop.unwrap().remaining.is_none());
    });
    assert_eq!(painted_range(&mut visual), held);
    editor.update(&mut visual, |v, cx| {
        v.tick(cx);
        cx.notify();
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.pending_loop.unwrap().remaining, Some(2))
    });
    let earlier = hold_real_response(&editor, &mut visual);
    let own = hold_real_response(&editor, &mut visual);
    assert!(earlier.error.is_none() && own.error.is_none());
    install(&editor, &mut visual, earlier);
    assert_eq!(painted_range(&mut visual), held);
    install(&editor, &mut visual, own);
    assert_eq!(painted_range(&mut visual), held);
    editor.read_with(&visual, |v, _| assert!(v.pending_loop.is_none()));
}
#[gpui::test]
fn queue_refusal_restores_real_old_paint_without_inserting_loop_history(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, true).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let old_paint = painted_range(&mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    begin_paint(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        for _ in 0..crate::editor::UI_REQUEST_CAPACITY {
            v.pending_requests
                .push_back(Request::Edit(Command::MasterGain { gain: 0.25 }));
        }
    });
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("request queue full")
        );
        assert!(v.pending_loop.is_none() && v.loop_rail_gesture.is_none());
        assert_eq!(v.pending_requests.len(), crate::editor::UI_REQUEST_CAPACITY);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    assert_eq!(painted_range(&mut visual), old_paint);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.loop_region(), Some(old))
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
#[gpui::test]
fn escape_button_loss_and_focus_loss_cancel_held_loop_without_a_request(cx: &mut TestAppContext) {
    for cancel in 0..3 {
        let old = LoopRegion::new(24_000, 48_000, true).unwrap();
        let (_environment, editor, mut visual) = fixture(cx, old);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let old_paint = painted_range(&mut visual);
        begin_paint(&editor, &mut visual);
        match cancel {
            0 => visual.simulate_keystrokes("escape"),
            1 => {
                let end = rail_point(&mut visual, 0.5);
                visual.simulate_mouse_move(end, None, Modifiers::default());
            }
            _ => {
                let search = visual
                    .debug_bounds("browser-search")
                    .expect("actual focusable input");
                visual.simulate_mouse_down(
                    search.center(),
                    MouseButton::Left,
                    Modifiers::default(),
                );
                visual.simulate_mouse_up(search.center(), MouseButton::Left, Modifiers::default());
            }
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(
                v.loop_rail_gesture.is_none(),
                "actual cancellation mode {cancel}"
            );
            assert!(v.pending_loop.is_none() && v.pending_requests.is_empty());
            assert_eq!(v.worker_requests_pending, 0);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        assert_eq!(
            painted_range(&mut visual),
            old_paint,
            "completed cancellation scene {cancel}"
        );
        let end = rail_point(&mut visual, 0.5);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert_eq!(v.worker_requests_pending, 0));
    }
}
#[gpui::test]
fn active_loop_retires_on_real_revision_or_document_replacement(cx: &mut TestAppContext) {
    for replace in [false, true] {
        let (_environment, editor, mut visual) =
            fixture(cx, LoopRegion::new(24_000, 48_000, true).unwrap());
        begin_paint(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(if replace {
                Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                }
            } else {
                Request::Edit(Command::MasterGain { gain: 0.75 })
            }))
        });
        pump(&editor, &mut visual);
        let expected = editor.read_with(&visual, |v, _| {
            assert!(v.loop_rail_gesture.is_none() && v.pending_loop.is_none());
            v.state.as_ref().unwrap().project.snapshot()
        });
        let end = rail_point(&mut visual, 0.5);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
}

#[gpui::test]
fn reverse_free_final_release_and_repeated_replacement_keep_exact_worker_geometry(
    cx: &mut TestAppContext,
) {
    for enabled in [false, true] {
        let (_environment, editor, mut visual) =
            fixture(cx, LoopRegion::new(24_000, 48_000, enabled).unwrap());
        for (anchor, movement, release, modifiers) in [
            (0.7, 0.5, 0.4, Modifiers::default()),
            (
                0.2,
                0.4,
                0.613,
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let start = rail_point(&mut visual, anchor);
            let moved = rail_point(&mut visual, movement);
            let final_point = rail_point(&mut visual, release);
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
            // No final MouseMove: the MouseUp resolver must use its real final x/modifier.
            visual.simulate_mouse_up(final_point, MouseButton::Left, modifiers);
            visual.run_until_parked();
            let expected = editor.read_with(&visual, |v, _| v.pending_loop.unwrap().region);
            let submitted_paint = painted_range(&mut visual);
            let response = hold_real_response(&editor, &mut visual);
            assert!(response.error.is_none(), "{:?}", response.error);
            assert_eq!(response.state.project.loop_region(), Some(expected));
            assert_eq!(
                response
                    .playback
                    .as_ref()
                    .unwrap()
                    .prepared_revision()
                    .unwrap()
                    .arrangement()
                    .loop_region(),
                Some(expected)
            );
            install(&editor, &mut visual, response);
            assert_eq!(painted_range(&mut visual), submitted_paint);
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
            });
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
            pump(&editor, &mut visual);
            assert_eq!(painted_range(&mut visual), submitted_paint);
            let saved = _environment
                .root()
                .join(format!("replacement-{}.hex", expected.start()));
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::SaveNew(saved.clone())))
            });
            pump(&editor, &mut visual);
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(saved))));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert!(v.error.is_none(), "{:?}", v.error);
                assert_eq!(
                    v.state.as_ref().unwrap().project.loop_region(),
                    Some(expected)
                );
                assert!(v.pending_loop.is_none());
            });
        }
    }
}

#[gpui::test]
fn another_paint_while_deferred_keeps_the_original_pending_intent(cx: &mut TestAppContext) {
    let (_environment, editor, mut visual) =
        fixture(cx, LoopRegion::new(24_000, 48_000, true).unwrap());
    let held = begin_paint(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        v.pending_requests
            .push_back(Request::Edit(Command::MasterGain { gain: 0.75 }))
    });
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let start = rail_point(&mut visual, 0.7);
    let other = rail_point(&mut visual, 0.8);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(other, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.loop_rail_gesture.is_none());
        assert_eq!(v.pending_requests.len(), 2);
    });
    assert_eq!(painted_range(&mut visual), held);
    editor.update(&mut visual, |v, cx| {
        v.tick(cx);
        cx.notify();
    });
    let before = hold_real_response(&editor, &mut visual);
    let own = hold_real_response(&editor, &mut visual);
    assert!(before.error.is_none() && own.error.is_none());
    install(&editor, &mut visual, before);
    editor.read_with(&visual, |v, _| assert!(v.pending_loop.is_some()));
    assert_eq!(painted_range(&mut visual), held);
    install(&editor, &mut visual, own);
    editor.read_with(&visual, |v, _| assert!(v.pending_loop.is_none()));
    assert_eq!(painted_range(&mut visual), held);
}
