//! Read-only candidate accounting proof; no transition pool is installed.
use super::*;
use hex_project::{Command, Editor, Project};
use std::collections::BTreeMap;

#[path = "../../examples/harmonic_transition_layout.rs"]
mod layout;

#[test]
fn candidate_pool_charge_with_maximum_track_and_effect_inventory() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "layout".into(),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let template = snapshot.tracks.remove(0);
    let operator = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    )
    .unwrap();
    let operator = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &operator,
        hex_lattice_contract::ParameterId::from_static(82),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    let utility = crate::default_native_device_state(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let mut next = snapshot.next_id;
    for _ in 0..hex_project::MAX_TRACKS {
        let mut track = template.clone();
        track.id = next;
        next += 1;
        track.instrument.as_mut().unwrap().id = next;
        track.instrument.as_mut().unwrap().state = operator.clone();
        next += 1;
        for _ in 0..hex_project::MAX_DEVICES_PER_TRACK {
            track.devices.push(hex_project::DeviceSnapshot {
                id: next,
                state: utility.clone(),
                bypassed: false,
            });
            next += 1;
        }
        snapshot.tracks.push(track);
    }
    for _ in 0..hex_project::MAX_RETURN_TRACKS {
        let id = next;
        next += 1;
        let mut devices = Vec::new();
        for _ in 0..hex_project::MAX_DEVICES_PER_TRACK {
            devices.push(hex_project::DeviceSnapshot {
                id: next,
                state: utility.clone(),
                bypassed: false,
            });
            next += 1;
        }
        snapshot
            .return_tracks
            .push(hex_project::ReturnTrackSnapshot {
                id,
                name: "layout return".into(),
                gain: 1.0,
                muted: false,
                devices,
            });
    }
    snapshot.next_id = next;
    let project = Project::from_snapshot(snapshot).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let preflight = preroll::arrangement_runtime_storage_preflight_bytes(&revision).unwrap();
    let runtime = SessionPlayback::new(revision).unwrap();
    let actual = runtime.accounted_runtime_storage_bytes().unwrap();
    let pool = 2_639_456;
    eprintln!(
        "harmonic candidate inventory: tracks={} effects={} actual={} preflight={} pool={} combined_actual={} combined_preflight={} cap={}",
        hex_project::MAX_TRACKS,
        hex_project::MAX_TRACKS * hex_project::MAX_DEVICES_PER_TRACK,
        actual,
        preflight,
        pool,
        actual + pool,
        preflight + pool,
        MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
    );
    assert!(actual <= preflight);
    assert!(preflight + pool <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
    assert_eq!(
        std::mem::size_of::<crate::midi_note_bank::PreparedMidiNote>(),
        344
    );
}
