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
