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
pub(super) fn bounds(visual: &mut VisualTestContext, id: TrackId) -> gpui::Bounds<gpui::Pixels> {
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

pub(super) fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
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
    live_tracks(cx, 4, false)
}
pub(super) fn live_tracks(
    cx: &mut TestAppContext,
    count: usize,
    notes: bool,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
    live_tracks_with_notes(cx, count, if notes { 8 } else { 0 })
}

pub(super) fn live_tracks_with_notes(
    cx: &mut TestAppContext,
    count: usize,
    notes: u8,
) -> (VisualTestContext, gpui::Entity<EditorView>, Vec<TrackId>) {
    use hex_session::service::{Request, Service};
    cx.update(super::bind_editor_keys);
    let service = Service::new(48000, 32, 8000000).unwrap();
    let mut state = next(&service).state;
    for i in 0..count {
        service
            .submit(Request::Edit(Command::AddTrack {
                name: format!("track {i}"),
            }))
            .unwrap();
        state = next(&service).state;
        if notes != 0 {
            let track = state.project.tracks().last().unwrap().id();
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(16).unwrap(),
                }))
                .unwrap();
            let event = next(&service);
            let Some(Created::Clip(clip)) = event.created else {
                panic!("clip")
            };
            for pitch in 60..60 + notes {
                service
                    .submit(Request::Edit(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch,
                        channel: 0,
                        velocity: 100,
                    }))
                    .unwrap();
                state = next(&service).state;
            }
        }
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
pub(super) fn receive(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) {
    editor.update(visual, |v, cx| {
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
}
#[gpui::test]
fn dropping_on_folded_group_reparents_and_keeps_target_until_ack(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..1]).unwrap(),
            name: "group".into(),
        })));
    });
    receive(&mut visual, &editor);
    let (group, before) = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (
            project.tracks().iter().find(|t| t.is_group()).unwrap().id(),
            project.snapshot(),
        )
    });
    let fold = Box::leak(format!("group-fold-{}", group.value()).into_boxed_str());
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let from = bounds(&mut visual, ids[2]).center();
    let target = bounds(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-group-drop-target").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("track-group-drop-target").is_some(),
        "pending target must remain painted"
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
    receive(&mut visual, &editor);
    assert!(visual.debug_bounds("track-group-drop-target").is_none());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[2])
                .unwrap()
                .parent_group(),
            Some(group)
        );
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
    // A directly adjacent root can enter an empty group at the same flat boundary.
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(ids[0]))))
    });
    receive(&mut visual, &editor);
    let from = bounds(&mut visual, ids[1]).center();
    let target = bounds(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-group-drop-target").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[1])
                .unwrap()
                .parent_group(),
            Some(group)
        );
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[ids[1]]).unwrap(),
            name: "nested".into(),
        })));
    });
    receive(&mut visual, &editor);
    let nested = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| track.is_group() && track.parent_group() == Some(group))
            .unwrap()
            .id()
    });
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let from = bounds(&mut visual, ids[2]).center();
    let target = bounds(&mut visual, nested).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-group-drop-target").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[2])
                .unwrap()
                .parent_group(),
            Some(nested)
        )
    });
}

#[gpui::test]
fn group_bottom_drag_distinguishes_append_from_the_outside_gap(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;

    let (mut visual, editor, ids) = live_fixture(cx);
    let [first, last, following, dragged] = ids.as_slice() else {
        panic!("four tracks")
    };
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[*first, *last]).unwrap(),
            name: "group".into(),
        })));
    });
    receive(&mut visual, &editor);
    let (group, original) = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (project.tracks()[0].id(), project.snapshot())
    });
    let header = |visual: &mut VisualTestContext, id: TrackId| {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", id.value()).into_boxed_str(),
            ))
            .unwrap()
    };
    let modifiers = Modifiers::default();
    for parent in [None, Some(group)] {
        let from = bounds(&mut visual, *dragged).center();
        let last_header = header(&mut visual, *last);
        let boundary = header(&mut visual, *following).origin.y;
        let outside = point(from.x, boundary);
        let inside = point(from.x, boundary - px(4.0));
        visual.simulate_mouse_down(from, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(outside, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let outside_line = visual.debug_bounds("track-insertion-line").unwrap();
        assert_eq!(
            outside_line.origin.y + outside_line.size.height / 2.0,
            boundary
        );
        assert!(
            visual
                .debug_bounds("track-group-end-insertion-line")
                .is_none()
        );

        visual.simulate_mouse_move(inside, MouseButton::Left, modifiers);
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        let inside_line = visual
            .debug_bounds("track-group-end-insertion-line")
            .unwrap();
        assert_eq!(inside_line.origin.y + inside_line.size.height, boundary);
        assert_eq!(
            inside_line.origin.x,
            last_header.origin.x + px(super::track_hierarchy_indent::RAIL_WIDTH)
        );
        assert_eq!(
            inside_line.origin.x + inside_line.size.width,
            last_header.origin.x + last_header.size.width
        );

        let (release, selector, painted) = if parent.is_some() {
            (inside, "track-group-end-insertion-line", inside_line)
        } else {
            visual.simulate_mouse_move(outside, MouseButton::Left, modifiers);
            visual.run_until_parked();
            (outside, "track-insertion-line", outside_line)
        };
        visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(painted));
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
        });
        receive(&mut visual, &editor);
        assert!(visual.debug_bounds(selector).is_none());
        let placed = editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let order: Vec<_> = project.tracks().iter().map(|track| track.id()).collect();
            assert_eq!(order, [group, *first, *last, *dragged, *following]);
            assert_eq!(project.track(*dragged).unwrap().parent_group(), parent);
            project.snapshot()
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        receive(&mut visual, &editor);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
        });
        if parent.is_some() {
            editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
            receive(&mut visual, &editor);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), placed);
            });
        }
    }
}

#[gpui::test]
fn group_drop_refusal_keeps_target_until_terminal_installation(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..1]).unwrap(),
            name: "group".into(),
        })));
    });
    receive(&mut visual, &editor);
    let group = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap()
            .id()
    });
    // Let the worker advance without delivering its new revision to this view.
    editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::Edit(Command::AddTrack {
                name: "other".into(),
            }))
            .unwrap();
        assert!(next(&view.service).error.is_none());
    });
    let from = bounds(&mut visual, ids[1]).center();
    let target = bounds(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let highlight = visual.debug_bounds("track-group-drop-target").unwrap();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(&mut visual, |view, _| next(&view.service));
    assert!(terminal.request_complete && terminal.error.is_some());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("track-group-drop-target"),
            Some(highlight)
        );
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-group-drop-target").is_none());
    editor.read_with(&visual, |view, _| {
        assert!(!view.track_reorder_active());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[1])
                .unwrap()
                .parent_group(),
            None
        );
    });
}

#[gpui::test]
fn group_drag_never_paints_its_own_subtree_as_a_destination(cx: &mut TestAppContext) {
    use hex_project::TrackSelectionIds;
    use hex_session::service::Request;
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..2]).unwrap(),
            name: "group".into(),
        })));
    });
    receive(&mut visual, &editor);
    let (group, before, can_undo, can_redo) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (
            state
                .project
                .tracks()
                .iter()
                .find(|track| track.is_group())
                .unwrap()
                .id(),
            state.project.snapshot(),
            state.can_undo,
            state.can_redo,
        )
    });
    let from = bounds(&mut visual, group).center();
    let child = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[1].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, child.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("track-insertion-line").is_none(),
        "cannot move a group into its own descendant"
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.snapshot(), before);
        assert_eq!((state.can_undo, state.can_redo), (can_undo, can_redo));
        assert!(!view.track_reorder_active());
    });
    // The boundary directly after the complete subtree is also a no-op.
    let from = bounds(&mut visual, group).center();
    let sibling = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
        ))
        .unwrap();
    let unchanged = point(from.x, sibling.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(unchanged, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    visual.simulate_mouse_up(unchanged, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });

    // Folding hides children, but must not prevent a real subtree move.
    let fold = Box::leak(format!("group-fold-{}", group.value()).into_boxed_str());
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let from = bounds(&mut visual, group).center();
    let sibling = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
        ))
        .unwrap();
    let after = point(from.x, sibling.origin.y + sibling.size.height - px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(after, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_some());
    visual.simulate_mouse_up(after, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            [ids[2], group, ids[0], ids[1]]
        );
        assert_eq!(project.track(ids[0]).unwrap().parent_group(), Some(group));
        assert_eq!(project.track(ids[1]).unwrap().parent_group(), Some(group));
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn pointer_commits_first_middle_last_and_one_undo(cx: &mut TestAppContext) {
    for (source, target, below, expected) in [
        (3, 0, false, vec![3, 0, 1, 2]),
        (0, 2, false, vec![1, 0, 2, 3]),
        (0, 3, true, vec![1, 2, 3, 0]),
    ] {
        let (mut visual, editor, ids) = live_fixture(cx);
        // Establish real user selection; assigning a fixture snapshot directly
        // intentionally bypasses the controller's initial selection reconciliation.
        let select = bounds(&mut visual, ids[1]).center();
        visual.simulate_mouse_down(select, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(select, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
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

#[gpui::test]
fn unpainted_release_gap_is_canceled_instead_of_committing_unseen_order(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_fixture(cx);
    let from = bounds(&mut visual, ids[0]).center();
    let to = bounds(&mut visual, ids[2]).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_some());
    // The last up event first enters a new gap, with no completed repaint there.
    let last = bounds(&mut visual, ids[3]).center();
    visual.simulate_mouse_up(last, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.worker_requests_pending, 0);
        assert!(!v.track_reorder_active());
    });
    assert!(visual.debug_bounds("track-insertion-line").is_none());
}

#[gpui::test]
#[ignore = "opt-in 100-track actual worker/receive/repaint latency probe"]
fn track_reorder_full_ui_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, _) = live_tracks(cx, 100, true);
    let mut samples = Vec::new();
    for i in 0..12 {
        editor.update(&mut visual, |v, cx| {
            v.track_scroll.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.0),
            });
            cx.notify();
        });
        visual.run_until_parked();
        let ids = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .map(|t| t.id())
                .collect::<Vec<_>>()
        });
        let from = bounds(&mut visual, ids[0]).center();
        let row = visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
            ))
            .unwrap();
        let to = point(from.x, row.origin.y + px(1.0));
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        let start = Instant::now();
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let update = start.elapsed().as_secs_f64() * 1000.0;
        assert!(visual.debug_bounds("track-insertion-line").is_some());
        let start = Instant::now();
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let dispatch = start.elapsed().as_secs_f64() * 1000.0;
        let event = editor.update(&mut visual, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        let terminal = start.elapsed().as_secs_f64() * 1000.0;
        let repaint = Instant::now();
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let repaint_ms = repaint.elapsed().as_secs_f64() * 1000.0;
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.tracks()[1].id(), ids[0])
        });
        if i >= 2 {
            samples.push([update, dispatch, terminal, repaint_ms]);
        }
    }
    for (field, label) in [
        "update_repaint",
        "release_dispatch",
        "release_to_terminal",
        "receive_repaint",
    ]
    .iter()
    .enumerate()
    {
        let mut times = samples.iter().map(|row| row[field]).collect::<Vec<_>>();
        times.sort_by(f64::total_cmp);
        println!(
            "track_full_ui_ms {label} p50={} max={} tracks=100 clips=100 notes=800 window=1280x800 warmups=2 samples=10",
            times[5], times[9]
        );
    }
}

#[gpui::test]
fn scrolled_variable_height_drop_commits_the_visible_gap_and_retains_sizes(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, ids) = live_tracks(cx, 12, false);
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 6,
            offset_in_item: px(11.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    let handle = visual
        .debug_bounds(Box::leak(
            format!("track-height-resize-{}", ids[8].value()).into_boxed_str(),
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
    let height = editor.read_with(&visual, |v, _| v.track_height(ids[8]));
    let from = bounds(&mut visual, ids[7]).center();
    let target = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[9].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, target.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        (visual
            .debug_bounds("track-insertion-line")
            .unwrap()
            .origin
            .y
            - target.origin.y)
            .abs()
            <= px(1.0)
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        let tracks = v.state.as_ref().unwrap().project.tracks();
        assert_eq!(tracks[7].id(), ids[8]);
        assert_eq!(tracks[8].id(), ids[7]);
        assert_eq!(v.track_height(ids[8]), height);
    });
}

#[gpui::test]
fn canonical_reorder_does_not_rebuild_every_offscreen_track(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 100, false);
    let from = bounds(&mut visual, ids[0]).center();
    let target = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", ids[2].value()).into_boxed_str(),
        ))
        .unwrap();
    let to = point(from.x, target.origin.y + px(1.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| v.track_row_builds = 0);
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.tracks()[1].id(), ids[0]);
        println!("track row builds after reorder: {}", v.track_row_builds);
        assert!(
            v.track_row_builds < 100,
            "one reordered row must not rebuild100track elements: {}",
            v.track_row_builds
        );
    });
}

#[gpui::test]
fn selected_track_group_drags_as_one_block_with_exact_ack_and_one_history_step(
    cx: &mut TestAppContext,
) {
    for contiguous in [true, false] {
        let (mut visual, editor, ids) = live_fixture(cx);
        let first = bounds(&mut visual, ids[0]).center();
        visual.simulate_click(first, Modifiers::default());
        let last = bounds(&mut visual, ids[2]).center();
        visual.simulate_click(
            last,
            Modifiers {
                shift: contiguous,
                platform: !contiguous,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        let members = if contiguous {
            ids[..3].to_vec()
        } else {
            vec![ids[0], ids[2]]
        };
        let before = editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_track_ids(), members);
            v.state.as_ref().unwrap().project.as_ref().clone()
        });
        let from = bounds(&mut visual, ids[2]).center();
        let target = visual
            .debug_bounds(Box::leak(
                format!("arrangement-track-header-{}", ids[3].value()).into_boxed_str(),
            ))
            .unwrap();
        let to = point(from.x, target.bottom() - px(1.0));
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let line = visual.debug_bounds("track-insertion-line").unwrap();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_track_ids(), members);
            assert_eq!(*v.state.as_ref().unwrap().project, before);
        });
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(&mut visual, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds("track-insertion-line"), Some(line));
            editor.read_with(&visual, |v, _| {
                assert_eq!(*v.state.as_ref().unwrap().project, before)
            });
        }
        let after = terminal.state.project.clone();
        let expected = if contiguous {
            vec![ids[3], ids[0], ids[1], ids[2]]
        } else {
            vec![ids[1], ids[3], ids[0], ids[2]]
        };
        assert_eq!(
            after.tracks().iter().map(|t| t.id()).collect::<Vec<_>>(),
            expected,
            "all selected tracks move together"
        );
        editor.update(&mut visual, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), members));
        for (request, expected) in [
            (hex_session::service::Request::Undo, &before),
            (hex_session::service::Request::Redo, after.as_ref()),
        ] {
            editor.update(&mut visual, |v, _| assert!(v.enqueue(request)));
            receive(&mut visual, &editor);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.as_ref(), expected)
            });
        }
    }
}

#[gpui::test]
fn selected_group_noop_escape_and_stationary_click_keep_existing_selection_contract(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, ids) = live_fixture(cx);
    let a = bounds(&mut visual, ids[0]).center();
    let c = bounds(&mut visual, ids[2]).center();
    visual.simulate_click(a, Modifiers::default());
    visual.simulate_click(
        c,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    for cancel in [false, true] {
        let target = visual
            .debug_bounds(Box::leak(
                format!(
                    "arrangement-track-header-{}",
                    ids[if cancel { 3 } else { 0 }].value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        let to = point(
            c.x,
            if cancel {
                target.bottom() - px(1.0)
            } else {
                target.top() + px(1.0)
            },
        );
        visual.simulate_mouse_down(c, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("track-insertion-line").is_some(),
            cancel
        );
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("track-insertion-line").is_none());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_track_ids(), [ids[0], ids[2]]);
            assert_eq!(v.state.as_ref().unwrap().project, before);
            assert!(!v.track_reorder_active());
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
    // A stationary plain click still selects one member, rather than making a
    // permanent group or changing the existing header-click modifier policy.
    visual.simulate_click(c, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [ids[2]]);
        assert_eq!(v.state.as_ref().unwrap().project, before);
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[path = "track_reorder/capture_tests.rs"]
mod capture_tests;
