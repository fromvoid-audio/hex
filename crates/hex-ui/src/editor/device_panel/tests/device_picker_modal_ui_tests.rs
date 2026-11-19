//! A global picker owns captured workspace input as well as native text input.
use super::*;
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    pump_until(editor, visual, |view| !view.edit_pending());
    visual.run_until_parked();
}
fn click(visual: &mut VisualTestContext, id: &'static str) {
    let bounds = visual.debug_bounds(id).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn modal_captures_workspace_gestures_shortcuts_scroll_and_context_lifecycle(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ONE,
            length: hex_project::BeatTime::from_quarters(4).unwrap(),
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue(Request::EditArrangementSection {
            project_generation: view.project_generation,
            revision,
            command: Command::AddArrangementSection {
                name: "section".into(),
                color: hex_project::ElementColor::Orange,
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::from_quarters(8).unwrap()
            },
        }));
    });
    pump(&editor, &mut visual);
    for dimensions in [(880.0, 540.0), (1440.0, 800.0)] {
        visual.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
        editor.update(&mut visual, |view, cx| {
            view.mode = crate::editor::EditorMode::Arrangement;
            view.clip_view_open = true;
            view.device_panel_active = true;
            view.viewport = Some(crate::editor::viewport::Viewport {
                start: 0,
                length: 192_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let original = editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            (
                state.project.snapshot(),
                state.revision,
                view.viewport,
                view.browser.open,
            )
        });
        click(&mut visual, "device-add-toggle");
        let modal = visual.debug_bounds("device-picker-modal").unwrap();
        let card = visual.debug_bounds("device-add-menu").unwrap();
        assert!((card.center().x - modal.center().x).abs() < px(1.0));
        assert!((card.center().y - modal.center().y).abs() < px(1.0));
        // Padding inside the modal overlaps an authored Main section. Its press
        // must reach neither the section-move nor Main-range capture owner.
        let main = visual.debug_bounds("main-track-lane").unwrap();
        let padding = point(card.right() - px(2.0), main.center().y);
        assert!(card.contains(&padding) && main.contains(&padding));
        for held in [false, true] {
            editor.update(&mut visual, |view, _| view.time_selection_key_held = held);
            visual.simulate_mouse_down(padding, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert!(!view.sections.held());
                assert!(view.time_selection_gesture.is_none());
            });
            visual.simulate_mouse_up(padding, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        }
        editor.update(&mut visual, |view, _| view.time_selection_key_held = false);
        let ruler = visual.debug_bounds("timeline-ruler").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: ruler.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-120.0))),
            ..Default::default()
        });
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.viewport, original.2,
                "wheel capture belongs to the modal"
            )
        });
        visual.simulate_keystrokes("cmd-+ cmd-d cmd-z tab");
        pump(&editor, &mut visual);
        visual.update(|window, _| {
            assert!(
                window
                    .context_stack()
                    .iter()
                    .any(|context| context.contains("hex_text_input"))
            )
        });
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.viewport, original.2);
            assert_eq!(view.state.as_ref().unwrap().revision, original.1);
            assert_eq!(view.mode, crate::editor::EditorMode::Arrangement);
        });
        let chain = visual.debug_bounds("device-chain-scroll").unwrap();
        visual.simulate_mouse_down(chain.center(), MouseButton::Middle, Modifiers::default());
        visual.simulate_mouse_move(
            point(chain.center().x - px(40.0), chain.center().y),
            MouseButton::Middle,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(chain.center(), MouseButton::Middle, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(!view.device_panel.navigation.is_active())
        });
        // A real query/input drag overlaps the arranger at these compact sizes.
        visual.simulate_input("not a device");
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| view.time_selection_key_held = true);
        let input = visual.debug_bounds("device-picker-search").unwrap();
        let down = point(input.center().x, input.top() + px(1.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(15.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(down.x + px(15.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            assert!(view.time_selection_gesture.is_none());
            assert!(view.time_range_move_gesture.is_none());
            assert!(view.time_selection.is_none());
            view.time_selection_key_held = false;
        });
        click(&mut visual, "device-picker-search-clear");
        let list = visual.debug_bounds("device-picker-list").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: list.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-1000.0))),
            ..Default::default()
        });
        visual.run_until_parked();
        let last = hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument)
            .unwrap()
            .pop()
            .unwrap();
        let row = visual
            .debug_bounds(Box::leak(
                format!("add-native-instrument-{}", last.type_id.as_str().unwrap())
                    .into_boxed_str(),
            ))
            .unwrap();
        assert!(
            row.top() >= list.top() && row.bottom() <= list.bottom(),
            "modal list scrolls independently: {row:?} in {list:?}"
        );
        // Outside-click cancellation over the captured loop rail must not create a loop.
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let down = point(rail.right() - px(2.0), rail.center().y);
        assert!(
            !visual
                .debug_bounds("device-add-menu")
                .unwrap()
                .contains(&down)
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(40.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(down.x - px(40.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        pump(&editor, &mut visual);
        assert!(visual.debug_bounds("device-picker-modal").is_none());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original.0);
            assert_eq!(view.state.as_ref().unwrap().revision, original.1);
            assert_eq!(view.viewport, original.2);
            assert_eq!(view.browser.open, original.3);
            assert!(view.loop_rail_gesture.is_none());
        });
    }
    click(&mut visual, "device-add-toggle");
    click(&mut visual, "device-picker-close");
    assert!(visual.debug_bounds("device-picker-modal").is_none());
    click(&mut visual, "device-add-toggle");
    editor.update(&mut visual, |view, cx| {
        view.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("device-picker-modal").is_none(),
        "project replacement retires the modal"
    );
}
