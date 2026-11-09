use super::tests::{Directory, event, fixture, owner};
use super::*;
use crate::service::Request;
use hex_lattice_contract::NativeDeviceState;
use std::fs;

#[test]
fn worker_default_roundtrip_and_replacement_preserve_project_history_and_playback() {
    for (class, name) in [
        (NativeDeviceClass::Effect, "hex.utility"),
        (NativeDeviceClass::Instrument, "hex.instrument.operator"),
    ] {
        let root = Directory::new();
        let (service, initial, device) = fixture(class, name);
        let state = initial
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        let inactive = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            hex_lattice_contract::ParameterId::from_static(1),
            hex_lattice_contract::ParameterValue::Float(0.75),
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::ReplaceDeviceComparison {
                device,
                state,
                comparison: Some(hex_project::DeviceComparison::new(
                    hex_project::DeviceVariant::B,
                    inactive,
                )),
            }))
            .unwrap();
        let initial = event(&service);
        assert!(initial.error.is_none(), "{:?}", initial.error);
        let target = owner(&initial, device, class);
        assert!(
            load_default(&root.0, class, target.type_id)
                .unwrap()
                .is_none()
        );
        for _ in 0..2 {
            let request = Request::SaveDeviceDefault {
                owner: target,
                root: root.0.clone(),
            };
            assert!(super::super::stopped_arrangement::served_during_lease(
                &request
            ));
            service.submit(request).unwrap();
            let saved = event(&service);
            assert!(saved.error.is_none(), "{:?}", saved.error);
            assert_eq!(saved.state.project, initial.state.project);
            assert_eq!(saved.state.revision, initial.state.revision);
            assert_eq!(saved.state.can_undo, initial.state.can_undo);
            assert_eq!(saved.state.can_redo, initial.state.can_redo);
            assert_eq!(saved.state.unsaved, initial.state.unsaved);
            assert!(saved.playback.is_none());
            let stored = load_default(&root.0, class, target.type_id)
                .unwrap()
                .unwrap();
            let expected = initial.state.project.device(device).unwrap();
            assert_eq!(stored.state, *expected.state());
            assert_eq!(stored.comparison.as_ref(), expected.comparison());
        }
    }
}

#[test]
fn stale_owners_and_unsupported_trims_refuse_without_replacing_the_default() {
    let root = Directory::new();
    let (service, initial, device) = fixture(NativeDeviceClass::Effect, "hex.utility");
    let target = owner(&initial, device, NativeDeviceClass::Effect);
    service
        .submit(Request::SaveDeviceDefault {
            owner: target,
            root: root.0.clone(),
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    let path =
        hex_storage::preset::defaults::path(&root.0, storage_class(target.class), target.type_id)
            .unwrap();
    let old = fs::read(&path).unwrap();
    for stale in [
        Owner {
            project_generation: 1,
            ..target
        },
        Owner {
            revision: target.revision + 1,
            ..target
        },
        Owner {
            class: NativeDeviceClass::Instrument,
            ..target
        },
        Owner {
            type_id: DeviceTypeId::from_static("hex.saturator"),
            ..target
        },
    ] {
        service
            .submit(Request::SaveDeviceDefault {
                owner: stale,
                root: root.0.clone(),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert!(refused.playback.is_none());
        assert_eq!(fs::read(&path).unwrap(), old);
    }
    let trimmed = initial
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_millidb(-900)
        .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: trimmed,
        }))
        .unwrap();
    let edited = event(&service);
    assert!(edited.error.is_none());
    service
        .submit(Request::SaveDeviceDefault {
            owner: owner(&edited, device, target.class),
            root: root.0.clone(),
        })
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.as_deref().unwrap().contains("host trims"));
    assert_eq!(refused.state.project, edited.state.project);
    assert_eq!(refused.state.revision, edited.state.revision);
    assert_eq!(fs::read(&path).unwrap(), old);
}

#[test]
fn malformed_and_future_native_defaults_cannot_be_loaded_or_overwritten() {
    let root = Directory::new();
    let (service, initial, device) = fixture(NativeDeviceClass::Effect, "hex.utility");
    let target = owner(&initial, device, NativeDeviceClass::Effect);
    let original = initial.state.project.device(device).unwrap().state();
    let path =
        hex_storage::preset::defaults::path(&root.0, DeviceClass::Effect, target.type_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for state in [
        NativeDeviceState::new(target.type_id, u32::MAX, original.payload().to_vec()).unwrap(),
        NativeDeviceState::new(target.type_id, original.state_version(), vec![0]).unwrap(),
    ] {
        let bytes = hex_storage::preset::encode(&Preset {
            class: DeviceClass::Effect,
            state,
            comparison: None,
        })
        .unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(load_default(&root.0, target.class, target.type_id).is_err());
        service
            .submit(Request::SaveDeviceDefault {
                owner: target,
                root: root.0.clone(),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let unknown =
        NativeDeviceState::new(DeviceTypeId::from_static("hex.unknown.default"), 1, vec![1])
            .unwrap();
    hex_storage::preset::defaults::save(
        &root.0,
        &Preset {
            class: DeviceClass::Effect,
            state: unknown.clone(),
            comparison: None,
        },
    )
    .unwrap();
    assert!(load_default(&root.0, NativeDeviceClass::Effect, unknown.type_id()).is_err());
    let too_long = std::path::PathBuf::from("x".repeat(4097));
    for root in [Path::new(""), too_long.as_path()] {
        assert!(load_default(root, target.class, target.type_id).is_err());
    }
}
