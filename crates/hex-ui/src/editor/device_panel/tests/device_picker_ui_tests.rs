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
        let panel = visual.debug_bounds("device-picker-modal").unwrap();
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
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("device-chain-loading-slot").is_some(),
            "the Track picker must paint while the worker result is held"
        );
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
        });
        pump_until(&editor, &mut visual, |view| !view.edit_pending());
        visual.run_until_parked();
        assert!(visual.debug_bounds("device-chain-loading-slot").is_none());
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

#[gpui::test]
fn native_picker_replacement_keeps_the_instrument_until_its_terminal(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        let state = hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap();
        assert!(view.enqueue(Request::Edit(Command::InsertInstrument { track, state })));
    });
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    visual.run_until_parked();
    let original = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.clone()
    });
    let device = original.track(track).unwrap().instrument().unwrap().id();
    let card = Box::leak(format!("device-drop-card-{}", device.value()).into_boxed_str());

    click(&mut visual, "device-add-toggle");
    visual.simulate_input("hex.instrument.fm4");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let proposal = visual.debug_bounds("device-chain-loading-slot").unwrap();
    assert!(proposal.right() <= visual.debug_bounds(card).unwrap().left());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });

    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-chain-loading-slot").is_none());
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let after = state.project.track(track).unwrap();
        let instrument = after.instrument().unwrap();
        assert_eq!(instrument.id(), device);
        assert_eq!(
            instrument.state().type_id().as_str().unwrap(),
            "hex.instrument.fm4"
        );
        assert_eq!(after.devices(), original.track(track).unwrap().devices());
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.track(track),
            original.track(track)
        );
    });
}

#[gpui::test]
fn device_picker_plus_on_first_effect_of_multi_effect_track_inserts_native_instrument(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    let (service, state, track, first_effect, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(800.0)));
    editor.update(&mut visual, |view, _| {
        if view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_some()
        {
            assert!(view.enqueue(Request::Edit(Command::RemoveInstrument { track })));
        }
        let eq_state = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.eq"),
        )
        .unwrap();
        assert!(view.enqueue(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: eq_state,
        })));
    });
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    visual.run_until_parked();

    let (second_effect, effects_before) = editor.read_with(&visual, |view, _| {
        let track_ref = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert!(track_ref.instrument().is_none());
        assert_eq!(track_ref.devices().len(), 2);
        assert_eq!(track_ref.devices()[0].id(), first_effect);
        (track_ref.devices()[1].id(), track_ref.devices().to_vec())
    });

    // Click `+` on the first effect card (so `before == Some(second_effect)`), then pick a native instrument.
    let add_after =
        Box::leak(format!("device-{}-add-after", first_effect.value()).into_boxed_str());
    click(&mut visual, add_after);
    visual.simulate_input("hex.instrument.sine");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    visual.run_until_parked();

    editor.read_with(&visual, |view, _| {
        assert!(
            view.error.is_none(),
            "adding native instrument from first effect `+` must succeed: {:?}",
            view.error
        );
        let track_ref = view.state.as_ref().unwrap().project.track(track).unwrap();
        let instrument = track_ref
            .instrument()
            .expect("native instrument must be inserted");
        assert_eq!(
            instrument.state().type_id().as_str().unwrap(),
            "hex.instrument.sine"
        );
        assert_eq!(track_ref.devices(), effects_before.as_slice());
        assert_eq!(track_ref.devices()[1].id(), second_effect);
    });
}

#[gpui::test]
fn device_picker_hides_instruments_on_group_track_and_allows_effects(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_text_input_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1440.0), px(800.0)));
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })));
    });
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    let group_track = editor.update(&mut visual, |view, cx| {
        let group = view
            .state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .find(|t| t.is_group())
            .expect("group track")
            .id();
        view.selected_track = Some(group);
        view.device_panel.owner = Some(DevicePanelOwner::Track(group));
        cx.notify();
        group
    });
    visual.run_until_parked();

    click(&mut visual, "device-add-toggle");
    visual.simulate_input("instrument");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("add-native-instrument-hex.instrument.sine")
            .is_none(),
        "group track device picker must hide native instruments"
    );
    assert!(
        visual.debug_bounds("device-picker-empty").is_some(),
        "searching for instruments on a group track must show empty picker"
    );

    click(&mut visual, "device-picker-search-clear");
    visual.simulate_input("hex.utility");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("add-native-effect-hex.utility")
            .is_some(),
        "group track device picker must still offer native effects"
    );
    visual.simulate_keystrokes("enter");
    pump_until(&editor, &mut visual, |view| !view.edit_pending());
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let group = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(group_track)
            .unwrap();
        assert_eq!(group.devices().len(), 1);
        assert_eq!(
            group.devices()[0].state().type_id().as_str().unwrap(),
            "hex.utility"
        );
    });
}
