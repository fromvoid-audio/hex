use super::*;

#[gpui::test]
fn modulation_panel_maps_fm4_adsr_and_edits_all_controls(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::Adsr);
}

#[gpui::test]
fn modulation_panel_maps_authored_fm4_connection_depths(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::ConnectionDepths);
}

#[gpui::test]
fn modulation_panel_maps_vector_output_and_preserves_the_authored_state(cx: &mut TestAppContext) {
    exercise_vector_modulation_mapper(cx, Some("lfo"));
}

#[gpui::test]
fn modulation_panel_maps_vector_adsr_and_keeps_occurrence_pressure_fm4_only(
    cx: &mut TestAppContext,
) {
    exercise_vector_modulation_mapper(cx, Some("adsr"));
}

#[gpui::test]
fn modulation_panel_omits_note_sources_on_legacy_vector(cx: &mut TestAppContext) {
    exercise_vector_modulation_mapper(cx, None);
}

#[gpui::test]
fn modulation_panel_maps_only_the_fm4_host_output_stage(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::HostOutput);
}

#[gpui::test]
fn modulation_panel_keeps_fm4_steps_free_running(cx: &mut TestAppContext) {
    exercise_fm4_modulation_mapper(cx, Fm4MappingFixture::FreeSteps);
}

#[gpui::test]
fn modulation_panel_duplicates_a_mapped_source_and_undoes_it_in_one_edit(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let click = click_modulator_control;
    let bank = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    click(&mut visual, "device-modulator-toggle".into(), false);
    add_modulator_from_menu(&mut visual, device, "lfo");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| bank(view).sources()[0].id);
    click(&mut visual, format!("lfo-{}-map", source.value()), true);
    click(
        &mut visual,
        format!(
            "map-target-{}-{}",
            device.value(),
            hex_devices::utility::parameter::GAIN.value()
        ),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    visual.run_until_parked();
    let original = editor.read_with(cx, |view, _| bank(view));
    click(
        &mut visual,
        format!("lfo-{}-duplicate", source.value()),
        true,
    );
    assert!(editor.read_with(cx, |view, _| {
        view.worker_requests_pending > 0
            || !view.pending_requests.is_empty()
            || bank(view).sources().len() == 2
    }));
    pump_until(&editor, cx, |view| bank(view).sources().len() == 2);
    let duplicate = editor.read_with(cx, |view, _| {
        let bank = bank(view);
        assert_eq!(bank.sources()[0], original.sources()[0]);
        assert_ne!(bank.sources()[1].id, source);
        assert!(!bank.sources()[1].enabled);
        assert_eq!(bank.sources()[1].state, original.sources()[0].state);
        assert_eq!(bank.routes()[0], original.routes()[0]);
        assert_ne!(bank.routes()[1].id, original.routes()[0].id);
        assert_eq!(bank.routes()[1].source, bank.sources()[1].id);
        assert_eq!(
            bank.routes()[1].destination,
            original.routes()[0].destination
        );
        let project = &view.state.as_ref().unwrap().project;
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(reopened.device(device).unwrap().modulation(), &bank);
        bank
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view) == original);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == duplicate);
}

#[gpui::test]
fn modulation_panel_adds_and_maps_an_envelope_follower(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        hex_devices::utility::UTILITY_TYPE_ID,
        "follower",
        hex_engine::follower_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

#[gpui::test]
fn modulation_panel_adds_and_maps_a_manual_macro(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    assert_modulation_mapping(
        cx,
        hex_devices::utility::UTILITY_TYPE_ID,
        "macro",
        hex_engine::macro_modulator::TYPE_ID,
        &[parameter::GAIN, parameter::WIDTH],
    );
}

#[gpui::test]
fn modulation_panel_maps_independent_macro_four_outputs_and_an_incoming_control(
    cx: &mut TestAppContext,
) {
    use hex_devices::utility::parameter;
    use hex_engine::macro_four_modulator as macro_four;
    use hex_project::{DeviceModulation, ModulationDestination, ModulationOutputId};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["macro-4", "lfo"].into_iter().enumerate() {
        add_modulator_from_menu(&mut visual, device, kind);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let source = slots.sources()[0].id;
    let lfo = slots.sources()[1].id;
    let controls = |view: &EditorView| {
        macro_four::Settings::decode(&bank(view).sources()[0].state)
            .unwrap()
            .values
    };
    let gain_overlay = || format!("map-target-{}-{}", device.value(), parameter::GAIN.value());
    for (index, output) in [1, 3].into_iter().enumerate() {
        click(
            &mut visual,
            format!("macro-4-{}-{output}-more", source.value()),
            true,
        );
        pump_until(&editor, cx, |view| controls(view)[output - 1] == 0.05);
        visual.run_until_parked();
        click(
            &mut visual,
            format!("macro-4-{}-{output}-map", source.value()),
            true,
        );
        click(&mut visual, gain_overlay(), false);
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }
    let second = editor.read_with(cx, |view, _| bank(view).routes()[1]);
    for expected in [0.05_f32, 0.0, -0.05] {
        click(
            &mut visual,
            format!("modulation-route-{}-less", second.id.value()),
            true,
        );
        pump_until(&editor, cx, |view| {
            bank(view).routes()[1].amount.get() == expected
        });
        visual.run_until_parked();
    }
    click(&mut visual, format!("lfo-{}-map", lfo.value()), true);
    click(
        &mut visual,
        format!("map-macro-4-{}-{}-3", device.value(), source.value()),
        true,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 3);
    visual.run_until_parked();
    let mapped = editor.read_with(cx, |view, _| {
        assert_eq!(controls(view), [0.05, 0.0, 0.05, 0.0]);
        let mapped = bank(view);
        let routes = mapped.routes();
        assert_eq!([routes[0].source, routes[1].source], [source; 2]);
        assert_eq!(routes[0].output, ModulationOutputId::MAIN);
        assert_eq!(routes[1].output, ModulationOutputId::new(3).unwrap());
        assert_eq!(routes[0].destination, routes[1].destination);
        assert_eq!(
            [routes[0].amount.get(), routes[1].amount.get()],
            [0.1, -0.05]
        );
        assert_eq!(routes[2].source, lfo);
        assert_eq!(
            routes[2].destination,
            ModulationDestination::Modulator {
                source,
                module_type: macro_four::TYPE_ID,
                parameter: macro_four::VALUE_PARAMETERS[2],
            }
        );
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..2]
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == mapped);
}

#[gpui::test]
fn modulation_panel_maps_two_math_inputs_and_the_result(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    use hex_engine::math_modulator as math;
    use hex_project::{DeviceModulation, ModulationDestination};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["lfo", "macro", "math"].into_iter().enumerate() {
        add_modulator_from_menu(&mut visual, device, kind);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let lfo = slots.sources()[0].id;
    let manual = slots.sources()[1].id;
    let math_source = slots.sources()[2].id;
    let settings =
        |view: &EditorView| math::Settings::decode(&bank(view).sources()[2].state).unwrap();
    let key = |suffix| format!("math-{}-{suffix}", math_source.value());
    click(&mut visual, key("operation"), true);
    pump_until(&editor, cx, |view| {
        settings(view).operation == math::Operation::Add
    });
    visual.run_until_parked();
    click(&mut visual, key("a-more"), true);
    pump_until(&editor, cx, |view| settings(view).inputs[0] == 0.05);
    visual.run_until_parked();
    click(&mut visual, key("b-less"), true);
    pump_until(&editor, cx, |view| settings(view).inputs[1] == 0.95);
    visual.run_until_parked();
    for (index, (name, upstream, input)) in [("lfo", lfo, "a"), ("macro", manual, "b")]
        .into_iter()
        .enumerate()
    {
        click(
            &mut visual,
            format!("{name}-{}-map", upstream.value()),
            true,
        );
        click(
            &mut visual,
            format!(
                "map-math-{input}-{}-{}",
                device.value(),
                math_source.value()
            ),
            true,
        );
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }
    click(&mut visual, key("map"), true);
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), parameter::GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 3);
    visual.run_until_parked();
    let mapped = editor.read_with(cx, |view, _| {
        let mapped = bank(view);
        let routes = mapped.routes();
        for (route, (source, parameter)) in routes
            .iter()
            .zip([(lfo, math::INPUT_A), (manual, math::INPUT_B)])
        {
            assert_eq!(route.source, source);
            assert_eq!(
                route.destination,
                ModulationDestination::Modulator {
                    source: math_source,
                    module_type: math::TYPE_ID,
                    parameter,
                }
            );
        }
        assert_eq!(routes[2].source, math_source);
        assert_eq!(
            routes[2].destination,
            ModulationDestination::Device {
                device_type: base.type_id(),
                target: hex_project::DeviceAutomationTarget::Parameter(parameter::GAIN),
                vst3_class: None,
            }
        );
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..2]
    });
}

#[gpui::test]
fn modulation_panel_closes_the_add_menu_when_its_device_is_removed(cx: &mut TestAppContext) {
    let (service, state, track, device) =
        actor_native_effect_fixture(hex_devices::utility::UTILITY_TYPE_ID);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    click_modulator_control(
        &mut visual,
        format!("add-modulator-{}", device.value()),
        true,
    );
    assert!(visual.debug_bounds("context-menu-panel").is_some());
    assert!(editor.read_with(cx, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|item| item.label != "note pressure")
    }));

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .is_none()
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("context-menu-panel").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.context_menu.is_none());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .is_none()
        );
        assert!(view.error.is_none());
    });
}

#[gpui::test]
fn modulation_panel_maps_all_three_mix_inputs_and_the_result(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter::GAIN;
    use hex_engine::mix_modulator as mix;
    use hex_project::{DeviceAutomationTarget, DeviceModulation, ModulationDestination};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    let source_kinds = ["lfo", "macro", "random", "mix"];
    for (index, kind) in source_kinds.into_iter().enumerate() {
        add_modulator_from_menu(&mut visual, device, kind);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let mix_source = slots.sources()[3].id;
    let key = |suffix| format!("mix-{}-{suffix}", mix_source.value());
    let settings =
        |view: &EditorView| mix::Settings::decode(&bank(view).sources()[3].state).unwrap();
    let edits = [
        (
            "a-more",
            mix::Settings {
                inputs: [0.05, 1.0],
                blend: 0.5,
            },
        ),
        (
            "b-less",
            mix::Settings {
                inputs: [0.05, 0.95],
                blend: 0.5,
            },
        ),
        (
            "blend-more",
            mix::Settings {
                inputs: [0.05, 0.95],
                blend: 0.55,
            },
        ),
    ];
    for (suffix, expected) in edits {
        click(&mut visual, key(suffix), true);
        pump_until(&editor, cx, |view| settings(view) == expected);
        visual.run_until_parked();
    }
    let target_key = |input| format!("map-mix-{input}-{}-{}", device.value(), mix_source.value());
    let drag = |visual: &mut VisualTestContext, input, delta| {
        let start = modulator_control_position(visual, target_key(input), true);
        let end = point(start.x, start.y + px(delta));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    };
    for (index, input) in ["a", "b", "blend"].into_iter().enumerate() {
        let upstream = slots.sources()[index].id;
        click(
            &mut visual,
            format!("{}-{}-map", source_kinds[index], upstream.value()),
            true,
        );
        if input == "blend" {
            drag(&mut visual, input, 30.0);
        } else {
            click(&mut visual, target_key(input), true);
        }
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }
    let first = editor.read_with(cx, |view, _| bank(view));
    assert_eq!(first.routes()[2].amount.get(), -0.3);
    drag(&mut visual, "blend", -60.0);
    pump_until(&editor, cx, |view| {
        bank(view).routes()[2].amount.get() == 0.3
    });
    assert_eq!(
        editor.read_with(cx, |view, _| bank(view).routes()[2].id),
        first.routes()[2].id
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| bank(view) == first);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes()[2].amount.get() == 0.3
    });
    visual.run_until_parked();
    click(&mut visual, key("map"), true);
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 4);
    let mapped = editor.read_with(cx, |view, _| {
        let mapped = bank(view);
        assert_eq!(mapped.sources(), first.sources());
        assert_eq!(mapped.routes()[2].amount.get(), 0.3);
        for (index, parameter) in [mix::INPUT_A, mix::INPUT_B, mix::BLEND]
            .into_iter()
            .enumerate()
        {
            assert_eq!(mapped.routes()[index].source, slots.sources()[index].id);
            assert_eq!(
                mapped.routes()[index].destination,
                ModulationDestination::Modulator {
                    source: mix_source,
                    module_type: mix::TYPE_ID,
                    parameter
                }
            );
        }
        assert_eq!(mapped.routes()[3].source, mix_source);
        assert!(matches!(
            mapped.routes()[3].destination,
            ModulationDestination::Device {
                target: DeviceAutomationTarget::Parameter(GAIN),
                ..
            }
        ));
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..3]
    });
}

#[gpui::test]
fn modulation_panel_maps_an_lfo_into_a_macro_and_then_a_device_parameter(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter;
    use hex_project::{DeviceModulation, ModulationDestination};
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    click_modulator_control(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["lfo", "macro"].into_iter().enumerate() {
        add_modulator_from_menu(&mut visual, device, kind);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let lfo = slots.sources()[0].id;
    let manual = slots.sources()[1].id;
    click_modulator_control(&mut visual, format!("lfo-{}-map", lfo.value()), true);
    click_modulator_control(
        &mut visual,
        format!("map-macro-{}-{}", device.value(), manual.value()),
        true,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 1);
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("macro-{}-map", manual.value()), true);
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-macro-{}-{}", device.value(), manual.value()).into_boxed_str()
            ))
            .is_none()
    );
    click_modulator_control(
        &mut visual,
        format!("map-target-{}-{}", device.value(), parameter::GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 2);
    let mapped = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let mapped = bank(view);
        assert_eq!(mapped.routes()[0].source, lfo);
        assert_eq!(
            mapped.routes()[0].destination,
            ModulationDestination::Modulator {
                source: manual,
                module_type: hex_engine::macro_modulator::TYPE_ID,
                parameter: hex_engine::macro_modulator::VALUE_PARAMETER,
            }
        );
        assert_eq!(mapped.routes()[1].source, manual);
        assert_eq!(mapped.sources(), slots.sources());
        let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(loaded.device(device).unwrap().modulation(), &mapped);
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..1]
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == mapped);
    visual.run_until_parked();
    click_modulator_control(&mut visual, format!("lfo-{}-remove", lfo.value()), true);
    pump_until(&editor, cx, |view| {
        let bank = bank(view);
        bank.sources() == &mapped.sources()[1..] && bank.routes() == &mapped.routes()[1..]
    });
}

#[gpui::test]
fn modulation_panel_maps_both_lfo_rate_modes_and_preserves_assignments_on_sync(
    cx: &mut TestAppContext,
) {
    use hex_devices::utility::parameter;
    use hex_engine::lfo_modulator as lfo;
    use hex_project::{DeviceModulation, ModulationDestination};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    for (index, kind) in ["lfo", "lfo", "macro"].into_iter().enumerate() {
        add_modulator_from_menu(&mut visual, device, kind);
        pump_until(&editor, cx, |view| bank(view).sources().len() == index + 1);
        visual.run_until_parked();
    }
    let slots = editor.read_with(cx, |view, _| bank(view));
    let [free, synced, manual] = std::array::from_fn(|index| slots.sources()[index].id);
    let rate_key = |source: hex_project::ModulationSourceId| {
        format!("map-lfo-rate-{}-{}", device.value(), source.value())
    };
    click(&mut visual, format!("lfo-{}-sync", synced.value()), true);
    pump_until(&editor, cx, |view| {
        matches!(
            lfo::Settings::decode(&bank(view).sources()[1].state)
                .unwrap()
                .rate,
            lfo::Rate::BeatsPerCycle(_)
        )
    });
    visual.run_until_parked();

    click(&mut visual, format!("macro-{}-map", manual.value()), true);
    for (index, target) in [synced, free].into_iter().enumerate() {
        click(&mut visual, rate_key(target), true);
        pump_until(&editor, cx, |view| bank(view).routes().len() == index + 1);
        visual.run_until_parked();
    }
    click(&mut visual, rate_key(free), true);
    assert!(
        editor.read_with(cx, |view, _| {
            !view.edit_pending() && bank(view).routes().len() == 2
        }),
        "clicking an assigned input keeps its single route"
    );
    click(&mut visual, format!("macro-{}-map", manual.value()), true);
    let synced_rate = |view: &EditorView| {
        lfo::Settings::decode(&bank(view).sources()[1].state)
            .unwrap()
            .rate
    };
    let original_synced = editor.read_with(cx, |view, _| synced_rate(view));
    let assigned = editor.read_with(cx, |view, _| bank(view).routes().to_vec());
    for is_free in [true, false] {
        click(&mut visual, format!("lfo-{}-sync", synced.value()), true);
        pump_until(&editor, cx, |view| {
            matches!(synced_rate(view), lfo::Rate::Hertz(_)) == is_free
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(bank(view).routes(), assigned);
            let project = &view.state.as_ref().unwrap().project;
            let version = if is_free {
                hex_storage::LFO_RATE_ROUTE_SCHEMA_VERSION
            } else {
                hex_storage::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
            };
            assert_eq!(hex_storage::schema_version_for(project), version);
        });
        visual.run_until_parked();
    }
    assert_eq!(
        editor.read_with(cx, |view, _| synced_rate(view)),
        original_synced
    );
    let rate_route = editor.read_with(cx, |view, _| bank(view).routes()[0]);
    assert_eq!(rate_route.source, manual);
    assert_eq!(
        rate_route.destination,
        ModulationDestination::Modulator {
            source: synced,
            module_type: lfo::TYPE_ID,
            parameter: lfo::RATE_PARAMETER,
        }
    );
    for expected in [0.05, 0.0, -0.05] {
        click(
            &mut visual,
            format!("modulation-route-{}-less", rate_route.id.value()),
            true,
        );
        pump_until(&editor, cx, |view| {
            (bank(view).routes()[0].amount.get() - expected).abs() < 1e-6
        });
        visual.run_until_parked();
    }

    click(&mut visual, format!("lfo-{}-map", synced.value()), true);
    assert!(
        visual
            .debug_bounds(Box::leak(rate_key(synced).into_boxed_str()))
            .is_none(),
        "self-route"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("map-macro-{}-{}", device.value(), manual.value()).into_boxed_str()
            ))
            .is_none(),
        "cycle"
    );
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), parameter::GAIN.value()),
        false,
    );
    pump_until(&editor, cx, |view| bank(view).routes().len() == 3);
    let mapped = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let mapped = bank(view);
        assert_eq!(mapped.routes()[2].source, synced);
        assert_eq!(mapped.sources()[0], slots.sources()[0]);
        assert_eq!(
            hex_storage::schema_version_for(project),
            hex_storage::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
        );
        let loaded = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(loaded.device(device).unwrap().modulation(), &mapped);
        mapped
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        bank(view).routes() == &mapped.routes()[..2]
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| bank(view) == mapped);
}

#[gpui::test]
fn modulation_panel_adds_and_maps_seeded_random(cx: &mut TestAppContext) {
    assert_modulation_mapping(
        cx,
        hex_devices::utility::UTILITY_TYPE_ID,
        "random",
        hex_engine::random_modulator::TYPE_ID,
        &[hex_devices::utility::parameter::GAIN],
    );
}

#[gpui::test]
fn modulation_panel_maps_and_toggles_a_button(cx: &mut TestAppContext) {
    use hex_devices::utility::{UTILITY_TYPE_ID, parameter::GAIN};
    assert_modulation_mapping(
        cx,
        UTILITY_TYPE_ID,
        "button",
        hex_engine::button_modulator::TYPE_ID,
        &[GAIN],
    );
}

#[gpui::test]
fn modulation_panel_maps_and_toggles_two_independent_buttons(cx: &mut TestAppContext) {
    use hex_devices::utility::parameter::GAIN;
    use hex_engine::buttons_modulator as buttons;
    use hex_project::{DeviceModulation, ModulationOutputId};

    let (service, state, track, device, _, _) = actor_utility_fixture();
    let base = state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let bank = |view: &EditorView| -> DeviceModulation {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .clone()
    };
    let switches = |view: &EditorView| {
        buttons::Settings::decode(&bank(view).sources()[0].state)
            .unwrap()
            .on
    };
    let click = click_modulator_control;
    click(&mut visual, "device-modulator-toggle".into(), false);
    add_modulator_from_menu(&mut visual, device, "buttons");
    pump_until(&editor, cx, |view| bank(view).sources().len() == 1);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| {
        assert_eq!(switches(view), [false, false]);
        bank(view).sources()[0].id
    });
    let target = format!("map-target-{}-{}", device.value(), GAIN.value());
    for output in 1..=2 {
        click(
            &mut visual,
            format!("buttons-{}-{output}-map", source.value()),
            true,
        );
        click(&mut visual, target.clone(), false);
        pump_until(&editor, cx, |view| bank(view).routes().len() == output);
        visual.run_until_parked();
    }
    let mapped = editor.read_with(cx, |view, _| {
        let mapped = bank(view);
        assert_eq!(mapped.routes()[0].output, ModulationOutputId::MAIN);
        assert_eq!(
            mapped.routes()[1].output,
            ModulationOutputId::new(2).unwrap()
        );
        assert_eq!(
            mapped.routes()[0].destination,
            mapped.routes()[1].destination
        );
        mapped
    });
    for (output, expected) in [(1, [true, false]), (2, [true, true])] {
        click(
            &mut visual,
            format!("buttons-{}-{output}-toggle", source.value()),
            true,
        );
        pump_until(&editor, cx, |view| switches(view) == expected);
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.device(device).unwrap().state(), &base);
        assert_eq!(bank(view).routes(), mapped.routes());
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(&reopened, project.as_ref());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| switches(view) == [true, false]);
}

#[gpui::test]
fn modulation_panel_edits_shared_source_effect_on_receiver_and_contributor(
    cx: &mut TestAppContext,
) {
    use hex_project::{DeviceAutomationTarget, DeviceOwner, ModulationDestination};

    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    let edit = |command| {
        service.submit(Request::Edit(command)).unwrap();
        let event = receive_service_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let add_track = |name: &str| {
        let Some(Created::Track(track)) = edit(Command::AddMidiTrack { name: name.into() }).created
        else {
            panic!("track fixture");
        };
        track
    };
    let receiver = add_track("receiver");
    let contributor = add_track("contributor");
    let device_type = hex_lattice_contract::DeviceTypeId::from_static("hex.utility");
    let base = hex_engine::default_native_device_state(device_type).unwrap();
    let created = edit(Command::CreateNativeSource {
        name: "shared".into(),
        devices: vec![
            hex_project::default_instrument_state().unwrap(),
            base.clone(),
        ],
        instrument_index: 0,
    });
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source fixture");
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    assert!(!hex_engine::project_device_host_gain_modulation_supported(
        &created.state.project,
        device
    ));
    edit(Command::BindTrackSource {
        track: receiver,
        source: Some(source),
    });
    let bound = edit(Command::BindTrackSource {
        track: contributor,
        source: Some(source),
    });
    assert!(hex_engine::project_device_host_gain_modulation_supported(
        &bound.state.project,
        device
    ));
    let gain = hex_lattice_contract::ParameterId::from_static(1);
    assert!(hex_engine::project_device_modulation_target_supported(
        &bound.state.project,
        device,
        gain,
    ));
    assert!(!hex_engine::project_device_modulation_target_supported(
        &bound.state.project,
        device,
        hex_lattice_contract::ParameterId::from_static(11),
    ));

    let bound = edit(Command::InsertDevice {
        track: contributor,
        before: None,
        state: base.clone(),
    });

    let (handle, editor) = actor_editor_window(cx, service, bound.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector: &'static str =
        Box::leak(format!("source-effect-modulation-{}", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(selector)
        .expect("source effect on its receiver");
    let click = |visual: &mut VisualTestContext, key: String| {
        let key = Box::leak(key.into_boxed_str());
        let element = visual
            .debug_bounds(key)
            .unwrap_or_else(|| panic!("missing {key}"));
        assert!(
            bounds.contains(&element.center()),
            "{key} belongs to another card"
        );
        visual.simulate_click(element.center(), Modifiers::default());
        visual.run_until_parked();
    };
    click(&mut visual, "device-modulator-toggle".into());
    click(&mut visual, format!("add-modulator-{}", device.value()));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let source_slot = |view: &EditorView| {
        view.state
            .as_ref()?
            .project
            .device(device)?
            .modulation()
            .sources()
            .first()
            .map(|slot| slot.id)
    };
    pump_until(&editor, cx, |view| source_slot(view).is_some());
    let slot = editor.read_with(cx, |view, _| source_slot(view).unwrap());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let device = project.device(device).unwrap();
        assert!(!super::super::modulators::source_utility_gain_available(
            project, device
        ));
    });
    let gain_control = Box::leak(
        format!("device-{}-parameter-{}-value", device.value(), gain.value()).into_boxed_str(),
    );
    assert!(visual.debug_bounds(gain_control).is_none());
    let balance_control =
        Box::leak(format!("device-{}-parameter-2-value", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(balance_control).is_none());
    let width_control =
        Box::leak(format!("device-{}-parameter-3-value", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(width_control).is_none());
    let mid_side_control =
        Box::leak(format!("device-{}-parameter-10-value", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(mid_side_control).is_none());
    click_modulator_control(&mut visual, format!("lfo-{}-map", slot.value()), true);
    let mut mapping_targets = 0;
    for parameter in 1..=10 {
        let key = Box::leak(format!("map-target-{}-{parameter}", device.value()).into_boxed_str());
        if let Some(target) = visual.debug_bounds(key) {
            mapping_targets += 1;
            if !bounds.contains(&target.center()) {
                visual.simulate_event(gpui::ScrollWheelEvent {
                    position: point(bounds.center().x, bounds.bottom() - px(14.0)),
                    delta: gpui::ScrollDelta::Pixels(point(
                        px(0.0),
                        bounds.center().y - target.center().y,
                    )),
                    ..Default::default()
                });
                visual.run_until_parked();
            }
            let target = visual.debug_bounds(key).unwrap();
            assert!(
                bounds.contains(&target.center()),
                "mapping target {parameter} unreachable: {target:?} vs {bounds:?}"
            );
        }
    }
    assert!(mapping_targets > 1);
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(bounds.center().x, bounds.bottom() - px(14.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(1000.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    click_modulator_control(
        &mut visual,
        format!("map-output-gain-{}", device.value()),
        true,
    );

    let route_count = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .routes()
            .len()
    };
    let dispatched = |cx: &mut TestAppContext, expected| {
        editor.read_with(cx, |view, _| {
            view.worker_requests_pending > 0
                || !view.pending_requests.is_empty()
                || route_count(view) == expected
        })
    };
    assert!(dispatched(cx, 1), "source output trim did not dispatch");
    pump_until(&editor, cx, |view| route_count(view) == 1);
    visual.run_until_parked();
    click(
        &mut visual,
        format!("map-target-{}-{}", device.value(), gain.value()),
    );
    assert!(dispatched(cx, 2), "source provider gain did not dispatch");
    pump_until(&editor, cx, |view| route_count(view) == 2);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let location = project.device_location(device).unwrap();
        assert_eq!(location.owner, DeviceOwner::Source(source));
        assert_eq!(location.device.state(), &base);
        let route = location.device.modulation().routes()[0];
        assert_eq!(route.source, slot);
        assert!(matches!(
            route.destination,
            ModulationDestination::Device {
                target: DeviceAutomationTarget::HostOutputGain,
                ..
            }
        ));
        let matrix_route = location.device.modulation().routes()[1];
        assert_eq!(matrix_route.source, slot);
        assert!(
            matches!(matrix_route.destination, ModulationDestination::Device {
            target: DeviceAutomationTarget::Parameter(id), ..
        } if id == gain)
        );
        let reopened = hex_storage::decode(&hex_storage::encode(project).unwrap()).unwrap();
        assert_eq!(
            reopened.device(device).unwrap().modulation().routes(),
            &[route, matrix_route]
        );
    });

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| route_count(view) == 1);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| route_count(view) == 2);
    visual.run_until_parked();
    let last_target = Box::leak(format!("map-target-{}-10", device.value()).into_boxed_str());
    let target = visual
        .debug_bounds(last_target)
        .expect("last supported Utility target");
    if !bounds.contains(&target.center()) {
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: point(bounds.center().x, bounds.bottom() - px(14.0)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), bounds.center().y - target.center().y)),
            ..Default::default()
        });
        visual.run_until_parked();
    }
    let target = visual.debug_bounds(last_target).unwrap();
    assert!(bounds.contains(&target.center()));
    visual.simulate_click(target.center(), Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| route_count(view) == 3);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| route_count(view) == 2);
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(bounds.center().x, bounds.bottom() - px(14.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(1000.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(contributor);
        cx.notify();
    });
    visual.run_until_parked();
    visual
        .debug_bounds(selector)
        .expect("shared source effect on contributor");
    let (receiver_local, contributor_local) = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (
            project.track(receiver).unwrap().devices().to_vec(),
            project.track(contributor).unwrap().devices().to_vec(),
        )
    });
    assert!(!contributor_local.is_empty());
    add_modulator_from_menu(&mut visual, device, "lfo");
    let sources = |view: &EditorView| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .modulation()
            .sources()
            .len()
    };
    pump_until(&editor, cx, |view| sources(view) == 2);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.device_location(device).unwrap().owner,
            DeviceOwner::Source(source)
        );
        assert_eq!(project.track(receiver).unwrap().source_id(), Some(source));
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
        assert_eq!(project.track(receiver).unwrap().devices(), receiver_local);
        assert_eq!(
            project.track(contributor).unwrap().devices(),
            contributor_local
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| sources(view) == 1);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| sources(view) == 2);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(receiver);
        cx.notify();
    });
    visual.run_until_parked();
    visual
        .debug_bounds(selector)
        .expect("same source effect on receiver");
    editor.read_with(cx, |view, _| assert_eq!(sources(view), 2));
}
