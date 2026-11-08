//! Group subtree deletion remains explicit, focus-scoped and undoable.
use super::{
    EditorMode, EditorView,
    track_reorder_ui_test::{live_tracks, receive},
};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{Command, TrackId, TrackSelectionIds};
use hex_session::service::Request;
fn group(visual: &mut VisualTestContext, editor: &Entity<EditorView>, ids: &[TrackId]) -> TrackId {
    editor.update(visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(ids).unwrap(),
            name: "group".into()
        })))
    });
    receive(visual, editor);
    editor.read_with(visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        project
            .tracks()
            .iter()
            .find(|t| {
                t.is_group()
                    && ids
                        .iter()
                        .all(|id| project.track(*id).unwrap().parent_group() == Some(t.id()))
            })
            .unwrap()
            .id()
    })
}
fn setup(
    cx: &mut TestAppContext,
    mode: EditorMode,
) -> (VisualTestContext, Entity<EditorView>, Vec<TrackId>, TrackId) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, true);
    let inner = group(&mut visual, &editor, &ids[..2]);
    let outer = group(&mut visual, &editor, &[inner]);
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: gpui::px(0.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    if mode == EditorMode::Session {
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.set_mode(mode);
                v.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
    }
    (visual, editor, ids, outer)
}
fn history(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    before: &hex_project::Snapshot,
    survivor: TrackId,
) {
    receive(visual, editor);
    editor.read_with(visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            [survivor]
        )
    });
    editor.update(visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(visual, editor);
    editor.read_with(visual, |v, _| {
        assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(visual, |v, _| assert!(v.enqueue(Request::Redo)));
    receive(visual, editor);
    editor.read_with(visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            [survivor]
        )
    });
}
fn context_delete(cx: &mut TestAppContext, mode: EditorMode) {
    let (mut visual, editor, ids, outer) = setup(cx, mode);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let prefix = if mode == EditorMode::Session {
        "session-track"
    } else {
        "arrangement-track-header"
    };
    let selector = Box::leak(format!("{prefix}-{}", outer.value()).into_boxed_str());
    let at = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let index = editor.read_with(&visual, |v, _| {
        let menu = &v.context_menu.as_ref().unwrap().items;
        assert!(menu.iter().any(|item| item.label == "ungroup tracks"));
        menu.iter()
            .position(|item| item.label == "delete group and tracks")
            .expect("explicit group subtree deletion")
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    history(&mut visual, &editor, &before, ids[2]);
}
#[gpui::test]
fn arrangement_group_context_deletes_subtree_and_restores_history(cx: &mut TestAppContext) {
    context_delete(cx, EditorMode::Arrangement);
}
#[gpui::test]
fn console_group_context_deletes_subtree_and_restores_history(cx: &mut TestAppContext) {
    context_delete(cx, EditorMode::Session);
}
#[gpui::test]
fn console_group_header_backspace_deletes_subtree_and_restores_history(cx: &mut TestAppContext) {
    let (mut visual, editor, ids, outer) = setup(cx, EditorMode::Session);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", outer.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.worker_requests_pending, 1,
            "header delete must submit one edit"
        )
    });
    history(&mut visual, &editor, &before, ids[2]);
}
#[gpui::test]
fn console_group_delete_refuses_during_track_drag(cx: &mut TestAppContext) {
    let (mut visual, editor, _ids, outer) = setup(cx, EditorMode::Session);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", outer.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let inner = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| track.is_group() && track.parent_group() == Some(outer))
            .unwrap()
            .id()
    });
    let target = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", inner.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.track_reorder_active()));
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(!v.track_reorder_active()));
}
#[gpui::test]
fn console_backspace_deletes_selected_group_and_sibling_in_one_history_step(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, ids, outer) = setup(cx, EditorMode::Session);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let fold = visual
        .debug_bounds(Box::leak(
            format!("console-group-fold-{}", outer.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(fold, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(fold, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for (id, toggle) in [(outer, false), (ids[2], true)] {
        let at = visual
            .debug_bounds(Box::leak(
                format!("session-track-{}", id.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        let modifiers = Modifiers {
            platform: toggle,
            ..Default::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [outer, ids[2]])
    });
    visual.simulate_keystrokes("backspace");
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.tracks().is_empty())
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
