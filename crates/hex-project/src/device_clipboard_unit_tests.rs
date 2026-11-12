use super::*;
use crate::{DeviceComparison, DeviceVariant, Editor};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

fn fixture(kind: usize) -> (Editor, DeviceChainOwner, Vec<DeviceId>, AssetId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "local".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "nested.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let owner = match kind {
        0 => DeviceChainOwner::Main,
        1 => DeviceChainOwner::Return(bus),
        _ => DeviceChainOwner::Track(track),
    };
    let mut ids = Vec::new();
    for _ in 0..4 {
        let state = crate::default_instrument_state().unwrap();
        let command = match owner {
            DeviceChainOwner::Main => Command::InsertMainDevice {
                before: None,
                state,
            },
            DeviceChainOwner::Return(return_track) => Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            },
            DeviceChainOwner::Track(track) => Command::InsertDevice {
                track,
                before: None,
                state,
            },
        };
        let Created::Device(id) = editor.execute(command).unwrap() else {
            panic!("effect")
        };
        ids.push(id);
    }
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device: ids[1],
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            points: vec![crate::AutomationPoint {
                beat: crate::BeatTime::ZERO,
                value: crate::AutomationValue::new(0.25).unwrap(),
                outgoing: crate::AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let devices = match owner {
        DeviceChainOwner::Main => &mut snapshot.main_devices,
        DeviceChainOwner::Return(_) => &mut snapshot.return_tracks[0].devices,
        DeviceChainOwner::Track(_) => &mut snapshot.tracks[0].devices,
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
    devices[2].comparison = Some(DeviceComparison::new(DeviceVariant::A, inactive));
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        owner,
        ids,
        asset,
    )
}
fn devices(project: &Project, owner: DeviceChainOwner) -> &[Device] {
    match owner {
        DeviceChainOwner::Main => project.main_devices(),
        DeviceChainOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|t| t.id() == id)
            .unwrap()
            .devices(),
        DeviceChainOwner::Track(id) => project.track(id).unwrap().devices(),
    }
}
fn capture(editor: &Editor, owner: DeviceChainOwner, ids: &[DeviceId]) -> Arc<DeviceClipboard> {
    Arc::new(
        editor
            .project()
            .capture_devices(owner, &DeviceSelectionIds::new(ids).unwrap())
            .unwrap(),
    )
}

#[test]
fn serial_capture_normalizes_overlaps_and_paste_remaps_complete_authored_units() {
    for (kind, cross_owner) in [(0, true), (1, true), (2, true), (0, false)] {
        let (mut editor, owner, ids, _) = fixture(kind);
        let destination = if !cross_owner {
            owner
        } else {
            match kind {
                0 => DeviceChainOwner::Return(editor.project().return_tracks()[0].id()),
                1 => DeviceChainOwner::Track(editor.project().tracks()[0].id()),
                _ => DeviceChainOwner::Main,
            }
        };
        let before = editor.project().snapshot();
        let clipboard = capture(&editor, owner, &[ids[2], ids[0]]);
        editor
            .execute(Command::PasteDevices {
                clipboard,
                owner: destination,
                before: None,
            })
            .unwrap();
        let chain = devices(editor.project(), destination);
        let start = if cross_owner { 0 } else { 4 };
        assert_eq!(chain.len(), start + 3);
        for i in 0..3 {
            let copied = &chain[start + i];
            assert!(!ids.contains(&copied.id()));
            let mut expected = devices(editor.project(), owner)[i].clone();
            expected.id = copied.id();
            expected.parent = if i == 0 {
                None
            } else {
                Some(chain[start + i - 1].id())
            };
            assert_eq!(copied, &expected);
        }
        Project::from_snapshot(editor.project().snapshot()).unwrap();
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert_eq!(devices(editor.project(), owner).len(), 4);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert_eq!(editor.project().snapshot().next_id, after.next_id);
        assert_ne!(before.next_id, after.next_id);
    }
}

#[test]
fn selected_nested_subtree_copies_as_a_root_and_bulk_cut_removes_each_unit_once() {
    for kind in 0..3 {
        let (mut editor, owner, ids, asset) = fixture(kind);
        let clipboard = capture(&editor, owner, &[ids[1]]);
        editor
            .execute(Command::PasteDevices {
                clipboard,
                owner,
                before: Some(ids[3]),
            })
            .unwrap();
        let chain = devices(editor.project(), owner);
        assert_eq!(chain.len(), 6);
        assert_eq!(chain[3].parent(), None);
        assert_eq!(chain[4].parent(), Some(chain[3].id()));
        assert!(chain[4].bypassed());
        assert!(editor.undo());
        let before = editor.project().snapshot();
        editor
            .execute(Command::RemoveDevices {
                owner,
                selected: DeviceSelectionIds::new(&[ids[0], ids[1], ids[2]]).unwrap(),
            })
            .unwrap();
        assert_eq!(
            devices(editor.project(), owner)
                .iter()
                .map(Device::id)
                .collect::<Vec<_>>(),
            vec![ids[3]]
        );
        editor.execute(Command::RemoveAsset(asset)).unwrap();
        assert!(editor.undo());
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[test]
fn serial_clipboard_refusals_preserve_media_ids_and_redo() {
    let (mut editor, owner, ids, asset) = fixture(0);
    let clipboard = capture(&editor, owner, &[ids[0]]);
    assert!(
        editor
            .project()
            .capture_devices(
                owner,
                &DeviceSelectionIds::new(&[ids[0], DeviceId(u64::MAX)]).unwrap()
            )
            .is_err()
    );
    assert!(
        editor
            .project()
            .capture_devices(
                DeviceChainOwner::Track(editor.project().tracks()[0].id()),
                &DeviceSelectionIds::new(&[ids[0]]).unwrap()
            )
            .is_err()
    );
    let before = editor.project().snapshot();
    let rejected: Result<_, Invalid> = editor.execute_prepared(
        Command::PasteDevices {
            clipboard: Arc::clone(&clipboard),
            owner,
            before: None,
        },
        |candidate| {
            assert_eq!(candidate.main_devices().len(), 7);
            Err::<(), _>(Invalid("preparation refused"))
        },
    );
    assert!(rejected.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    for anchor in [ids[1], DeviceId(u64::MAX)] {
        assert!(
            editor
                .execute(Command::PasteDevices {
                    clipboard: Arc::clone(&clipboard),
                    owner,
                    before: Some(anchor)
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
    }
    let mut full = before.clone();
    while full.main_devices.len() < crate::MAX_DEVICES_PER_TRACK - 1 {
        let mut leaf = full.main_devices[3].clone();
        leaf.id = full.next_id;
        full.next_id += 1;
        full.main_devices.push(leaf);
    }
    let mut crowded = Editor::new(Project::from_snapshot(full.clone()).unwrap(), 16).unwrap();
    assert!(
        crowded
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner,
                before: None
            })
            .is_err()
    );
    assert_eq!(crowded.project().snapshot(), full);
    assert!(!crowded.can_undo());
    let mut changed = before.clone();
    changed.assets[0].path = "different.wav".into();
    let mut changed = Editor::new(Project::from_snapshot(changed).unwrap(), 16).unwrap();
    let unchanged = changed.project().snapshot();
    assert!(
        changed
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner,
                before: None
            })
            .is_err()
    );
    assert_eq!(changed.project().snapshot(), unchanged);
    editor.execute(Command::RemoveDevice(ids[0])).unwrap();
    editor.execute(Command::RemoveAsset(asset)).unwrap();
    let missing = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner,
                before: None
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), missing);
    assert!(editor.undo());
    assert!(editor.can_redo());
    assert!(
        editor
            .execute(Command::PasteDevices {
                clipboard: Arc::clone(&clipboard),
                owner,
                before: Some(DeviceId(u64::MAX))
            })
            .is_err()
    );
    assert!(editor.can_redo());
    let mut other_rate = Editor::new(Project::new(44_100).unwrap(), 16).unwrap();
    assert!(
        other_rate
            .execute(Command::PasteDevices {
                clipboard,
                owner: DeviceChainOwner::Main,
                before: None
            })
            .is_err()
    );
    assert!(!other_rate.can_undo());
}
