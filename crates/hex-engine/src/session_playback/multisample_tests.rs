use super::*;
use crate::audio_transform::AudioTransformPreparation;
use crate::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use crate::{AudioStorageLedgerReservation, OwnedAudioSource};
use hex_devices::multisample::{
    GAIN, KeyZoneSpec, MULTISAMPLE_TYPE_ID, MultisampleState, OVERLAP_POLICY, VOICE_LIMIT,
    ZoneOverlapPolicy,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, ParameterValue};
use hex_project::{AssetId, BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;
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

fn insert_resident(
    store: &mut ResidentDeviceAssets,
    asset: AssetId,
    sample_rate: u32,
    frames: Vec<[f32; 2]>,
) {
    let cancel = AtomicBool::new(false);
    let len = frames.len() as u64;
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(sample_rate, frames)
                    .ok()
                    .unwrap(),
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
fn multisample_arrangement_session_simultaneous_steal_refusal_history_and_stop() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "keys".into(),
        })
        .unwrap()
    else {
        panic!("expected midi track");
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "s1".into() })
        .unwrap()
    else {
        panic!("expected scene");
    };
    let Created::Asset(asset_low) = editor
        .execute(Command::ImportDeviceAsset {
            path: "low-44k1.wav".into(),
            sample_rate: 44_100,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("expected asset_low");
    };
    let Created::Asset(asset_high) = editor
        .execute(Command::ImportDeviceAsset {
            path: "high-96k.wav".into(),
            sample_rate: 96_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("expected asset_high");
    };

    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    let refs = vec![
        DeviceAssetReference {
            slot: s1,
            asset: DeviceAssetKey::new(asset_low.value()).unwrap(),
        },
        DeviceAssetReference {
            slot: s2,
            asset: DeviceAssetKey::new(asset_high.value()).unwrap(),
        },
    ];
    let authored = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 8,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 36,
                high_key: 59,
                low_velocity: 1,
                high_velocity: 127,
                root_key: 48,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.005,
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 60,
                high_key: 84,
                low_velocity: 1,
                high_velocity: 127,
                root_key: 72,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.005,
            },
        ],
    };
    let native_state = authored.to_native_with_assets(refs.clone()).unwrap();
    let Created::Device(device_id) = editor
        .execute(Command::InsertInstrument {
            track,
            state: native_state.clone(),
        })
        .unwrap()
    else {
        panic!("expected device");
    };

    // Arrangement clip:
    // - Low zone pitch 48 (zone 0, 44.1 kHz, level 0.2 / -0.2) at beat 0..1/4
    // - High zone pitch 72 (zone 1, 96 kHz, level 0.5 / 0.1) at beat 0..1/4
    // - Unmapped pitch 96 (outside all zones -> silence) at beat 0..1/4
    let Created::Clip(arr_clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("expected arr clip");
    };
    for pitch in [48, 72, 96] {
        editor
            .execute(Command::AddMidiNote {
                clip: arr_clip,
                start: BeatTime::ZERO,
                duration: beat_frac(1, 4),
                pitch,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }

    // Session clip on the same track triggering only high zone (pitch 72).
    let Created::SessionClip(sess_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session-high".into(),
            source_length: beat(1),
        })
        .unwrap()
    else {
        panic!("expected session clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: sess_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 2),
            pitch: 72,
            channel: 0,
            velocity: 127,
        })
        .unwrap();

    // Populate resident device assets (44.1 kHz & 96 kHz).
    let mut store = ResidentDeviceAssets::default();
    insert_resident(&mut store, asset_low, 44_100, vec![[0.2, -0.2]; 4096]);
    insert_resident(&mut store, asset_high, 96_000, vec![[0.5, 0.1]; 4096]);

    // 1. Admitted Arrangement playback: low zone (48) + high zone (72) + unmapped (96 -> silence).
    let (_res, mut playback) = prepare_playback(editor.project(), &store).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut arr_buf = vec![[0.0_f32; 2]; 128];
    playback.render(&mut arr_buf).unwrap();
    // Expected: [0.2, -0.2] + [0.5, 0.1] = [0.7, -0.1] (unmapped pitch 96 adds 0.0).
    for &frame in &arr_buf[..64] {
        assert!(
            (frame[0] - 0.7).abs() < 1e-4 && (frame[1] - (-0.1)).abs() < 1e-4,
            "unexpected arrangement frame {frame:?}"
        );
    }

    // Stop clears active voices immediately without stuck notes.
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    let mut stop_buf = vec![[0.0_f32; 2]; 64];
    playback.render(&mut stop_buf).unwrap();
    assert!(
        stop_buf.iter().all(|f| *f == [0.0, 0.0]),
        "stop must silence immediately with no stuck notes"
    );

    // 2. Admitted Session playback on the same multisample track!
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: sess_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut sess_buf = vec![[0.0_f32; 2]; 128];
    playback.render(&mut sess_buf).unwrap();
    // Session clip plays only pitch 72 -> [0.5, 0.1].
    for &frame in &sess_buf[..64] {
        assert!(
            (frame[0] - 0.5).abs() < 1e-4 && (frame[1] - 0.1).abs() < 1e-4,
            "unexpected session frame {frame:?}"
        );
    }

    // 3. Missing resource & stale/refused preparation preserve the running playback graph!
    let empty_store = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    assert!(
        PreparedDeviceResources::prepare_on_control(
            &empty_store,
            editor.project(),
            1_000_000,
            &cancel
        )
        .is_err()
    );
    // Existing admitted playback still renders cleanly after refusal.
    let mut after_refusal = vec![[0.0_f32; 2]; 64];
    playback.render(&mut after_refusal).unwrap();
    assert!((after_refusal[0][0] - 0.5).abs() < 1e-4);

    // 4. Undo/Redo + Snapshot round-trip preserves multisample state and asset references.
    let patched = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &native_state,
        GAIN,
        ParameterValue::Float(0.5),
    )
    .unwrap();
    let patched = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &patched,
        OVERLAP_POLICY,
        ParameterValue::Choice(2),
    )
    .unwrap();
    let patched = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &patched,
        VOICE_LIMIT,
        ParameterValue::Float(1.0),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: patched.clone(),
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &patched
    );

    // Snapshot save/reopen round-trip!
    let snapshot = editor.project().snapshot();
    let reopened = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        reopened.track(track).unwrap().instrument().unwrap().state(),
        &patched
    );
    assert_eq!(
        reopened
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .type_id(),
        MULTISAMPLE_TYPE_ID
    );
    assert_eq!(
        reopened
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .asset_references(),
        refs.as_slice()
    );

    // Undo restores original state; Redo restores patched state.
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &native_state
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
        &patched
    );

    // 5. Voice stealing through admitted Arrangement playback with voice_limit = 1:
    // Note 1 plays 0..1/64 (release 5 ms = 240 frames), Note 2 starts at 1/64 (steals Note 1!).
    let mut steal_editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(s_track) = steal_editor
        .execute(Command::AddMidiTrack {
            name: "mono-ms".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Asset(s_a1) = steal_editor
        .execute(Command::ImportDeviceAsset {
            path: "a1.wav".into(),
            sample_rate: RATE,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("a1");
    };
    let Created::Asset(s_a2) = steal_editor
        .execute(Command::ImportDeviceAsset {
            path: "a2.wav".into(),
            sample_rate: RATE,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("a2");
    };
    let mono_state = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 1,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 0,
                high_key: 60,
                low_velocity: 1,
                high_velocity: 127,
                root_key: 60,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.01,
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 61,
                high_key: 127,
                low_velocity: 1,
                high_velocity: 127,
                root_key: 72,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.01,
            },
        ],
    }
    .to_native_with_assets(vec![
        DeviceAssetReference {
            slot: s1,
            asset: DeviceAssetKey::new(s_a1.value()).unwrap(),
        },
        DeviceAssetReference {
            slot: s2,
            asset: DeviceAssetKey::new(s_a2.value()).unwrap(),
        },
    ])
    .unwrap();
    steal_editor
        .execute(Command::InsertInstrument {
            track: s_track,
            state: mono_state,
        })
        .unwrap();
    let Created::Clip(s_clip) = steal_editor
        .execute(Command::PlaceMidiClip {
            track: s_track,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    // At 120 BPM, 1 beat = 24_000 frames; 1/64 beat = 375 frames.
    steal_editor
        .execute(Command::AddMidiNote {
            clip: s_clip,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 64),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    steal_editor
        .execute(Command::AddMidiNote {
            clip: s_clip,
            start: beat_frac(1, 64),
            duration: beat_frac(1, 4),
            pitch: 72,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let mut steal_store = ResidentDeviceAssets::default();
    insert_resident(&mut steal_store, s_a1, RATE, vec![[0.4, 0.4]; 4096]);
    insert_resident(&mut steal_store, s_a2, RATE, vec![[0.2, 0.2]; 4096]);
    let (_s_res, mut steal_pb) = prepare_playback(steal_editor.project(), &steal_store).unwrap();
    steal_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut steal_out = vec![[0.0_f32; 2]; 512];
    steal_pb.render(&mut steal_out).unwrap();
    // Before 375: note 1 (0.4). At 375: note 1 stolen with declick + note 2 starts (0.2 + ~0.4).
    // After 375 + 48 = 423: declick finished, only note 2 (0.2) remains!
    assert!((steal_out[10][0] - 0.4).abs() < 1e-4);
    assert!(
        steal_out[375][0] > 0.5,
        "expected stolen declick + new note at 375, got {}",
        steal_out[375][0]
    );
    assert!(
        (steal_out[450][0] - 0.2).abs() < 1e-4,
        "expected only note 2 after declick, got {}",
        steal_out[450][0]
    );

    // 6. Simultaneous same-note (pitch 48 from two contributor tracks sharing one
    //    retained NativeSource) with independent NoteOff!
    let mut shared_editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Asset(sh_a) = shared_editor
        .execute(Command::ImportDeviceAsset {
            path: "shared-low.wav".into(),
            sample_rate: RATE,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("sh_a");
    };
    let shared_state = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::FirstMatch,
        voice_limit: 8,
        zones: vec![KeyZoneSpec {
            slot: s1,
            low_key: 36,
            high_key: 60,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 48,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.0,
        }],
    }
    .to_native_with_assets(vec![DeviceAssetReference {
        slot: s1,
        asset: DeviceAssetKey::new(sh_a.value()).unwrap(),
    }])
    .unwrap();
    let Created::NativeSource(source_id) = shared_editor
        .execute(Command::CreateNativeSource {
            name: "shared-ms".into(),
            devices: vec![shared_state],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source");
    };
    let Created::Track(ta) = shared_editor
        .execute(Command::AddMidiTrack { name: "a".into() })
        .unwrap()
    else {
        panic!("ta");
    };
    let Created::Track(tb) = shared_editor
        .execute(Command::AddMidiTrack { name: "b".into() })
        .unwrap()
    else {
        panic!("tb");
    };
    for t in [ta, tb] {
        shared_editor
            .execute(Command::BindTrackSource {
                track: t,
                source: Some(source_id),
            })
            .unwrap();
    }
    let Created::Clip(ca) = shared_editor
        .execute(Command::PlaceMidiClip {
            track: ta,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("ca");
    };
    let Created::Clip(cb) = shared_editor
        .execute(Command::PlaceMidiClip {
            track: tb,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("cb");
    };
    shared_editor
        .execute(Command::AddMidiNote {
            clip: ca,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 4),
            pitch: 48,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    shared_editor
        .execute(Command::AddMidiNote {
            clip: cb,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 64), // ends at frame 375
            pitch: 48,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let mut shared_store = ResidentDeviceAssets::default();
    insert_resident(&mut shared_store, sh_a, RATE, vec![[0.25, -0.25]; 4096]);
    let (_sh_res, mut shared_pb) =
        prepare_playback(shared_editor.project(), &shared_store).unwrap();
    shared_pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut shared_out = vec![[0.0_f32; 2]; 512];
    shared_pb.render(&mut shared_out).unwrap();
    // Both simultaneous pitch-48 occurrences sound at frame 10 -> [0.5, -0.5].
    assert!(
        (shared_out[10][0] - 0.5).abs() < 1e-4 && (shared_out[10][1] - (-0.5)).abs() < 1e-4,
        "expected simultaneous same-note sum [0.5, -0.5], got {:?}",
        shared_out[10]
    );
    // After contributor b releases at frame 375, contributor a holds -> [0.25, -0.25].
    assert!(
        (shared_out[400][0] - 0.25).abs() < 1e-4 && (shared_out[400][1] - (-0.25)).abs() < 1e-4,
        "expected remaining note [0.25, -0.25] after independent NoteOff, got {:?}",
        shared_out[400]
    );
}

#[test]
fn multisample_velocity_layers_shared_same_note_source_replacement_and_history() {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 32).unwrap();
    let Created::Asset(a_soft) = editor
        .execute(Command::ImportDeviceAsset {
            path: "vel-soft-44k1.wav".into(),
            sample_rate: 44_100,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("a_soft");
    };
    let Created::Asset(a_hard) = editor
        .execute(Command::ImportDeviceAsset {
            path: "vel-hard-96k.wav".into(),
            sample_rate: 96_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("a_hard");
    };
    let Created::Asset(a_replacement) = editor
        .execute(Command::ImportDeviceAsset {
            path: "vel-hard-alt-48k.wav".into(),
            sample_rate: 48_000,
            frames: 4096,
        })
        .unwrap()
    else {
        panic!("a_replacement");
    };

    let s1 = DeviceAssetSlot::new(1).unwrap();
    let s2 = DeviceAssetSlot::new(2).unwrap();
    // Soft layer: vel 1..=60 (slot 1), Hard layer: vel 80..=127 (slot 2), gap: 61..=79 -> silence.
    let initial_state = MultisampleState {
        gain: 1.0,
        overlap_policy: ZoneOverlapPolicy::LayerAll,
        voice_limit: 8,
        zones: vec![
            KeyZoneSpec {
                slot: s1,
                low_key: 48,
                high_key: 72,
                low_velocity: 1,
                high_velocity: 60,
                root_key: 60,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.0,
            },
            KeyZoneSpec {
                slot: s2,
                low_key: 48,
                high_key: 72,
                low_velocity: 80,
                high_velocity: 127,
                root_key: 60,
                detune_cents: 0.0,
                gain: 1.0,
                release_seconds: 0.0,
            },
        ],
    }
    .to_native_with_assets(vec![
        DeviceAssetReference {
            slot: s1,
            asset: DeviceAssetKey::new(a_soft.value()).unwrap(),
        },
        DeviceAssetReference {
            slot: s2,
            asset: DeviceAssetKey::new(a_hard.value()).unwrap(),
        },
    ])
    .unwrap();

    // Two contributor tracks sharing ONE retained multisample NativeSource trigger
    // the SAME pitch (60) simultaneously at different velocities (50 = soft, 100 = hard),
    // while a third note on track A at velocity 70 sits in the velocity gap -> silence!
    let Created::NativeSource(source_id) = editor
        .execute(Command::CreateNativeSource {
            name: "vel-ms".into(),
            devices: vec![initial_state.clone()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source");
    };
    let Created::Track(ta) = editor
        .execute(Command::AddMidiTrack {
            name: "soft-in".into(),
        })
        .unwrap()
    else {
        panic!("ta");
    };
    let Created::Track(tb) = editor
        .execute(Command::AddMidiTrack {
            name: "hard-in".into(),
        })
        .unwrap()
    else {
        panic!("tb");
    };
    for t in [ta, tb] {
        editor
            .execute(Command::BindTrackSource {
                track: t,
                source: Some(source_id),
            })
            .unwrap();
    }
    let Created::Clip(ca) = editor
        .execute(Command::PlaceMidiClip {
            track: ta,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("ca");
    };
    let Created::Clip(cb) = editor
        .execute(Command::PlaceMidiClip {
            track: tb,
            start: BeatTime::ZERO,
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("cb");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: ca,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 4),
            pitch: 60,
            channel: 0,
            velocity: 50, // soft layer (0.4 * 50/127)
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip: ca,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 4),
            pitch: 64,
            channel: 0,
            velocity: 70, // velocity gap 61..=79 -> silence
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip: cb,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 64), // releases at frame 375
            pitch: 60,
            channel: 0,
            velocity: 100, // hard layer (0.8 * 100/127)
        })
        .unwrap();

    let mut store = ResidentDeviceAssets::default();
    insert_resident(&mut store, a_soft, 44_100, vec![[0.4, -0.4]; 4096]);
    insert_resident(&mut store, a_hard, 96_000, vec![[0.8, 0.2]; 4096]);

    let (_res1, mut pb1) = prepare_playback(editor.project(), &store).unwrap();
    pb1.apply(SessionPlaybackCommand::Play).unwrap();
    let mut out1 = vec![[0.0_f32; 2]; 512];
    pb1.render(&mut out1).unwrap();
    let exp_soft_l = 0.4 * (50.0 / 127.0);
    let exp_hard_l = 0.8 * (100.0 / 127.0);
    assert!(
        (out1[10][0] - (exp_soft_l + exp_hard_l)).abs() < 1e-4,
        "expected soft+hard sum {}, got {}",
        exp_soft_l + exp_hard_l,
        out1[10][0]
    );
    // After track B's hard note releases at frame 375, only track A's soft layer sounds!
    assert!(
        (out1[400][0] - exp_soft_l).abs() < 1e-4,
        "expected only soft layer {exp_soft_l}, got {}",
        out1[400][0]
    );

    // Source replacement: point slot 2 to a_replacement BEFORE loading it -> prepare refused,
    // original playback pb1 keeps playing cleanly; then load a_replacement -> new PCM plays!
    let device_id = editor
        .project()
        .track(ta)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let replaced_state = MultisampleState::from_native(&initial_state)
        .unwrap()
        .to_native_with_assets(vec![
            DeviceAssetReference {
                slot: s1,
                asset: DeviceAssetKey::new(a_soft.value()).unwrap(),
            },
            DeviceAssetReference {
                slot: s2,
                asset: DeviceAssetKey::new(a_replacement.value()).unwrap(),
            },
        ])
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: device_id,
            state: replaced_state.clone(),
        })
        .unwrap();
    // Refusal before a_replacement is resident in store preserves pb1!
    assert!(prepare_playback(editor.project(), &store).is_err());
    let mut still_ok = vec![[0.0_f32; 2]; 32];
    pb1.render(&mut still_ok).unwrap();
    assert!((still_ok[0][0] - exp_soft_l).abs() < 1e-4);

    // Now admit a_replacement in store -> prepare succeeds with replacement PCM!
    insert_resident(&mut store, a_replacement, 48_000, vec![[0.3, 0.3]; 4096]);
    let (_res2, mut pb2) = prepare_playback(editor.project(), &store).unwrap();
    pb2.apply(SessionPlaybackCommand::Play).unwrap();
    let mut out2 = vec![[0.0_f32; 2]; 64];
    pb2.render(&mut out2).unwrap();
    let exp_repl_l = 0.3 * (100.0 / 127.0);
    assert!(
        (out2[10][0] - (exp_soft_l + exp_repl_l)).abs() < 1e-4,
        "expected soft+replacement {}, got {}",
        exp_soft_l + exp_repl_l,
        out2[10][0]
    );

    // Undo restores original asset binding cleanly.
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .track(ta)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        &initial_state
    );
}
