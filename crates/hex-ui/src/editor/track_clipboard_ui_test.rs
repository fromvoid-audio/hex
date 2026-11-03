//! Real header shortcuts/menu routing and exact delayed ordinary completion.
use super::{
    EditorView,
    track_reorder_ui_test::{bounds, live_tracks, live_tracks_with_notes, receive},
};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::TrackId;
fn select(v: &mut VisualTestContext, id: TrackId, toggle: bool) {
    let at = bounds(v, id).center();
    let modifiers = Modifiers {
        platform: toggle,
        ..Default::default()
    };
    v.simulate_mouse_down(at, MouseButton::Left, modifiers);
    v.simulate_mouse_up(at, MouseButton::Left, modifiers);
    v.run_until_parked();
}
fn snapshot(v: &VisualTestContext, e: &Entity<EditorView>) -> hex_project::Snapshot {
    e.read_with(v, |e, _| e.state.as_ref().unwrap().project.snapshot())
}

#[gpui::test]
fn g_groups_focused_track_headers_with_undo_and_redo(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[1], true);
    let before = snapshot(&visual, &editor);
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(hex_project::Command::GroupTracks { .. })
        ));
    });
    receive(&mut visual, &editor);
    let after = snapshot(&visual, &editor);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let group = project
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap();
        for id in &ids {
            assert_eq!(project.track(*id).unwrap().parent_group(), Some(group.id()));
        }
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Undo))
    });
    receive(&mut visual, &editor);
    let mut restored = snapshot(&visual, &editor);
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Redo))
    });
    receive(&mut visual, &editor);
    assert_eq!(snapshot(&visual, &editor), after);
}

#[gpui::test]
fn g_groups_session_track_headers(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[1], true);
    editor.update(&mut visual, |view, cx| {
        view.mode = super::EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("session-track-{}", ids[1].value()).into_boxed_str());
    let at = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(
        at,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_click(
        at,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit,
            Some(hex_project::Command::GroupTracks { .. })
        ));
    });
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().project.has_group_tracks());
    });
}

#[gpui::test]
fn g_does_not_group_a_single_track_or_track_name_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    select(&mut visual, ids[0], false);
    let before = snapshot(&visual, &editor);
    visual.simulate_keystrokes("g");
    assert_eq!(snapshot(&visual, &editor), before);
    select(&mut visual, ids[1], true);
    visual.simulate_keystrokes("cmd-r g");
    editor.read_with(&visual, |view, _| {
        assert!(view.track_name.is_editing());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!matches!(
            view.last_enqueued_edit,
            Some(hex_project::Command::GroupTracks { .. })
        ));
    });
}

#[gpui::test]
fn g_refuses_tracks_with_different_parents_without_partial_group(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[1], true);
    visual.simulate_keystrokes("g");
    receive(&mut visual, &editor);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[2], true);
    let before = snapshot(&visual, &editor);
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert!(view.worker_requests_pending > 0);
    });
    editor.update(&mut visual, |view, cx| {
        let event = super::track_reorder_ui_test::next(&view.service);
        assert!(event.error.is_some());
        view.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(snapshot(&visual, &editor), before);
}

fn group_track_header_menu(cx: &mut TestAppContext, mode: super::EditorMode) {
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[1], true);
    editor.update(&mut visual, |view, cx| {
        view.mode = mode;
        cx.notify();
    });
    visual.run_until_parked();
    let prefix = if mode == super::EditorMode::Arrangement {
        "arrangement-track-header"
    } else {
        "session-track"
    };
    let right_click = |visual: &mut VisualTestContext, id: TrackId| {
        let selector = Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
        let point = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(point, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
    };
    let choose = |visual: &mut VisualTestContext, label: &str| {
        let index = editor.read_with(visual, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == label)
                .unwrap()
        });
        for _ in 0..index {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
        receive(visual, &editor);
    };
    right_click(&mut visual, ids[0]);
    choose(&mut visual, "group tracks");
    let group = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let group = project
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap();
        for id in &ids {
            assert_eq!(project.track(*id).unwrap().parent_group(), Some(group.id()));
        }
        group.id()
    });
    let selector = Box::leak(format!("group-track-marker-{}", group.value()).into_boxed_str());
    assert!(visual.debug_bounds(selector).is_some());
    for id in &ids {
        let selector = Box::leak(format!("track-hierarchy-indent-{}", id.value()).into_boxed_str());
        assert_eq!(
            f32::from(visual.debug_bounds(selector).unwrap().size.width),
            4.0
        );
    }
    if mode == super::EditorMode::Arrangement {
        let fold = Box::leak(format!("group-fold-{}", group.value()).into_boxed_str());
        let click_fold = |visual: &mut VisualTestContext| {
            let point = visual.debug_bounds(fold).unwrap().center();
            visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        };
        let before = snapshot(&visual, &editor);
        let selected = editor.read_with(&visual, |view, _| view.selected_track_ids());
        click_fold(&mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track_ids(), selected);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert!(view.arrangement_track_visible(group));
            assert!(ids.iter().all(|id| !view.arrangement_track_visible(*id)));
            assert_eq!(
                view.focused_neighbor_track(true).map(|(_, track, _)| track),
                Some(group)
            );
        });
        for id in &ids {
            let selector =
                Box::leak(format!("arrangement-track-header-{}", id.value()).into_boxed_str());
            assert!(visual.debug_bounds(selector).is_none());
        }
        click_fold(&mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(ids.iter().all(|id| view.arrangement_track_visible(*id)));
            assert_eq!(view.selected_track_ids(), selected);
        });
    }
    if mode == super::EditorMode::Session {
        let fold = Box::leak(format!("console-group-fold-{}", group.value()).into_boxed_str());
        let click_fold = |visual: &mut VisualTestContext| {
            let point = visual.debug_bounds(fold).unwrap().center();
            visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        };
        let before = snapshot(&visual, &editor);
        let selected = editor.read_with(&visual, |view, _| view.selected_track_ids());
        click_fold(&mut visual);
        for id in &ids {
            let selector = Box::leak(format!("session-track-{}", id.value()).into_boxed_str());
            assert!(visual.debug_bounds(selector).is_none());
        }
        editor.update(&mut visual, |view, cx| {
            view.set_mode(super::EditorMode::Arrangement);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(ids.iter().all(|id| !view.arrangement_track_visible(*id)));
            assert_eq!(view.selected_track_ids(), selected);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
        editor.update(&mut visual, |view, cx| {
            view.set_mode(super::EditorMode::Session);
            cx.notify();
        });
        visual.run_until_parked();
        click_fold(&mut visual);
        for id in &ids {
            let selector = Box::leak(format!("session-track-{}", id.value()).into_boxed_str());
            assert!(visual.debug_bounds(selector).is_some());
        }
    }
    right_click(&mut visual, group);
    choose(&mut visual, "ungroup tracks");
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.track(group).is_none());
        for id in &ids {
            assert_eq!(project.track(*id).unwrap().parent_group(), None);
        }
    });
    for id in &ids {
        let selector = Box::leak(format!("track-hierarchy-indent-{}", id.value()).into_boxed_str());
        assert!(visual.debug_bounds(selector).is_none());
    }
    visual.simulate_keystrokes("cmd-z");
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(group)
                .unwrap()
                .is_group()
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    receive(&mut visual, &editor);
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().project.track(group).is_none())
    });
}

#[gpui::test]
fn group_track_header_menu_arrangement(cx: &mut TestAppContext) {
    group_track_header_menu(cx, super::EditorMode::Arrangement);
}

#[gpui::test]
fn group_track_header_menu_session(cx: &mut TestAppContext) {
    group_track_header_menu(cx, super::EditorMode::Session);
}

#[gpui::test]
fn console_fold_hides_cells_navigation_and_invalidates_column_frame(cx: &mut TestAppContext) {
    use hex_project::{Command, TrackSelectionIds};
    use hex_session::service::Request;
    let (mut visual, editor, ids) = live_tracks(cx, 8, false);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..2]).unwrap(),
            name: "group".into(),
        })));
    });
    receive(&mut visual, &editor);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddScene {
            name: "scene".into()
        })))
    });
    receive(&mut visual, &editor);
    let (group, scene) = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (
            project
                .tracks()
                .iter()
                .find(|track| track.is_group())
                .unwrap()
                .id(),
            project.scenes()[0].id(),
        )
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.set_mode(super::EditorMode::Session);
            view.selected_cell = Some((ids[0], scene));
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let frame = visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            super::console_columns::ColumnFrame::capture(
                view,
                window,
                view.state.as_ref().unwrap().revision,
            )
        })
    });
    let fold = Box::leak(format!("console-group-fold-{}", group.value()).into_boxed_str());
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(
                !frame.current(view, window),
                "old drop geometry cannot survive a fold"
            );
            assert_eq!(
                view.selected_cell,
                Some((ids[0], scene)),
                "fold retains hidden selection"
            );
        })
    });
    for id in &ids[..2] {
        let cell =
            Box::leak(format!("console-cell-{}-{}", id.value(), scene.value()).into_boxed_str());
        assert!(visual.debug_bounds(cell).is_none());
    }
    let next_cell =
        Box::leak(format!("console-cell-{}-{}", ids[2].value(), scene.value()).into_boxed_str());
    assert!(visual.debug_bounds(next_cell).is_some());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_cell, Some((ids[2], scene)))
    });
    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_cell, Some((group, scene)))
    });
    let next_page = visual.debug_bounds("next tracks").unwrap().center();
    visual.simulate_mouse_down(next_page, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(next_page, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.session_track_start > 0));
    let last_header = Box::leak(format!("session-track-{}", ids[7].value()).into_boxed_str());
    let last_cell =
        Box::leak(format!("console-cell-{}-{}", ids[7].value(), scene.value()).into_boxed_str());
    let last_edge =
        Box::leak(format!("console-column-header-before-{}", ids[7].value()).into_boxed_str());
    assert!(visual.debug_bounds(last_header).is_some());
    assert!(visual.debug_bounds(last_cell).is_some());
    assert!(visual.debug_bounds(last_edge).is_some());
}

#[gpui::test]
fn track_clipboard_copy_and_duplicate_from_header(cx: &mut TestAppContext) {
    let (mut v, e, ids) = live_tracks_with_notes(cx, 3, 1);
    select(&mut v, ids[0], false);
    select(&mut v, ids[2], true);
    let original = snapshot(&v, &e);
    v.simulate_keystrokes("cmd-c");
    receive(&mut v, &e);
    assert_eq!(
        snapshot(&v, &e),
        original,
        "copy has no project/history edit"
    );
    v.simulate_keystrokes("cmd-d");
    for _ in 0..3 {
        v.run_until_parked();
        assert_eq!(snapshot(&v, &e), original);
    }
    receive(&mut v, &e);
    let copied = e.read_with(&v, |e, _| e.selected_track_ids());
    assert_eq!(copied.len(), 2);
    assert!(copied.iter().all(|id| !ids.contains(id)));
    e.read_with(&v, |e, _| {
        let p = &e.state.as_ref().unwrap().project;
        assert_eq!(p.tracks().len(), 5);
        assert_eq!(p.track(copied[0]).unwrap().name(), "track 0");
        assert_eq!(p.track(copied[1]).unwrap().name(), "track 2");
        assert_eq!(
            p.track(copied[0]).unwrap().midi_clips()[0]
                .source()
                .notes()
                .len(),
            1
        );
    });
    v.simulate_keystrokes("cmd-z");
    receive(&mut v, &e);
}

#[gpui::test]
fn track_clipboard_cut_paste_and_setup_only_menu(cx: &mut TestAppContext) {
    let (mut v, e, ids) = live_tracks_with_notes(cx, 3, 1);
    select(&mut v, ids[0], false);
    select(&mut v, ids[2], true);
    v.simulate_keystrokes("cmd-x");
    receive(&mut v, &e);
    assert_eq!(snapshot(&v, &e).tracks.len(), 1);
    v.simulate_keystrokes("cmd-v");
    receive(&mut v, &e);
    assert_eq!(snapshot(&v, &e).tracks.len(), 3);
    let pasted = e.read_with(&v, |e, _| e.selected_track_ids());
    assert_eq!(pasted.len(), 2);
    // Right-click a selected member retains the sparse block as the menu target.
    let at = bounds(&mut v, pasted[0]).center();
    v.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    v.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    v.run_until_parked();
    let index = e.read_with(&v, |e, _| {
        e.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|i| i.label == "duplicate tracks without clips")
            .unwrap()
    });
    for _ in 0..index {
        v.simulate_keystrokes("down");
    }
    v.simulate_keystrokes("enter");
    receive(&mut v, &e);
    e.read_with(&v, |e, _| {
        let selected = e.selected_track_ids();
        assert_eq!(selected.len(), 2);
        let p = &e.state.as_ref().unwrap().project;
        assert_eq!(p.tracks().len(), 5);
        for id in selected {
            let track = p.track(id).unwrap();
            assert!(track.clips().is_empty() && track.midi_clips().is_empty());
        }
        for id in pasted {
            assert_eq!(p.track(id).unwrap().midi_clips().len(), 1);
        }
    });
    // An unselected context target is a singleton even while a block is selected.
    let at = bounds(&mut v, ids[1]).center();
    v.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    v.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    v.run_until_parked();
    e.read_with(&v, |e, _| {
        use super::context_menu::EditorContextAction;
        use hex_session::service::TrackClipboardAction;
        let menu = e.context_menu.as_ref().unwrap();
        let item = menu
            .items
            .iter()
            .find(|i| i.label == "duplicate tracks without clips")
            .unwrap();
        let EditorContextAction::TrackClipboard {
            action: TrackClipboardAction::Duplicate { tracks, .. },
            ..
        } = &item.action
        else {
            panic!("duplicate action")
        };
        assert_eq!(tracks.ids(), &[ids[1]]);
    });
    v.simulate_keystrokes("escape");
}

#[gpui::test]
fn track_clipboard_keys_stay_in_name_editor(cx: &mut TestAppContext) {
    let (mut v, e, ids) = live_tracks(cx, 3, false);
    select(&mut v, ids[1], false);
    v.simulate_keystrokes("cmd-r");
    v.run_until_parked();
    assert!(e.read_with(&v, |e, _| e.track_name.is_editing()));
    let before = snapshot(&v, &e);
    for key in ["cmd-c", "cmd-x", "cmd-v", "cmd-d"] {
        v.simulate_keystrokes(key);
    }
    v.run_until_parked();
    e.read_with(&v, |e, _| assert!(e.pending_track_clipboard.is_none()));
    assert_eq!(
        snapshot(&v, &e),
        before,
        "text editing owns its clipboard keys"
    );
    v.simulate_keystrokes("escape");
}
#[gpui::test]
fn track_clipboard_delayed_completion_preserves_newer_header_selection(cx: &mut TestAppContext) {
    let (mut v, e, ids) = live_tracks(cx, 3, true);
    select(&mut v, ids[0], false);
    select(&mut v, ids[2], true);
    let original = snapshot(&v, &e);
    v.simulate_keystrokes("cmd-d");
    select(&mut v, ids[1], false);
    for _ in 0..3 {
        v.run_until_parked();
        assert_eq!(snapshot(&v, &e), original);
    }
    receive(&mut v, &e);
    assert_eq!(
        snapshot(&v, &e).tracks.len(),
        5,
        "admitted copy still commits"
    );
    assert_eq!(
        e.read_with(&v, |e, _| e.selected_track_ids()),
        vec![ids[1]],
        "exact terminal cannot steal a newer selection"
    );
    assert!(e.read_with(&v, |e, _| e.pending_track_clipboard.is_none()));
}

#[gpui::test]
fn track_clipboard_cut_all_then_paste_immediately_restores_tracks_and_keeps_clip_context_separate(
    cx: &mut TestAppContext,
) {
    let (mut v, e, ids) = live_tracks(cx, 3, false);
    for (i, id) in ids.iter().enumerate() {
        select(&mut v, *id, i != 0);
    }
    v.simulate_keystrokes("cmd-x");
    receive(&mut v, &e);
    assert!(snapshot(&v, &e).tracks.is_empty());
    v.simulate_keystrokes("cmd-v");
    assert!(
        e.read_with(&v, |e, _| e.pending_track_clipboard.is_some()),
        "paste is dispatched after the focused track is removed"
    );
    receive(&mut v, &e);
    assert_eq!(
        snapshot(&v, &e).tracks.len(),
        3,
        "paste keeps the track context after cut removed the focused header"
    );
    let restored = e.read_with(&v, |e, _| e.selected_track_ids());
    assert_eq!(restored.len(), 3);
    assert!(restored.iter().all(|id| !ids.contains(id)));
    v.simulate_keystrokes("cmd-d");
    receive(&mut v, &e);
    assert_eq!(
        snapshot(&v, &e).tracks.len(),
        6,
        "track editing continues after the pasted selection"
    );
    let lane = v
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", restored[0].value()).into_boxed_str(),
        ))
        .unwrap();
    let at = lane.center();
    v.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    v.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    v.run_until_parked();
    v.simulate_keystrokes("cmd-d");
    v.run_until_parked();
    e.read_with(&v, |e, _| {
        assert!(e.pending_track_clipboard.is_none());
        assert_eq!(
            e.worker_requests_pending, 0,
            "a fresh empty-lane selection owns clip/time input, not track duplication"
        );
    });
    assert_eq!(snapshot(&v, &e).tracks.len(), 6);
}

#[gpui::test]
fn track_clipboard_context_menu_copy_then_keyboard_paste_retains_its_input_scope(
    cx: &mut TestAppContext,
) {
    let (mut v, e, ids) = live_tracks(cx, 3, false);
    let before = snapshot(&v, &e);
    let at = bounds(&mut v, ids[1]).center();
    v.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    v.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    v.run_until_parked();
    let index = e.read_with(&v, |e, _| {
        e.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|i| i.label == "copy tracks · cmd+c")
            .unwrap()
    });
    for _ in 0..index {
        v.simulate_keystrokes("down");
    }
    v.simulate_keystrokes("enter");
    receive(&mut v, &e);
    assert_eq!(snapshot(&v, &e), before);
    v.simulate_keystrokes("cmd-v");
    assert!(e.read_with(&v, |e, _| e.pending_track_clipboard.is_some()));
    receive(&mut v, &e);
    e.read_with(&v, |e, _| {
        let ids = e.selected_track_ids();
        assert_eq!(ids.len(), 1);
        let state = e.state.as_ref().unwrap();
        assert_eq!(state.project.tracks().len(), 4);
        assert_eq!(state.project.track(ids[0]).unwrap().name(), "track 1");
    });
}
