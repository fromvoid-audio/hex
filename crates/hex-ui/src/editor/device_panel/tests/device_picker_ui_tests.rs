//! Native search input, list keyboard ownership and real catalog activation.
use super::*;
fn click(visual: &mut VisualTestContext, id: &'static str) {
    let bounds = visual.debug_bounds(id).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn native_picker_search_clear_keyboard_cancel_and_one_insertion(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for dimensions in [(880.0, 540.0), (1440.0, 800.0)] {
        visual.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
        visual.run_until_parked();
        let original = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        click(&mut visual, "device-add-toggle");
        let panel = visual.debug_bounds("device-panel").unwrap();
        let popup = visual.debug_bounds("device-add-menu").unwrap();
        let input = visual.debug_bounds("device-picker-search").unwrap();
        assert!(popup.size.width <= px(360.0));
        assert!(popup.top() >= panel.top() && popup.bottom() <= panel.bottom());
        assert!(input.left() >= popup.left() && input.right() <= popup.right());
        visual.simulate_input("delay");
        visual.run_until_parked();
        assert!(visual.debug_bounds("add-native-effect-hex.delay").is_some());
        assert!(
            visual
                .debug_bounds("add-native-effect-hex.utility")
                .is_none()
        );
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("no such device");
        visual.run_until_parked();
        assert!(visual.debug_bounds("device-picker-empty").is_some());
        visual.simulate_keystrokes("cmd-z");
        pump_until(&editor, &mut visual, |view| !view.edit_pending());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original)
        });
        click(&mut visual, "device-picker-search-clear");
        assert!(visual.debug_bounds("device-picker-empty").is_none());
        assert!(
            visual
                .debug_bounds("add-native-effect-hex.utility")
                .is_some()
        );
        visual.simulate_keystrokes("down");
        visual.simulate_keystrokes("up");
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_background("add-native-effect-hex.utility"),
            Some(gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into())
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("device-add-menu").is_none());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original)
        });
        click(&mut visual, "device-add-toggle");
        visual.simulate_input("hex.utility");
        visual.simulate_keystrokes("enter");
        pump_until(&editor, &mut visual, |view| !view.edit_pending());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(!view.device_panel.add_menu_open);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .devices()
                    .len(),
                2
            );
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| assert!(view.focus.is_focused(window)))
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, &mut visual, |view| !view.edit_pending());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let mut restored = view.state.as_ref().unwrap().project.snapshot();
            restored.next_id = original.next_id;
            assert_eq!(restored, original);
        });
    }
    // Buses never advertise instruments as working insertion choices.
    editor.update(&mut visual, |view, cx| {
        view.selected_track = None;
        view.device_panel.main_selected = Some(view.project_generation);
        cx.notify();
    });
    visual.run_until_parked();
    click(&mut visual, "device-add-toggle");
    visual.simulate_input("instrument");
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-picker-empty").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    // Catalog failures stay visible without modifying canonical project data.
    let (original, catalog) = editor.update(&mut visual, |view, cx| {
        let original = view.state.as_ref().unwrap().project.snapshot();
        let catalog = view
            .device_panel
            .effect_catalog
            .replace(Err("fixture catalog failure".into()));
        cx.notify();
        (original, catalog)
    });
    click(&mut visual, "device-add-toggle");
    assert!(visual.debug_bounds("device-picker-error-0").is_some());
    visual.simulate_keystrokes("escape");
    editor.update(&mut visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
        view.device_panel.effect_catalog = catalog;
    });
}
