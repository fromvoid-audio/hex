use super::*;
use crate::ResidentNativeReservation;
use hex_engine::{AudioStorageLedgerReservation, SessionPlayback, SessionPlaybackCommand};
use hex_project::{BeatTime, Command, Created, Project};
use std::{num::NonZeroU64, sync::atomic::AtomicBool};

const RATE: u32 = 48_000;

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hex_drum_drop_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write_wav(&self, name: &str, samples: &[f32]) -> PathBuf {
        let path = self.0.join(name);
        let data_bytes = (samples.len() * 2) as u32;
        let riff_size = 36 + data_bytes;
        let mut wav = Vec::with_capacity((44 + data_bytes) as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for &s in samples {
            let pcm = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            wav.extend_from_slice(&pcm.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        path
    }
    fn write_bytes(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn beat(q: u64) -> BeatTime {
    BeatTime::from_raw(q << 32)
}

fn beat_frac(num: u64, den: u64) -> BeatTime {
    BeatTime::from_raw((num << 32) / den)
}

fn build_shared_drum_rack_session() -> (
    Session,
    hex_project::SourceId,
    DeviceId,
    hex_project::TrackId,
    hex_project::TrackId,
) {
    let mut session = Session::new(RATE, 32, 8_000_000)
        .unwrap()
        .with_native_reservation(ResidentNativeReservation {
            storage: AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 128,
                root_rows: 64,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            max_scan_frames: 2_000_000,
        })
        .unwrap();
    let empty_rack = hex_engine::drum_rack_drop::default_drum_rack_with_empty_pads(
        NonZeroU64::new(10).unwrap(),
        36,
        4,
    )
    .unwrap();
    let Created::NativeSource(source) = session
        .editor
        .execute(Command::CreateNativeSource {
            name: "drum-rack".into(),
            devices: vec![empty_rack],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("expected native source");
    };
    let Created::Track(track_a) = session
        .editor
        .execute(Command::AddMidiTrack {
            name: "kick-contrib".into(),
        })
        .unwrap()
    else {
        panic!("track a");
    };
    let Created::Track(track_b) = session
        .editor
        .execute(Command::AddMidiTrack {
            name: "snare-contrib".into(),
        })
        .unwrap()
    else {
        panic!("track b");
    };
    for track in [track_a, track_b] {
        if session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .is_some()
        {
            session
                .editor
                .execute(Command::RemoveInstrument { track })
                .unwrap();
        }
        session
            .editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
    }
    let device = session
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    (session, source, device, track_a, track_b)
}

#[test]
fn drop_sample_and_instrument_onto_drum_rack_pads_renders_distinct_audible_pads_with_undo_save_and_refusals()
 {
    let dir = TempDir::new("e2e");
    // 256-frame impulse/square kick wave so Sampler output is unmistakably audible.
    let kick_samples: Vec<f32> = (0..256)
        .map(|i| if i % 8 < 4 { 0.6 } else { -0.6 })
        .collect();
    let wav_path = dir.write_wav("Kick808.wav", &kick_samples);
    let corrupt_path = dir.write_bytes("corrupt.wav", b"not-a-valid-wav-file");
    let unsupported_path = dir.write_bytes("notes.txt", b"hello world");

    let (mut session, source, device, track_a, track_b) = build_shared_drum_rack_session();
    let cancel = AtomicBool::new(false);
    let generation = 7_u64;

    // Schedule MIDI note 36 (Pad 1) on Track A at beat 0, and note 37 (Pad 2) on Track B at beat 1/32.
    let Created::Clip(clip_a) = session
        .editor
        .execute(Command::PlaceMidiClip {
            track: track_a,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip a");
    };
    session
        .editor
        .execute(Command::AddMidiNote {
            clip: clip_a,
            start: BeatTime::ZERO,
            duration: beat_frac(1, 64),
            pitch: 36,
            channel: 0,
            velocity: 120,
        })
        .unwrap();
    let Created::Clip(clip_b) = session
        .editor
        .execute(Command::PlaceMidiClip {
            track: track_b,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip b");
    };
    session
        .editor
        .execute(Command::AddMidiNote {
            clip: clip_b,
            start: beat_frac(1, 32),
            duration: beat_frac(1, 32),
            pitch: 37,
            channel: 0,
            velocity: 110,
        })
        .unwrap();

    // 1. Drop real WAV sample onto Pad 1 (note 36 -> Sampler with host-leased asset).
    let rev_before_sample = session.history_revision();
    let sample_receipt = session
        .drop_onto_drum_rack_pad(
            generation,
            DrumRackPadDropRequest {
                project_generation: generation,
                expected_revision: rev_before_sample,
                device,
                pad_id: NonZeroU64::new(1).unwrap(),
                input: DrumRackPadDropInput::SampleFile {
                    path: wav_path.clone(),
                    pad_name: None,
                },
            },
            &cancel,
            None,
        )
        .unwrap();
    let imported_asset = sample_receipt
        .imported_asset
        .expect("imported sample asset");
    assert!(session.native_source(imported_asset).is_some());
    assert_eq!(session.project().native_sources().len(), 1);

    // 2. Drop real native Subtractive instrument onto Pad 2 (note 37).
    let rev_before_inst = session.history_revision();
    let inst_receipt = session
        .drop_onto_drum_rack_pad(
            generation,
            DrumRackPadDropRequest {
                project_generation: generation,
                expected_revision: rev_before_inst,
                device,
                pad_id: NonZeroU64::new(2).unwrap(),
                input: DrumRackPadDropInput::NativeInstrument {
                    type_id: DeviceTypeId::from_static("hex.instrument.subtractive"),
                    pad_name: Some("synth snare".into()),
                },
            },
            &cancel,
            Some(sample_receipt.prepared_revision.as_ref()),
        )
        .unwrap();
    assert!(inst_receipt.imported_asset.is_none());

    let summaries = hex_engine::drum_rack_drop::pad_summaries(
        session.project().device(device).unwrap().state(),
    )
    .unwrap();
    assert_eq!(summaries[0].name, "kick808");
    assert!(summaries[0].has_sample_asset);
    assert_eq!(
        summaries[0].occupied_type,
        Some(DeviceTypeId::from_static("hex.instrument.sampler"))
    );
    assert_eq!(summaries[1].name, "synth snare");
    assert!(!summaries[1].has_sample_asset);
    assert_eq!(
        summaries[1].occupied_type,
        Some(DeviceTypeId::from_static("hex.instrument.subtractive"))
    );

    // 3. Render through real SessionPlayback::new_with_device_resources using the exact
    //    prepared_resources admitted with inst_receipt -> both pads are distinctly audible!
    let resources = inst_receipt
        .prepared_resources
        .as_ref()
        .expect("drum rack drop receipt carries matching prepared device resources");
    let mut pb = SessionPlayback::new_with_device_resources(
        Arc::clone(&inst_receipt.prepared_revision),
        resources,
    )
    .unwrap();
    pb.apply(SessionPlaybackCommand::Play).unwrap();
    let mut rendered = vec![[0.0_f32; 2]; 1_600];
    pb.render(&mut rendered).unwrap();

    let pad1_energy: f32 = rendered[0..256].iter().map(|f| f[0].abs()).sum();
    let pad2_energy: f32 = rendered[750..1_400].iter().map(|f| f[0].abs()).sum();
    assert!(
        pad1_energy > 20.0,
        "dropped WAV Sampler pad must be audible: {pad1_energy}"
    );
    assert!(
        pad2_energy > 5.0,
        "dropped Subtractive pad must be audible: {pad2_energy}"
    );

    // 4. Refusal paths leave prior pads, project state, and media 100% intact!
    let snapshot_before_refusals = session.project().clone();
    let asset_count_before = session.project().assets().count();

    // 4a. Stale revision refused.
    assert!(
        session
            .drop_onto_drum_rack_pad(
                generation,
                DrumRackPadDropRequest {
                    project_generation: generation,
                    expected_revision: rev_before_sample,
                    device,
                    pad_id: NonZeroU64::new(3).unwrap(),
                    input: DrumRackPadDropInput::SampleFile {
                        path: wav_path.clone(),
                        pad_name: None,
                    },
                },
                &cancel,
                None,
            )
            .is_err()
    );
    // 4b. Project replacement (wrong generation) refused.
    assert!(
        session
            .drop_onto_drum_rack_pad(
                generation,
                DrumRackPadDropRequest {
                    project_generation: generation + 1,
                    expected_revision: session.history_revision(),
                    device,
                    pad_id: NonZeroU64::new(3).unwrap(),
                    input: DrumRackPadDropInput::NativeInstrument {
                        type_id: DeviceTypeId::from_static("hex.instrument.subtractive"),
                        pad_name: None,
                    },
                },
                &cancel,
                None,
            )
            .is_err()
    );
    // 4c. Stale/missing pad_id refused.
    assert!(
        session
            .drop_onto_drum_rack_pad(
                generation,
                DrumRackPadDropRequest {
                    project_generation: generation,
                    expected_revision: session.history_revision(),
                    device,
                    pad_id: NonZeroU64::new(999).unwrap(),
                    input: DrumRackPadDropInput::NativeInstrument {
                        type_id: DeviceTypeId::from_static("hex.instrument.subtractive"),
                        pad_name: None,
                    },
                },
                &cancel,
                None,
            )
            .is_err()
    );
    // 4d. Nested Drum Rack refused.
    assert!(
        session
            .drop_onto_drum_rack_pad(
                generation,
                DrumRackPadDropRequest {
                    project_generation: generation,
                    expected_revision: session.history_revision(),
                    device,
                    pad_id: NonZeroU64::new(3).unwrap(),
                    input: DrumRackPadDropInput::NativeInstrument {
                        type_id: DeviceTypeId::from_static("hex.rack.drum"),
                        pad_name: None,
                    },
                },
                &cancel,
                None,
            )
            .is_err()
    );
    // 4e. Unsupported file & corrupt WAV refused.
    for bad_path in [unsupported_path, corrupt_path] {
        assert!(
            session
                .drop_onto_drum_rack_pad(
                    generation,
                    DrumRackPadDropRequest {
                        project_generation: generation,
                        expected_revision: session.history_revision(),
                        device,
                        pad_id: NonZeroU64::new(3).unwrap(),
                        input: DrumRackPadDropInput::SampleFile {
                            path: bad_path,
                            pad_name: None,
                        },
                    },
                    &cancel,
                    None,
                )
                .is_err()
        );
    }
    // 4f. Cancellation refused cleanly.
    let cancelled = AtomicBool::new(true);
    assert!(
        session
            .drop_onto_drum_rack_pad(
                generation,
                DrumRackPadDropRequest {
                    project_generation: generation,
                    expected_revision: session.history_revision(),
                    device,
                    pad_id: NonZeroU64::new(3).unwrap(),
                    input: DrumRackPadDropInput::SampleFile {
                        path: wav_path,
                        pad_name: None,
                    },
                },
                &cancelled,
                None,
            )
            .is_err()
    );

    assert_eq!(*session.project(), snapshot_before_refusals);
    assert_eq!(session.project().assets().count(), asset_count_before);

    // 5. Save/reopen snapshot roundtrip preserves both dropped pads and asset bindings!
    let reopened = Project::from_snapshot(session.project().snapshot()).unwrap();
    assert_eq!(reopened, *session.project());
    assert_eq!(
        reopened.native_source(source).unwrap().receiver(),
        Some(track_a)
    );

    // 6. Canonical single-step Undo/Redo reverts instrument drop, then reverts sample drop + asset!
    assert!(session.editor.undo());
    let after_undo_inst = hex_engine::drum_rack_drop::pad_summaries(
        session.project().device(device).unwrap().state(),
    )
    .unwrap();
    assert!(after_undo_inst[0].occupied_type.is_some());
    assert!(after_undo_inst[1].occupied_type.is_none());

    assert!(session.editor.undo());
    let after_undo_sample = hex_engine::drum_rack_drop::pad_summaries(
        session.project().device(device).unwrap().state(),
    )
    .unwrap();
    assert!(after_undo_sample[0].occupied_type.is_none());
    assert!(session.project().asset(imported_asset).is_none());

    assert!(session.editor.redo());
    assert!(session.editor.redo());
    assert_eq!(*session.project(), snapshot_before_refusals);
    // Redone project re-prepares cleanly with retained ResidentDeviceAssets custody!
    let redone_resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &session.native_assets,
            session.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
    let _ = redone_resources;
}
