//! Console header selection is visible, fold-aware, and shared with drag.
use super::{
    EditorMode, EditorView,
    track_reorder_ui_test::{live_tracks, receive},
};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{Command, TrackId, TrackSelectionIds};
use hex_session::service::Request;
fn header(visual: &mut VisualTestContext, id: TrackId) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", id.value()).into_boxed_str(),
        ))
        .unwrap()
}
fn click(visual: &mut VisualTestContext, id: TrackId, modifiers: Modifiers) {
    let at = header(visual, id).center();
    visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
fn group(visual: &mut VisualTestContext, editor: &Entity<EditorView>, ids: &[TrackId]) -> TrackId {
    editor.update(visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(ids).unwrap(),
            name: "group".into()
        })))
    });
    receive(visual, editor);
    editor.read_with(visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.is_group())
            .unwrap()
            .id()
    })
}
fn session(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    visual.update(|w, cx| {
        editor.update(cx, |v, cx| {
            v.set_mode(EditorMode::Session);
            v.focus.focus(w);
            cx.notify();
        })
    });
    visual.run_until_parked();
}
#[gpui::test]
fn console_multi_selection_is_visible_and_drags_as_one_history_step(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 4, false);
    let group = group(&mut visual, &editor, &ids[..1]);
    session(&mut visual, &editor);
    click(&mut visual, ids[1], Modifiers::default());
    click(
        &mut visual,
        ids[2],
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [ids[1], ids[2]])
    });
    for id in [ids[1], ids[2]] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("selected-console-track-{}", id.value()).into_boxed_str()
                ))
                .is_some()
        );
    }
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let from = header(&mut visual, ids[2]).center();
    let to = header(&mut visual, group).center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("console-track-group-target").is_some());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        for id in [ids[1], ids[2]] {
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(id)
                    .unwrap()
                    .parent_group(),
                Some(group)
            );
        }
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.update(&mut visual, |v, cx| {
        v.set_mode(EditorMode::Arrangement);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.selected_track_ids(), [ids[1], ids[2]]);
    });
}

#[gpui::test]
fn console_solo_applies_to_the_selected_tracks(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    session(&mut visual, &editor);
    click(&mut visual, ids[0], Modifiers::default());
    click(
        &mut visual,
        ids[1],
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track_ids(), [ids[0], ids[1]])
    });
    let solo = visual
        .debug_bounds(Box::leak(
            format!("console-track-solo-{}", ids[1].value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(solo, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(solo, MouseButton::Left, Modifiers::default());
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.track(ids[0]).unwrap().soloed());
        assert!(project.track(ids[1]).unwrap().soloed());
        assert!(!project.track(ids[2]).unwrap().soloed());
        assert_eq!(view.selected_track_ids(), [ids[0], ids[1]]);
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(!project.track(ids[0]).unwrap().soloed());
        assert!(!project.track(ids[1]).unwrap().soloed());
    });
}
#[gpui::test]
fn console_shift_range_respects_folded_tracks_and_retains_additive_hidden_selection(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, ids) = live_tracks(cx, 5, false);
    let group = group(&mut visual, &editor, &ids[..2]);
    session(&mut visual, &editor);
    click(&mut visual, ids[0], Modifiers::default());
    let fold = visual
        .debug_bounds(Box::leak(
            format!("console-group-fold-{}", group.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(fold, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(fold, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), [ids[0]]));
    click(
        &mut visual,
        ids[4],
        Modifiers {
            shift: true,
            platform: true,
            ..Default::default()
        },
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [ids[0], ids[2], ids[3], ids[4]])
    });
    click(&mut visual, group, Modifiers::default());
    click(
        &mut visual,
        ids[4],
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), [group, ids[2], ids[3], ids[4]])
    });
}
