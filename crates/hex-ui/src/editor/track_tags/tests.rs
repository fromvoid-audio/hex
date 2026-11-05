//! Actual native-widget headers and worker edits; no physical app acceptance.
use super::*;
use crate::editor::{
    EditorMode,
    track_reorder_ui_test::{live_tracks, receive},
};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use hex_project::{Command, TrackSelectionIds};
use hex_session::service::Request;
fn selector(prefix: &str, id: TrackId) -> &'static str {
    Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str())
}
fn click(visual: &mut VisualTestContext, name: &'static str, modifiers: Modifiers) {
    let at = visual
        .debug_bounds(name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
fn edit(visual: &mut VisualTestContext, view: &Entity<EditorView>, command: Command) {
    view.update(visual, |v, _| assert!(v.enqueue(Request::Edit(command))));
    receive(visual, view);
}
fn choose(
    visual: &mut VisualTestContext,
    view: &Entity<EditorView>,
    find: impl Fn(&Action) -> bool,
) {
    let index = view.read_with(visual, |v, _| {
        v.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(
                |item| matches!(&item.action,EditorContextAction::Tags(action) if find(action)),
            )
            .unwrap()
    });
    // Keyboard navigation goes through the real focused popup dispatch.
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
}
fn tags(values: &[&str]) -> Vec<TrackTag> {
    values.iter().map(|s| TrackTag::new(s).unwrap()).collect()
}
#[gpui::test]
fn rendered_tag_selection_modifiers_overflow_and_filter_are_view_only(cx: &mut TestAppContext) {
    let (mut visual, view, ids) = live_tracks(cx, 3, false);
    for (track, values) in [
        (ids[0], vec!["drums", "lowend"]),
        (ids[1], vec!["drums"]),
        (ids[2], vec!["bass"]),
    ] {
        edit(
            &mut visual,
            &view,
            Command::SetTrackTags {
                track,
                tags: tags(&values),
            },
        );
    }
    let canonical = view.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    let from = visual
        .debug_bounds(selector("track-tag-first", ids[0]))
        .unwrap()
        .center();
    let to = visual
        .debug_bounds(selector("arrangement-track-header", ids[2]))
        .unwrap()
        .center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    view.read_with(&visual, |v, _| {
        assert!(!v.track_reorder_active());
        assert_eq!(v.state.as_ref().unwrap().project, canonical);
    });
    click(
        &mut visual,
        selector("track-tag-first", ids[0]),
        Modifiers::default(),
    );
    view.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), ids[..2]));
    click(
        &mut visual,
        selector("track-tag-first", ids[0]),
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    view.read_with(&visual, |v, _| assert!(v.selected_track_ids().is_empty()));
    click(
        &mut visual,
        selector("track-name", ids[2]),
        Modifiers::default(),
    );
    click(
        &mut visual,
        selector("track-tag-first", ids[0]),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    view.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), ids));
    click(
        &mut visual,
        selector("track-tag-first", ids[0]),
        Modifiers {
            shift: true,
            platform: true,
            ..Default::default()
        },
    );
    view.read_with(&visual, |v, _| {
        assert_eq!(
            v.selected_track_ids(),
            ids,
            "combined modifiers add rather than remove the matching cohort"
        )
    });
    click(
        &mut visual,
        selector("track-tag-first", ids[0]),
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    view.read_with(&visual, |v, _| assert_eq!(v.selected_track_ids(), [ids[2]]));
    click(
        &mut visual,
        selector("track-tag-more", ids[0]),
        Modifiers::default(),
    );
    choose(
        &mut visual,
        &view,
        |action| matches!(action,Action::Filter { tag:Some(tag),.. } if tag.as_str()=="lowend"),
    );
    view.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_track_indices(), [0]);
        assert_eq!(v.arrangement_focus.projection.console_indices, [0]);
        assert_eq!(
            v.selected_track_ids(),
            [ids[2]],
            "a filter preserves explicit hidden selection"
        );
        assert_eq!(v.state.as_ref().unwrap().project, canonical);
    });
    assert!(
        visual
            .debug_bounds(selector("track-tag-first", ids[0]))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("track-tag-first", ids[1]))
            .is_none()
    );
    click(&mut visual, "clear-track-tag-filter", Modifiers::default());
    click(&mut visual, "track-tag-filter-menu", Modifiers::default());
    view.read_with(&visual, |v, _| {
        assert!(v.context_menu.as_ref().unwrap().items.len() <= PAGE + 3)
    });
    choose(
        &mut visual,
        &view,
        |action| matches!(action,Action::Filter { tag:Some(tag),.. } if tag.as_str()=="drums"),
    );
    view.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_track_indices(), [0, 1])
    });
    for track in &ids[..2] {
        edit(
            &mut visual,
            &view,
            Command::SetTrackTags {
                track: *track,
                tags: Vec::new(),
            },
        );
    }
    view.read_with(&visual, |v, _| {
        assert!(v.arrangement_track_indices().is_empty())
    });
    click(&mut visual, "clear-track-tag-filter", Modifiers::default());
    view.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_track_indices(), [0, 1, 2])
    });
}
#[gpui::test]
fn rendered_tag_text_commits_one_worker_edit_and_escape_and_invalid_input_are_safe(
    cx: &mut TestAppContext,
) {
    let (mut visual, view, ids) = live_tracks(cx, 2, false);
    let at = visual
        .debug_bounds(selector("track-name", ids[0]))
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    choose(
        &mut visual,
        &view,
        |action| matches!(action,Action::Edit {track,..} if *track==ids[0]),
    );
    assert!(
        visual
            .debug_bounds(selector("track-tags-input", ids[0]))
            .is_some()
    );
    visual.simulate_input(" Drums, LOWEND ");
    visual.simulate_keystrokes("enter");
    receive(&mut visual, &view);
    view.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[0])
                .unwrap()
                .tags(),
            tags(&["drums", "lowend"])
        )
    });
    view.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &view);
    view.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[0])
                .unwrap()
                .tags()
                .is_empty()
        )
    });
    view.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    receive(&mut visual, &view);
    click(
        &mut visual,
        selector("track-tag-more", ids[0]),
        Modifiers::default(),
    );
    choose(&mut visual, &view, |action| {
        matches!(action, Action::Edit { .. })
    });
    let before = view.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    visual.simulate_input("drums,,lowend");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    view.read_with(&visual, |v, _| {
        assert!(v.track_name.is_editing());
        assert!(v.error.is_some());
        assert_eq!(v.state.as_ref().unwrap().revision, before);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    view.read_with(&visual, |v, _| {
        assert!(!v.track_name.is_editing());
        assert_eq!(v.state.as_ref().unwrap().revision, before);
    });
}
#[gpui::test]
fn folded_matches_stay_selected_and_filter_applies_to_console_headers(cx: &mut TestAppContext) {
    let (mut visual, view, ids) = live_tracks(cx, 3, false);
    for track in &ids[..2] {
        edit(
            &mut visual,
            &view,
            Command::SetTrackTags {
                track: *track,
                tags: tags(&["drums"]),
            },
        );
    }
    edit(
        &mut visual,
        &view,
        Command::GroupTracks {
            tracks: TrackSelectionIds::new(&ids[..1]).unwrap(),
            name: "group".into(),
        },
    );
    let group = view.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.is_group())
            .unwrap()
            .id()
    });
    click(
        &mut visual,
        selector("group-fold", group),
        Modifiers::default(),
    );
    click(
        &mut visual,
        selector("track-tag-first", ids[1]),
        Modifiers::default(),
    );
    view.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track_ids(), ids[..2]);
        assert_eq!(
            v.selected_track,
            Some(ids[1]),
            "the clicked visible header keeps focus while folded matches join selection"
        );
    });
    visual.update(|w, cx| {
        view.update(cx, |v, cx| {
            v.set_mode(EditorMode::Session);
            v.focus.focus(w);
            cx.notify();
        })
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("track-tag-first", ids[1]))
            .is_some()
    );
    click(&mut visual, "track-tag-filter-menu", Modifiers::default());
    choose(
        &mut visual,
        &view,
        |action| matches!(action,Action::Filter {tag:Some(tag),..} if tag.as_str()=="drums"),
    );
    view.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        let visible: Vec<_> = v
            .arrangement_focus
            .projection
            .console_indices
            .iter()
            .map(|i| p.tracks()[*i].id())
            .collect();
        assert_eq!(visible, [group, ids[1]]);
        assert_eq!(v.selected_track_ids(), ids[..2]);
    });
    assert!(
        visual
            .debug_bounds(selector("session-track", ids[2]))
            .is_none()
    );
    click(&mut visual, "clear-track-tag-filter", Modifiers::default());
    view.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_focus.projection.console_indices.len(), 3)
    });
    click(
        &mut visual,
        selector("track-tag-more", ids[1]),
        Modifiers::default(),
    );
    choose(&mut visual, &view, |action| {
        matches!(action, Action::Edit { .. })
    });
    assert!(
        visual
            .debug_bounds(selector("track-tags-input", ids[1]))
            .is_some()
    );
    visual.simulate_input("drums, live");
    visual.simulate_keystrokes("enter");
    receive(&mut visual, &view);
    view.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Session);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[1])
                .unwrap()
                .tags(),
            tags(&["drums", "live"])
        );
    });
}

#[gpui::test]
fn filter_catalog_pages_are_bounded_and_keep_later_tags_reachable(cx: &mut TestAppContext) {
    let (mut visual, view, ids) = live_tracks(cx, 3, false);
    for (index, track) in ids.iter().enumerate() {
        edit(
            &mut visual,
            &view,
            Command::SetTrackTags {
                track: *track,
                tags: (0..16)
                    .map(|n| TrackTag::new(&format!("tag {:02}", index * 16 + n)).unwrap())
                    .collect(),
            },
        );
    }
    click(&mut visual, "track-tag-filter-menu", Modifiers::default());
    view.read_with(&visual, |v, _| {
        assert_eq!(v.context_menu.as_ref().unwrap().items.len(), PAGE + 2)
    });
    choose(&mut visual, &view, |action| {
        matches!(action, Action::Page { page: 1, .. })
    });
    view.read_with(&visual, |v, _| {
        assert_eq!(v.context_menu.as_ref().unwrap().items.len(), PAGE + 2)
    });
    choose(
        &mut visual,
        &view,
        |action| matches!(action,Action::Filter {tag:Some(tag),..} if tag.as_str()=="tag 47"),
    );
    view.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_track_indices(), [2])
    });
}

#[gpui::test]
fn invalid_tag_blur_retires_the_draft_without_committing_or_blocking_other_headers(
    cx: &mut TestAppContext,
) {
    let (mut visual, view, ids) = live_tracks(cx, 2, false);
    edit(
        &mut visual,
        &view,
        Command::SetTrackTags {
            track: ids[0],
            tags: tags(&["drums"]),
        },
    );
    click(
        &mut visual,
        selector("track-tag-more", ids[0]),
        Modifiers::default(),
    );
    choose(&mut visual, &view, |action| {
        matches!(action, Action::Edit { .. })
    });
    visual.simulate_input("drums,,lowend");
    let before = view.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    click(
        &mut visual,
        selector("track-name", ids[1]),
        Modifiers::default(),
    );
    view.read_with(&visual, |v, _| {
        assert!(
            !v.track_name.is_editing(),
            "a refused blurred tag edit must not leave an active header draft"
        );
        assert!(v.error.is_some());
        assert_eq!(v.state.as_ref().unwrap().project, before);
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn valid_tag_blur_commits_once_and_history_restores_the_original_assignment(
    cx: &mut TestAppContext,
) {
    let (mut visual, view, ids) = live_tracks(cx, 2, false);
    edit(
        &mut visual,
        &view,
        Command::SetTrackTags {
            track: ids[0],
            tags: tags(&["drums"]),
        },
    );
    let before = view.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    click(
        &mut visual,
        selector("track-tag-more", ids[0]),
        Modifiers::default(),
    );
    choose(&mut visual, &view, |action| {
        matches!(action, Action::Edit { .. })
    });
    visual.simulate_input("live");
    click(
        &mut visual,
        selector("track-name", ids[1]),
        Modifiers::default(),
    );
    receive(&mut visual, &view);
    view.read_with(&visual, |v, _| {
        assert!(!v.track_name.is_editing());
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(ids[0])
                .unwrap()
                .tags(),
            tags(&["live"])
        );
    });
    view.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    receive(&mut visual, &view);
    view.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
}
