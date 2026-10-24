//! Real header shortcuts/menu routing and exact delayed ordinary completion.
use super::{
    EditorView,
    track_reorder_ui_test::{bounds, live_tracks, receive},
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
fn track_clipboard_header_duplicate_cut_copy_paste_and_setup_only_menu(cx: &mut TestAppContext) {
    let (mut v, e, ids) = live_tracks(cx, 3, true);
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
            8
        );
    });
    v.simulate_keystrokes("cmd-z");
    receive(&mut v, &e);
    select(&mut v, ids[0], false);
    select(&mut v, ids[2], true);
    v.simulate_keystrokes("cmd-x");
    receive(&mut v, &e);
    assert_eq!(snapshot(&v, &e).tracks.len(), 1);
    select(&mut v, ids[1], false);
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
