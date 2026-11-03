use super::*;
use crate::service::{Event, Request, Service};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-preset-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn event(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("event: worker event absent: {error}"),
    }
}
fn owner(event: &Event, device: DeviceId, class: NativeDeviceClass) -> Owner {
    Owner {
        project_generation: 0,
        revision: event.state.revision,
        device,
        class,
        type_id: event
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    }
}
fn fixture(class: NativeDeviceClass, type_name: &'static str) -> (Service, Event, DeviceId) {
    let service = Service::new(48_000, 32, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "preset owner".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let added = event(&service);
    let track = added.state.project.tracks()[0].id();
    if class == NativeDeviceClass::Instrument {
        service
            .submit(Request::ReplaceNativeDevice {
                device: added
                    .state
                    .project
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id(),
                class,
                type_id: Box::new(DeviceTypeId::from_static(type_name)),
            })
            .unwrap();
    } else {
        service
            .submit(Request::AddNativeDevice {
                track,
                class,
                type_id: Box::new(DeviceTypeId::from_static(type_name)),
                before: None,
            })
            .unwrap();
    }
    let added = event(&service);
    assert!(added.error.is_none(), "{:?}", added.error);
    let device = if class == NativeDeviceClass::Instrument {
        added
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id()
    } else {
        added.state.project.track(track).unwrap().devices()[0].id()
    };
    // Real neighbors and a freely positioned instrument make whole-project
    // equality exercise chain ownership/order, not only an isolated device.
    for _ in 0..2 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(DeviceTypeId::from_static("hex.utility")),
                before: None,
            })
            .unwrap();
        let neighbor = event(&service);
        assert!(neighbor.error.is_none());
    }
    if class == NativeDeviceClass::Instrument {
        service
            .submit(Request::Edit(Command::MoveTrackDevice {
                device,
                track,
                before: None,
            }))
            .unwrap();
        let moved = event(&service);
        assert!(moved.error.is_none());
        let second = moved.state.project.track(track).unwrap().devices()[1].id();
        service
            .submit(Request::Edit(Command::MoveTrackDevice {
                device,
                track,
                before: Some(second),
            }))
            .unwrap();
    } else {
        service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .unwrap();
    }
    let positioned = event(&service);
    assert!(positioned.error.is_none());
    (service, positioned, device)
}

#[test]
fn device_preset_complete_state_one_undo_redo_preserves_identity_and_neighbors() {
    for (class, type_name) in [
        (NativeDeviceClass::Instrument, "hex.instrument.operator"),
        (NativeDeviceClass::Effect, "hex.utility"),
    ] {
        let dir = Directory::new();
        let path = dir.0.join("Exact.hex-device-preset.json");
        let (service, added, device) = fixture(class, type_name);
        let trimmed = added
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone()
            .with_output_gain_millidb(-12_345)
            .unwrap()
            .with_input_gain_millidb(-6_789)
            .unwrap();
        service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device,
                state: trimmed,
            }))
            .unwrap();
        let added = event(&service);
        assert!(added.error.is_none());
        let original = added.state.project.clone();
        let mut state = original.device(device).unwrap().state().clone();
        let mut edits = vec![(1, ParameterValue::Float(0.37))];
        if class == NativeDeviceClass::Instrument {
            assert_eq!(
                state.state_version(),
                15,
                "current canonical harmonic, filter and enable-state history is required"
            );
            assert_eq!(
                state.payload().len(),
                1_728,
                "complete current payload retained"
            );
            // Non-default values span level, tuning, envelope, signed routing
            // and supported LFO/harmonic/global/controller fields, including a hidden partial.
            // Compare complete opaque state below.
            edits.extend([
                // Start parallel before authoring a custom A-to-B edge; the
                // default serial B-to-A edge would otherwise form a cycle.
                (600, ParameterValue::Choice(11)),
                (601, ParameterValue::Choice(5)),
                (605, ParameterValue::Float(-37.5)),
                (628, ParameterValue::Choice(5)),
                (629, ParameterValue::Float(23.6)),
                (357, ParameterValue::Bool(false)),
                (358, ParameterValue::Float(0.25)),
                (359, ParameterValue::Bool(true)),
                (360, ParameterValue::Float(0.5)),
                (361, ParameterValue::Float(-0.3)),
                (362, ParameterValue::Choice(1)),
                (363, ParameterValue::Float(0.25)),
                (364, ParameterValue::Choice(4)),
                (513, ParameterValue::Bool(false)),
                (514, ParameterValue::Bool(true)),
                (515, ParameterValue::Bool(false)),
                (516, ParameterValue::Bool(true)),
                (2, ParameterValue::Float(0.27)),
                (14, ParameterValue::Float(-0.21)),
                (29, ParameterValue::Float(0.4)),
                (36, ParameterValue::Float(0.025)),
                (63, ParameterValue::Float(440.0)),
                (82, ParameterValue::Bool(true)),
                (85, ParameterValue::Float(1.25)),
                (57, ParameterValue::Choice(4)),
                (108, ParameterValue::Float(0.61)),
                (156, ParameterValue::Float(0.23)),
                (349, ParameterValue::Choice(0)),
                (353, ParameterValue::Choice(2)),
            ]);
        }
        for (id, value) in edits {
            state = hex_engine::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
        service
            .submit(Request::Edit(Command::ReplaceDeviceState { device, state }))
            .unwrap();
        let authored = event(&service);
        assert!(authored.error.is_none(), "{:?}", authored.error);
        service
            .submit(Request::SaveDevicePreset {
                owner: owner(&authored, device, class),
                path: path.clone(),
            })
            .unwrap();
        let saved = event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        assert_eq!(saved.state.project, authored.state.project);
        assert_eq!(saved.state.revision, authored.state.revision);
        service.submit(Request::Undo).unwrap();
        let reset = event(&service);
        assert_eq!(reset.state.project, original);
        service
            .submit(Request::LoadDevicePreset {
                owner: owner(&reset, device, class),
                path: path.clone(),
            })
            .unwrap();
        let loaded = event(&service);
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        assert_eq!(
            loaded
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .input_gain_millidb(),
            -6_789,
            "loading must retain the target's host input trim",
        );
        assert_eq!(loaded.state.project, authored.state.project);
        assert_eq!(loaded.state.revision, reset.state.revision + 1);
        service.submit(Request::Undo).unwrap();
        let undone = event(&service);
        assert_eq!(undone.state.project, original);
        service.submit(Request::Redo).unwrap();
        let redone = event(&service);
        assert_eq!(redone.state.project, authored.state.project);
        assert_eq!(
            hex_storage::preset::load(&path).unwrap().state,
            authored
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .clone()
                .without_host_gains()
        );
        assert_eq!(
            redone
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .output_gain_millidb(),
            -12_345
        );
        assert_eq!(redone.state.project.device(device).unwrap().id(), device);
    }
}

#[test]
fn device_preset_refusals_preserve_revision_history_and_audio() {
    let dir = Directory::new();
    let path = dir.0.join("input.json");
    let (service, added, device) = fixture(NativeDeviceClass::Effect, "hex.utility");
    let target = owner(&added, device, NativeDeviceClass::Effect);
    let native = added.state.project.device(device).unwrap().state().clone();
    let valid = Preset {
        comparison: None,
        class: DeviceClass::Effect,
        state: native.clone(),
    };
    let inputs = [
        b"broken".to_vec(),
        vec![b' '; hex_storage::preset::MAX_PRESET_BYTES + 1],
        hex_storage::preset::encode(&Preset {
            comparison: None,
            class: DeviceClass::Instrument,
            state: native.clone(),
        })
        .unwrap(),
        hex_storage::preset::encode(&Preset {
            comparison: None,
            class: DeviceClass::Effect,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.saturator"),
            )
            .unwrap(),
        })
        .unwrap(),
        hex_storage::preset::encode(&Preset {
            comparison: None,
            class: DeviceClass::Effect,
            state: NativeDeviceState::new(native.type_id(), u32::MAX, native.payload().to_vec())
                .unwrap(),
        })
        .unwrap(),
        hex_storage::preset::encode(&Preset {
            comparison: None,
            class: DeviceClass::Effect,
            state: NativeDeviceState::new(native.type_id(), native.state_version(), vec![0])
                .unwrap(),
        })
        .unwrap(),
    ];
    for bytes in inputs {
        fs::write(&path, bytes).unwrap();
        service
            .submit(Request::LoadDevicePreset {
                owner: target,
                path: path.clone(),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, added.state.project);
        assert_eq!(refused.state.revision, added.state.revision);
        assert_eq!(refused.state.can_undo, added.state.can_undo);
        assert_eq!(refused.state.can_redo, added.state.can_redo);
        assert!(refused.playback.is_none());
    }
    fs::remove_file(&path).unwrap();
    service
        .submit(Request::LoadDevicePreset {
            owner: target,
            path: path.clone(),
        })
        .unwrap();
    assert!(event(&service).error.is_some());
    hex_storage::preset::save_new(&path, &valid).unwrap();
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
        let output = dir.0.join("stale.json");
        service
            .submit(Request::SaveDevicePreset {
                owner: stale,
                path: output.clone(),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert!(!output.exists());
        service
            .submit(Request::LoadDevicePreset {
                owner: stale,
                path: path.clone(),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, added.state.project);
        assert!(refused.playback.is_none());
    }
    let bytes = fs::read(&path).unwrap();
    service
        .submit(Request::SaveDevicePreset {
            owner: target,
            path: path.clone(),
        })
        .unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("choose a new filename")
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(refused.state.project, added.state.project);
}

#[test]
fn device_preset_with_an_above_nyquist_fixed_tone_loads_and_compiles() {
    let dir = Directory::new();
    let path = dir.0.join("nyquist.hex-device-preset.json");
    let (service, added, device) =
        fixture(NativeDeviceClass::Instrument, "hex.instrument.operator");
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let placed = event(&service);
    assert!(placed.error.is_none(), "{:?}", placed.error);
    let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let notes = event(&service);
    assert!(notes.error.is_none());
    service
        .submit(Request::Edit(Command::AddScene {
            name: "redo guard".into(),
        }))
        .unwrap();
    event(&service);
    service.submit(Request::Undo).unwrap();
    let before = event(&service);
    assert!(before.state.can_redo);
    let artifact = before
        .playback
        .as_ref()
        .unwrap()
        .prepared_revision()
        .unwrap()
        .clone();
    let mut state = before.state.project.device(device).unwrap().state().clone();
    for (id, value) in [
        (63, ParameterValue::Float(12_000.0)),
        (64, ParameterValue::Float(2.0)),
        (62, ParameterValue::Bool(true)),
    ] {
        state = hex_engine::patch_native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(id),
            value,
        )
        .unwrap();
    }
    hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, &state)
        .expect("valid native metadata; admission is stricter");
    hex_storage::preset::save_new(
        &path,
        &Preset {
            comparison: None,
            class: DeviceClass::Instrument,
            state,
        },
    )
    .unwrap();
    service
        .submit(Request::LoadDevicePreset {
            owner: owner(&before, device, NativeDeviceClass::Instrument),
            path,
        })
        .unwrap();
    // A fixed tone at nyquist silences only that operator (Pulse #3957).
    let loaded = event(&service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert_ne!(loaded.state.project, before.state.project);
    let compiled = loaded.playback.expect("above-nyquist preset compiles");
    assert!(!std::sync::Arc::ptr_eq(
        &artifact,
        compiled.prepared_revision().unwrap()
    ));
}

#[test]
fn device_preset_valid_native_state_can_fail_live_admission_without_mutation() {
    let dir = Directory::new();
    let path = dir.0.join("long-release.hex-device-preset.json");
    let (service, added, device) =
        fixture(NativeDeviceClass::Instrument, "hex.instrument.operator");
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(12).unwrap(),
        }))
        .unwrap();
    let placed = event(&service);
    assert!(placed.error.is_none(), "{:?}", placed.error);
    let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
    // 65 short notes an eighth of a beat apart fit the 64-voice track
    // capacity with the default release tails.
    for index in 0..65_u8 {
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::from_ratio(u64::from(index), 8).unwrap(),
                duration: hex_project::BeatTime::from_ratio(1, 32).unwrap(),
                pitch: 36 + index % 48,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        let added = event(&service);
        assert!(added.error.is_none(), "{:?}", added.error);
    }
    service
        .submit(Request::Edit(Command::AddScene {
            name: "redo guard".into(),
        }))
        .unwrap();
    event(&service);
    service.submit(Request::Undo).unwrap();
    let before = event(&service);
    assert!(before.state.can_redo);
    let artifact = before
        .playback
        .as_ref()
        .unwrap()
        .prepared_revision()
        .unwrap()
        .clone();
    // Maximum release tails overlap all 65 notes: valid state, refused density.
    let mut state = before.state.project.device(device).unwrap().state().clone();
    for release in [38, 44, 50, 56] {
        // Operator 1-4 release, at its 60 s maximum.
        state = hex_engine::patch_native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(release),
            ParameterValue::Float(60.0),
        )
        .unwrap();
    }
    hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, &state)
        .expect("valid native metadata; admission is stricter");
    hex_storage::preset::save_new(
        &path,
        &Preset {
            comparison: None,
            class: DeviceClass::Instrument,
            state,
        },
    )
    .unwrap();
    service
        .submit(Request::LoadDevicePreset {
            owner: owner(&before, device, NativeDeviceClass::Instrument),
            path,
        })
        .unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("voice density")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.project, before.state.project);
    assert_eq!(refused.state.revision, before.state.revision);
    assert!(refused.state.can_redo);
    assert!(refused.playback.is_none());
    service
        .submit(Request::RestorePlayback { generation: 7 })
        .unwrap();
    let restored = event(&service);
    assert!(std::sync::Arc::ptr_eq(
        &artifact,
        restored
            .playback
            .as_ref()
            .unwrap()
            .prepared_revision()
            .unwrap()
    ));
}

#[test]
fn comparison_preset_roundtrip_restores_both_variants_but_not_global_trims_or_bypass() {
    use hex_project::{DeviceComparisonAction as Action, DeviceVariant as Slot};
    for (class, type_name) in [
        (NativeDeviceClass::Instrument, "hex.instrument.operator"),
        (NativeDeviceClass::Effect, "hex.utility"),
    ] {
        let directory = Directory::new();
        let path = directory.0.join("Comparison.hex-device-preset.json");
        let (service, initial, device) = fixture(class, type_name);
        let a = initial
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        service
            .submit(Request::CompareDevice {
                owner: owner(&initial, device, class),
                expected: Slot::A,
                action: Action::Switch,
            })
            .unwrap();
        let switched = event(&service);
        assert!(switched.error.is_none());
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::from_static(1),
                value: ParameterValue::Float(0.37),
            })
            .unwrap();
        let authored = event(&service);
        assert!(authored.error.is_none());
        let b = authored
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        assert_ne!(a.payload(), b.payload());
        service
            .submit(Request::SaveDevicePreset {
                owner: owner(&authored, device, class),
                path: path.clone(),
            })
            .unwrap();
        let saved = event(&service);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        let original_file = fs::read(&path).unwrap();
        let preset = hex_storage::preset::decode(&original_file).unwrap();
        assert_eq!(preset.state.payload(), b.payload());
        assert_eq!(preset.comparison.as_ref().unwrap().active(), Slot::B);
        assert_eq!(
            preset.comparison.as_ref().unwrap().inactive().payload(),
            a.payload()
        );
        let trimmed = b
            .clone()
            .with_input_gain_millidb(-1_234)
            .unwrap()
            .with_output_gain_millidb(4_321)
            .unwrap();
        service
            .submit(Request::Edit(Command::ReplaceDeviceState {
                device,
                state: trimmed,
            }))
            .unwrap();
        let trimmed = event(&service);
        assert!(trimmed.error.is_none());
        service
            .submit(Request::CompareDevice {
                owner: owner(&trimmed, device, class),
                expected: Slot::B,
                action: Action::Switch,
            })
            .unwrap();
        assert!(event(&service).error.is_none());
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: ParameterId::from_static(1),
                value: ParameterValue::Float(0.62),
            })
            .unwrap();
        let edited = event(&service);
        assert!(edited.error.is_none());
        let bypassed = !edited.state.project.device(device).unwrap().bypassed();
        service
            .submit(Request::Edit(Command::SetDeviceBypass { device, bypassed }))
            .unwrap();
        let before = event(&service);
        assert!(before.error.is_none());
        let expected_state = b
            .with_input_gain_millidb(-1_234)
            .unwrap()
            .with_output_gain_millidb(4_321)
            .unwrap();
        let expected_pair = preset
            .comparison
            .unwrap()
            .with_global_trims_from(&expected_state)
            .unwrap();
        let mut expected = hex_project::Editor::new((*before.state.project).clone(), 1).unwrap();
        expected
            .execute(Command::ReplaceDeviceComparison {
                device,
                state: expected_state,
                comparison: Some(expected_pair),
            })
            .unwrap();
        service
            .submit(Request::LoadDevicePreset {
                owner: owner(&before, device, class),
                path: path.clone(),
            })
            .unwrap();
        let loaded = event(&service);
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        assert_eq!(loaded.state.project.as_ref(), expected.project());
        assert_eq!(loaded.state.revision, before.state.revision + 1);
        assert_eq!(
            loaded.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        assert_eq!(fs::read(&path).unwrap(), original_file);
        service.submit(Request::Undo).unwrap();
        let undone = event(&service);
        assert_eq!(undone.state.project, before.state.project);
        service.submit(Request::Redo).unwrap();
        let redone = event(&service);
        assert_eq!(redone.state.project, loaded.state.project);
        let mut malformed: serde_json::Value = serde_json::from_slice(&original_file).unwrap();
        malformed["comparison"]["state_version"] = u32::MAX.into();
        let invalid_path = directory.0.join("Invalid.hex-device-preset.json");
        fs::write(&invalid_path, serde_json::to_vec(&malformed).unwrap()).unwrap();
        service
            .submit(Request::LoadDevicePreset {
                owner: owner(&redone, device, class),
                path: invalid_path,
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, redone.state.project);
        assert_eq!(refused.state.revision, redone.state.revision);
        assert!(refused.playback.is_none());
        service.submit(Request::Undo).unwrap();
        assert_eq!(
            event(&service).state.project,
            before.state.project,
            "invalid inactive payload must not add history"
        );
    }
}
