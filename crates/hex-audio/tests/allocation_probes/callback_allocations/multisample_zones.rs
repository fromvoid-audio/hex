//! Real host resource-lease admission, zero-allocation callback probe, and
//! control-side reclamation for bounded multisample key-zone selection (#3461).
use super::*;
use hex_devices::multisample::{
    KeyZoneSpec, MultisampleError, MultisampleLimits, PreparedMultisampleMap, ZoneOverlapPolicy,
};
use hex_engine::native_device_resources::{
    ResidentDeviceAssets, ResourcePreparationError, control_drop_probe,
};
use hex_engine::{AudioStorageLedgerReservation, OwnedAudioSource};
use hex_lattice::{BorrowedDeviceResources, NativeInstrumentEvent, TuningTable};
use hex_lattice_contract::DeviceAssetSlot;
use hex_project::{Command, Created, Editor, Project};
use std::sync::atomic::AtomicBool;

fn ledger() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 32,
        root_rows: 16,
        max_retained_bytes: 8_000_000,
        max_construction_peak_bytes: 8_000_000,
    }
}

#[test]
fn resident_multisample_zones_render_with_zero_callback_allocations_and_control_reclamation() {
    let control_thread = std::thread::current().id();
    let (_observer, dropped) = control_drop_probe::observe(control_thread);
    let cancel = AtomicBool::new(false);

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut import_asset = |name: &str, rate: u32, frames: u64| {
        let Created::Asset(id) = editor
            .execute(Command::ImportDeviceAsset {
                path: name.into(),
                sample_rate: rate,
                frames,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        id
    };
    let asset_low = import_asset("zone-low.wav", 44_100, 1024);
    let asset_mid = import_asset("zone-mid.wav", 48_000, 1024);
    let asset_high = import_asset("zone-high.wav", 96_000, 1024);

    let mut store = ResidentDeviceAssets::default();
    for (asset, rate, level) in [
        (asset_low, 44_100, 0.2_f32),
        (asset_mid, 48_000, 0.35_f32),
        (asset_high, 96_000, 0.5_f32),
    ] {
        store
            .insert_on_control(
                (
                    asset,
                    OwnedAudioSource::try_resident(rate, vec![[level, -level]; 1024])
                        .ok()
                        .unwrap(),
                ),
                0,
                ledger(),
                8_192,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
    }

    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let s3 = DeviceAssetSlot::new(3).unwrap();
    let views = [
        store.resource_view(s1, asset_low).unwrap(),
        store.resource_view(s2, asset_mid).unwrap(),
        store.resource_view(s3, asset_high).unwrap(),
    ];
    let resources = BorrowedDeviceResources(&views);
    let specs = [
        KeyZoneSpec {
            slot: s1,
            low_key: 36,
            high_key: 55,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 48,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
        KeyZoneSpec {
            slot: s2,
            low_key: 52,
            high_key: 72,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 60,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
        KeyZoneSpec {
            slot: s3,
            low_key: 76,
            high_key: 96,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 84,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
    ];
    let map = PreparedMultisampleMap::prepare_on_control(
        &specs,
        &resources,
        48_000,
        &TuningTable::default(),
        ZoneOverlapPolicy::LayerAll,
        MultisampleLimits::DEFAULT,
    )
    .unwrap();
    let mut rt = map.create_runtime();
    let mut out = [[0.0_f32; 2]; 128];

    // Measured callback: multi-zone triggers (low, overlap, unmapped gap, high) + NoteOff
    // must allocate and free zero bytes (both total and signed-net == 0).
    let info = measure(|| {
        rt.process_block(
            &map,
            &[
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 48, // low zone
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 16,
                    occurrence_id: 2,
                    pitch: 54, // overlap low + mid -> 2 layered voices
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 32,
                    occurrence_id: 3,
                    pitch: 74, // unmapped gap -> clean no-op
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 48,
                    occurrence_id: 4,
                    pitch: 84, // high zone
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 64,
                    occurrence_id: 1,
                },
            ],
            &mut out,
            &resources,
        )
        .unwrap();
    });
    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(out[0][0] > 0.15);
    assert!(out[48][0] > 0.9);
    assert_eq!(rt.active_voice_count(), 4);

    // Stale/swapped resource at slot 1 refuses cleanly with zero allocations and preserves active voices.
    let stale_views = [
        store.resource_view(s1, asset_high).unwrap(),
        store.resource_view(s2, asset_mid).unwrap(),
        store.resource_view(s3, asset_high).unwrap(),
    ];
    let stale = BorrowedDeviceResources(&stale_views);
    let info_stale = measure(|| {
        assert_eq!(
            rt.process_block(&map, &[], &mut out, &stale),
            Err(MultisampleError::SourceUnavailable)
        );
    });
    assert_eq!(info_stale.count_total, 0);
    assert_eq!(info_stale.count_current, 0);
    assert!(out.iter().flatten().all(|&s| s == 0.0));
    assert_eq!(rt.active_voice_count(), 4);

    // No resident bank was dropped during callback rendering.
    assert!(dropped.try_recv().is_err());

    // Dropping the control-side store reclaims all 3 resident banks strictly on the control thread.
    drop(store);
    for _ in 0..3 {
        assert_eq!(dropped.try_recv().unwrap(), control_thread);
    }
}

#[test]
fn registered_multisample_instrument_playback_zero_alloc_reclamation_and_storage_migration() {
    use hex_devices::multisample::{MULTISAMPLE_TYPE_ID, MultisampleState};
    use hex_engine::audio_transform::AudioTransformPreparation;
    use hex_engine::native_device_resources::PreparedDeviceResources;
    use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference};
    use hex_project::BeatTime;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let control_thread = std::thread::current().id();
    let (_observer, dropped) = control_drop_probe::observe(control_thread);
    let cancel = AtomicBool::new(false);

    // 1. Open and migrate an explicit supported prior-schema project (v63 base project)
    //    before adding hex.instrument.multisample and saving/reopening at current schema.
    let prior_project = Project::new(48_000).unwrap();
    assert_eq!(
        hex_storage::schema_version_for(&prior_project),
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    let prior_bytes = hex_storage::encode(&prior_project).unwrap();
    let prior_text = std::str::from_utf8(&prior_bytes).unwrap();
    assert!(
        prior_text.contains(&format!(
            "\"version\":{}",
            hex_storage::CURRENT_SCHEMA_VERSION
        )),
        "expected prior schema version in saved JSON"
    );
    let migrated_prior = hex_storage::decode(&prior_bytes).unwrap();

    let mut editor = Editor::new(migrated_prior, 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "multisample".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Asset(a1) = editor
        .execute(Command::ImportDeviceAsset {
            path: "ms-low.wav".into(),
            sample_rate: 44_100,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("a1");
    };
    let Created::Asset(a2) = editor
        .execute(Command::ImportDeviceAsset {
            path: "ms-high.wav".into(),
            sample_rate: 96_000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("a2");
    };
    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let state = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::LayerAll,
        voice_limit: 8,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 36,
                high_key: 64,
                low_velocity: 1,
                high_velocity: 100,
                root_key: 48,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.005,
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 56,
                high_key: 84,
                low_velocity: 60,
                high_velocity: 127,
                root_key: 72,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.005,
            },
        ],
    }
    .to_native_with_assets(vec![
        DeviceAssetReference {
            slot: s1,
            asset: DeviceAssetKey::new(a1.value()).unwrap(),
        },
        DeviceAssetReference {
            slot: s2,
            asset: DeviceAssetKey::new(a2.value()).unwrap(),
        },
    ])
    .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state.clone(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(1_u64 << 32),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1_u64 << 30),
            pitch: 60, // overlaps both zones in key AND velocity (80 in 1..=100 & 60..=127)
            channel: 0,
            velocity: 80,
        })
        .unwrap();

    // Also verify explicit historical v1 (HMS1) saved-project fixture migration -> v2 (HMS2):
    let mut v1_payload = Vec::with_capacity(12 + 20);
    v1_payload.extend_from_slice(b"HMS1");
    v1_payload.push(0); // FirstMatch
    v1_payload.push(16); // voice_limit
    v1_payload.extend_from_slice(&1_u16.to_le_bytes());
    v1_payload.extend_from_slice(&1.0_f32.to_le_bytes());
    v1_payload.extend_from_slice(&1_u32.to_le_bytes()); // slot 1
    v1_payload.extend_from_slice(&[36, 72, 60, 0]);
    v1_payload.extend_from_slice(&0.0_f32.to_le_bytes());
    v1_payload.extend_from_slice(&1.0_f32.to_le_bytes());
    v1_payload.extend_from_slice(&0.005_f32.to_le_bytes());
    let v1_state = hex_lattice_contract::NativeDeviceState::new(
        MULTISAMPLE_TYPE_ID,
        hex_devices::multisample::MULTISAMPLE_STATE_VERSION_V1,
        v1_payload,
    )
    .unwrap()
    .with_asset_references(vec![DeviceAssetReference {
        slot: s1,
        asset: DeviceAssetKey::new(a1.value()).unwrap(),
    }])
    .unwrap();
    let mut v1_editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(v1_track) = v1_editor
        .execute(Command::AddMidiTrack { name: "v1".into() })
        .unwrap()
    else {
        panic!("v1 track");
    };
    let Created::Asset(v1_a) = v1_editor
        .execute(Command::ImportDeviceAsset {
            path: "v1.wav".into(),
            sample_rate: 48_000,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("v1 asset");
    };
    let v1_state = v1_state
        .with_asset_references(vec![DeviceAssetReference {
            slot: s1,
            asset: DeviceAssetKey::new(v1_a.value()).unwrap(),
        }])
        .unwrap();
    v1_editor
        .execute(Command::InsertInstrument {
            track: v1_track,
            state: v1_state,
        })
        .unwrap();
    let v1_saved = hex_storage::encode(v1_editor.project()).unwrap();
    let v1_reopened = hex_storage::decode(&v1_saved).unwrap();
    let v1_loaded_state = v1_reopened
        .track(v1_track)
        .unwrap()
        .instrument()
        .unwrap()
        .state();
    let migrated_v2 = hex_lattice_contract::NativeDeviceMetadata::migrate_state(
        &hex_devices::multisample::MULTISAMPLE_DEFINITION,
        v1_loaded_state,
    )
    .unwrap();
    assert_eq!(
        migrated_v2.state_version(),
        hex_devices::multisample::MULTISAMPLE_STATE_VERSION
    );
    let migrated_authored = MultisampleState::from_native(&migrated_v2).unwrap();
    assert_eq!(migrated_authored.zones.len(), 1);
    assert_eq!(migrated_authored.zones[0].low_velocity, 1);
    assert_eq!(migrated_authored.zones[0].high_velocity, 127);

    // 2. Save and reopen via hex-storage; verify exact device type, payload, and asset references.
    let saved_bytes = hex_storage::encode(editor.project()).unwrap();
    let reopened = hex_storage::decode(&saved_bytes).unwrap();
    let reopened_state = reopened.track(track).unwrap().instrument().unwrap().state();
    assert_eq!(reopened_state.type_id(), MULTISAMPLE_TYPE_ID);
    assert_eq!(reopened_state, &state);

    // 3. Admit resident original assets and construct ordinary SessionPlayback on control.
    let mut store = ResidentDeviceAssets::default();
    for (asset, rate, val) in [(a1, 44_100, 0.25_f32), (a2, 96_000, 0.4_f32)] {
        store
            .insert_on_control(
                (
                    asset,
                    OwnedAudioSource::try_resident(rate, vec![[val, -val]; 1024])
                        .ok()
                        .unwrap(),
                ),
                0,
                ledger(),
                8_192,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
    }
    let resources =
        PreparedDeviceResources::prepare_on_control(&store, &reopened, 1_000_000, &cancel).unwrap();
    let scan_frames = resources.compiler_scan_work_on_control(&reopened).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            &reopened,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None,
            &AudioTransformPreparation::default(),
            &resources,
            scan_frames.max(1),
            &cancel,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
    let mut out = [[0.0_f32; 2]; 128];

    // 4. Measure callback Play + render + Stop: strictly zero total & net allocations/bytes.
    let info = measure(|| {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut out).unwrap();
        playback.apply(SessionPlaybackCommand::Stop).unwrap();
    });
    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    let expected_vel = (0.25 + 0.40) * (80.0 / 127.0);
    assert!(
        (out[0][0] - expected_vel).abs() < 1e-4,
        "expected layered velocity-scaled {expected_vel}, got {}",
        out[0][0]
    );

    // 5. Verify no resident banks dropped during callback, and all drop on control thread!
    assert!(dropped.try_recv().is_err());
    drop(playback);
    drop(resources);
    drop(store);
    for _ in 0..2 {
        assert_eq!(dropped.try_recv().unwrap(), control_thread);
    }
}
