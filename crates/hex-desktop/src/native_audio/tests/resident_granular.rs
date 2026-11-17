use super::resident_preview::ResidentHarness;
use super::*;

mod transport;

fn play(h: &mut ResidentHarness) {
    play_level_at(h, 0, 0.25);
}

fn play_level_at(h: &mut ResidentHarness, frame: u64, level: f32) {
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(frame))
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        h.pump();
        let pcm = h.render();
        if pcm
            .iter()
            .flatten()
            .all(|sample| (*sample - level).abs() < 0.0001)
        {
            if frame != 0 {
                // The two rate cases have opposite signs in this window.
                // A later block at the requested level does not prove the read.
                let end = h
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .position
                    .load(Acquire);
                assert!(
                    (frame..=frame + 1_600).contains(&end),
                    "matched level {level} outside the expected read window at {end}"
                );
            }
            break;
        }
        assert!(
            Instant::now() < end,
            "expected {level} near frame {frame}, recent block started with {:?}",
            pcm[0]
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
}

#[test]
fn long_warp_off_uses_raw_pcm_and_reopens_with_authored_settings() {
    use hex_project::{Command, Tempo, WarpMode};
    use std::io::{Seek, SeekFrom, Write};
    let _native = native_fixture();
    let files = Files::new();
    let path = super::granular_playback::long_source_at_rate(&files, 48_000);
    let mut wav = fs::OpenOptions::new().write(true).open(&path).unwrap();
    wav.seek(SeekFrom::Start(44 + 12_000 * 2)).unwrap();
    wav.write_all(&(-8192_i16).to_le_bytes().repeat(12_000))
        .unwrap();
    drop(wav);
    let mut h = ResidentHarness::with_frame_limit_at_rate(16_000, 48_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path,
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    let project = &imported.state.project;
    let clip = project.tracks()[0].clips()[0].id();
    let asset = project.clip(clip).unwrap().asset();
    let settings = hex_project::default_audio_warp(
        project.clip(clip).unwrap(),
        project.tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: Some(settings.clone()),
    }));
    (applied, _) = h.applied(applied);
    let active = h.run(Request::Edit(Command::SetProjectTempo {
        tempo: Tempo::new(90_000_000).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    // 13,600 reads source frame 10,200 at 3/4 speed, before the sign change.
    play_level_at(&mut h, 13_600, 0.25);
    let inactive = h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: None,
    }));
    let raw = &inactive
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .sources()[&asset];
    let prior = &imported
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .sources()[&asset];
    let (AudioSourceBinding::VerifiedFile(raw), AudioSourceBinding::VerifiedFile(prior)) =
        (raw, prior)
    else {
        panic!("same file-backed source required");
    };
    assert_eq!(raw.fingerprint(), prior.fingerprint());
    assert_eq!(
        inactive
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .candidate_reservation_quote()
            .unwrap()
            .prepared_pcm_frames,
        0
    );
    (applied, _) = h.applied(applied);
    play_level_at(&mut h, 13_600, -0.25);
    assert_eq!(
        h.run(Request::Undo).state.project.snapshot(),
        active.state.project.snapshot()
    );
    (applied, _) = h.applied(applied);
    play_level_at(&mut h, 13_600, 0.25);
    assert_eq!(
        h.run(Request::Redo).state.project.snapshot(),
        inactive.state.project.snapshot()
    );
    h.applied(applied);

    let saved = files.0.join("inactive.hex");
    assert!(h.run(Request::SaveNew(saved.clone())).save_complete);
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved));
    assert!(reopened.open_complete);
    assert_eq!(
        reopened.state.project.snapshot(),
        inactive.state.project.snapshot()
    );
    super::resident_lifecycle::finish_replacement(&mut h, &reopened, output);
    play_level_at(&mut h, 13_600, -0.25);
    let restored = reopened
        .state
        .project
        .audio_warp_settings_for_enable(clip)
        .unwrap();
    assert_eq!(restored, settings);
    let applied = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire);
    h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: Some(restored),
    }));
    h.applied(applied);
    play_level_at(&mut h, 13_600, 0.25);
}

#[test]
fn enabling_default_warp_on_long_file_publishes_pcm_and_preserves_history() {
    let _native = native_fixture();
    let files = Files::new();
    let path = super::granular_playback::long_source_at_rate(&files, 48_000);
    let mut h = ResidentHarness::with_frame_limit_at_rate(16_000, 48_000);
    h.next();
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path,
        before_track: None,
        start: 0,
    });
    let (applied, _) = h.applied(applied);
    let source = &imported.state.project.tracks()[0].clips()[0];
    let clip = source.id();
    assert_eq!(source.source_length(), 20_000_000);
    assert!(source.retained_fade().is_neutral());
    let settings = imported
        .state
        .project
        .audio_warp_settings_for_enable(clip)
        .unwrap();
    assert!(matches!(settings.mode, hex_project::WarpMode::Tones { .. }));
    let enabled = h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
        clip,
        settings: Some(settings),
    }));
    let canonical = enabled.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(before) = &imported
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .sources()[&source.asset()]
    else {
        panic!("import must remain file-backed");
    };
    let AudioSourceBinding::VerifiedFile(after) = &canonical.sources()[&source.asset()] else {
        panic!("warping must retain the verified media");
    };
    assert_eq!(after.fingerprint(), before.fingerprint());
    assert_eq!(
        canonical
            .candidate_reservation_quote()
            .unwrap()
            .prepared_pcm_frames,
        0
    );
    let (applied, _) = h.applied(applied);
    play(&mut h);
    let undone = h.run(Request::Undo);
    assert_eq!(
        undone.state.project.snapshot(),
        imported.state.project.snapshot()
    );
    let (applied, _) = h.applied(applied);
    play(&mut h);
    let redone = h.run(Request::Redo);
    assert_eq!(
        redone.state.project.snapshot(),
        enabled.state.project.snapshot()
    );
    h.applied(applied);
    play(&mut h);
}

#[test]
fn ordinary_long_tones_file_renders_history_tempo_and_save_reopen() {
    ordinary_long_granular(hex_project::WarpMode::Tones { grain_ms: 32 });
}
#[test]
fn ordinary_long_texture_file_renders_history_tempo_and_save_reopen() {
    ordinary_long_granular(hex_project::WarpMode::Texture {
        grain_ms: 32,
        fluctuation: 35,
        seed: 42,
    });
}
fn ordinary_long_granular(mode: hex_project::WarpMode) {
    let _native = native_fixture();
    let files = Files::new();
    let path = super::granular_playback::long_converted_source(&files);
    let mut h = ResidentHarness::with_frame_limit_at_rate(16_000, 48_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path: path.clone(),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
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
    let edited = h.run(Request::Edit(hex_project::Command::SetAudioClipWarp {
        clip,
        settings: Some(warp),
    }));
    assert!(
        edited
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .all(|(_, source)| matches!(source, AudioSourceBinding::VerifiedFile(_)))
    );
    assert_eq!(
        edited
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .candidate_reservation_quote()
            .unwrap()
            .prepared_pcm_frames,
        0
    );
    (applied, _) = h.applied(applied);
    play(&mut h);
    h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    play(&mut h);
    h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    play(&mut h);
    let tempo_changed = h.run(Request::Edit(hex_project::Command::SetProjectTempo {
        tempo: hex_project::Tempo::new(90_000_000).unwrap(),
    }));
    (applied, _) = h.applied(applied);
    play(&mut h);
    let before = tempo_changed.state.project.clip(clip).unwrap();
    let end = before.duration() + 4_000_000;
    let stretched = h.run(Request::Edit(hex_project::Command::StretchAudioClipEdge {
        clip,
        edge: hex_project::TrimEdge::Right,
        frame: end,
    }));
    let after = stretched.state.project.clip(clip).unwrap();
    assert_eq!(after.duration(), end);
    assert_eq!(after.asset(), before.asset());
    assert_eq!(
        after
            .audio_edit()
            .unwrap()
            .domain
            .warp
            .as_ref()
            .unwrap()
            .mode,
        mode
    );
    h.applied(applied);
    play(&mut h);
    let saved = files.0.join("granular.hex");
    assert!(h.run(Request::SaveNew(saved.clone())).save_complete);
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved));
    assert!(reopened.open_complete);
    assert_eq!(
        reopened.state.project.snapshot(),
        stretched.state.project.snapshot()
    );
    super::resident_lifecycle::finish_replacement(&mut h, &reopened, output);
    play(&mut h);
    let before = reopened.state.project.snapshot();
    let actual = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_audio_source();
    fs::remove_file(path).unwrap();
    h.service
        .submit(Request::Edit(hex_project::Command::SetAudioClipPitch {
            clip,
            cents: 300,
        }))
        .unwrap();
    let refused = h.next();
    assert!(
        refused.error.is_some(),
        "missing granular media must refuse before commit"
    );
    assert_eq!(refused.state.project.snapshot(), before);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source(),
        actual
    );
}
