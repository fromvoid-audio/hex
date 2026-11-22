use super::*;
use hex_devices::filter::DEFINITION;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnTrackSnapshot, SendLevelSnapshot};

#[test]
fn filter_is_prepared_and_rendered_by_track_and_return_owners() {
    for on_return in [false, true] {
        let mut snapshot = project(0.8, 0, 8 << 32).snapshot();
        let id = snapshot.next_id;
        let bus = id + 1;
        snapshot.next_id = bus + 1;
        let mut device = DeviceSnapshot {
            id,
            state: DEFINITION.default_state().unwrap(),
            bypassed: true,
            automation: Vec::new(),
            parent: None,
            comparison: None,
            modulation: Default::default(),
        };
        device.state = DEFINITION
            .patch_parameter(
                &device.state,
                ParameterId::from_static(3),
                ParameterValue::Float(30.0),
            )
            .unwrap();
        if on_return {
            snapshot.tracks[0].sends.push(SendLevelSnapshot {
                return_track: bus,
                gain: 0.5,
            });
            snapshot.return_tracks.push(ReturnTrackSnapshot {
                id: bus,
                name: "filter return".into(),
                gain: 1.0,
                muted: false,
                devices: vec![],
            });
        }
        let build = |mut snapshot: hex_project::Snapshot, device: DeviceSnapshot| {
            if on_return {
                snapshot.return_tracks[0].devices.push(device);
            } else {
                snapshot.tracks[0].devices.push(device);
            }
            let project = Project::from_snapshot(snapshot).unwrap();
            let prepared = revision(&project, Arc::new(hex_devices::SineTable::new()));
            let mut playback = SessionPlayback::new(prepared).unwrap();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut result = [[0.0; 2]; 512];
            playback.render(&mut result).unwrap();
            result
        };
        let bypassed = build(snapshot.clone(), device.clone());
        device.bypassed = false;
        let active = build(snapshot, device);
        assert!(bypassed.iter().flatten().any(|sample| sample.abs() > 0.01));
        assert!(active.iter().flatten().all(|sample| sample.is_finite()));
        assert_ne!(active, bypassed);
    }
}
