use super::*;
use crate::audio_transform::AudioTransformPreparation;
use crate::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use crate::{AudioStorageLedgerReservation, OwnedAudioSource};
use hex_devices::wavetable::{
    TABLE_POSITION, WAVETABLE_TYPE_ID, WavetableInterpolation, WavetableState,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, ParameterValue};
use hex_project::{AssetId, BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::sync::atomic::AtomicBool;

const RATE: u32 = 48_000;

fn beat(quarter_notes: u64) -> BeatTime {
    BeatTime::from_raw(quarter_notes << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

fn ledger() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 32,
        root_rows: 16,
        max_retained_bytes: 8_000_000,
        max_construction_peak_bytes: 8_000_000,
    }
}

fn stereo_two_table(frame_size: usize, h0: usize, h1: usize) -> Vec<[f32; 2]> {
    let mut frames = Vec::with_capacity(frame_size * 2);
    for h in [h0, h1] {
        for i in 0..frame_size {
            let phase = (i as f64) / (frame_size as f64);
            let s = (TAU * (h as f64) * phase).sin() as f32;
            frames.push([s, -s]);
        }
    }
    frames
}

fn dft_magnitude(signal: &[f32], freq_hz: f64, sample_rate: u32) -> f64 {
    let n = signal.len() as f64;
    let mut re = 0.0_f64;
    let mut im = 0.0_f64;
    for (i, &s) in signal.iter().enumerate() {
        let angle = TAU * freq_hz * (i as f64) / f64::from(sample_rate);
        re += f64::from(s) * angle.cos();
        im -= f64::from(s) * angle.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n
}

fn insert_resident(store: &mut ResidentDeviceAssets, asset: AssetId, frames: Vec<[f32; 2]>) {
    let cancel = AtomicBool::new(false);
    let len = frames.len() as u64;
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(RATE, frames).ok().unwrap(),
            ),
            0,
            ledger(),
            len.max(4) * 4,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
}

fn prepare_playback(
    project: &Project,
    store: &ResidentDeviceAssets,
) -> Result<(PreparedDeviceResources, SessionPlayback), crate::Invalid> {
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(store, project, 1_000_000, &cancel)
        .map_err(|_| crate::Invalid("device resources preparation failed"))?;
    let scan_frames = resources
        .compiler_scan_work_on_control(project)
        .map_err(|_| crate::Invalid("scan work quote failed"))?;
    let table = Arc::new(hex_devices::SineTable::new());
    let revision = Arc::new(PreparedRevision::prepare_with_device_resources(
        project,
        &BTreeMap::new(),
        table,
        None,
        &AudioTransformPreparation::default(),
        &resources,
        scan_frames.max(1),
        &cancel,
    )?);
    let playback = SessionPlayback::new_with_device_resources(revision, &resources)
        .map_err(|_| crate::Invalid("session playback construction failed"))?;
    Ok((resources, playback))
}

#[test]
fn wavetable_arrangement_session_loop_seek_stop_replacement_refusal_and_history() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack { name: "wt".into() })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "s1".into() })
        .unwrap()
    else {
        panic!("scene");
    };
    let Created::Asset(asset_a) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wt-a.wav".into(),
            sample_rate: RATE,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("asset_a");
    };
    let Created::Asset(asset_b) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wt-b.wav".into(),
            sample_rate: RATE,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("asset_b");
    };
    let Created::Asset(asset_bad) = editor
        .execute(Command::ImportDeviceAsset {
            path: "wt-unaligned.wav".into(),
            sample_rate: RATE,
            frames: 300, // not a multiple of 256!
        })
        .unwrap()
    else {
        panic!("asset_bad");
    };

    let s1 = DeviceAssetSlot::new(1).unwrap();
    let ref_a = DeviceAssetReference {
        slot: s1,
        asset: DeviceAssetKey::new(asset_a.value()).unwrap(),
    };
    let state_pos0 = WavetableState {
        slot: Some(s1),
        frame_size: 256,
        table_position: 0.0,
        gain: 1.0,
        attack_seconds: 0.0,
        release_seconds: 0.005,
        interpolation: WavetableInterpolation::CubicHermite,
        voice_limit: 8,
    }
    .to_native_with_asset(ref_a)
    .unwrap();
    let Created::Device(device_id) = editor
        .execute(Command::InsertInstrument {
            track,
            state: state_pos0.clone(),
        })
        .unwrap()
    else {
        panic!("device");
    };

    // Arrangement clip: overlapping A4 (69, 440 Hz) + E5 (76, ~659.25 Hz) with release.
    let Created::Clip(arr_clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("arr clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arr_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip: arr_clip,
            start: beat_frac(1, 8),
            duration: beat_frac(1, 4),
            pitch: 76,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    // Session clip: single A4 (69, 440 Hz).
    let Created::SessionClip(sess_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "wt-session".into(),
            source_length: beat(1),
        })
        .unwrap()
    else {
        panic!("sess clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: sess_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 2),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    // Set a 1-beat arrangement loop (24_000 frames at 120 BPM, 48 kHz) to verify loop wrap + seek.
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 24_000,
            enabled: true,
        })
        .unwrap();

    let mut store = ResidentDeviceAssets::default();
    // Table A: table 0 = 1st harmonic (440 Hz), table 1 = 3rd harmonic (1320 Hz).
    insert_resident(&mut store, asset_a, stereo_two_table(256, 1, 3));
    // Table B: table 0 = 2nd harmonic (880 Hz), table 1 = 4th harmonic (1760 Hz).
    insert_resident(&mut store, asset_b, stereo_two_table(256, 2, 4));
    insert_resident(&mut store, asset_bad, vec![[0.1, 0.1]; 300]);

    // 1. Arrangement playback at table_position = 0.0 -> strong 440 Hz fundamental, ~0 at 1320 Hz.
    let (_res0, mut pb0) = prepare_playback(editor.project(), &store).unwrap();
    pb0.apply(SessionPlaybackCommand::Play).unwrap();
    let mut arr0 = vec![[0.0_f32; 2]; 2_048];
    pb0.render(&mut arr0).unwrap();
    let left0: Vec<f32> = arr0.iter().map(|f| f[0]).collect();
    assert!(dft_magnitude(&left0, 440.0, RATE) > 0.8);
    assert!(dft_magnitude(&left0, 1_320.0, RATE) < 0.05);

    // Seeking while a mutable instrument voice is active is refused cleanly (MutableInstrumentActive),
    // and Stop -> Seek(0) -> Play reproduces bit-identical audio with zero stuck notes!
    assert_eq!(
        pb0.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    pb0.apply(SessionPlaybackCommand::Stop).unwrap();
    pb0.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    pb0.apply(SessionPlaybackCommand::Play).unwrap();
    let mut arr0_seek = vec![[0.0_f32; 2]; 2_048];
    pb0.render(&mut arr0_seek).unwrap();
    assert_eq!(arr0, arr0_seek);

    // Loop wrap (1 beat = 24_000 frames at 120 BPM): Stop -> Seek(23_900) -> Play across wrap.
    pb0.apply(SessionPlaybackCommand::Stop).unwrap();
    pb0.apply(SessionPlaybackCommand::Seek(23_900)).unwrap();
    pb0.apply(SessionPlaybackCommand::Play).unwrap();
    let mut wrap_buf = vec![[0.0_f32; 2]; 512];
    pb0.render(&mut wrap_buf).unwrap();
    // First 100 frames are silent tail after notes ended; frames 100..512 restart note at beat 0!
    assert!(wrap_buf[..100].iter().flatten().all(|&s| s == 0.0));
    assert!(wrap_buf[100..].iter().flatten().any(|s| s.abs() > 0.5));

    // Stop clears immediately without stuck notes.
    pb0.apply(SessionPlaybackCommand::Stop).unwrap();
    let mut stopped = vec![[0.0_f32; 2]; 64];
    pb0.render(&mut stopped).unwrap();
    assert!(stopped.iter().flatten().all(|&s| s == 0.0));

    // 2. Session playback launches cleanly on the same wavetable track!
    pb0.apply(SessionPlaybackCommand::LaunchClip {
        clip: sess_clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    pb0.apply(SessionPlaybackCommand::Play).unwrap();
    let mut sess_buf = vec![[0.0_f32; 2]; 2_048];
    pb0.render(&mut sess_buf).unwrap();
    let sess_l: Vec<f32> = sess_buf.iter().map(|f| f[0]).collect();
    assert!(dft_magnitude(&sess_l, 440.0, RATE) > 0.8);

    // 3. Patch TABLE_POSITION -> 1.0 shifts spectrum cleanly from 440 Hz to 1320 Hz!
    let state_pos1 = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &state_pos0,
        TABLE_POSITION,
        ParameterValue::Float(1.0),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: state_pos1.clone(),
        })
        .unwrap();
    let (_res1, mut pb1) = prepare_playback(editor.project(), &store).unwrap();
    pb1.apply(SessionPlaybackCommand::Play).unwrap();
    let mut arr1 = vec![[0.0_f32; 2]; 2_048];
    pb1.render(&mut arr1).unwrap();
    let left1: Vec<f32> = arr1.iter().map(|f| f[0]).collect();
    assert!(dft_magnitude(&left1, 1_320.0, RATE) > 0.8);
    assert!(dft_magnitude(&left1, 440.0, RATE) < 0.05);

    // 4. Invalid (unaligned 300-frame) source replacement refuses prepare while preserving pb0/pb1!
    let bad_state = WavetableState {
        slot: Some(s1),
        frame_size: 256,
        table_position: 0.0,
        gain: 1.0,
        attack_seconds: 0.0,
        release_seconds: 0.005,
        interpolation: WavetableInterpolation::CubicHermite,
        voice_limit: 8,
    }
    .to_native_with_asset(DeviceAssetReference {
        slot: s1,
        asset: DeviceAssetKey::new(asset_bad.value()).unwrap(),
    })
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: bad_state,
        })
        .unwrap();
    assert!(prepare_playback(editor.project(), &store).is_err());
    // Previously admitted pb1 still renders cleanly after failed preparation!
    let mut still_ok = vec![[0.0_f32; 2]; 64];
    pb1.render(&mut still_ok).unwrap();
    assert!(still_ok.iter().flatten().any(|s| s.abs() > 0.1));

    // 5. Valid source replacement to asset_b (table 0 = 2nd harmonic = 880 Hz) updates spectrum!
    let state_b = WavetableState {
        slot: Some(s1),
        frame_size: 256,
        table_position: 0.0,
        gain: 1.0,
        attack_seconds: 0.0,
        release_seconds: 0.005,
        interpolation: WavetableInterpolation::CubicHermite,
        voice_limit: 8,
    }
    .to_native_with_asset(DeviceAssetReference {
        slot: s1,
        asset: DeviceAssetKey::new(asset_b.value()).unwrap(),
    })
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: state_b.clone(),
        })
        .unwrap();
    let (_res_b, mut pb_b) = prepare_playback(editor.project(), &store).unwrap();
    pb_b.apply(SessionPlaybackCommand::Play).unwrap();
    let mut arr_b = vec![[0.0_f32; 2]; 2_048];
    pb_b.render(&mut arr_b).unwrap();
    let left_b: Vec<f32> = arr_b.iter().map(|f| f[0]).collect();
    assert!(dft_magnitude(&left_b, 880.0, RATE) > 0.8);
    assert!(dft_magnitude(&left_b, 440.0, RATE) < 0.05);

    // 6. Snapshot round-trip & Undo/Redo history integrity!
    let reopened = Project::from_snapshot(editor.project().snapshot()).unwrap();
    let reopened_state = reopened.track(track).unwrap().instrument().unwrap().state();
    assert_eq!(reopened_state.type_id(), WAVETABLE_TYPE_ID);
    assert_eq!(reopened_state, &state_b);

    assert!(editor.undo()); // back to bad_state
    assert!(editor.undo()); // back to state_pos1
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &state_pos1
    );
    assert!(editor.undo()); // back to state_pos0
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &state_pos0
    );
    assert!(editor.redo());
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &state_pos1
    );
}
