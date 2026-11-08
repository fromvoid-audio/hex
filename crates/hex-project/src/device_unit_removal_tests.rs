use crate::{Command, Created, DeviceId, Editor, Invalid, Project, TrackId};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

fn fixture(
    owner: usize,
    instrument_position: usize,
) -> (Editor, Vec<DeviceId>, TrackId, crate::AssetId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "child.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let mut ids = Vec::new();
    for _ in 0..4 {
        let state = crate::default_instrument_state().unwrap();
        let command = match owner {
            0 => Command::InsertMainDevice {
                before: None,
                state,
            },
            1 => Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            },
            _ => Command::InsertDevice {
                track,
                before: None,
                state,
            },
        };
        let Created::Device(id) = editor.execute(command).unwrap() else {
            panic!("device")
        };
        ids.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].instrument_position = Some(instrument_position);
    let devices = match owner {
        0 => &mut snapshot.main_devices,
        1 => &mut snapshot.return_tracks[0].devices,
        _ => &mut snapshot.tracks[0].devices,
    };
    devices[1].parent = Some(ids[0].value());
    devices[2].parent = Some(ids[1].value());
    devices[2].bypassed = true;
    let inactive = devices[2]
        .state
        .clone()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    devices[2].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::A,
        inactive,
    ));
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        ids,
        track,
        asset,
    )
}

#[test]
fn root_removal_is_one_prepared_history_edge_with_complete_descendant_media_custody() {
    for owner in 0..3 {
        let (mut editor, ids, track, asset) = fixture(owner, if owner == 2 { 3 } else { 0 });
        let before = editor.project().snapshot();
        assert!(editor.execute(Command::RemoveAsset(asset)).is_err());
        let mut prepared = false;
        let refused = editor.execute_prepared(Command::RemoveDevice(ids[0]), |candidate| {
            prepared = true;
            assert!(ids[..3].iter().all(|id| candidate.device(*id).is_none()));
            Err::<(), Invalid>(Invalid("publication refused"))
        });
        assert!(prepared, "complete unit must reach preparation");
        assert!(refused.is_err());
        assert_eq!(editor.project().snapshot(), before);
        editor.execute(Command::RemoveDevice(ids[0])).unwrap();
        assert!(
            ids[..3]
                .iter()
                .all(|id| editor.project().device(*id).is_none())
        );
        assert!(editor.project().device(ids[3]).is_some());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_some()
        );
        assert_eq!(
            editor.project().track(track).unwrap().instrument_position(),
            Some(0)
        );
        let removed = editor.project().snapshot();
        editor.execute(Command::RemoveAsset(asset)).unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), removed);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), removed);
    }
}

#[test]
fn nested_removal_preserves_parent_siblings_and_instrument_gap_and_stale_edits_refuse() {
    for owner in 0..3 {
        for position in if owner == 2 { vec![0, 3] } else { vec![0] } {
            let (mut editor, ids, track, _) = fixture(owner, position);
            let before = editor.project().snapshot();
            editor.execute(Command::RemoveDevice(ids[1])).unwrap();
            assert!(editor.project().device(ids[0]).is_some());
            assert!(editor.project().device(ids[3]).is_some());
            assert!(editor.project().device(ids[1]).is_none());
            assert!(editor.project().device(ids[2]).is_none());
            assert_eq!(
                editor.project().track(track).unwrap().instrument_position(),
                Some(if position == 3 { 1 } else { 0 })
            );
            let removed = editor.project().snapshot();
            assert!(editor.execute(Command::RemoveDevice(ids[1])).is_err());
            assert_eq!(editor.project().snapshot(), removed);
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), removed);
        }
    }
}
