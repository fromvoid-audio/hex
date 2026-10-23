//! Rendered track-order interaction and opt-in dispatch latency probes.
use super::{EditorView, clip_drag_ui_test::editor_window};
use gpui::{
    AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackId};
use std::{sync::Arc, time::Instant};

fn fixture(
    cx: &mut TestAppContext,
    count: usize,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
    cx.update(super::bind_editor_keys);
    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for index in 0..count {
        let Created::Track(id) = model
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        ids.push(id);
        let Created::Clip(clip) = model
            .execute(Command::PlaceMidiClip {
                track: id,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(16).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for pitch in 60..68 {
            model
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
    }
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    (visual, editor, ids)
}
fn bounds(visual: &mut VisualTestContext, id: TrackId) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("track-name-{}", id.value()).into_boxed_str(),
        ))
        .unwrap()
}

#[gpui::test]
#[ignore = "opt-in 100-track rendered dispatch latency probe"]
fn track_header_dispatch_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = fixture(cx, 100);
    let from = bounds(&mut visual, ids[0]).center();
    let second = bounds(&mut visual, ids[1]).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    let mut times = Vec::new();
    for n in 0..120 {
        let to = point(second.x, second.y + px((n % 12) as f32 - 6.0));
        let start = Instant::now();
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if n >= 20 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    let start = Instant::now();
    visual.simulate_mouse_up(second, MouseButton::Left, Modifiers::default());
    let release = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "track_dispatch_ms update_p50={} update_p95={} update_max={} release={} tracks=100 clips=100 notes=800 window=1280x800",
        times[50], times[95], times[99], release
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.tracks().len(), 100)
    });
}

fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "worker completion");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn live_fixture(
    cx: &mut TestAppContext,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
    use hex_session::service::{Request, Service};
    cx.update(super::bind_editor_keys);
    let service = Service::new(48000, 32, 8000000).unwrap();
    let mut state = next(&service).state;
    for i in 0..4 {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("track {i}"),
            }))
            .unwrap();
        state = next(&service).state;
    }
    let ids = state.project.tracks().iter().map(|t| t.id()).collect();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline track ui test".into()))
            });
            editor.update(cx, |v, cx| {
                v.state = Some(state);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    (visual, editor, ids)
}
fn receive(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) {
    editor.update(visual, |v, cx| {
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
}
#[gpui::test]
fn pointer_commits_first_middle_last_and_one_undo(cx: &mut TestAppContext) {
    for (source, target, below, expected) in [
        (3, 0, false, vec![3, 0, 1, 2]),
        (0, 2, false, vec![1, 0, 2, 3]),
        (0, 3, true, vec![1, 2, 3, 0]),
    ] {
        let (mut visual, editor, ids) = live_fixture(cx);
        let selected = editor.read_with(&visual, |v, _| {
            (v.selected_track, v.selected_clip, v.selected_track_ids())
        });
        let from = bounds(&mut visual, ids[source]).center();
        let target_row = visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", ids[target].value()).into_boxed_str(),
            ))
            .unwrap();
        let to = point(
            from.x,
            if below {
                target_row.origin.y + target_row.size.height - px(1.0)
            } else {
                target_row.origin.y + px(1.0)
            },
        );
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let line = visual
            .debug_bounds("track-insertion-line")
            .expect("painted exact insertion line");
        let edge = if below {
            target_row.origin.y + target_row.size.height
        } else {
            target_row.origin.y
        };
        assert!(
            (line.origin.y - edge).abs() <= px(1.0),
            "{line:?} != {edge:?}"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>(),
                ids
            )
        });
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        // Explicit completed repaints before the worker terminal: no vanished line
        // or speculative canonical order while preparation runs.
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert!(visual.debug_bounds("track-insertion-line").is_some());
        }
        receive(&mut visual, &editor);
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>(),
                expected.iter().map(|i| ids[*i]).collect::<Vec<_>>()
            )
        });
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                (v.selected_track, v.selected_clip, v.selected_track_ids()),
                selected
            )
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(hex_session::service::Request::Undo));
        });
        receive(&mut visual, &editor);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>(),
                ids
            )
        });
    }
}
#[gpui::test]
fn pointer_noop_escape_capture_loss_and_outside_release_do_not_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_fixture(cx);
    let from = bounds(&mut visual, ids[0]).center();
    let target = bounds(&mut visual, ids[2]).center();
    for mode in 0..4 {
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        let to = if mode == 0 { from } else { target };
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        match mode {
            1 => visual.simulate_keystrokes("escape"),
            2 => visual.simulate_event(gpui::MouseMoveEvent {
                position: to,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            3 => visual.simulate_mouse_move(
                point(px(-100.0), px(-100.0)),
                MouseButton::Left,
                Modifiers::default(),
            ),
            _ => {}
        }
        visual.simulate_mouse_up(
            if mode == 3 {
                point(px(-100.0), px(-100.0))
            } else {
                to
            },
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| {
            assert!(!v.track_reorder_active());
            assert_eq!(v.worker_requests_pending, 0);
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>(),
                ids
            );
        });
    }
}

#[gpui::test]
fn scrolled_variable_rows_and_held_edge_autoscroll_map_to_painted_boundaries(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, ids) = fixture(cx, 100);
    // Resize through the real handle, retaining the same metadata as hit mapping.
    let handle = visual
        .debug_bounds(Box::leak(
            format!("track-height-resize-{}", ids[1].value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(handle, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(handle.x, handle.y + px(47.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(handle.x, handle.y + px(47.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let from = bounds(&mut visual, ids[0]).center();
    let row = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, row.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        (visual
            .debug_bounds("track-insertion-line")
            .unwrap()
            .origin
            .y
            - row.origin.y)
            .abs()
            <= px(1.0)
    );
    visual.simulate_keystrokes("escape");
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 48,
            offset_in_item: px(23.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    let from = bounds(&mut visual, ids[49]).center();
    let row = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[51].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, row.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        (visual
            .debug_bounds("track-insertion-line")
            .unwrap()
            .origin
            .y
            - row.origin.y)
            .abs()
            <= px(1.0)
    );
    // Wheel/scrollbar movement with a stationary held pointer must repaint
    // the new nearest gap rather than retaining an obsolete viewport Y.
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 49,
            offset_in_item: px(10.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    let shifted = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[52].value()).into_boxed_str(),
        ))
        .unwrap();
    assert!(
        (visual
            .debug_bounds("track-insertion-line")
            .unwrap()
            .origin
            .y
            - shifted.origin.y)
            .abs()
            <= px(1.0)
    );
    let body = visual.debug_bounds("arrangement-content").unwrap();
    let edge = point(from.x, body.origin.y + body.size.height - px(2.0));
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    let old = editor.read_with(&visual, |v, _| v.track_scroll.list.logical_scroll_top());
    editor.update(&mut visual, |v, cx| {
        assert!(v.tick_track_reorder(Instant::now() + std::time::Duration::from_millis(100)));
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let new = v.track_scroll.list.logical_scroll_top();
        assert!(new.item_ix > old.item_ix || new.offset_in_item > old.offset_in_item);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
}

#[gpui::test]
fn source_revision_and_focus_loss_cancel_held_track_reorder(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_fixture(cx);
    for stale in [true, false] {
        let from = bounds(&mut visual, ids[0]).center();
        let to = bounds(&mut visual, ids[2]).center();
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_some());
        if stale {
            editor.update(&mut visual, |v, _| {
                v.service
                    .submit(hex_session::service::Request::Edit(Command::RenameTrack {
                        track: ids[3],
                        name: "renamed".into(),
                    }))
                    .unwrap();
            });
            receive(&mut visual, &editor);
        } else {
            visual.update(|window, cx| {
                editor.update(cx, |v, cx| {
                    v.focus.focus(window);
                    cx.notify();
                })
            });
            visual.run_until_parked();
        }
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| assert!(!v.track_reorder_active()));
    }
}

#[gpui::test]
fn main_is_not_a_drop_target_and_tab_does_not_switch_views_during_capture(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_fixture(cx);
    let from = bounds(&mut visual, ids[0]).center();
    let to = bounds(&mut visual, ids[2]).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, super::EditorMode::Arrangement)
    });
    assert!(visual.debug_bounds("track-insertion-line").is_some());
    let main = visual.debug_bounds("main-track-header").unwrap().center();
    visual.simulate_mouse_move(main, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(main, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| assert_eq!(v.worker_requests_pending, 0));
}

#[gpui::test]
fn window_deactivation_cancels_track_capture(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_fixture(cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let from = bounds(&mut visual, ids[0]).center();
    let to = bounds(&mut visual, ids[2]).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_some());
    visual.deactivate_window();
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active());
        assert_eq!(v.worker_requests_pending, 0);
    });
}
