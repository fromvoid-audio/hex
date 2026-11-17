use super::resident_preview::ResidentHarness;
use super::*;

mod transport;

fn play(h: &mut ResidentHarness) {
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        h.pump();
        let pcm = h.render();
        if pcm
            .iter()
            .flatten()
            .all(|sample| (*sample - 0.25).abs() < 0.0001)
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "ordinary granular pages did not reach the callback"
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
