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
