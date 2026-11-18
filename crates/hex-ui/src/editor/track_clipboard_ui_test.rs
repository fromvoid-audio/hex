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
fn nested_group_plates_and_poles_join_follow_selection_and_close_at_visible_ends(
    cx: &mut TestAppContext,
) {
    use gpui::{px, size};
    use hex_project::{Command, ElementColor, TrackSelectionIds};
    use hex_session::service::Request;
    fn selector(prefix: &str, id: TrackId) -> &'static str {
        Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str())
    }
    fn rail(track: TrackId, group: TrackId) -> &'static str {
        Box::leak(format!("track-group-rail-{}-{}", track.value(), group.value()).into_boxed_str())
    }
    let (mut visual, editor, ids) = live_tracks(cx, 3, false);
    visual.simulate_resize(size(px(1440.0), px(1100.0)));
    let edit = |visual: &mut VisualTestContext, command| {
        editor.update(visual, |view, _| {
            assert!(view.enqueue(Request::Edit(command)))
        });
        receive(visual, &editor);
    };
    edit(
        &mut visual,
        Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..2]).unwrap(),
            name: "inner".into(),
        },
    );
    let inner = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|track| track.name() == "inner")
            .unwrap()
            .id()
    });
    edit(
        &mut visual,
        Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[inner, ids[2]]).unwrap(),
            name: "outer".into(),
        },
    );
    let outer = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    edit(
        &mut visual,
        Command::SetTrackColor {
            track: outer,
            color: ElementColor::Red,
        },
    );
    edit(
        &mut visual,
        Command::SetTrackColor {
            track: inner,
            color: ElementColor::Violet,
        },
    );
    visual.run_until_parked();
    let fold_button = selector("group-fold", outer);
    let unselected_fold_bounds = visual.debug_bounds(fold_button).unwrap();
    select(&mut visual, outer, false);
    assert_eq!(
        visual.debug_bounds(fold_button),
        Some(unselected_fold_bounds)
    );
    let before = snapshot(&visual, &editor);
    let scale = visual.update(|window, _| window.scale_factor());
    let check_rail =
        |visual: &mut VisualTestContext, track: TrackId, group: TrackId, last: bool| {
            let key = rail(track, group);
            let b = visual.debug_bounds(key).unwrap();
            let header = visual
                .debug_bounds(selector("arrangement-track-header", track))
                .unwrap();
            assert_eq!(b.top(), header.top());
            assert_eq!(b.bottom(), header.bottom());
            assert_eq!(b.size.width, px(4.0));
            let (corners, _) = visual.rendered_surface_geometry(key).unwrap();
            assert_eq!(
                f64::from(corners.bottom_left),
                if last { 2.0 * f64::from(scale) } else { 0.0 }
            );
            let plate = visual.rendered_background(selector("track-name-plate", group));
            let pole = visual.rendered_background(key).expect("group pole fill");
            assert!(
                plate == Some(pole),
                "the group plate and its entire pole use the same selected/unselected fill"
            );
        };
    for track in [outer, inner] {
        let plate = visual
            .debug_bounds(selector("track-name-plate", track))
            .unwrap();
        let header = visual
            .debug_bounds(selector("arrangement-track-header", track))
            .unwrap();
        assert_eq!(plate.top(), header.top());
        assert_eq!(plate.bottom(), header.bottom());
        let own_rail = visual
            .debug_bounds(selector("track-color-strip", track))
            .unwrap();
        assert_eq!(plate.left(), own_rail.right(), "title joins its own rail");
        assert_eq!(plate.top(), own_rail.top());
        assert!(
            !visual
                .rendered_background(selector("track-name-plate", track))
                .is_none(),
            "group name plate is painted"
        );
    }
    // Resizing a group grows only its plate, keeping the working controls
    // in the compact top slot instead of following the middle of the row.
    let original_height = visual
        .debug_bounds(selector("arrangement-track-header", outer))
        .unwrap()
        .size
        .height;
    let edge = visual
        .debug_bounds(selector("track-height-resize", outer))
        .unwrap()
        .center();
    let to = gpui::point(edge.x, edge.y + px(80.0));
    visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let grown = visual
        .debug_bounds(selector("arrangement-track-header", outer))
        .unwrap();
    assert_eq!(grown.size.height, original_height + px(88.0));
    let name = visual.debug_bounds(selector("track-name", outer)).unwrap();
    assert!(
        name.top() - grown.top() <= px(4.0),
        "group controls stay in their top slot"
    );
    assert_eq!(
        visual
            .debug_bounds(selector("track-name-area", outer))
            .unwrap()
            .size
            .height,
        grown.size.height
    );
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.simulate_keystrokes("cmd-h");
    visual.run_until_parked();
    assert_eq!(
        visual
            .debug_bounds(selector("arrangement-track-header", outer))
            .unwrap()
            .size
            .height,
        original_height
    );
    for (track, group, last) in [
        (inner, outer, false),
        (ids[0], outer, false),
        (ids[1], outer, false),
        (ids[2], outer, true),
        (ids[0], inner, false),
        (ids[1], inner, true),
    ] {
        check_rail(&mut visual, track, group, last);
    }
    for pair in [outer, inner, ids[0], ids[1], ids[2]].windows(2) {
        assert_eq!(
            visual
                .debug_bounds(selector("arrangement-track-header", pair[0]))
                .unwrap()
                .bottom(),
            visual
                .debug_bounds(selector("arrangement-track-header", pair[1]))
                .unwrap()
                .top()
        );
    }
    select(&mut visual, ids[2], false);
    check_rail(&mut visual, inner, outer, false);
    check_rail(&mut visual, ids[2], outer, true);
    let fold = |visual: &mut VisualTestContext, group| {
        let at = visual
            .debug_bounds(selector("group-fold", group))
            .unwrap()
            .center();
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };
    fold(&mut visual, inner);
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", ids[0]))
            .is_none()
    );
    let (corners, _) = visual
        .rendered_surface_geometry(selector("track-color-strip", inner))
        .unwrap();
    assert_eq!(f64::from(corners.bottom_left), 2.0 * f64::from(scale));
    check_rail(&mut visual, inner, outer, false);
    check_rail(&mut visual, ids[2], outer, true);
    fold(&mut visual, outer);
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", inner))
            .is_none()
    );
    let (corners, _) = visual
        .rendered_surface_geometry(selector("track-color-strip", outer))
        .unwrap();
    assert_eq!(f64::from(corners.bottom_left), 2.0 * f64::from(scale));
    fold(&mut visual, outer);
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", inner))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", ids[0]))
            .is_none(),
        "inner fold survives reopening its parent"
    );
    assert_eq!(snapshot(&visual, &editor), before);
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
fn g_groups_a_single_track_in_arrangement_and_console(cx: &mut TestAppContext) {
    use super::EditorMode;
    use hex_session::service::Request;
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        let (mut visual, editor, ids) = live_tracks(cx, 2, false);
        if mode == EditorMode::Session {
            editor.update(&mut visual, |view, cx| {
                view.set_mode(mode);
                cx.notify();
            });
            visual.run_until_parked();
            let header = Box::leak(format!("session-track-{}", ids[0].value()).into_boxed_str());
            let at = visual.debug_bounds(header).unwrap().center();
            visual.simulate_click(at, Modifiers::default());
        } else {
            select(&mut visual, ids[0], false);
        }
        let before = snapshot(&visual, &editor);
        visual.simulate_keystrokes("g");
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                &view.last_enqueued_edit,
                Some(hex_project::Command::GroupTracks { tracks, .. }) if tracks.ids() == &ids[..1]
            ));
        });
        receive(&mut visual, &editor);
        let after = snapshot(&visual, &editor);
        editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let [group, child, sibling] = project.tracks() else {
                panic!("expected a group, its child and the unselected track");
            };
            assert!(group.is_group());
            assert_eq!(child.id(), ids[0]);
            assert_eq!(child.parent_group(), Some(group.id()));
            assert_eq!(sibling.id(), ids[1]);
            assert_eq!(sibling.parent_group(), None);
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        receive(&mut visual, &editor);
        let mut restored = snapshot(&visual, &editor);
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
        receive(&mut visual, &editor);
        assert_eq!(snapshot(&visual, &editor), after);
    }
}

#[gpui::test]
fn g_does_not_group_during_track_name_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, ids) = live_tracks(cx, 2, false);
    select(&mut visual, ids[0], false);
    select(&mut visual, ids[1], true);
    let before = snapshot(&visual, &editor);
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
fn grouping_mixed_parents_uses_the_last_selected_header(cx: &mut TestAppContext) {
    for anchor_inside in [false, true] {
        let (mut visual, editor, ids) = live_tracks(cx, 4, false);
        let [inside, sibling, between, outside] = ids.try_into().unwrap();
        select(&mut visual, inside, false);
        select(&mut visual, sibling, true);
        visual.simulate_keystrokes("g");
        receive(&mut visual, &editor);
        let parent = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[0].id()
        });
        let (first, last) = if anchor_inside {
            (outside, inside)
        } else {
            (inside, outside)
        };
        select(&mut visual, first, false);
        select(&mut visual, last, true);
        if anchor_inside {
            let point = bounds(&mut visual, first).center();
            visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(point, MouseButton::Right, Modifiers::default());
            visual.run_until_parked();
            let index = editor.read_with(&visual, |view, _| {
                view.context_menu
                    .as_ref()
                    .unwrap()
                    .items
                    .iter()
                    .position(|item| item.label == "group tracks")
                    .unwrap()
            });
            for _ in 0..index {
                visual.simulate_keystrokes("down");
            }
            visual.simulate_keystrokes("enter");
        } else {
            visual.simulate_keystrokes("g");
        }
        receive(&mut visual, &editor);
        editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let group = project.track(inside).unwrap().parent_group().unwrap();
            assert_ne!(group, parent);
            assert_eq!(project.track(outside).unwrap().parent_group(), Some(group));
            assert_eq!(project.track(sibling).unwrap().parent_group(), Some(parent));
            assert_eq!(
                project.track(group).unwrap().parent_group(),
                anchor_inside.then_some(parent)
            );
            let expected = if anchor_inside {
                [parent, group, inside, outside, sibling, between]
            } else {
                [parent, sibling, between, group, inside, outside]
            };
            assert!(project.tracks().iter().map(|track| track.id()).eq(expected));
        });
    }
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
    for id in &ids {
        let selector = Box::leak(format!("track-hierarchy-indent-{}", id.value()).into_boxed_str());
        assert_eq!(
            f32::from(visual.debug_bounds(selector).unwrap().size.width),
            4.0
        );
    }
    if mode == super::EditorMode::Arrangement {
        for id in &ids {
            let rail = Box::leak(
                format!("track-group-rail-{}-{}", id.value(), group.value()).into_boxed_str(),
            );
            let header =
                Box::leak(format!("arrangement-track-header-{}", id.value()).into_boxed_str());
            let rail = visual
                .debug_bounds(rail)
                .expect("member has its group rail");
            let header = visual.debug_bounds(header).unwrap();
            assert_eq!(f32::from(rail.size.width), 4.0);
            assert!(f32::from(rail.size.height) >= f32::from(header.size.height) - 2.0);
            let lane_rail = Box::leak(
                format!("lane-group-rail-{}-{}", id.value(), group.value()).into_boxed_str(),
            );
            let lane = Box::leak(format!("arrangement-lane-{}", id.value()).into_boxed_str());
            let lane_rail = visual
                .debug_bounds(lane_rail)
                .expect("member lane has its group cue");
            let lane = visual.debug_bounds(lane).unwrap();
            assert_eq!(f32::from(lane_rail.size.width), 2.0);
            assert!(f32::from(lane_rail.size.height) >= f32::from(lane.size.height) - 2.0);
        }
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
            let rail = Box::leak(
                format!("track-group-rail-{}-{}", id.value(), group.value()).into_boxed_str(),
            );
            assert!(visual.debug_bounds(rail).is_none());
            let lane_rail = Box::leak(
                format!("lane-group-rail-{}-{}", id.value(), group.value()).into_boxed_str(),
            );
            assert!(visual.debug_bounds(lane_rail).is_none());
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
        if mode == super::EditorMode::Arrangement {
            let rail = Box::leak(
                format!("track-group-rail-{}-{}", id.value(), group.value()).into_boxed_str(),
            );
            assert!(visual.debug_bounds(rail).is_none());
        }
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
fn duplicate_and_undo_keep_row_measurements_and_scroll_anchor(cx: &mut TestAppContext) {
    use gpui::{ListOffset, point, px};

    let (mut visual, editor, ids) = live_tracks(cx, 100, false);
    let [anchor, source, following] = [ids[1], ids[2], ids[3]];
    editor.update(&mut visual, |view, cx| {
        view.track_scroll.list.scroll_to(ListOffset {
            item_ix: 1,
            offset_in_item: px(5.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    let following_header =
        Box::leak(format!("arrangement-track-header-{}", following.value()).into_boxed_str());
    let original_height = visual.debug_bounds(following_header).unwrap().size.height;
    let resize = visual
        .debug_bounds(Box::leak(
            format!("track-height-resize-{}", following.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let released = point(resize.x, resize.y + px(20.0));
    visual.simulate_mouse_down(resize, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(released, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(released, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let anchor_before = bounds(&mut visual, anchor);
    let following_before = visual.debug_bounds(following_header).unwrap();
    assert_eq!(following_before.size.height, original_height + px(22.0));
    select(&mut visual, source, false);

    for (shortcut, count, following_index) in [("cmd-d", 101, 4), ("cmd-z", 100, 3)] {
        editor.update(&mut visual, |view, _| view.track_row_builds = 0);
        visual.simulate_keystrokes(shortcut);
        receive(&mut visual, &editor);
        editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(project.tracks().len(), count);
            assert_eq!(project.tracks()[2].id(), source);
            assert_eq!(project.tracks()[following_index].id(), following);
            assert!(
                view.track_row_builds < ids.len(),
                "{shortcut} rebuilt {} track rows",
                view.track_row_builds
            );
        });
        assert_eq!(bounds(&mut visual, anchor), anchor_before);
        assert_eq!(
            visual.debug_bounds(following_header).unwrap().size,
            following_before.size
        );
    }
    assert_eq!(
        visual.debug_bounds(following_header),
        Some(following_before)
    );
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
