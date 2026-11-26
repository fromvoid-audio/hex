//! Rendered GPUI tests for opening a linked Arrangement scene in Console (#3443).

use super::*;
use hex_project::SceneId;

fn linked_fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    SceneId,
    [SectionId; 3],
) {
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
    let mut state = event.state;

    // 1. Standalone section (0..4), 2. Linked chorus #1 (4..8), 3. Repeated chorus #2 (12..16).
    let mut sections = Vec::new();
    for cmd in [
        Command::AddArrangementSection {
            name: "intro".into(),
            color: ElementColor::Gray,
            start: BeatTime::ZERO,
            end: BeatTime::from_quarters(4).unwrap(),
        },
        Command::PlaceLinkedScene {
            scene: chorus,
            start: BeatTime::from_quarters(4).unwrap(),
            end: BeatTime::from_quarters(8).unwrap(),
            color: ElementColor::Blue,
        },
        Command::PlaceLinkedScene {
            scene: chorus,
            start: BeatTime::from_quarters(12).unwrap(),
            end: BeatTime::from_quarters(16).unwrap(),
            color: ElementColor::Rose,
        },
    ] {
        service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: state.revision,
                command: cmd,
            })
            .unwrap();
        let event = next(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let Some(Created::ArrangementSection(id)) = event.created else {
            panic!("expected section");
        };
        sections.push(id);
        state = event.state;
    }

    let (handle, editor) = section_test_window(cx, service, state);
    (
        handle,
        editor,
        chorus,
        [sections[0], sections[1], sections[2]],
    )
}

#[gpui::test]
fn right_click_linked_arrangement_scene_opens_exact_console_scene_and_propagates_subsequent_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, chorus, [standalone, first, second]) = linked_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let revision_before =
        editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);

    // 1. Standalone section does NOT expose `edit scene in console`.
    let standalone_bounds = visual
        .debug_bounds("arrangement-section-header-0")
        .expect("standalone section header");
    visual.simulate_mouse_down(
        standalone_bounds.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().expect("standalone menu");
        assert!(
            !menu
                .items
                .iter()
                .any(|item| item.label == "edit scene in console"),
            "standalone section must not offer edit scene in console"
        );
    });

    // Dismiss standalone context menu with Escape before right-clicking the linked placement.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
    });

    // 2. Right-click repeated placement #2 (`second`) and choose lowercase `edit scene in console`.
    let second_bounds = visual
        .debug_bounds("arrangement-section-header-2")
        .expect("repeated linked section header");
    visual.simulate_mouse_down(
        second_bounds.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    visual.run_until_parked();

    let edit_item = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().expect("linked section menu");
        assert_eq!(menu.items[0].label, "edit scene in console");
        menu.items[0].action.clone()
    });
    editor.update_in(&mut visual, |view, window, cx| {
        view.context_menu = None;
        if let crate::editor::context_menu::EditorContextAction::Section(action) = edit_item {
            view.section_menu_action(action, window, cx);
        } else {
            panic!("expected section context action");
        }
    });
    visual.run_until_parked();

    // Verify exact Console navigation with ZERO unintended edits or transport launches.
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.mode, EditorMode::Session);
        assert_eq!(view.session_grid.selected_scene, Some(chorus));
        assert!(view.context_menu.is_none());
        assert!(view.error.is_none());
        assert_eq!(view.state.as_ref().unwrap().revision, revision_before);
        assert_eq!(view.state.as_ref().unwrap().project.scenes().len(), 1);
    });

    // 3. Subsequent canonical edit on selected Console scene propagates to BOTH repeated placements.
    let selected = editor.read_with(&visual, |view, _| view.session_grid.selected_scene.unwrap());
    let renamed = editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::Edit(Command::RenameScene {
                scene: selected,
                name: "final chorus".into(),
            }))
            .unwrap();
        let event = next(&view.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event.state
    });
    assert_eq!(
        renamed
            .project
            .arrangement_section(standalone)
            .unwrap()
            .name(),
        "intro"
    );
    assert_eq!(
        renamed.project.arrangement_section(first).unwrap().name(),
        "final chorus"
    );
    assert_eq!(
        renamed.project.arrangement_section(second).unwrap().name(),
        "final chorus"
    );
}

#[gpui::test]
fn edit_scene_in_console_refuses_stale_deleted_and_replaced_projects_without_mutating_state(
    cx: &mut TestAppContext,
) {
    let (handle, editor, chorus, [_standalone, first, _second]) = linked_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let first_bounds = visual
        .debug_bounds("arrangement-section-header-1")
        .expect("linked section header");
    visual.simulate_mouse_down(
        first_bounds.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let captured_action = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().expect("linked menu");
        match menu.items[0].action.clone() {
            crate::editor::context_menu::EditorContextAction::Section(a) => a,
            _ => panic!("expected section action"),
        }
    });

    // Simulate project replacement (generation bump) before invoking the captured action.
    editor.update_in(&mut visual, |view, window, cx| {
        view.context_menu = None;
        view.project_generation = view.project_generation.wrapping_add(1);
        view.section_menu_action(captured_action.clone(), window, cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.mode, EditorMode::Arrangement);
        assert_eq!(view.session_grid.selected_scene, None);
        assert!(view.error.is_some());
    });

    // Create and remove a second scene, then verify targeting that deleted scene ID is refused cleanly.
    let deleted_scene = editor.update(&mut visual, |view, _| {
        view.service
            .submit(Request::Edit(Command::AddScene {
                name: "temp".into(),
            }))
            .unwrap();
        let event = next(&view.service);
        let Some(Created::Scene(temp)) = event.created else {
            panic!("temp scene");
        };
        view.service
            .submit(Request::Edit(Command::RemoveScene(temp)))
            .unwrap();
        let event = next(&view.service);
        view.state = Some(event.state);
        temp
    });
    editor.update_in(&mut visual, |view, window, cx| {
        view.error = None;
        let stale_missing = MenuAction {
            generation: view.project_generation,
            revision: view.state.as_ref().unwrap().revision,
            section: Some(first),
            kind: MenuKind::EditSceneInConsole(deleted_scene),
        };
        view.section_menu_action(stale_missing, window, cx);
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.mode, EditorMode::Arrangement);
        assert_eq!(view.session_grid.selected_scene, None);
        assert!(view.error.is_some());
        assert_eq!(
            view.state.as_ref().unwrap().project.scenes()[0].id(),
            chorus
        );
    });
}
