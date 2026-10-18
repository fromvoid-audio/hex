use super::super::super::clip_drag_ui_test::{audio_project, editor_window};
use super::super::super::musical_time::{SnapGrid, grid_lines};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, px};
use std::sync::Arc;

#[gpui::test]
fn adaptive_grid_menu_right_click_keyboard_choice_and_cancel_preserve_project_and_focus(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    let (project, track, _, _) = audio_project();
    let original = project.clone();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let bounds = visual.debug_bounds(lane).unwrap();
    let position = gpui::point(bounds.right() - px(20.0), bounds.origin.y + px(24.0));
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let menu = view
            .context_menu
            .as_ref()
            .expect("actual arranger right click opens menu");
        assert_eq!(menu.items[0].label, "insert track");
        let choices = menu
            .items
            .iter()
            .filter(|item| matches!(item.action, EditorContextAction::AdaptiveGrid { .. }))
            .collect::<Vec<_>>();
        assert_eq!(choices.len(), 3);
        assert_eq!(
            choices.iter().map(|item| item.checked).collect::<Vec<_>>(),
            [false, true, false]
        );
    });
    visual.simulate_keystrokes("down enter");
    editor.read_with(cx, |view, app| {
        assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Fine);
        assert!(view.context_menu.is_none());
        assert_eq!(*view.state.as_ref().unwrap().project, original);
        assert!(view.last_enqueued_edit.is_none());
        let _ = app;
    });
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| assert!(view.focus.is_focused(window)))
    });
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.simulate_keystrokes("down down escape");
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Fine
    );
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    let index = editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let checked = menu
            .items
            .iter()
            .filter(|item| {
                matches!(item.action, EditorContextAction::AdaptiveGrid { .. }) && item.checked
            })
            .collect::<Vec<_>>();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].label, AdaptiveGridDensity::Fine.label());
        menu.items
            .iter()
            .position(|item| {
                matches!(
                    item.action,
                    EditorContextAction::AdaptiveGrid {
                        density: AdaptiveGridDensity::Coarse,
                        ..
                    }
                )
            })
            .unwrap()
    });
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    visual.simulate_click(
        gpui::point(
            panel.origin.x + px(16.0),
            panel.origin.y + px(2.0 + 24.0 * index as f32 + 12.0),
        ),
        Modifiers::default(),
    );
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Coarse
    );
}

#[gpui::test]
fn adaptive_grid_menu_disabled_choice_and_stale_project_refusal_are_ui_only(
    cx: &mut TestAppContext,
) {
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.snap_grid = SnapGrid::Off;
            view.open_arranger_grid_context_menu(gpui::point(px(400.0), px(100.0)), window, cx);
            let generation = view.project_generation;
            view.select_context_item(
                EditorContextAction::AdaptiveGrid {
                    generation,
                    density: AdaptiveGridDensity::Coarse,
                },
                window,
                cx,
            );
            assert!(view.snap_grid.is_off());
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            view.toggle_snap_grid();
            assert!(!view.snap_grid.is_off());
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            view.select_context_item(
                EditorContextAction::AdaptiveGrid {
                    generation: generation.wrapping_sub(1),
                    density: AdaptiveGridDensity::Fine,
                },
                window,
                cx,
            );
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            assert!(view.last_enqueued_edit.is_none());
        })
    });
}

#[gpui::test]
fn adaptive_grid_menu_choice_renders_the_same_landmark_used_by_keyboard_edit(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::super::bind_editor_keys);
    for density in AdaptiveGridDensity::ALL {
        let (project, track, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = Some(track);
                view.selected_clip = Some(clip);
                view.focus.focus(window);
                view.open_arranger_grid_context_menu(gpui::point(px(400.0), px(100.0)), window, cx);
                view.select_context_item(
                    EditorContextAction::AdaptiveGrid {
                        generation: view.project_generation,
                        density,
                    },
                    window,
                    cx,
                );
            })
        });
        visual.run_until_parked();
        let next = editor.read_with(cx, |view, _| {
            let (grid, viewport, width) = view.nudge_grid_context(false).unwrap();
            let project = &view.state.as_ref().unwrap().project;
            let next = grid
                .adjacent_frame(project, 0, viewport, width, true)
                .unwrap();
            assert!(
                grid_lines(project, viewport, width, grid)
                    .iter()
                    .any(|line| line.frame == next)
            );
            next
        });
        let selector = Box::leak(format!("arrangement-grid-line-{next}").into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_some(),
            "selected subdivision is actually rendered"
        );
        visual.simulate_keystrokes("right");
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.last_enqueued_edit,
            Some(hex_project::Command::MoveClip {start,..}) if start==next))
        });
    }
}

#[gpui::test]
fn adaptive_grid_menu_empty_arranger_choice_survives_keyboard_snap_toggle(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let project = hex_project::Project::new(48_000).unwrap();
    let original = project.clone();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body = visual.debug_bounds("arrangement-content").unwrap();
    visual.simulate_mouse_down(body.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(body.center(), MouseButton::Right, Modifiers::default());
    editor.read_with(cx, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(menu.items.len(), 4);
        assert_eq!(menu.items[0].label, "insert track");
    });
    visual.simulate_keystrokes("up enter");
    assert_eq!(
        editor.read_with(cx, |view, _| view.snap_grid.density()),
        AdaptiveGridDensity::Coarse
    );
    for off in [true, false] {
        visual.simulate_keystrokes("cmd-4");
        editor.read_with(cx, |view, _| {
            assert_eq!(view.snap_grid.is_off(), off);
            assert_eq!(view.snap_grid.density(), AdaptiveGridDensity::Coarse);
            assert_eq!(*view.state.as_ref().unwrap().project, original);
            assert!(view.last_enqueued_edit.is_none());
        });
    }
}
