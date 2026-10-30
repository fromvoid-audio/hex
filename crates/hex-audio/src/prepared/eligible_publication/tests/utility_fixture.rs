//! Real closed Utility construction reused by native handoff/pressure controls.
use hex_project::{Command, Created, Editor};
pub(super) fn project() -> (hex_project::Project, hex_project::TrackId) {
    project_with_loop(8)
}

pub(super) fn project_with_loop(duration: u64) -> (hex_project::Project, hex_project::TrackId) {
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "audio".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration,
            source_offset: 0,
            source_length: 8,
            looping: duration > 8,
        })
        .unwrap();
    let state = hex_engine::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter(
        &state,
        hex_lattice_contract::ParameterId::from_static(11),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state,
        });
    snapshot.next_id += 1;
    (
        hex_project::Project::from_snapshot(snapshot).unwrap(),
        track,
    )
}
pub(super) fn owner(project: &hex_project::Project) -> hex_engine::AudioCandidateOwner {
    owner_with_bank(project).0
}
pub(super) fn owner_with_bank(
    project: &hex_project::Project,
) -> (hex_engine::AudioCandidateOwner, (usize, usize, u64, u64)) {
    let asset = project.assets().next().unwrap().id();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(
            asset,
            hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.1]; 8])
                .ok()
                .unwrap(),
        )],
        hex_engine::AudioStorageReservation {
            allocation_items: 64,
            handle_obligations: 256,
            max_prepared_data_bytes: 96 * 1024 * 1024,
            max_construction_data_bytes: 96 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let bank_identity = bank.test_bank_identity();
    let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            hex_engine::AudioArrangementInput::new(project, 11, bank)
                .with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    (
        hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap(),
        bank_identity,
    )
}
pub(super) fn abc_project() -> (hex_project::Project, [hex_project::TrackId; 3]) {
    let (base, x) = project();
    let asset = base.assets().next().unwrap().id();
    let mut edit = Editor::new(base, 16).unwrap();
    let mut ids = vec![x];
    for name in ["y", "z"] {
        let Created::Track(track) = edit
            .execute(Command::InsertTrackWithDevice {
                name: name.into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        edit.execute(Command::RemoveInstrument { track }).unwrap();
        edit.execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
        ids.push(track);
    }
    let mut snapshot = edit.project().snapshot();
    for (index, track) in snapshot.tracks.iter_mut().enumerate().skip(1) {
        let state = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(if index == 1 { 12 } else { 11 }),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
        let state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(13),
            hex_lattice_contract::ParameterValue::Float(500.0),
        )
        .unwrap();
        track.devices.push(hex_project::DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state,
        });
        snapshot.next_id += 1;
    }
    (
        hex_project::Project::from_snapshot(snapshot).unwrap(),
        [ids[0], ids[1], ids[2]],
    )
}
