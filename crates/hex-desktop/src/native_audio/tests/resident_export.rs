//! Actual Service/native publication stays independent of captured offline resource rendering.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{ExportControl, ExportSignal, Stem};
pub(super) fn completed(h: &mut ResidentHarness) -> hex_session::service::ExportEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        match h.service.try_export_event() {
            Ok(event) => return event,
            Err(mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "export completion timed out");
                std::thread::yield_now();
            }
            Err(error) => panic!("export: {error}"),
        }
    }
}
fn pcm(path: &std::path::Path) -> Vec<[i16; 2]> {
    fs::read(path).unwrap()[44..]
        .chunks_exact(4)
        .map(|frame| {
            [
                i16::from_le_bytes([frame[0], frame[1]]),
                i16::from_le_bytes([frame[2], frame[3]]),
            ]
        })
        .collect()
}
fn quantize(level: f32) -> [i16; 2] {
    [(level * 32767.0).round() as i16; 2]
}
/// PCM16 export applies ±1 LSB TPDF dither: within one LSB of rounding.
fn assert_pcm_near(actual: &[[i16; 2]], expected: &[[i16; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (i32::from(*actual) - i32::from(*expected)).abs() <= 1,
                "frame {frame}: {actual} not within one lsb of {expected}"
            );
        }
    }
}
#[test]
fn loaded_resident_service_exports_exact_default_range_captures_and_stems_without_monitor_input() {
    let _native = native_fixture();
    for frame_limit in [32_768, 8192] {
        loaded_resident_export_case(frame_limit);
    }
}
fn loaded_resident_export_case(frame_limit: u64) {
    let files = Files::new();
    let audio_file = Files::new();
    let mut h = ResidentHarness::with_frame_limit(frame_limit);
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        slot: hex_session::service::SamplerLoadSlot::New(
            hex_project::InstrumentLoadPlacement::First,
        ),
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let imported = h.run(Request::ImportToNewTrack {
        path: audio_file.wav(48_000, 512),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    assert_eq!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_))),
        frame_limit == 8192
    );
    let audio = imported
        .state
        .project
        .tracks()
        .iter()
        .find(|entry| !entry.clips().is_empty())
        .unwrap()
        .id();
    h.run(Request::Edit(Command::Gain {
        track: audio,
        gain: 0.5,
    }));
    (applied, _) = h.applied(applied);
    let returned = h.run(Request::Edit(Command::AddReturnTrack {
        name: "bus".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = returned.created else {
        panic!("return")
    };
    (applied, _) = h.applied(applied);
    for (track, gain) in [(track, 0.5), (audio, 0.25)] {
        h.run(Request::Edit(Command::SetReturnSend {
            track,
            return_track: bus,
            gain,
        }));
        (applied, _) = h.applied(applied);
    }
    h.run(Request::Edit(Command::ReturnGain {
        return_track: bus,
        gain: 0.5,
    }));
    (applied, _) = h.applied(applied);
    let gained = h.run(Request::Edit(Command::MasterGain { gain: 0.5 }));
    (applied, _) = h.applied(applied);
    // Metadata-only alias: the report uses current authored revision while the actual compiler
    // Arc/scope remains the one accepted before this identical A/B switch.
    let state = h.run(Request::CompareDevice {
        owner: hex_session::service::preset::Owner {
            project_generation: 0,
            revision: gained.state.revision,
            device,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: gained
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id(),
        },
        expected: hex_project::DeviceVariant::A,
        action: hex_project::DeviceComparisonAction::Switch,
    });
    assert!(state.state.comparison_audio.is_some());
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: state.state.revision,
        request_id: 1,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    let output = h.audio.as_ref().unwrap().control.output_identity();
    let full = files.0.join("full.wav");
    let exported = h.run(Request::Export(full.clone()));
    assert_eq!(exported.state.project, state.state.project);
    let done = completed(&mut h);
    assert!(done.result.is_ok(), "{:?}", done.result);
    assert_eq!(done.revision, state.state.revision);
    // The default range ends at the one-beat clip end plus the sampler's
    // declared release tail, which now includes the prepared ~2 ms declick
    // floor (96 frames at 48 kHz) even though the one-shot ends earlier.
    let expected: Vec<_> = (0..24_000 + 96)
        .map(|frame| {
            let sample = if frame < 8192 { 0.25 } else { 0.0 };
            let audio = if frame < 512 { 0.125 } else { 0.0 };
            quantize((sample + audio + (sample * 0.5 + audio * 0.25) * 0.5) * 0.5)
        })
        .collect();
    assert_pcm_near(&pcm(&full), &expected);
    let range = files.0.join("range.wav");
    let control = ExportControl::default();
    h.run(Request::ExportRange {
        path: range.clone(),
        start: 129,
        frames: Some(320),
        signal: ExportSignal::Main,
        control: control.clone(),
        format: hex_export::ExportFormat::Pcm16,
        overwrite: false,
    });
    assert!(completed(&mut h).result.is_ok());
    // Dither is timeline-anchored, so the range is exactly the full slice.
    assert_eq!(pcm(&range), pcm(&full)[129..449]);
    assert_eq!(control.progress().processed_frames, 449);
    let capture = files.0.join("capture.wav");
    h.run(Request::ExportRange {
        path: capture.clone(),
        start: 0,
        frames: Some(512),
        signal: ExportSignal::CapturedMix(vec![
            hex_engine::OutputCaptureId::Track(track),
            hex_engine::OutputCaptureId::Return(bus),
        ]),
        control: ExportControl::default(),
        format: hex_export::ExportFormat::Pcm16,
        overwrite: false,
    });
    assert!(completed(&mut h).result.is_ok());
    assert_pcm_near(&pcm(&capture), &[quantize(0.328125); 512]);
    let stems = files.0.join("stems");
    h.run(Request::ExportRange {
        path: stems.clone(),
        start: 0,
        frames: Some(512),
        signal: ExportSignal::Stems(vec![
            Stem {
                id: hex_engine::OutputCaptureId::Track(track),
                name: "sampler".into(),
            },
            Stem {
                id: hex_engine::OutputCaptureId::Return(bus),
                name: "bus".into(),
            },
        ]),
        control: ExportControl::default(),
        format: hex_export::ExportFormat::Pcm16,
        overwrite: false,
    });
    assert!(completed(&mut h).result.is_ok());
    assert_pcm_near(
        &pcm(&stems.join(format!("track-{}-sampler.wav", track.value()))),
        &[quantize(0.25); 512],
    );
    assert_pcm_near(
        &pcm(&stems.join(format!("return-{}-bus.wav", bus.value()))),
        &[quantize(0.078125); 512],
    );
    assert!(!input.is_cancelled());
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    // Known publication collision and final admission refusal leave project/history/current audio.
    let original = fs::read(&full).unwrap();
    h.run(Request::Export(full.clone()));
    assert!(completed(&mut h).result.is_err());
    assert_eq!(fs::read(&full).unwrap(), original);
    h.budget_refuse
        .store(true, std::sync::atomic::Ordering::Release);
    let refused_path = files.0.join("refused.wav");
    let refused = h.run(Request::Export(refused_path.clone()));
    assert!(completed(&mut h).result.is_err());
    assert!(!refused_path.exists());
    assert_eq!(refused.state.project, state.state.project);
    assert_eq!(refused.state.revision, state.state.revision);
    assert_eq!(refused.state.can_undo, state.state.can_undo);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
}

#[test]
fn streamed_long_repitch_export_matches_resident_prefix() {
    use std::io::Write;
    let _native = native_fixture();
    let files = Files::new();
    fn source(path: &std::path::Path, frames: u32) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + frames).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&8u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&frames.to_le_bytes()).unwrap();
        let prefix: Vec<u8> = (0..8192).map(|i| 96 + ((i / 8) % 64) as u8).collect();
        file.write_all(&prefix).unwrap();
        file.set_len(44 + u64::from(frames)).unwrap();
    }
    let long = files.0.join("long.wav");
    let short = files.0.join("short.wav");
    source(&long, 20_000_000);
    source(&short, 8192);
    fn export(
        path: std::path::PathBuf,
        output: std::path::PathBuf,
        streamed: bool,
    ) -> Vec<[i16; 2]> {
        let mut h = ResidentHarness::with_frame_limit(65_536);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::ImportToNewTrack {
            path,
            before_track: None,
            start: 0,
        });
        (applied, _) = h.applied(applied);
        let has_stream = imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)));
        assert_eq!(has_stream, streamed);
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = h.run(Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }));
        (applied, _) = h.applied(applied);
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            hex_project::WarpMode::RePitch,
        )
        .unwrap();
        h.run(Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }));
        (applied, _) = h.applied(applied);
        h.run(Request::Edit(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(240_000_000).unwrap(),
        }));
        h.applied(applied);
        if streamed {
            let existing = output.with_file_name("existing-output.wav");
            fs::write(&existing, b"existing file").unwrap();
            let control = ExportControl::default();
            assert!(control.cancel());
            h.run(Request::ExportRange {
                path: existing.clone(),
                start: 128,
                frames: Some(512),
                signal: ExportSignal::Main,
                control,
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            });
            assert!(completed(&mut h).result.is_err());
            assert_eq!(fs::read(existing).unwrap(), b"existing file");
        }
        h.run(Request::ExportRange {
            path: output.clone(),
            start: 128,
            frames: Some(512),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        });
        let result = completed(&mut h);
        assert!(result.result.is_ok(), "{:?}", result.result);
        pcm(&output)
    }
    let reference = export(short, files.0.join("short-output.wav"), false);
    let actual = export(long, files.0.join("long-output.wav"), true);
    assert_pcm_near(&actual, &reference);
}

#[test]
fn streamed_long_pitch_fade_export_matches_resident_prefix() {
    use std::io::Write;
    let _native = native_fixture();
    let files = Files::new();
    fn source(path: &std::path::Path, frames: u32) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + frames).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&8u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&frames.to_le_bytes()).unwrap();
        let prefix: Vec<u8> = (0..8192).map(|i| 96 + ((i / 8) % 64) as u8).collect();
        file.write_all(&prefix).unwrap();
        file.set_len(44 + u64::from(frames)).unwrap();
    }
    let long = files.0.join("long.wav");
    let short = files.0.join("short.wav");
    source(&long, 20_000_000);
    source(&short, 8192);
    fn export(
        path: std::path::PathBuf,
        output: std::path::PathBuf,
        streamed: bool,
    ) -> Vec<[i16; 2]> {
        let mut h = ResidentHarness::with_frame_limit(65_536);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::ImportToNewTrack {
            path,
            before_track: None,
            start: 0,
        });
        (applied, _) = h.applied(applied);
        let has_stream = imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)));
        assert_eq!(has_stream, streamed);
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        h.run(Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 2048,
            fade_out: 0,
        }));
        (applied, _) = h.applied(applied);
        h.run(Request::Edit(Command::SetAudioClipPitch {
            clip,
            cents: 700,
        }));
        h.applied(applied);
        if streamed {
            let existing = output.with_file_name("existing-output.wav");
            fs::write(&existing, b"existing file").unwrap();
            let control = ExportControl::default();
            assert!(control.cancel());
            h.run(Request::ExportRange {
                path: existing.clone(),
                start: 128,
                frames: Some(512),
                signal: ExportSignal::Main,
                control,
                format: hex_export::ExportFormat::Pcm16,
                overwrite: true,
            });
            assert!(completed(&mut h).result.is_err());
            assert_eq!(fs::read(existing).unwrap(), b"existing file");
        }
        h.run(Request::ExportRange {
            path: output.clone(),
            start: 128,
            frames: Some(512),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        });
        let result = completed(&mut h);
        assert!(result.result.is_ok(), "{:?}", result.result);
        pcm(&output)
    }
    let reference = export(short, files.0.join("short-output.wav"), false);
    let actual = export(long, files.0.join("long-output.wav"), true);
    assert_pcm_near(&actual, &reference);
}
