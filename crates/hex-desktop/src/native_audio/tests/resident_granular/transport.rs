//! Ordinary source capture, independent seek pages, publication and loop wrap.
use super::*;
use hex_project::Command;
use hex_session::service::{ResidentPublicationAcceptance, ResidentSeekHistory};
use std::io::Write;
use std::path::Path;

#[test]
fn long_tones_seek_and_loop_match_converted_stereo_resident_prefix() {
    compare(hex_project::WarpMode::Tones { grain_ms: 32 });
}

#[test]
fn long_texture_seek_and_loop_match_converted_stereo_resident_prefix() {
    compare(hex_project::WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
}

fn source(path: &Path, frames: u32) {
    let mut file = fs::File::create(path).unwrap();
    let bytes = frames * 4;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&24_000u32.to_le_bytes()).unwrap();
    file.write_all(&96_000u32.to_le_bytes()).unwrap();
    file.write_all(&4u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    // Both full sources choose the same louder reference channel. The varied
    // shared prefix covers every grain and conversion tap used by these cases;
    // sparse silence beyond it changes neither the reference choice nor PCM.
    let mut prefix = Vec::with_capacity(24_000 * 4);
    for frame in 0..24_000 {
        let x = ((frame as f32 * 0.071).sin() * 6000.0) as i16;
        prefix.extend_from_slice(&x.to_le_bytes());
        prefix.extend_from_slice(&(-x * 2).to_le_bytes());
    }
    file.write_all(&prefix).unwrap();
    file.set_len(44 + u64::from(bytes)).unwrap();
}

fn position(h: &ResidentHarness) -> u64 {
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire)
}

fn collect(
    h: &mut ResidentHarness,
    destination: u64,
    loop_end: Option<u64>,
) -> Vec<(u64, [hex_engine::Frame; 64])> {
    if loop_end.is_some() {
        // A playing section has consumed its first callback at installation.
        assert_eq!(position(h), destination + 64);
    } else {
        assert_eq!(position(h), destination);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut blocks = Vec::new();
    let mut advancing = 0;
    while blocks.len() < 32 {
        h.pump();
        let before = position(h);
        let pcm = h.render();
        let after = position(h);
        if before == after {
            assert!(
                Instant::now() < deadline,
                "granular transport did not advance"
            );
            std::thread::yield_now();
            continue;
        }
        let expected = match loop_end {
            Some(end) => destination + (before - destination + 64) % (end - destination),
            None => before + 64,
        };
        assert_eq!(after, expected, "callback must advance exactly one block");
        // Transport declick has its own law. Compare settled output, including
        // all subsequent loop-end fades and attacks, at exact callback positions.
        if advancing >= FADE_BLOCKS {
            blocks.push((before, pcm));
        }
        advancing += 1;
        assert!(
            Instant::now() < deadline,
            "granular transport comparison timed out"
        );
    }
    blocks
}

fn render(
    path: &Path,
    streamed: bool,
    mode: hex_project::WarpMode,
) -> Vec<(u64, [hex_engine::Frame; 64])> {
    // Retain the short oracle's raw and transformed history, while the long
    // 25.2M-frame converted source remains beyond the resident domain cap,
    // including its 16.8M-frame output at the tested tempo.
    let mut h = ResidentHarness::with_frame_limit_at_rate(1_048_576, 48_000);
    assert!(h.next().error.is_none());
    let mut applied = h.applied(0).0;
    let imported = h.run(Request::ImportToNewTrack {
        path: path.to_path_buf(),
        before_track: None,
        start: 0,
    });
    applied = h.applied(applied).0;
    assert_eq!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .any(|(_, source)| matches!(source, AudioSourceBinding::VerifiedFile(_))),
        streamed
    );
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let neutral = h.run(Request::Edit(Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    applied = h.applied(applied).0;
    let warp = hex_project::default_audio_warp(
        neutral.state.project.clip(clip).unwrap(),
        neutral.state.project.tempo_map(),
        mode,
    )
    .unwrap();
    h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: Some(warp),
    }));
    applied = h.applied(applied).0;
    let stretched = h.run(Request::Edit(Command::SetProjectTempo {
        tempo: hex_project::Tempo::new(180_000_000).unwrap(),
    }));
    // Whole initial beats make both independent endpoint projections exactly
    // 3:2. Fractional-beat files would have different rounded map slopes.
    let original_frames = imported.state.project.clip(clip).unwrap().duration();
    assert_eq!(original_frames % 3, 0);
    assert_eq!(
        stretched.state.project.clip(clip).unwrap().duration(),
        original_frames / 3 * 2
    );
    if streamed {
        assert!(original_frames / 3 * 2 > hex_project::MAX_AUDIO_DOMAIN_FRAMES);
    }
    applied = h.applied(applied).0;
    let filter = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.utility",
    ))
    .unwrap();
    let filter = hex_engine::patch_native_device_parameter(
        &filter,
        hex_session::device_gesture::ParameterId::new(11).unwrap(),
        hex_session::device_gesture::ParameterValue::Bool(true),
    )
    .unwrap();
    let mut current = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: filter,
    }));
    applied = h.applied(applied).0;
    if streamed {
        assert_eq!(
            current
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
    }
    let mut result = Vec::new();
    for (id, destination, loop_end) in [(1, 512, None), (3, 24_004, None), (5, 4093, Some(4605))] {
        h.audio.as_ref().unwrap().control.stop();
        for _ in 0..=FADE_BLOCKS {
            h.render();
        }
        if let Some(end) = loop_end {
            current = h.run(Request::Edit(Command::SetLoopRegion {
                start: destination,
                end,
                enabled: true,
            }));
            applied = h.applied(applied).0;
        }
        let mut intent = h.seek_intent(id, current.state.revision, None, None);
        intent.destination_frame = destination;
        intent.start_playing = false;
        intent.history = ResidentSeekHistory::Preserve;
        // Sections intentionally start fresh; history-preserving sections are
        // unsupported. Linear cases above exercise the separate history worker.
        if let Some(end) = loop_end {
            intent.history = ResidentSeekHistory::Fresh;
            intent.start_playing = true;
            intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(destination, end, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Immediate,
            });
        }
        h.service.submit_resident_arrangement_seek(intent).unwrap();
        assert!(h.next().error.is_none());
        let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
            h.seek_result().result.unwrap()
        else {
            panic!("actual seek publication required")
        };
        let (installed, metadata) = h.applied(applied);
        assert_eq!(installed, audio_revision);
        assert_eq!(metadata.seek_request, Some(id));
        applied = installed;
        let blocks = collect(&mut h, destination, loop_end);
        assert!(
            blocks
                .iter()
                .flat_map(|(_, block)| block.iter().flatten())
                .any(|x| x.abs() > 0.01),
            "each seek/loop case must exercise nonzero audio"
        );
        result.extend(blocks);
        if id == 1 {
            let mut cancelled = h.seek_intent(2, current.state.revision, None, None);
            cancelled.history = ResidentSeekHistory::Preserve;
            cancelled
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            h.service
                .submit_resident_arrangement_seek(cancelled)
                .unwrap();
            let _terminal = h.next();
            assert!(h.seek_result().result.is_err());
            h.render();
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
    }
    if streamed {
        h.audio.as_ref().unwrap().control.stop();
        for _ in 0..=FADE_BLOCKS {
            h.render();
        }
        current = h.run(Request::Edit(Command::SetLoopRegion {
            start: 4093,
            end: 4605,
            enabled: false,
        }));
        applied = h.applied(applied).0;
        fs::remove_file(path).unwrap();
        let mut missing = h.seek_intent(7, current.state.revision, None, None);
        missing.history = ResidentSeekHistory::Preserve;
        missing.destination_frame = 24_004;
        h.service.submit_resident_arrangement_seek(missing).unwrap();
        let _terminal = h.next();
        assert!(h.seek_result().result.is_err());
        h.render();
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
    result
}

fn compare(mode: hex_project::WarpMode) {
    let _native = native_fixture();
    let files = Files::new();
    let short = files.0.join("short-stereo.wav");
    let long = files.0.join("long-stereo.wav");
    source(&short, 24_000);
    source(&long, 12_600_000);
    let expected = render(&short, false, mode);
    let actual = render(&long, true, mode);
    assert_eq!(actual.len(), expected.len());
    assert!(
        expected
            .iter()
            .flat_map(|(_, block)| block.iter().flatten())
            .any(|x| x.abs() > 0.01)
    );
    for ((at, actual), (expected_at, expected)) in actual.iter().zip(&expected) {
        assert_eq!(at, expected_at);
        let error = actual
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            error < 1e-6,
            "granular transport PCM difference {error} at {at}"
        );
    }
}
