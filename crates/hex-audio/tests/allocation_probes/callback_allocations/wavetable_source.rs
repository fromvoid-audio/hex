//! Real host resource-lease lifetime, zero-allocation callback probe, and
//! control-side reclamation test for the bounded wavetable source (#3481).
use super::*;
use hex_devices::wavetable::{
    PreparedWavetableSource, WavetableError, WavetableInterpolation, WavetableLimits,
    WavetableOscillatorSettings,
};
use hex_engine::native_device_resources::{
    ResidentDeviceAssets, ResourcePreparationError, control_drop_probe,
};
use hex_engine::{AudioStorageLedgerReservation, OwnedAudioSource};
use hex_lattice::BorrowedDeviceResources;
use hex_lattice_contract::DeviceAssetSlot;
use hex_project::{Command, Created, Editor, Project};
use std::f64::consts::TAU;
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

fn stereo_wavetable(frame_size: usize, harmonic_a: usize, harmonic_b: usize) -> Vec<[f32; 2]> {
    let mut pcm = Vec::with_capacity(frame_size * 2);
    for h in [harmonic_a, harmonic_b] {
        for i in 0..frame_size {
            let phase = TAU * (h as f64) * (i as f64) / (frame_size as f64);
            pcm.push([phase.sin() as f32, (phase + 0.5).sin() as f32]);
        }
    }
    pcm
}

#[test]
fn resident_wavetable_source_renders_with_zero_callback_allocations_and_control_reclamation() {
    let control_thread = std::thread::current().id();
    let (_observer, dropped) = control_drop_probe::observe(control_thread);
    let cancel = AtomicBool::new(false);
    let slot = DeviceAssetSlot::new(1).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset_a) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wavetable-a.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("asset a")
    };
    let Created::Asset(asset_b) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wavetable-b.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("asset b")
    };

    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset_a,
                OwnedAudioSource::try_resident(48_000, stereo_wavetable(256, 1, 3))
                    .ok()
                    .unwrap(),
            ),
            0,
            ledger(),
            4_096,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    store
        .insert_on_control(
            (
                asset_b,
                OwnedAudioSource::try_resident(48_000, stereo_wavetable(256, 2, 5))
                    .ok()
                    .unwrap(),
            ),
            0,
            ledger(),
            4_096,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();

    let view_a = store
        .resource_view(slot, asset_a)
        .expect("wavetable lease A");
    let prepared_a =
        PreparedWavetableSource::prepare_on_control(view_a, 256, WavetableLimits::DEFAULT).unwrap();
    let mut osc = prepared_a
        .oscillator(WavetableOscillatorSettings {
            output_sample_rate: 48_000,
            frequency_hz: 440.0,
            table_position: 0.25,
            initial_phase: 0.0,
            gain: 0.8,
            interpolation: WavetableInterpolation::CubicHermite,
        })
        .unwrap();

    let resources_a = BorrowedDeviceResources(std::slice::from_ref(&view_a));
    let mut out = [[0.0_f32; 2]; 128];

    // Measured callback scope: rendering + frequency & morph updates must perform
    // zero allocations and zero frees (both total and signed-net == 0).
    let info = measure(|| {
        osc.render_block(&resources_a, &mut out).unwrap();
        osc.set_table_position(0.75).unwrap();
        osc.set_frequency_hz(660.0).unwrap();
        osc.render_block(&resources_a, &mut out).unwrap();
    });
    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(out.iter().flatten().all(|s| s.is_finite()));
    assert!(out.iter().flatten().any(|s| s.abs() > 0.05));

    // Presenting a different host asset (different content identity) at the same slot
    // refuses cleanly with exact silence and preserves the oscillator's phase.
    let view_b = store
        .resource_view(slot, asset_b)
        .expect("wavetable lease B");
    let resources_b = BorrowedDeviceResources(std::slice::from_ref(&view_b));
    let phase_before = osc.phase();
    let info_refusal = measure(|| {
        assert_eq!(
            osc.render_block(&resources_b, &mut out),
            Err(WavetableError::SourceUnavailable)
        );
    });
    assert_eq!(info_refusal.count_total, 0);
    assert_eq!(info_refusal.count_current, 0);
    assert!(out.iter().flatten().all(|&s| s == 0.0));
    assert_eq!(osc.phase(), phase_before);

    // No resident bank was dropped during callback rendering.
    assert!(dropped.try_recv().is_err());

    // Dropping the control-side store reclaims both resident banks on the control thread.
    drop(store);
    assert_eq!(dropped.try_recv().unwrap(), control_thread);
    assert_eq!(dropped.try_recv().unwrap(), control_thread);
}

#[test]
fn registered_wavetable_instrument_playback_zero_alloc_reclamation_and_storage_migration() {
    use hex_devices::wavetable::{WAVETABLE_TYPE_ID, WavetableState};
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

    // 1. Open and migrate an explicit supported prior-schema project (v63) before adding
    //    hex.instrument.wavetable and saving/reopening at current schema.
    let prior_project = Project::new(48_000).unwrap();
    assert_eq!(
        hex_storage::schema_version_for(&prior_project),
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    let prior_bytes = hex_storage::encode(&prior_project).unwrap();
    let migrated_prior = hex_storage::decode(&prior_bytes).unwrap();

    let mut editor = Editor::new(migrated_prior, 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "wavetable".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wt-inst.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let slot = DeviceAssetSlot::new(1).unwrap();
    let state = WavetableState {
        slot: Some(slot),
        frame_size: 256,
        table_position: 0.5,
        gain: 0.9,
        attack_seconds: 0.0,
        release_seconds: 0.005,
        interpolation: WavetableInterpolation::CubicHermite,
        voice_limit: 8,
    }
    .to_native_with_asset(DeviceAssetReference {
        slot,
        asset: DeviceAssetKey::new(asset.value()).unwrap(),
    })
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
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    // 2. Save and reopen via hex-storage; verify exact type, payload, and asset reference.
    let saved_bytes = hex_storage::encode(editor.project()).unwrap();
    let reopened = hex_storage::decode(&saved_bytes).unwrap();
    let reopened_state = reopened.track(track).unwrap().instrument().unwrap().state();
    assert_eq!(reopened_state.type_id(), WAVETABLE_TYPE_ID);
    assert_eq!(reopened_state, &state);

    // 3. Admit resident wavetable asset and prepare ordinary SessionPlayback on control.
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(48_000, stereo_wavetable(256, 1, 3))
                    .ok()
                    .unwrap(),
            ),
            0,
            ledger(),
            4_096,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
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

    // 4. Measured callback Play + render + Stop + Seek + Play + render + Stop:
    //    strictly zero total & net allocs/bytes.
    let info = measure(|| {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut out).unwrap();
        playback.apply(SessionPlaybackCommand::Stop).unwrap();
        playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut out).unwrap();
        playback.apply(SessionPlaybackCommand::Stop).unwrap();
    });
    assert_eq!(info.count_total, 0, "callback allocated: {info:?}");
    assert_eq!(info.bytes_total, 0, "callback bytes: {info:?}");
    assert_eq!(info.count_current, 0, "callback net count: {info:?}");
    assert_eq!(info.bytes_current, 0, "callback net bytes: {info:?}");
    assert!(out.iter().flatten().all(|s| s.is_finite()));
    assert!(out.iter().flatten().any(|s| s.abs() > 0.1));

    // 5. No resident bank dropped during callback; dropping on control reclaims strictly on control.
    assert!(dropped.try_recv().is_err());
    drop(playback);
    drop(resources);
    drop(store);
    assert_eq!(dropped.try_recv().unwrap(), control_thread);
}
