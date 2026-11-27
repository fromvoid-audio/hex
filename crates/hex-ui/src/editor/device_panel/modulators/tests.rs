//! Modulator panel pure unit tests.
use super::*;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn enabling_a_route_that_closes_a_cycle_preserves_the_existing_bank() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };
    for _ in 0..2 {
        let modulation = apply(editor.project(), device, Edit::Add(SourceKind::Macro)).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
    }
    let bank = editor.project().device(device).unwrap().modulation();
    let [first, second] = [bank.sources()[0].id, bank.sources()[1].id];
    let mapped = apply(
        editor.project(),
        device,
        Edit::Map(
            MappingOutput::main(first),
            MappingTarget::Macro(second),
            DEFAULT_MAPPING_DEPTH,
        ),
    )
    .unwrap();
    let mut routes = mapped.routes().to_vec();
    let id = ModulationRouteId::new(editor.project().next_identity() + 1).unwrap();
    routes.push(DeviceModulationRoute {
        id,
        source: second,
        destination: macro_destination(first),
        enabled: false,
        ..routes[0]
    });
    let modulation = DeviceModulation::new(mapped.sources().to_vec(), routes).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: modulation.clone(),
        })
        .unwrap();

    assert_eq!(
        apply(editor.project(), device, Edit::ToggleRoute(id)),
        Err(Invalid("modulation routes form a cycle")),
    );
    assert_eq!(
        editor.project().device(device).unwrap().modulation(),
        &modulation
    );
}

#[test]
fn added_sources_receive_distinct_canonical_colors_and_effective_feedback_matches_range_law() {
    use hex_devices::utility::parameter::GAIN;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("utility fixture");
    };

    for kind in [SourceKind::Macro, SourceKind::Lfo, SourceKind::Button] {
        let modulation = apply(editor.project(), device, Edit::Add(kind)).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
    }
    let sources = editor
        .project()
        .device(device)
        .unwrap()
        .modulation()
        .sources()
        .to_vec();
    assert_eq!(sources.len(), 3);
    assert_ne!(sources[0].color, sources[1].color);
    assert_ne!(sources[1].color, sources[2].color);

    // Raise macro value to 0.50 and map to GAIN with depth +0.40 => static_delta = +0.20.
    for _ in 0..10 {
        let modulation = apply(
            editor.project(),
            device,
            Edit::MacroValue(sources[0].id, 0.05),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
    }
    let mapped = apply(
        editor.project(),
        device,
        Edit::Map(
            MappingOutput::main(sources[0].id),
            MappingTarget::Device(GAIN),
            0.4,
        ),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: mapped,
        })
        .unwrap();
    let bank = editor.project().device(device).unwrap().modulation();
    let eff = effective_parameter_modulation(bank, GAIN).expect("macro effective feedback");
    assert!(
        (eff.static_delta - 0.2).abs() < 1e-5,
        "got {}",
        eff.static_delta
    );
    assert_eq!(eff.dynamic_span, 0.0);
    assert_eq!(eff.active_routes, 1);
    assert_eq!(
        eff.accent_color,
        theme::element_colors(sources[0].color).accent
    );

    // Also map LFO with -0.30 depth => dynamic_span = 0.30, static_delta preserved.
    let mapped_lfo = apply(
        editor.project(),
        device,
        Edit::Map(
            MappingOutput::main(sources[1].id),
            MappingTarget::Device(GAIN),
            -0.3,
        ),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: mapped_lfo,
        })
        .unwrap();
    let bank = editor.project().device(device).unwrap().modulation();
    let eff = effective_parameter_modulation(bank, GAIN).expect("combined effective feedback");
    assert!((eff.static_delta - 0.2).abs() < 1e-5);
    assert!((eff.dynamic_span - 0.3).abs() < 1e-5);
    assert_eq!(eff.active_routes, 2);

    // Disabling both sources clears effective modulation feedback.
    for source_id in [sources[0].id, sources[1].id] {
        let modulation = apply(editor.project(), device, Edit::ToggleSource(source_id)).unwrap();
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
    }
    let bank = editor.project().device(device).unwrap().modulation();
    assert!(effective_parameter_modulation(bank, GAIN).is_none());
}
