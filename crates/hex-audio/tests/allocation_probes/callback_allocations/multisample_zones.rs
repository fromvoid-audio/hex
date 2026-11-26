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
            root_key: 48,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
        KeyZoneSpec {
            slot: s2,
            low_key: 52,
            high_key: 72,
            root_key: 60,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
        KeyZoneSpec {
            slot: s3,
            low_key: 76,
            high_key: 96,
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
