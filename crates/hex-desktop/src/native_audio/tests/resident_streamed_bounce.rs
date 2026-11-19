//! Pulse #4148: a resident document bounces its file-backed (streamed) clips
//! through the production `VerifiedPages` bouncer and native sink, matching the
//! source PCM, while the transport keeps playing; a changed original refuses.
use super::resident_preview::ResidentHarness;
use super::resident_sections::run_callback;
use super::*;
use hex_project::{AssetId, BounceAdoption, BounceTarget, TimeRange};
use std::path::Path;

mod long_reverse;

const FRAMES: u32 = 24_000;
/// 16-bit mono ramp: every frame differs, so any offset or crop error shows.
fn ramp(path: &Path, step: i32) {
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(36 + FRAMES * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(FRAMES * 2).to_le_bytes());
    for i in 0..FRAMES as i32 {
        bytes.extend_from_slice(&((i * step) % 30_000 - 15_000).to_le_bytes()[..2]);
    }
    fs::write(path, bytes).unwrap();
}
fn decoded(path: &Path) -> Vec<hex_engine::Frame> {
    hex_media::decode_wav_file(path, hex_media::Limits::new(64 << 20, 1 << 22))
        .unwrap()
        .audio()
        .frames()
        .to_vec()
}
fn added_asset(before: &hex_project::Project, after: &hex_project::Project) -> AssetId {
    let added: Vec<AssetId> = after
        .assets()
        .map(|asset| asset.id())
        .filter(|id| before.asset(*id).is_none())
        .collect();
    assert_eq!(added.len(), 1, "one bounced asset");
    added[0]
}
fn assert_matches(actual: &[hex_engine::Frame], expected: &[hex_engine::Frame], case: &str) {
    assert!(actual.len() >= expected.len(), "{case}: short render");
    let error = actual
        .iter()
        .zip(expected)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs()))
        .fold(0.0_f32, f32::max);
    assert!(error < 1e-6, "{case}: max sample error {error}");
}
fn bounce(h: &mut ResidentHarness, state: &Event, target: BounceTarget, in_place: bool) -> Event {
    let generation = state
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    run_callback(
        h,
        Request::BounceArrangement {
            project_generation: generation,
            revision: state.state.revision,
            target,
            in_place,
            adoption: BounceAdoption::NewTracks,
            control: hex_export::ExportControl::default(),
        },
    )
}

#[test]
fn streamed_clip_bounces_match_source_while_playing_and_changed_media_refuses() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.0.join("long.wav");
    ramp(&path, 7);
    let source = decoded(&path);
    // The original exceeds the resident working set, so import keeps it file-backed.
    let mut h = ResidentHarness::with_frame_limit(16_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path: path.clone(),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .all(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_))),
        "the fixture clip must be streamed"
    );
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();

    let range = TimeRange::new(vec![track], 1_000, 5_000).unwrap();
    let ranged = bounce(&mut h, &imported, BounceTarget::Range(range), false);
    assert!(ranged.error.is_none(), "{:?}", ranged.error);
    (applied, _) = h.applied(applied);
    let asset = added_asset(&imported.state.project, &ranged.state.project);
    let file = ranged
        .state
        .project
        .asset(asset)
        .unwrap()
        .path()
        .to_path_buf();
    assert_matches(&decoded(&file), &source[1_000..5_000], "trimmed range");

    // Grouping in place while the transport plays: the render borrows no native
    // custody, so playback continues and the grouped clip carries the exact crop.
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render();
    let crop = TimeRange::new(vec![track], 6_000, 9_000).unwrap();
    let grouped = bounce(
        &mut h,
        &ranged,
        BounceTarget::AudioGroup {
            clips: vec![clip],
            range: Some(crop),
        },
        true,
    );
    assert!(grouped.error.is_none(), "{:?}", grouped.error);
    (_, _) = h.applied(applied);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(std::sync::atomic::Ordering::Acquire),
        "transport keeps running through the bounce"
    );
    let asset = added_asset(&ranged.state.project, &grouped.state.project);
    let file = grouped
        .state
        .project
        .asset(asset)
        .unwrap()
        .path()
        .to_path_buf();
    assert_matches(&decoded(&file), &source[6_000..9_000], "grouped crop");

    // The same original with different bytes: verification refuses explicitly
    // and the project is untouched.
    ramp(&path, 11);
    let range = TimeRange::new(vec![track], 1_000, 5_000).unwrap();
    let refused = bounce(&mut h, &grouped, BounceTarget::Range(range), false);
    let error = refused.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("changed"),
        "changed media must refuse: {error:?}"
    );
    assert_eq!(refused.state.project, grouped.state.project);
    assert_eq!(refused.state.revision, grouped.state.revision);
}

#[test]
fn long_streamed_repitch_bounce_matches_resident_prefix() {
    long_warp_bounce(hex_project::WarpMode::RePitch);
}
#[test]
fn long_streamed_tones_bounce_matches_resident_prefix() {
    long_warp_bounce(hex_project::WarpMode::Tones { grain_ms: 32 });
}
#[test]
fn long_streamed_texture_bounce_matches_resident_prefix() {
    long_warp_bounce(hex_project::WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
}
fn long_warp_bounce(mode: hex_project::WarpMode) {
    use std::io::Write;
    let _native = native_fixture();
    let files = Files::new();
    fn source(path: &Path, frames: u32) {
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
    fn render(path: &Path, streamed: bool, mode: hex_project::WarpMode) -> Vec<hex_engine::Frame> {
        let mut h = ResidentHarness::with_frame_limit(65_536);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::ImportToNewTrack {
            path: path.to_path_buf(),
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
        let track = imported.state.project.tracks()[0].id();
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = h.run(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }));
        (applied, _) = h.applied(applied);
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            mode,
        )
        .unwrap();
        h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }));
        (applied, _) = h.applied(applied);
        let current = h.run(Request::Edit(hex_project::Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(240_000_000).unwrap(),
        }));
        (applied, _) = h.applied(applied);
        let result = bounce(
            &mut h,
            &current,
            BounceTarget::Range(TimeRange::new(vec![track], 128, 640).unwrap()),
            false,
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        let asset = added_asset(&current.state.project, &result.state.project);
        let rendered = decoded(result.state.project.asset(asset).unwrap().path());
        if streamed {
            h.applied(applied);
            fs::write(path, b"changed source").unwrap();
            let refused = bounce(
                &mut h,
                &result,
                BounceTarget::Range(TimeRange::new(vec![track], 128, 640).unwrap()),
                false,
            );
            assert!(refused.error.is_some(), "changed source must refuse");
            assert_eq!(refused.state.project, result.state.project);
            assert_eq!(refused.state.revision, result.state.revision);
        }
        rendered
    }
    let short = files.0.join("short.wav");
    let long = files.0.join("long.wav");
    source(&short, 8192);
    source(&long, 20_000_000);
    let expected = render(&short, false, mode);
    let actual = render(&long, true, mode);
    assert_matches(&actual, &expected, "warp bounce");
}

#[test]
fn long_streamed_pitch_fade_bounce_matches_resident_prefix() {
    use std::io::Write;
    let _native = native_fixture();
    let files = Files::new();
    fn source(path: &Path, frames: u32) {
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
    fn render(path: &Path, streamed: bool) -> Vec<hex_engine::Frame> {
        let mut h = ResidentHarness::with_frame_limit(65_536);
        h.next();
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::ImportToNewTrack {
            path: path.to_path_buf(),
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
        let track = imported.state.project.tracks()[0].id();
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        h.run(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 2048,
            fade_out: 0,
        }));
        (applied, _) = h.applied(applied);
        let current = h.run(Request::Edit(hex_project::Command::SetAudioClipPitch {
            clip,
            cents: 700,
        }));
        (applied, _) = h.applied(applied);
        let result = bounce(
            &mut h,
            &current,
            BounceTarget::Range(TimeRange::new(vec![track], 128, 640).unwrap()),
            false,
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        let asset = added_asset(&current.state.project, &result.state.project);
        let rendered = decoded(result.state.project.asset(asset).unwrap().path());
        if streamed {
            h.applied(applied);
            fs::write(path, b"changed source").unwrap();
            let refused = bounce(
                &mut h,
                &result,
                BounceTarget::Range(TimeRange::new(vec![track], 128, 640).unwrap()),
                false,
            );
            assert!(refused.error.is_some(), "changed source must refuse");
            assert_eq!(refused.state.project, result.state.project);
            assert_eq!(refused.state.revision, result.state.revision);
        }
        rendered
    }
    let short = files.0.join("short.wav");
    let long = files.0.join("long.wav");
    source(&short, 8192);
    source(&long, 20_000_000);
    let expected = render(&short, false);
    let actual = render(&long, true);
    assert_matches(&actual, &expected, "pitch and fade bounce");
}
