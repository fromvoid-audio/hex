use super::*;
use crate::{
    AssetId, DeviceChainOwner, DeviceSelectionIds, Editor, TrackDestination, TrackSelectionIds,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};

#[test]
fn all_source_member_media_is_checked_before_pasting_a_frozen_unit() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut add_track = |name: &str| {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .unwrap()
        else {
            panic!("MIDI track")
        };
        track
    };
    let origin = add_track("original");
    let destination = add_track("destination");
    let mut add_asset = |path: &str| {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: path.into(),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        asset
    };
    let active = add_asset("child-active.wav");
    let inactive = add_asset("child-inactive.wav");
    let child_state = |asset: AssetId| {
        NativeDeviceState::new(DeviceTypeId::from_static("test.source-child"), 1, vec![])
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap()
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![
                crate::default_instrument_state().unwrap(),
                child_state(active),
            ],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("retained source")
    };
    let mut snapshot = editor.project().snapshot();
    let devices = &mut snapshot.native_sources[0].devices;
    devices[1].parent = Some(devices[0].id);
    devices[1].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::A,
        child_state(inactive),
    ));
    let mut project = Project::from_snapshot(snapshot).unwrap();
    // Exercise the real clipboard boundary before user-facing binding admission.
    let unit = SourceBinding(project.native_source(source).unwrap().clone());
    let copied = project.copy_source_binding(&unit, origin).unwrap();
    let instrument = copied.id();
    let track = project.track_mut(origin).unwrap();
    track.instrument = Some(copied);
    track.instrument_position = Some(0);

    let selection = TrackSelectionIds::new(&[origin]).unwrap();
    let full = project.capture_tracks(&selection, false).unwrap();
    let setup = project.capture_tracks(&selection, true).unwrap();
    let devices = project
        .capture_devices(
            DeviceChainOwner::Track(origin),
            &DeviceSelectionIds::new(&[instrument]).unwrap(),
        )
        .unwrap();

    for changed in [active, inactive] {
        let mut stale = project.clone();
        stale.assets.get_mut(&changed).unwrap().path = "changed.wav".into();
        let previous = stale.snapshot();
        for clipboard in [&full, &setup] {
            assert_eq!(
                crate::track_clipboard::paste(
                    &mut stale,
                    clipboard,
                    TrackDestination {
                        parent_group: None,
                        before: None
                    },
                )
                .unwrap_err(),
                Invalid("copied media no longer matches the project")
            );
        }
        assert_eq!(
            crate::device_clipboard::paste(
                &mut stale,
                &devices,
                DeviceChainOwner::Track(destination),
                None,
            )
            .unwrap_err(),
            Invalid("copied media no longer matches the project")
        );
        assert_eq!(stale.snapshot(), previous);
    }
}
