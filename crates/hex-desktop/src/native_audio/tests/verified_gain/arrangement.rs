//! Live gains run through the genuine verified catalog, driver and callback.
use super::*;
use hex_project::{ClipGain, ClipId, Command};
use hex_session::service::{ArrangementGainAudition, GainEdit};

#[test]
fn verified_arrangement_gain_auditions_cancels_commits_and_undoes() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = open_verified();
    let path = files.wav(48_000, 100_000);
    let original = fs::read(&path).unwrap();
    let imported = h.run(Request::Import(path.clone()));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    // A supported transformed clip must keep using the verified raw source
    // with its authored reverse read direction.
    let base = h.run(Request::Edit(Command::ReverseClips(vec![clip])));
    let context = h.context.unwrap();
    assert!(matches!(
        base.state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    h.applied(context);
    let domain = h.domain(context).0;
    start_playing(&mut h);
    expect_gain(&h, ClipGain::UNITY);

    let dip = ClipGain::new(-12_000).unwrap();
    audition(&h, clip, dip);
    expect_gain(&h, dip);
    let half = ClipGain::new(-6_000).unwrap();
    for value in [dip, ClipGain::new(-18_000).unwrap(), half] {
        audition(&h, clip, value);
    }
    expect_gain(&h, half);
    assert_eq!(h.domain(context).0, domain);
    h.service.as_ref().unwrap().audition_arrangement_gain(None);
    expect_gain(&h, ClipGain::UNITY);

    audition(&h, clip, half);
    expect_gain(&h, half);
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_gain_audition();
    let release_wait = Instant::now() + Duration::from_millis(45);
    while Instant::now() < release_wait {
        assert!(matches_gain(h.render()[63], half));
        std::thread::sleep(Duration::from_millis(1));
    }
    let committed = h.run(Request::Edit(Command::SetAudioClipGain {
        clip,
        millidb: half.millidb(),
    }));
    assert_eq!(committed.state.project.clip(clip).unwrap().gain(), half);
    h.applied(h.context.unwrap());
    expect_gain(&h, half);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, base.state.project);
    h.applied(h.context.unwrap());
    expect_gain(&h, ClipGain::UNITY);
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn verified_arrangement_play_start_auditions_cancels_commits_and_undoes() {
    use hex_project::Command;
    use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};
    use std::io::{Seek, SeekFrom, Write};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = open_verified();
    let segment = 96_000_u32;
    let path = files.constant_wav("verified-play-start.wav", 48_000, segment * 3, &[8192]);
    {
        let mut wav = fs::OpenOptions::new().write(true).open(&path).unwrap();
        let seg_bytes = i64::from(segment) * size_of::<i16>() as i64;
        wav.seek(SeekFrom::End(-seg_bytes * 2)).unwrap();
        wav.write_all(&4096_i16.to_le_bytes().repeat(segment as usize))
            .unwrap();
        wav.write_all(&2048_i16.to_le_bytes().repeat(segment as usize))
            .unwrap();
    }
    let original = fs::read(&path).unwrap();

    let imported = h.run(Request::Import(path.clone()));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    assert!(matches!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    h.run(Request::Edit(Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    let base = h.run(Request::Edit(Command::TrimClip {
        clip,
        start: 0,
        duration: u64::from(segment),
        source_offset: 0,
        source_length: u64::from(segment * 3),
    }));
    let context = h.context.unwrap();
    h.applied(context);
    let domain = h.domain(context).0;
    start_playing(&mut h);
    expect_pcm_level(&h, 0.25);

    // 1. Plain streamed TrimClip (AudioTrimStart): coalesce rapid slips without decoding behind catalog.
    for offset in [segment / 2, segment, segment * 2] {
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
                context,
                edit: PlayStartEdit::AudioTrimStart {
                    clip,
                    start: 0,
                    duration: u64::from(segment),
                    source_offset: u64::from(offset),
                    source_length: u64::from(segment * 3 - offset),
                },
            }));
    }
    expect_pcm_level(&h, 0.0625);
    assert_eq!(h.domain(context).0, domain);
    assert!(h.service.as_ref().unwrap().try_event().is_err());

    // Cancel restores 0.25.
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(None);
    expect_pcm_level(&h, 0.25);
    assert_eq!(h.domain(context).0, domain);

    // Release + commit + undo.
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context,
            edit: PlayStartEdit::AudioTrimStart {
                clip,
                start: 0,
                duration: u64::from(segment),
                source_offset: u64::from(segment * 2),
                source_length: u64::from(segment),
            },
        }));
    expect_pcm_level(&h, 0.0625);
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_play_start_audition();
    let release_wait = Instant::now() + Duration::from_millis(45);
    while Instant::now() < release_wait {
        assert!(
            h.render()[63]
                .into_iter()
                .all(|s| (s - 0.0625).abs() < 1e-6)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let committed = h.run(Request::Edit(Command::TrimClip {
        clip,
        start: 0,
        duration: u64::from(segment),
        source_offset: u64::from(segment * 2),
        source_length: u64::from(segment),
    }));
    assert_eq!(
        committed.state.project.clip(clip).unwrap().source_offset(),
        u64::from(segment * 2)
    );
    h.service
        .as_ref()
        .unwrap()
        .retire_arrangement_play_start_audition();
    h.applied(h.context.unwrap());
    expect_pcm_level(&h, 0.0625);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, base.state.project);
    h.applied(h.context.unwrap());
    expect_pcm_level(&h, 0.25);

    // 2. Looped SetClipSourcePhase (AudioSourcePhase) in VerifiedStreaming.
    let looped = h.run(Request::Edit(Command::SetClipLooping {
        clip,
        looping: true,
    }));
    let looped_context = h.context.unwrap();
    h.applied(looped_context);
    let looped_domain = h.domain(looped_context).0;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Seek(0))
        .unwrap();
    expect_pcm_level(&h, 0.25);

    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context: looped_context,
            edit: PlayStartEdit::AudioSourcePhase {
                clip,
                phase: u64::from(segment),
            },
        }));
    expect_pcm_level(&h, 0.125);
    assert_eq!(h.domain(looped_context).0, looped_domain);

    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(None);
    expect_pcm_level(&h, 0.25);

    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context: looped_context,
            edit: PlayStartEdit::AudioSourcePhase {
                clip,
                phase: u64::from(segment * 2),
            },
        }));
    expect_pcm_level(&h, 0.0625);
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_play_start_audition();
    let phase_committed = h.run(Request::Edit(Command::SetClipSourcePhase {
        clip,
        phase: u64::from(segment * 2),
    }));
    assert_eq!(
        phase_committed
            .state
            .project
            .clip(clip)
            .unwrap()
            .source_phase(),
        u64::from(segment * 2)
    );
    h.service
        .as_ref()
        .unwrap()
        .retire_arrangement_play_start_audition();
    h.applied(h.context.unwrap());
    expect_pcm_level(&h, 0.0625);
    let phase_undone = h.run(Request::Undo);
    assert_eq!(phase_undone.state.project, looped.state.project);
    h.applied(h.context.unwrap());
    expect_pcm_level(&h, 0.25);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn verified_arrangement_play_start_refuses_transformed_streamed_clips_without_interrupting_playback()
 {
    use hex_project::{BeatTime, Command, WarpMarker, WarpMode, WarpSettings};
    use hex_session::service::{ArrangementPlayStartAudition, PlayStartEdit};
    use std::sync::atomic::Ordering::Acquire;

    let _native = native_fixture();
    let files = Files::new();
    let mut h = open_verified();
    let segment = 96_000_u32;
    let path = files.wav(48_000, segment * 2);
    let imported = h.run(Request::Import(path));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    h.run(Request::Edit(Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    let context = h.context.unwrap();
    let canonical = h.applied(context);
    let domain = h.domain(context).0;
    start_playing(&mut h);
    expect_pcm_level(&h, 0.25);

    // Case 1: Placed audio_edit on a plain VerifiedFile clip is refused cleanly.
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context,
            edit: PlayStartEdit::Audio {
                clip,
                position: u64::from(segment),
            },
        }));
    let wait_until = Instant::now() + Duration::from_millis(60);
    while Instant::now() < wait_until {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(!status.render_fault.load(Acquire));
        assert_eq!(status.applied_audio_source(), Some(canonical));
        assert!(pcm.into_iter().flatten().all(|s| (s - 0.25).abs() < 1e-6));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(h.domain(context).0, domain);
    assert!(h.service.as_ref().unwrap().try_event().is_err());
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(None);

    // Case 2: Warped VerifiedFile clip refuses AudioTrimStart without interrupting playback.
    let warped = h.run(Request::Edit(Command::SetAudioClipWarp {
        clip,
        settings: Some(WarpSettings {
            mode: WarpMode::RePitch,
            markers: vec![
                WarpMarker {
                    source_frame: 0,
                    beat: BeatTime::ZERO,
                },
                WarpMarker {
                    source_frame: u64::from(segment * 2),
                    beat: BeatTime::from_quarters(8).unwrap(),
                },
            ],
        }),
    }));
    let warped_context = h.context.unwrap();
    let warped_canonical = h.applied(warped_context);
    let warped_domain = h.domain(warped_context).0;
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(Some(ArrangementPlayStartAudition {
            context: warped_context,
            edit: PlayStartEdit::AudioTrimStart {
                clip,
                start: 0,
                duration: u64::from(segment * 2),
                source_offset: u64::from(segment),
                source_length: u64::from(segment),
            },
        }));
    let wait_until = Instant::now() + Duration::from_millis(60);
    while Instant::now() < wait_until {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!(!status.render_fault.load(Acquire));
        assert_eq!(status.applied_audio_source(), Some(warped_canonical));
        assert!(pcm.into_iter().flatten().all(|s| (s - 0.25).abs() < 1e-6));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(h.domain(warped_context).0, warped_domain);
    assert!(h.service.as_ref().unwrap().try_event().is_err());
    assert_eq!(warped.state.project.clip(clip).unwrap().source_offset(), 0);
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_play_start(None);
}

#[test]
fn verified_graph_edit_refuses_the_retired_gain_preview_until_canonical_audio_installs() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = open_verified();
    let imported = h.run(Request::Import(files.wav(48_000, 100_000)));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let context = h.context.unwrap();
    let canonical = h.applied(context);
    start_playing(&mut h);
    expect_gain(&h, ClipGain::UNITY);

    let preview_gain = ClipGain::new(-12_000).unwrap();
    audition(&h, clip, preview_gain);
    expect_gain(&h, preview_gain);
    let applied = |h: &Harness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
    };
    let preview = applied(&h).unwrap();
    assert_eq!(preview.context, canonical.context);
    assert_ne!(preview.audio_revision, canonical.audio_revision);

    // Wait for the worker's canonical restoration submission without rendering
    // the callback. The old preview still owns the applied audio revision.
    h.service.as_ref().unwrap().audition_arrangement_gain(None);
    let deadline = Instant::now() + h.wait;
    let restoration = loop {
        if let Ok(EditorAudioEvent::Publication {
            context: event_context,
            output_identity,
            state: EditorPublicationState::Submitted { audio_revision },
        }) = h.events.try_recv()
            && event_context == context
            && output_identity == canonical.output_identity
            && audio_revision > preview.audio_revision
        {
            break audio_revision;
        }
        assert!(
            Instant::now() < deadline,
            "the canonical source was not queued again"
        );
        std::thread::yield_now();
    };
    assert_eq!(applied(&h), Some(preview));

    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    let command = Command::InsertMainDevice {
        before: None,
        state,
    };
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::Edit(command.clone()))
        .unwrap();
    let refused = h.next();
    assert!(
        refused.request_complete,
        "the preview must not become an edit predecessor"
    );
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, imported.state.project);
    assert_eq!(refused.state.revision, imported.state.revision);
    assert_eq!(applied(&h), Some(preview));

    expect_gain(&h, ClipGain::UNITY);
    let restored = applied(&h).unwrap();
    assert_eq!(restored.context, canonical.context);
    assert_eq!(restored.output_identity, canonical.output_identity);
    assert_eq!(restored.audio_revision, restoration);
    let committed = h.run_verified_device_insert(command);
    assert_eq!(committed.state.project.main_devices().len(), 1);
    assert_eq!(committed.state.revision, imported.state.revision + 1);
}

#[test]
fn verified_arrangement_gain_is_retired_on_output_and_document_replacement() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = open_verified();
    let path = files.wav(48_000, 100_000);
    let before = h.run(Request::Import(path.clone()));
    let old_context = h.context.unwrap();
    let clip = before.state.project.tracks()[0].clips()[0].id();
    h.applied(old_context);
    start_playing(&mut h);
    let preview = ClipGain::new(-24_000).unwrap();
    audition(&h, clip, preview);
    expect_gain(&h, preview);
    let first_output = h.audio.as_ref().unwrap().control.output_identity();
    h.run(Request::RestartAudio {
        project_generation: old_context.project_generation,
        request_id: 1,
    });
    h.audio = None;
    h.backend.lock().unwrap().take();
    h.open();
    let restarted = h.audio.as_ref().unwrap().control.output_identity();
    assert_ne!(restarted, first_output);
    h.applied(h.context.unwrap());
    start_playing(&mut h);
    expect_gain(&h, ClipGain::UNITY);
    audition(&h, clip, preview);
    expect_gain(&h, preview);

    h.run(new_verified_project());
    h.open();
    let next = h.run(Request::Import(path));
    let next_context = h.context.unwrap();
    assert_ne!(
        old_context.project_generation,
        next_context.project_generation
    );
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        restarted
    );
    assert_eq!(
        next.state.project.tracks()[0].clips()[0].gain(),
        ClipGain::UNITY
    );
    h.applied(next_context);
    start_playing(&mut h);
    expect_gain(&h, ClipGain::UNITY);
}

fn new_verified_project() -> Request {
    Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    }
}

fn open_verified() -> Harness {
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(2);
    h.next();
    h.open();
    h.run(new_verified_project());
    h.open();
    h.applied(h.context.unwrap());
    h
}

fn start_playing(h: &mut Harness) {
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
}

fn audition(h: &Harness, clip: ClipId, gain: ClipGain) {
    h.service
        .as_ref()
        .unwrap()
        .audition_arrangement_gain(Some(ArrangementGainAudition {
            context: h.context.unwrap(),
            edit: GainEdit::Clip { clip, gain },
        }));
}

fn matches_gain(frame: hex_engine::Frame, gain: ClipGain) -> bool {
    frame
        .into_iter()
        .all(|sample| (sample - 0.25 * gain.linear()).abs() < 1e-6)
}

fn expect_pcm_level(h: &Harness, level: f32) {
    let until = Instant::now() + h.wait;
    loop {
        for event in h.events.try_iter() {
            match event {
                EditorAudioEvent::Publication {
                    state:
                        EditorPublicationState::Refused(error)
                        | EditorPublicationState::VerifiedArrangementGainRefused(error)
                        | EditorPublicationState::VerifiedArrangementPlayStartRefused(error),
                    ..
                } => panic!("{error}"),
                EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
        let pcm = h.render();
        if pcm
            .into_iter()
            .all(|frame| frame.into_iter().all(|s| (s - level).abs() < 1e-6))
        {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {level}, heard {:?}",
            pcm[63]
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn expect_gain(h: &Harness, gain: ClipGain) {
    let until = Instant::now() + h.wait;
    loop {
        for event in h.events.try_iter() {
            match event {
                EditorAudioEvent::Publication {
                    state:
                        EditorPublicationState::Refused(error)
                        | EditorPublicationState::VerifiedArrangementGainRefused(error),
                    ..
                } => panic!("{error}"),
                EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
        let pcm = h.render();
        if pcm.into_iter().all(|frame| matches_gain(frame, gain)) {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {gain:?}, heard {:?}",
            pcm[63]
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
