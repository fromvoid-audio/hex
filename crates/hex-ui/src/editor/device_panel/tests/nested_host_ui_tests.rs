//! Shared native device host nested-unit rendering, picker/browser insertion,
//! UI-only collapse, effective parent bypass, subtree copy, and refusal proofs (#6849).
use super::*;

#[gpui::test]
fn vector_to_delay_renders_full_card_meter_marker_collapse_power_and_descendant_edits(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };

    let Some(Created::Track(track)) = edit(Command::AddMidiTrack {
        name: "vector".into(),
    })
    .created
    else {
        panic!("track")
    };
    let operator_state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let inserted = edit(Command::InsertInstrument {
        track,
        state: operator_state,
    });
    let Some(Created::Device(vector_id)) = inserted.created else {
        panic!("vector")
    };
    let source_id = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .unwrap();

    let (handle, editor) = actor_editor_window(cx, service, inserted.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();

    // Open searchable picker from Vector's nested insertion target and insert hex.delay.
    let nested_add_sel: &'static str =
        Box::leak(format!("device-{}-add-nested", vector_id.value()).into_boxed_str());
    let nested_add_bounds = visual
        .debug_bounds(nested_add_sel)
        .expect("vector nested insertion target");
    visual.simulate_click(nested_add_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("add-nested-effect-hex.delay").is_some(),
        "admitted source effect hex.delay must appear in nested picker"
    );
    assert!(
        visual
            .debug_bounds("add-nested-effect-hex.limiter")
            .is_none(),
        "nonzero-latency hex.limiter must not appear in source nested picker"
    );
    let delay_row = visual
        .debug_bounds("add-nested-effect-hex.delay")
        .expect("delay picker row");
    visual.simulate_click(delay_row.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.native_source(source_id))
            .is_some_and(|s| s.post_instrument_effects().len() == 1)
    });
    visual.run_until_parked();

    let delay_id = editor.read_with(&visual, |view, _| {
        let source = view
            .state
            .as_ref()
            .unwrap()
            .project
            .native_source(source_id)
            .unwrap();
        let child = &source.post_instrument_effects()[0];
        assert_eq!(child.parent(), Some(vector_id));
        assert_eq!(child.state().type_id().as_str(), Ok("hex.delay"));
        child.id()
    });

    // Full 200px Delay card geometry, intervening meter/add, membership marker, and source housing.
    let delay_card_sel: &'static str =
        Box::leak(format!("effect-device-{}", delay_id.value()).into_boxed_str());
    let delay_card = visual
        .debug_bounds(delay_card_sel)
        .expect("full delay child card");
    assert_eq!(delay_card.size.height, px(DEVICE_FRAME_HEIGHT));
    assert_eq!(
        delay_card.size.width,
        px(delay_panel::FRAME_WIDTH + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    let delay_meter_sel: &'static str =
        Box::leak(format!("device-{}-output-meter", delay_id.value()).into_boxed_str());
    let delay_add_sel: &'static str =
        Box::leak(format!("device-{}-add-after", delay_id.value()).into_boxed_str());
    let delay_marker_sel: &'static str =
        Box::leak(format!("device-nested-marker-{}", delay_id.value()).into_boxed_str());
    let delay_parent_depth_sel: &'static str =
        Box::leak(format!("device-nested-parent-{}-depth-1", vector_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(delay_meter_sel).is_some());
    assert!(visual.debug_bounds(delay_add_sel).is_some());
    assert!(visual.debug_bounds(delay_marker_sel).is_some());
    assert!(visual.debug_bounds(delay_parent_depth_sel).is_some());
    let housing = visual
        .debug_bounds("source-chain-housing")
        .expect("source chain housing");
    assert!(housing.contains(&delay_card.center()));

    // UI-only collapse hides Delay without sending any worker request or mutating project state.
    let (rev_before_collapse, project_before_collapse) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (state.revision, state.project.clone())
    });
    let toggle_sel: &'static str =
        Box::leak(format!("device-{}-nested-toggle", vector_id.value()).into_boxed_str());
    let toggle_bounds = visual
        .debug_bounds(toggle_sel)
        .expect("vector nested collapse toggle");
    visual.simulate_click(toggle_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(delay_card_sel).is_none(),
        "collapsed parent hides nested child cards in the UI"
    );
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, rev_before_collapse);
        assert_eq!(state.project, project_before_collapse);
        assert_eq!(view.worker_requests_pending, 0);
    });
    // Expand again restores the full child card.
    let toggle_bounds = visual
        .debug_bounds(toggle_sel)
        .expect("vector nested expand toggle");
    visual.simulate_click(toggle_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(delay_card_sel).is_some());

    // Parent on/off uses effective unit bypass while preserving Delay's authored switch.
    let vector_bypass_sel: &'static str =
        Box::leak(format!("device-bypass-{}", vector_id.value()).into_boxed_str());
    let parent_off_marker_sel: &'static str =
        Box::leak(format!("device-bypass-parent-off-{}", delay_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(parent_off_marker_sel).is_none());
    let bypass_bounds = visual
        .debug_bounds(vector_bypass_sel)
        .expect("vector power button");
    visual.simulate_click(bypass_bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.device(vector_id))
            .is_some_and(hex_project::Device::bypassed)
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(parent_off_marker_sel).is_some(),
        "child must reflect effective ancestor bypass"
    );
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.device(vector_id).unwrap().bypassed());
        assert!(!project.device(delay_id).unwrap().bypassed());
        assert!(hex_engine::project_device_ancestor_bypassed(
            project, delay_id
        ));
    });
    // Turn Vector back on: Delay's authored switch (enabled) is preserved.
    let bypass_bounds = visual
        .debug_bounds(vector_bypass_sel)
        .expect("vector power button");
    visual.simulate_click(bypass_bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.device(vector_id))
            .is_some_and(|d| !d.bypassed())
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(parent_off_marker_sel).is_none());

    // Descendant edit on Delay inside Vector + Undo restores original value.
    let dry_wet = hex_engine::delay::DRY_WET;
    let initial_wet = editor.read_with(&visual, |view, _| {
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(delay_id)
                .unwrap()
                .state(),
            dry_wet,
        )
        .unwrap()
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device: delay_id,
            parameter: dry_wet,
            value: ParameterValue::Float(0.75),
        }));
    });
    pump_until(&editor, cx, |view| {
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(delay_id)
                .unwrap()
                .state(),
            dry_wet,
        )
        .ok()
            == Some(ParameterValue::Float(0.75))
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(delay_id)
                .unwrap()
                .state(),
            dry_wet,
        )
        .ok()
            == Some(initial_wet)
    });
}

#[gpui::test]
fn effect_host_nested_browser_drop_subtree_copy_and_refusal_preserve_project(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };

    let Some(Created::Track(track)) = edit(Command::AddMidiTrack { name: "fx".into() }).created
    else {
        panic!("track")
    };
    let midi_type = hex_engine::native_device_catalog_for_class(NativeDeviceClass::MidiProcessor)
        .unwrap()
        .remove(0)
        .type_id;
    let midi_ev = edit(Command::InsertMidiProcessor {
        track,
        state: hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::MidiProcessor,
            midi_type,
        )
        .unwrap(),
    });
    let Some(Created::Device(midi_id)) = midi_ev.created else {
        panic!("midi processor")
    };
    let utility_type = hex_lattice_contract::DeviceTypeId::from_static("hex.utility");
    let delay_type = hex_lattice_contract::DeviceTypeId::from_static("hex.delay");
    let root_ev = edit(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Effect,
            utility_type,
        )
        .unwrap(),
    });
    let Some(Created::Device(root_id)) = root_ev.created else {
        panic!("root utility")
    };

    let (handle, editor) = actor_editor_window(cx, service, root_ev.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();

    // No decorative nested slot on MIDI processor.
    let midi_nested_sel: &'static str =
        Box::leak(format!("device-{}-nested-slot", midi_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(midi_nested_sel).is_none());

    // Browser drop into effect host nested slot inserts Delay as child of Utility.
    let (generation, revision) = editor.read_with(&visual, |view, _| {
        (
            view.project_generation,
            view.state.as_ref().unwrap().revision,
        )
    });
    let browser_delay = drag::NativeDeviceDrag {
        project_generation: generation,
        revision,
        source: drag::NativeDeviceDragSource::Browser {
            class: NativeDeviceClass::Effect,
            type_id: delay_type,
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
        return_destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    editor.update(&mut visual, |view, cx| {
        view.drop_browser_device_into_nested(&browser_delay, root_id, cx);
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.track(track))
            .is_some_and(|t| t.devices().len() == 2)
    });
    visual.run_until_parked();

    let child_id = editor.read_with(&visual, |view, _| {
        let devices = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices();
        assert_eq!(devices[0].id(), root_id);
        assert_eq!(devices[1].parent(), Some(root_id));
        assert_eq!(devices[1].state().type_id(), delay_type);
        devices[1].id()
    });
    let child_marker_sel: &'static str =
        Box::leak(format!("device-nested-marker-{}", child_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(child_marker_sel).is_some());

    // Invalid (Instrument) and stale-revision browser drops into nested slot refuse cleanly.
    let (baseline_project, current_rev) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (state.project.clone(), state.revision)
    });
    let invalid_instrument = drag::NativeDeviceDrag {
        project_generation: generation,
        revision: current_rev,
        source: drag::NativeDeviceDragSource::Browser {
            class: NativeDeviceClass::Instrument,
            type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
        return_destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    let stale_delay = drag::NativeDeviceDrag {
        project_generation: generation,
        revision: current_rev.saturating_sub(1),
        source: drag::NativeDeviceDragSource::Browser {
            class: NativeDeviceClass::Effect,
            type_id: delay_type,
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
        return_destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    editor.update(&mut visual, |view, cx| {
        view.drop_browser_device_into_nested(&invalid_instrument, root_id, cx);
        assert!(view.device_panel.message.is_some());
        view.drop_browser_device_into_nested(&stale_delay, root_id, cx);
        assert!(view.device_panel.message.is_some());
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, baseline_project);
        assert_eq!(view.worker_requests_pending, 0);
    });

    // Subtree duplicate (`d`) copies root + child atomically and one Undo removes the copy.
    let root_title_sel: &'static str =
        Box::leak(format!("device-drag-title-{}", root_id.value()).into_boxed_str());
    let root_bounds = visual.debug_bounds(root_title_sel).expect("root title");
    visual.simulate_click(root_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("d");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.track(track))
            .is_some_and(|t| t.devices().len() == 4)
    });
    editor.read_with(&visual, |view, _| {
        let devices = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices();
        assert_eq!(devices[0].id(), root_id);
        assert_eq!(devices[1].parent(), Some(root_id));
        assert_eq!(devices[2].parent(), None);
        assert_eq!(devices[3].parent(), Some(devices[2].id()));
        assert_eq!(devices[3].state().type_id(), delay_type);
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|s| s.project.track(track))
            .is_some_and(|t| t.devices().len() == 2)
    });
}
