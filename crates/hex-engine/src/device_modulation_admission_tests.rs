//! Authored banks must not reach an engine that would silently omit them.
use super::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceMetadata, NativeDeviceState};
use hex_project::{Command, Created, DeviceModulation, Editor, ModulationSourceId, Modulator};

#[test]
fn arrangement_session_direct_preparation_and_storage_quotes_refuse_unprepared_banks() {
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Device(id) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    assert!(validate_project(editor.project()).is_ok());
    let before = editor.project().snapshot();
    let refused = editor.execute_prepared(
        Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: ModulationSourceId::new(before.next_id).unwrap(),
                    enabled: true,
                    state: NativeDeviceState::new(
                        DeviceTypeId::from_static("test.modulator"),
                        1,
                        vec![1],
                    )
                    .unwrap(),
                }],
                vec![],
            )
            .unwrap(),
        },
        |candidate| crate::compile::project(candidate, &Default::default()),
    );
    assert!(refused.is_err());
    assert_eq!(editor.project().snapshot(), before);
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(
                vec![Modulator {
                    id: ModulationSourceId::new(editor.project().snapshot().next_id).unwrap(),
                    enabled: true,
                    state: NativeDeviceState::new(
                        DeviceTypeId::from_static("test.modulator"),
                        1,
                        vec![1],
                    )
                    .unwrap(),
                }],
                vec![],
            )
            .unwrap(),
        })
        .unwrap();
    let project = editor.project();
    let device = project.device(id).unwrap();
    assert_eq!(
        quote(device),
        Err(crate::AudioStorageError::UnsupportedConstructor)
    );
    assert_eq!(
        prepare(device).err().unwrap().0,
        "authored device modulation requires playback preparation"
    );
    assert_eq!(
        crate::compile::project(project, &Default::default())
            .err()
            .unwrap()
            .0,
        "authored device modulation requires playback preparation"
    );
    assert_eq!(
        crate::PreparedSessionInventory::prepare(
            project,
            std::sync::Arc::new(hex_devices::SineTable::new()),
            None
        )
        .err()
        .unwrap()
        .0,
        "authored device modulation requires playback preparation"
    );
}
