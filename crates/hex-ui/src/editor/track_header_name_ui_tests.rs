//! Canonical names and colors in the actual resident header, including blank-area input.
use super::*;
fn setup(
    cx: &mut TestAppContext,
) -> (
    Arc<Environment>,
    VisualTestContext,
    Entity<EditorView>,
    [TrackId; 2],
) {
    let (environment, handle, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.), px(800.)));
    visual.run_until_parked();
    let tracks = editor.read_with(&visual, |v, _| {
        let t = v.state.as_ref().unwrap().project.tracks();
        assert_eq!(
            v.document_audio_mode,
            hex_session::DocumentAudioMode::Resident
        );
        [t[0].id(), t[1].id()]
    });
    (environment, visual, editor, tracks)
}
fn blank(visual: &mut VisualTestContext, track: TrackId) -> gpui::Point<Pixels> {
    let area = bounds(visual, "track-name-area", track.value());
    point(area.right() - px(6.), area.bottom() - px(4.))
}
fn assert_color(visual: &mut VisualTestContext, editor: &Entity<EditorView>, track: TrackId) {
    let expected = editor.read_with(visual, |v, _| {
        let c = theme::element_colors(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .color(),
        );
        if v.track_selection.contains(track) {
            c.accent
        } else {
            c.muted
        }
    });
    assert_eq!(
        visual.rendered_background(selector("track-name-area", track.value())),
        Some(gpui::rgb(expected).into())
    );
}
#[gpui::test]
fn colored_name_column_is_readable_and_its_blank_area_selects_with_modifiers(
    cx: &mut TestAppContext,
) {
    let (_environment, mut visual, editor, tracks) = setup(cx);
    let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let name = bounds(&mut visual, "track-name", tracks[0].value());
    assert!(
        name.size.width >= px(100.),
        "ordinary desktop must leave room for track names: {name:?}"
    );
    for track in tracks {
        let area = bounds(&mut visual, "track-name-area", track.value());
        assert!(area.size.height >= px(36.));
        assert_color(&mut visual, &editor, track);
    }
    let at = blank(&mut visual, tracks[0]);
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    visual.update(|w, cx| {
        editor.read_with(cx, |v, _| {
            assert_eq!(v.selected_track, Some(tracks[0]));
            assert_eq!(v.selected_track_ids(), [tracks[0]]);
            assert!(v.track_header_focused(w));
        })
    });
    let at = blank(&mut visual, tracks[1]);
    visual.simulate_click(
        at,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(tracks.iter().all(|id| v.selected_track_ids().contains(id)));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original);
        assert_eq!(v.worker_requests_pending, 0);
    });
    for track in tracks {
        assert_color(&mut visual, &editor, track);
    }
}
#[gpui::test]
fn name_color_and_inline_rename_follow_canonical_updates_and_undo(cx: &mut TestAppContext) {
    let (_environment, mut visual, editor, tracks) = setup(cx);
    let track = tracks[0];
    let original = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .name()
            .to_owned()
    });
    let name = "Long CANONICAL Track Name 🎹";
    edit(
        &editor,
        &mut visual,
        Command::RenameTrack {
            track,
            name: name.into(),
        },
    );
    edit(
        &editor,
        &mut visual,
        Command::SetTrackColor {
            track,
            color: hex_project::ElementColor::Rose,
        },
    );
    assert_color(&mut visual, &editor, track);
    let at = blank(&mut visual, track);
    visual.simulate_click(at, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-name-input").is_some());
    editor.read_with(&visual, |v, _| assert!(v.track_name.is_editing()));
    let final_name = "UI CANONICAL Track Name 🎛";
    visual.simulate_input(final_name);
    visual.simulate_keystrokes("enter");
    clip_drag_ui_test::pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .name(),
            final_name
        )
    });
    for _ in 0..3 {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        clip_drag_ui_test::pump_move(&editor, &mut visual);
    }
    assert_color(&mut visual, &editor, track);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .name(),
            original
        )
    });
    for _ in 0..3 {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        clip_drag_ui_test::pump_move(&editor, &mut visual);
    }
    assert_color(&mut visual, &editor, track);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .name(),
            final_name
        )
    });
}
#[gpui::test]
fn colored_names_stay_bounded_when_headers_or_rows_are_compacted(cx: &mut TestAppContext) {
    let (_environment, mut visual, editor, tracks) = setup(cx);
    visual.simulate_resize(size(px(880.), px(540.)));
    visual.run_until_parked();
    for track in tracks {
        let header = bounds(&mut visual, "arrangement-track-header", track.value());
        let name = bounds(&mut visual, "track-name", track.value());
        let area = bounds(&mut visual, "track-name-area", track.value());
        assert!(name.size.width > px(0.));
        assert!(name.left() >= area.left() && name.right() <= area.right());
        assert!(area.left() >= header.left() && area.right() <= header.right());
    }
    let edge = bounds(&mut visual, "track-height-resize", tracks[0].value()).center();
    visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        point(edge.x, px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let header = bounds(&mut visual, "arrangement-track-header", tracks[0].value());
    let name = bounds(&mut visual, "track-name", tracks[0].value());
    assert_eq!(header.size.height, px(23.));
    assert!(name.top() >= header.top() && name.bottom() <= header.bottom());
    assert!(
        visual
            .debug_bounds(selector("track-controls", tracks[0].value()))
            .is_none()
    );
    editor.read_with(&visual, |v, _| assert_eq!(v.worker_requests_pending, 0));
}
