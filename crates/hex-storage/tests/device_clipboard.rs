//! Clipboard-created devices use the existing complete saved-project contract.
use hex_project::{Command, Created, DeviceChainOwner, DeviceSelectionIds, Editor, Project};
use std::sync::Arc;

#[test]
fn copied_device_and_independent_instrument_roundtrip() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(source) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("source");
    };
    let Created::Track(destination) = editor
        .execute(Command::AddTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("destination");
    };
    let Created::Device(instrument) = editor
        .execute(Command::InsertInstrument {
            track: source,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument");
    };
    let effect_state = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.effect"),
        1,
        vec![1, 2],
    )
    .unwrap();
    let Created::Device(effect) = editor
        .execute(Command::InsertDevice {
            track: source,
            before: None,
            state: effect_state,
        })
        .unwrap()
    else {
        panic!("effect");
    };
    editor
        .execute(Command::SetDeviceBypass {
            device: effect,
            bypassed: true,
        })
        .unwrap();
    let selection = DeviceSelectionIds::new(&[instrument, effect]).unwrap();
    let clipboard = Arc::new(
        editor
            .project()
            .capture_devices(DeviceChainOwner::Track(source), &selection)
            .unwrap(),
    );
    editor
        .execute(Command::PasteDevices {
            clipboard,
            owner: DeviceChainOwner::Track(destination),
            before: None,
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), *editor.project());
}
