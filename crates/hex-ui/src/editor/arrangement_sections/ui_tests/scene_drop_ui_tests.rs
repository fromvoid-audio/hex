//! Rendered GPUI tests for dragging a Console scene onto the Arrangement Main track (#3444).

use super::*;
use hex_project::SceneId;

fn console_scene_fixture(
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, SceneId) {
    cx.update(super::super::super::bind_editor_keys);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddScene {
            name: "chorus".into(),
        }))
        .unwrap();
    let event = next(&service);
    let Some(Created::Scene(chorus)) = event.created else {
        panic!("expected chorus scene");
    };
    let track = event.state.project.tracks()[0].id();
    // Give the scene an active 4-beat (1-bar) MIDI cell so default placement duration is 4 quarters.
    service
        .submit(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene: chorus,
            name: "chords".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    assert!(event.error.is_none(), "{:?}", event.error);
    let (handle, editor) = section_test_window(cx, service, event.state);
    (handle, editor, chorus)
}

#[gpui::test]
fn drag_console_scene_to_main_track_places_linked_scene_repeats_undoes_and_persists(
    cx: &mut TestAppContext,
) {
    let (handle, editor, chorus) = console_scene_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let main_lane = visual
        .debug_bounds("main-track-lane")
        .expect("arrangement main track lane");

    // 1. Capture stable Console scene drag payload (default duration = 4 quarters from active cell).
    let drag_payload = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        ConsoleSceneDrag {
            project_generation: view.project_generation,
            revision: state.revision,
            scene: chorus,
            name: "chorus".into(),
            duration: state.project.default_scene_placement_length(chorus),
            color: ElementColor::Blue,
        }
    });
    assert_eq!(drag_payload.duration, BeatTime::from_quarters(4).unwrap());

    // Hover over main-track-lane -> shows live candidate preview without mutating canonical state.
    let drop_x = f32::from(main_lane.origin.x) + 4.0;
    let drop_y = f32::from(main_lane.center().y);
    editor.update(&mut visual, |view, cx| {
        view.begin_console_scene_drag(drag_payload.clone());
        assert!(view.hover_console_scene_on_main_track(
            &drag_payload,
            drop_x,
            drop_y,
            Modifiers::default()
        ));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("arrangement-section-0").is_some(),
        "live hover preview must paint on main-track-lane"
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .is_empty(),
            "hover must never mutate canonical project state"
        );
    });

    // Drop onto main-track-lane -> holds frozen preview in pending_edit across repaints until exact ACK!
    editor.update(&mut visual, |view, cx| {
        assert!(view.drop_console_scene_on_main_track(
            &drag_payload,
            drop_x,
            drop_y,
            Modifiers::default()
        ));
        cx.notify();
    });
    // Delayed terminal/repaint check (AGENTS.md Golden Rule #6): run multiple repaints BEFORE delivering terminal.
    visual.run_until_parked();
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.sections.pending(),
            "frozen preview receipt must stay pending until exact acknowledgement"
        );
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .is_empty()
        );
    });
    assert!(
        visual.debug_bounds("arrangement-section-0").is_some(),
        "frozen final preview must remain painted across repaints before terminal acknowledgement"
    );

    // Deliver exact terminal acknowledgement from service and install canonical linked placement.
    let first_event = editor.update(&mut visual, |view, _| next(&view.service));
    assert!(first_event.error.is_none(), "{:?}", first_event.error);
    editor.update(&mut visual, |view, cx| {
        view.receive(first_event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(!view.sections.pending());
        let sections = view.state.as_ref().unwrap().project.arrangement_sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].linked_scene(), Some(chorus));
        assert_eq!(sections[0].name(), "chorus");
    });

    // 2. Repeated placement of the SAME Console scene later on the Main track.
    let second_payload = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        ConsoleSceneDrag {
            project_generation: view.project_generation,
            revision: state.revision,
            scene: chorus,
            name: "chorus".into(),
            duration: state.project.default_scene_placement_length(chorus),
            color: ElementColor::Rose,
        }
    });
    let second_x = f32::from(main_lane.origin.x) + f32::from(main_lane.size.width) * 0.6;
    editor.update(&mut visual, |view, cx| {
        assert!(view.drop_console_scene_on_main_track(
            &second_payload,
            second_x,
            drop_y,
            Modifiers::default()
        ));
        cx.notify();
    });
    let second_event = editor.update(&mut visual, |view, _| next(&view.service));
    assert!(second_event.error.is_none(), "{:?}", second_event.error);
    editor.update(&mut visual, |view, cx| {
        view.receive(second_event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.arrangement_sections().len(), 2);
        assert_eq!(project.linked_sections_for_scene(chorus).count(), 2);
    });

    // 3. One Undo removes the repeated placement; Redo restores it with the link intact.
    editor.update(&mut visual, |view, _| {
        view.service.submit(Request::Undo).unwrap();
    });
    let undo_event = editor.update(&mut visual, |view, _| next(&view.service));
    editor.update(&mut visual, |view, cx| {
        view.receive(undo_event);
        cx.notify();
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .linked_sections_for_scene(chorus)
                .count(),
            1
        );
    });
    editor.update(&mut visual, |view, _| {
        view.service.submit(Request::Redo).unwrap();
    });
    let redo_event = editor.update(&mut visual, |view, _| next(&view.service));
    editor.update(&mut visual, |view, cx| {
        view.receive(redo_event);
        cx.notify();
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .linked_sections_for_scene(chorus)
                .count(),
            2
        );
    });

    // 4. Save/reopen round-trip through hex-session retains both linked placements under schema 131.
    let save_dir = std::env::temp_dir().join(format!("hex-scene-drop-{}", std::process::id()));
    std::fs::create_dir_all(&save_dir).unwrap();
    let save_path = save_dir.join("linked-scenes.hex");
    editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::SaveNew(save_path.clone()))
            .unwrap();
    });
    let save_event = editor.update(&mut visual, |view, _| next(&view.service));
    assert!(save_event.error.is_none(), "{:?}", save_event.error);
    editor.update(&mut visual, |view, cx| {
        view.receive(save_event);
        cx.notify();
    });
    editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::Open(save_path.clone()))
            .unwrap();
    });
    let open_event = editor.update(&mut visual, |view, _| next(&view.service));
    assert!(open_event.error.is_none(), "{:?}", open_event.error);
    assert_eq!(
        open_event
            .state
            .project
            .linked_sections_for_scene(chorus)
            .count(),
        2
    );
    let _ = std::fs::remove_dir_all(&save_dir);
}

#[gpui::test]
fn console_scene_drop_rejects_main_bus_other_track_overlap_cancel_stale_and_replaced_project(
    cx: &mut TestAppContext,
) {
    let (handle, editor, chorus) = console_scene_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let main_lane = visual
        .debug_bounds("main-track-lane")
        .expect("arrangement main track lane");
    let main_bus_header = visual
        .debug_bounds("main-track-header")
        .expect("main bus header");
    let content_area = visual
        .debug_bounds("arrangement-content")
        .expect("ordinary tracks area");

    let payload = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        ConsoleSceneDrag {
            project_generation: view.project_generation,
            revision: state.revision,
            scene: chorus,
            name: "chorus".into(),
            duration: BeatTime::from_quarters(4).unwrap(),
            color: ElementColor::Blue,
        }
    });
    let initial_revision = payload.revision;

    // 1. Dropping on Main bus header (NOT Main track lane) is rejected.
    editor.update(&mut visual, |view, _| {
        assert!(!view.drop_console_scene_on_main_track(
            &payload,
            f32::from(main_bus_header.center().x),
            f32::from(main_bus_header.center().y),
            Modifiers::default(),
        ));
    });

    // 2. Dropping on ordinary track area is rejected.
    editor.update(&mut visual, |view, _| {
        assert!(!view.drop_console_scene_on_main_track(
            &payload,
            f32::from(content_area.center().x),
            f32::from(content_area.center().y),
            Modifiers::default(),
        ));
    });

    // 3. Escape cancels an active hover preview cleanly with zero canonical mutations.
    editor.update(&mut visual, |view, cx| {
        view.begin_console_scene_drag(payload.clone());
        assert!(view.hover_console_scene_on_main_track(
            &payload,
            f32::from(main_lane.origin.x) + 4.0,
            f32::from(main_lane.center().y),
            Modifiers::default(),
        ));
        assert!(view.cancel_section_pointer());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-section-0").is_none());

    // 4. Stale revision and replaced project generation are both refused cleanly.
    let stale_rev = ConsoleSceneDrag {
        revision: initial_revision.saturating_add(99),
        ..payload.clone()
    };
    let replaced_gen = ConsoleSceneDrag {
        project_generation: payload.project_generation.wrapping_add(1),
        ..payload.clone()
    };
    editor.update(&mut visual, |view, _| {
        assert!(!view.drop_console_scene_on_main_track(
            &stale_rev,
            f32::from(main_lane.origin.x) + 4.0,
            f32::from(main_lane.center().y),
            Modifiers::default(),
        ));
        assert!(!view.drop_console_scene_on_main_track(
            &replaced_gen,
            f32::from(main_lane.origin.x) + 4.0,
            f32::from(main_lane.center().y),
            Modifiers::default(),
        ));
        assert!(!view.sections.pending());
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .is_empty()
        );
    });
}
