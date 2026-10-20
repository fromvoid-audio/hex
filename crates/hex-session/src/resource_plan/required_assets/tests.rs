use super::*;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceState,
};
use hex_project::{Command, Created, Editor};
use std::sync::atomic::AtomicBool;

fn state(ids: &[AssetId]) -> NativeDeviceState {
    NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.required.assets"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(
        ids.iter()
            .enumerate()
            .map(|(index, id)| DeviceAssetReference {
                slot: DeviceAssetSlot::new(index as u32 + 1).unwrap(),
                asset: DeviceAssetKey::new(id.value()).unwrap(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn candidate_asset_scope_includes_bypassed_muted_instrument_effect_and_return() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for name in ["instrument.wav", "effect.wav", "return.wav", "history.wav"] {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: name.into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        ids.push(id);
    }
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "muted".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: state(&[ids[0], ids[0]]),
        })
        .unwrap();
    let Created::Device(effect) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(&[ids[1], ids[0]]),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "muted return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(return_effect) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state(&[ids[2], ids[1]]),
        })
        .unwrap()
    else {
        panic!("return effect")
    };
    for device in [instrument, effect, return_effect] {
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
    }
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    editor
        .execute(Command::ReturnMute {
            return_track: bus,
            muted: true,
        })
        .unwrap();
    let project = editor.project();
    let before = project.snapshot();
    let cancel = AtomicBool::new(false);
    assert_eq!(reference_count(project).unwrap(), 6);
    let exact = scratch_bytes(6).unwrap();
    validate_candidate_assets(project, &ids[..3], exact, &cancel).unwrap();
    // An explicitly retained history-only input is allowed; it cannot replace
    // any actual candidate binding, even when that binding is inaudible.
    validate_candidate_assets(project, &ids, exact, &cancel).unwrap();
    for omitted in 0..3 {
        let supplied: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| *id != ids[omitted])
            .collect();
        assert!(matches!(
            validate_candidate_assets(project, &supplied, exact, &cancel),
            Err(ResourcePreparationError::InvalidSource)
        ));
    }
    assert!(matches!(
        validate_candidate_assets(project, &ids, exact - 1, &cancel),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    assert!(matches!(
        validate_candidate_assets(project, &[ids[1], ids[0]], exact, &cancel),
        Err(ResourcePreparationError::InvalidSource)
    ));
    assert!(matches!(
        validate_candidate_assets(project, &[ids[0], ids[0]], exact, &cancel),
        Err(ResourcePreparationError::InvalidSource)
    ));
    assert_eq!(project.snapshot(), before);
}

#[test]
fn candidate_asset_scope_empty_capacity_and_cancel_refuse_before_production() {
    let project = Project::new(48_000).unwrap();
    let empty = scratch_bytes(0).unwrap();
    validate_candidate_assets(&project, &[], empty, &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        validate_candidate_assets(&project, &[], empty - 1, &AtomicBool::new(false)),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    assert!(matches!(
        validate_candidate_assets(&project, &[], empty, &AtomicBool::new(true)),
        Err(ResourcePreparationError::Cancelled)
    ));
    assert!(scratch_bytes(usize::MAX).is_err());
}

#[test]
fn candidate_asset_scope_quotes_all_references_without_capping_them_as_unique_assets() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "shared.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "shared bindings".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let references = vec![asset; hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES];
    for _ in 0..33 {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(&references),
            })
            .unwrap();
    }
    let count = 33 * references.len();
    assert!(count > hex_project::MAX_ASSETS);
    assert_eq!(reference_count(editor.project()).unwrap(), count);
    let cancel = AtomicBool::new(false);
    validate_candidate_assets(
        editor.project(),
        &[asset],
        scratch_bytes(count).unwrap(),
        &cancel,
    )
    .unwrap();
    assert!(matches!(
        validate_candidate_assets(
            editor.project(),
            &[asset],
            scratch_bytes(1).unwrap(),
            &cancel
        ),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
}
