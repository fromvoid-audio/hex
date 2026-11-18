//! Rendered Console header drag and captured receipt behavior.
use super::{
    EditorMode, EditorView,
    track_reorder_ui_test::{bounds, live_tracks, next, receive},
};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::{Command, TrackId, TrackSelectionIds};
use hex_session::service::Request;

fn header(visual: &mut VisualTestContext, id: TrackId) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", id.value()).into_boxed_str(),
        ))
        .unwrap()
}
fn switch(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.set_mode(EditorMode::Session);
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
}
fn group(visual: &mut VisualTestContext, editor: &Entity<EditorView>, ids: &[TrackId]) -> TrackId {
    editor.update(visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(ids).unwrap(),
            name: "group".into(),
        })))
    });
    receive(visual, editor);
    editor.read_with(visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| {
                track.is_group()
                    && ids.iter().all(|id| {
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .track(*id)
                            .unwrap()
                            .parent_group()
                            == Some(track.id())
                    })
            })
            .unwrap()
            .id()
    })
}

#[gpui::test]
fn nested_session_headers_show_ordered_ancestor_bands(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    let inner = group(&mut visual, &editor, &ids[..1]);
    let outer = group(&mut visual, &editor, &[inner, ids[1]]);
    switch(&mut visual, &editor);
    let band = |visual: &mut VisualTestContext, track: TrackId, ancestor: TrackId| {
        visual.debug_bounds(Box::leak(
            format!("session-group-band-{}-{}", track.value(), ancestor.value()).into_boxed_str(),
        ))
    };
    let outer_band = band(&mut visual, ids[0], outer).expect("outer band");
    let inner_band = band(&mut visual, ids[0], inner).expect("inner band");
    let h = header(&mut visual, ids[0]);
    for track in [ids[0], inner, outer] {
        let selector = Box::leak(format!("session-track-{}", track.value()).into_boxed_str());
        let expected = editor.read_with(&visual, |view, _| {
            super::theme::element_colors(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .color(),
            )
            .accent
        });
        assert_eq!(header(&mut visual, track).size.height, px(27.0));
        assert_eq!(
            visual.rendered_background(selector),
            Some(gpui::Background::from(gpui::rgb(expected)))
        );
    }

    assert!(f32::from(outer_band.size.width) >= f32::from(h.size.width) - 2.0);
    assert_eq!(f32::from(outer_band.size.height), 1.0);
    assert_eq!(
        inner_band.origin.y,
        outer_band.origin.y + outer_band.size.height
    );
    let own_strip =
        Box::leak(format!("session-track-color-strip-{}", ids[0].value()).into_boxed_str());
    let label = Box::leak(format!("session-track-label-{}", ids[0].value()).into_boxed_str());
    let strip = visual.debug_bounds(own_strip).unwrap();
    let label = visual.debug_bounds(label).unwrap();
    assert!(label.origin.y >= strip.origin.y + strip.size.height);
    assert!(band(&mut visual, ids[1], outer).is_some());
    assert!(band(&mut visual, ids[1], inner).is_none());
    assert!(band(&mut visual, ids[2], outer).is_none());
    let fold = Box::leak(format!("console-group-fold-{}", outer.value()).into_boxed_str());
    let point = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    assert!(band(&mut visual, ids[0], outer).is_none());
    visual.simulate_click(point, Modifiers::default());
    visual.run_until_parked();
    assert!(band(&mut visual, ids[0], outer).is_some());
}

#[gpui::test]
fn session_group_band_survives_paging_past_its_parent(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 12, false);
    let parent = group(&mut visual, &editor, &ids[5..7]);
    switch(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        let project = &view.state.as_ref().unwrap().project;
        let index = view
            .arrangement_focus
            .projection
            .console_indices
            .iter()
            .position(|&index| project.tracks()[index].id() == ids[5])
            .unwrap();
        view.session_track_start = index;
        cx.notify();
    });
    visual.run_until_parked();
    let band = Box::leak(
        format!("session-group-band-{}-{}", ids[5].value(), parent.value()).into_boxed_str(),
    );
    assert!(visual.debug_bounds(band).is_some());
    let parent_header = Box::leak(format!("session-track-{}", parent.value()).into_boxed_str());
    assert!(visual.debug_bounds(parent_header).is_none());
}

#[gpui::test]
fn deepest_session_group_keeps_label_below_colour_bands(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 8, false);
    let mut node = ids[0];
    for _ in 0..hex_project::MAX_GROUP_DEPTH {
        node = group(&mut visual, &editor, &[node]);
    }
    switch(&mut visual, &editor);
    editor.update(&mut visual, |view, cx| {
        let project = &view.state.as_ref().unwrap().project;
        view.session_track_start = view
            .arrangement_focus
            .projection
            .console_indices
            .iter()
            .position(|&index| project.tracks()[index].id() == ids[0])
            .unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    let strip = Box::leak(format!("session-track-color-strip-{}", ids[0].value()).into_boxed_str());
    let label = Box::leak(format!("session-track-label-{}", ids[0].value()).into_boxed_str());
    let strip = visual.debug_bounds(strip).unwrap();
    let label = visual.debug_bounds(label).unwrap();
    assert!(label.origin.y >= strip.origin.y + strip.size.height);
    assert!(f32::from(label.size.height) >= 12.0);
    for width in [640.0, 1280.0] {
        visual.simulate_resize(size(px(width), px(800.0)));
        visual.run_until_parked();
        let h = header(&mut visual, ids[0]);
        let label = visual
            .debug_bounds(Box::leak(
                format!("session-track-label-{}", ids[0].value()).into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(h.size.height, px(27.0));
        assert!(label.top() >= h.top() && label.bottom() <= h.bottom());
        assert!(label.size.height >= px(12.0));
    }
}

#[gpui::test]
fn console_group_drop_keeps_target_until_ack_and_undo(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    let group = group(&mut visual, &editor, &ids[..1]);
    for (id, toggle) in [(ids[1], false), (ids[2], true)] {
        let at = bounds(&mut visual, id).center();
        let modifiers = Modifiers {
            platform: toggle,
            ..Default::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    switch(&mut visual, &editor);
    let fold = Box::leak(format!("console-group-fold-{}", group.value()).into_boxed_str());
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, ids[2]).center();
    let to = header(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.track_reorder_active(),
            "capture candidate missing; from={from:?} to={to:?}"
        )
    });
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.track_reorder_active(),
            "capture lost after move; from={from:?} to={to:?}"
        )
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_some());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.simulate_resize(size(px(1100.0), px(800.0)));
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert!(visual.debug_bounds("console-track-group-target").is_some());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
    receive(&mut visual, &editor);
    assert!(visual.debug_bounds("console-track-group-target").is_none());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[2])
                .unwrap()
                .parent_group(),
            Some(group)
        )
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[1])
                .unwrap()
                .parent_group(),
            Some(group)
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    // Restore the original three-column viewport after checking the pending
    // receipt at1100px, before the independent second drag after Undo.
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("session-track-{}", ids[2].value()).into_boxed_str()
            ))
            .is_some(),
        "undo must expose the restored ungrouped track: {:?}",
        editor.read_with(&visual, |view, _| (
            view.session_track_start,
            view.session_visible_columns,
            view.arrangement_focus.projection.console_indices.clone(),
            view.mode
        ))
    );
    let from = header(&mut visual, ids[2]).center();
    let target = header(&mut visual, group);
    let to = point(target.origin.x + px(2.0), target.center().y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("console-track-insertion-line")
            .is_some()
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        let tracks = v.state.as_ref().unwrap().project.tracks();
        assert_eq!(
            tracks.iter().map(|t| t.id()).collect::<Vec<_>>(),
            [ids[1], ids[2], group, ids[0]]
        );
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn console_invalid_group_target_and_stale_page_do_not_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 4, false);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let inner = group(&mut visual, &editor, &ids[..1]);
    let outer = group(&mut visual, &editor, &[inner]);
    switch(&mut visual, &editor);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, outer).center();
    let to = header(&mut visual, inner).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_none());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });

    let from = header(&mut visual, ids[2]).center();
    let target = header(&mut visual, ids[1]);
    let to = point(target.origin.x + px(2.0), target.center().y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("console-track-insertion-line")
            .is_some()
    );
    editor.update(&mut visual, |view, cx| {
        view.session_track_start = 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn console_unpainted_release_and_refusal_preserve_state(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    let group = group(&mut visual, &editor, &ids[..1]);
    switch(&mut visual, &editor);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, ids[2]).center();
    let target = header(&mut visual, group).center();
    let other = header(&mut visual, ids[1]);
    let unseen = point(other.origin.x + px(2.0), other.center().y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_some());
    visual.simulate_mouse_up(unseen, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });

    editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::Edit(Command::AddTrack {
                name: "other".into(),
            }))
            .unwrap();
        assert!(next(&view.service).error.is_none());
    });
    let from = header(&mut visual, ids[2]).center();
    let target = header(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let highlight = visual.debug_bounds("console-track-group-target").unwrap();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(&mut visual, |v, _| next(&v.service));
    assert!(terminal.request_complete && terminal.error.is_some());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("console-track-group-target"),
            Some(highlight)
        );
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_none());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[2])
                .unwrap()
                .parent_group(),
            None
        )
    });
}

fn turn_page(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    editor.update(visual, |view, cx| {
        assert!(
            view.tick_track_reorder(std::time::Instant::now() + std::time::Duration::from_secs(1))
        );
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn console_track_drag_turns_left_to_a_group_and_undoes(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 8, false);
    let target_group = group(&mut visual, &editor, &ids[..1]);
    switch(&mut visual, &editor);
    let size = editor.read_with(&visual, |v, _| v.session_visible_columns);
    assert!(size > 1 && size < 8);
    editor.update(&mut visual, |v, cx| {
        v.session_track_start = size;
        cx.notify();
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, ids[size - 1]).center();
    let first = editor.read_with(&visual, |v, _| {
        let index = v.arrangement_focus.projection.console_indices[v.session_track_start];
        v.state.as_ref().unwrap().project.tracks()[index].id()
    });
    let first_bounds = header(&mut visual, first);
    let edge = point(first_bounds.origin.x + px(2.0), first_bounds.center().y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("console-track-page-previous-cue")
            .is_some()
    );
    turn_page(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.session_track_start, 0);
        assert!(v.track_reorder_active());
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            !v.tick_track_reorder(std::time::Instant::now() + std::time::Duration::from_secs(30))
        )
    });
    let target = header(&mut visual, target_group).center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[size - 1])
                .unwrap()
                .parent_group(),
            Some(target_group)
        )
    });
    let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    // Repeat the cross-page gesture at the group's boundary to reorder instead.
    editor.update(&mut visual, |v, cx| {
        v.session_track_start = size;
        cx.notify();
    });
    visual.run_until_parked();
    let from = header(&mut visual, ids[size - 1]).center();
    let first = editor.read_with(&visual, |v, _| {
        let index = v.arrangement_focus.projection.console_indices[v.session_track_start];
        v.state.as_ref().unwrap().project.tracks()[index].id()
    });
    let first_bounds = header(&mut visual, first);
    let edge = point(first_bounds.origin.x + px(2.0), first_bounds.center().y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    turn_page(&mut visual, &editor);
    let target = header(&mut visual, target_group);
    let to = point(target.origin.x + px(2.0), target.center().y);
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("console-track-insertion-line")
            .is_some()
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.tracks()[0].id(),
            ids[size - 1]
        )
    });
    let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn console_track_drag_turns_right_and_refuses_unpainted_transition(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 8, false);
    let target_group = group(&mut visual, &editor, &ids[6..7]);
    editor.update(&mut visual, |v, cx| {
        v.arrangement_focus.projection.toggle_fold(target_group);
        v.reconcile_arrangement_focus();
        cx.notify();
    });
    switch(&mut visual, &editor);
    let size = editor.read_with(&visual, |v, _| v.session_visible_columns);
    assert!(size > 1 && size < 8);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, ids[0]).center();
    let scenes = visual.debug_bounds("session-scenes-header").unwrap();
    let edge = point(scenes.origin.x - px(2.0), from.y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-page-next-cue").is_some());
    // A release before the new page paints cannot commit the old target.
    editor.update(&mut visual, |v, cx| {
        assert!(
            v.tick_track_reorder(std::time::Instant::now() + std::time::Duration::from_secs(1))
        );
        cx.notify();
    });
    visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!v.track_reorder_active());
    });
    editor.update(&mut visual, |v, cx| {
        v.session_track_start = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let from = header(&mut visual, ids[0]).center();
    let scenes = visual.debug_bounds("session-scenes-header").unwrap();
    let edge = point(scenes.origin.x - px(2.0), from.y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    turn_page(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(v.session_track_start > 0);
        assert!(v.track_reorder_active());
    });
    let target = header(&mut visual, target_group).center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[0])
                .unwrap()
                .parent_group(),
            Some(target_group)
        )
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, cx| {
        v.session_track_start = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let from = header(&mut visual, ids[0]).center();
    let scenes = visual.debug_bounds("session-scenes-header").unwrap();
    let edge = point(scenes.origin.x - px(2.0), from.y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    turn_page(&mut visual, &editor);
    let target = header(&mut visual, target_group);
    let to = point(target.origin.x + px(2.0), target.center().y);
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("console-track-insertion-line")
            .is_some()
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        let tracks = v.state.as_ref().unwrap().project.tracks();
        let source = tracks
            .iter()
            .position(|track| track.id() == ids[0])
            .unwrap();
        let group = tracks
            .iter()
            .position(|track| track.id() == target_group)
            .unwrap();
        assert_eq!(source + 1, group);
        assert_eq!(tracks[source].parent_group(), None);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn console_page_turn_rejects_fold_and_window_changes(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 8, false);
    let group = group(&mut visual, &editor, &ids[..1]);
    switch(&mut visual, &editor);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for change in ["fold", "window"] {
        editor.update(&mut visual, |v, cx| {
            v.session_track_start = 0;
            cx.notify();
        });
        visual.run_until_parked();
        let from = header(&mut visual, ids[1]).center();
        let scenes = visual.debug_bounds("session-scenes-header").unwrap();
        let edge = point(scenes.origin.x - px(2.0), from.y);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.update(&mut visual, |v, cx| {
            assert!(
                v.tick_track_reorder(std::time::Instant::now() + std::time::Duration::from_secs(1))
            );
            if change == "fold" {
                v.arrangement_focus.projection.toggle_fold(group);
                v.reconcile_arrangement_focus();
            }
            cx.notify();
        });
        if change == "window" {
            visual.simulate_resize(size(px(1120.0), px(800.0)));
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(!v.track_reorder_active());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
