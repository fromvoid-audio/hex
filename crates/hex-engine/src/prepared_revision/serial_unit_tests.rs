use super::*;
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created, Editor};

#[test]
fn changed_parent_is_not_the_same_mutable_effect_topology() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "mutable".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    for _ in 0..3 {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    let devices = &mut snapshot.tracks[0].devices;
    devices[1].parent = Some(devices[0].id);
    devices[1].bypassed = true;
    devices[2].parent = Some(devices[0].id);
    let before = Project::from_snapshot(snapshot.clone()).unwrap();
    snapshot.tracks[0].devices[2].parent = Some(snapshot.tracks[0].devices[1].id);
    let after = Project::from_snapshot(snapshot).unwrap();
    let table = Arc::new(SineTable::new());
    let old =
        PreparedRevision::prepare(&before, &BTreeMap::new(), Arc::clone(&table), None).unwrap();
    let new =
        PreparedRevision::prepare(&after, &BTreeMap::new(), table, Some(old.session())).unwrap();
    assert!(old.mutable_track_topology_matches(&old, track));
    assert!(!old.mutable_track_topology_matches(&new, track));
    assert!(!old.arrangement.tracks()[0].device_chain[2].bypassed);
    assert!(new.arrangement.tracks()[0].device_chain[2].bypassed);
}
