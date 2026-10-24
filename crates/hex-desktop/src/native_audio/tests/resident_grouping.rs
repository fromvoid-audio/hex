//! Grouping through the production Session/native publication path. Only output
//! device creation is substituted; callback allocation probes stay enabled.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, Created};

fn replay(h: &mut ResidentHarness) -> Vec<hex_engine::Frame> {
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let pcm = (0..4).flat_map(|_| h.render()).collect();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();
    pcm
}
#[test]
fn resident_grouping_publishes_exact_pcm_with_atomic_refusal_history_and_reopen() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let first_path = files.0.join("first.wav");
    fs::rename(files.wav(48_000, 64), &first_path).unwrap();
    let first = h.run(Request::Import(first_path));
    (applied, _) = h.applied(applied);
    let track = first.state.project.tracks()[0].id();
    let first_clip = first.state.project.tracks()[0].clips()[0].id();
    let second = h.run(Request::ImportToTrack {
        path: files.wav(48_000, 48),
        track,
        start: 96,
    });
    (applied, _) = h.applied(applied);
    let second_clip = second.state.project.track(track).unwrap().clips()[1].id();
    let extra_path = files.0.join("extra.wav");
    fs::rename(files.wav(48_000, 32), &extra_path).unwrap();
    h.run(Request::ImportToTrack {
        path: extra_path,
        track,
        start: 224,
    });
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::EditAudioEvents {
        clip: first_clip,
        operation: hex_project::AudioEventEdit::Delete { start: 11, end: 23 },
    }));
    (applied, _) = h.applied(applied);
    let before = h.run(Request::Edit(Command::SetAudioClipGain {
        clip: second_clip,
        millidb: -3_111,
    }));
    (applied, _) = h.applied(applied);
    let expected = replay(&mut h);
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(&expected[64..96], &[[0.0; 2]; 32]);
    let request = || Request::GroupClips {
        project_generation: before
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: before.state.revision,
        clips: vec![first_clip, second_clip],
    };
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(request()).unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert_eq!(refused.state.project, before.state.project);
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.can_undo, before.state.can_undo);
    assert_eq!(replay(&mut h), expected);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let grouped = h.run(request());
    (applied, _) = h.applied(applied);
    assert!(matches!(grouped.created, Some(Created::Clip(_))));
    assert_eq!(grouped.state.project.track(track).unwrap().clips().len(), 2);
    assert_eq!(replay(&mut h), expected);
    let undone = h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(undone.state.project, before.state.project);
    assert_eq!(replay(&mut h), expected);
    let redone = h.run(Request::Redo);
    (applied, _) = h.applied(applied);
    assert_eq!(redone.state.project, grouped.state.project);
    assert_eq!(replay(&mut h), expected);
    let saved = files.0.join("grouped.hex");
    h.run(Request::SaveNew(saved.clone()));
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved));
    assert!(reopened.open_complete && reopened.project_replaced);
    assert_eq!(reopened.state.project, redone.state.project);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    h.applied(0);
    assert_eq!(replay(&mut h), expected);
}

#[test]
fn resident_sampler_and_grouped_audio_retain_every_media_root_and_pcm() {
    use hex_project::BeatTime;
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let path = files.0.join("first.wav");
    fs::rename(files.wav(48_000, 64), &path).unwrap();
    let first = h.run(Request::Import(path));
    (applied, _) = h.applied(applied);
    let audio_track = first.state.project.tracks()[0].id();
    let clip = first.state.project.track(audio_track).unwrap().clips()[0].id();
    let path = files.0.join("second.wav");
    fs::rename(files.wav(48_000, 48), &path).unwrap();
    let second = h.run(Request::ImportToTrack {
        path,
        track: audio_track,
        start: 96,
    });
    (applied, _) = h.applied(applied);
    let second_clip = second.state.project.track(audio_track).unwrap().clips()[1].id();
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    (applied, _) = h.applied(applied);
    let Some(Created::Track(sampler_track)) = added.created else {
        panic!("track")
    };
    let path = files.0.join("sampler.wav");
    fs::rename(files.wav(8_000, 64), &path).unwrap();
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track: sampler_track,
        empty_sampler: None,
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    assert!(
        loaded
            .state
            .project
            .track(sampler_track)
            .unwrap()
            .instrument()
            .is_some()
    );
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track: sampler_track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    (applied, _) = h.applied(applied);
    let Some(Created::Clip(midi)) = placed.created else {
        panic!("midi")
    };
    let before = h.run(Request::Edit(Command::AddMidiNote {
        clip: midi,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let expected = replay(&mut h);
    assert!(
        expected[160..].iter().any(|frame| *frame != [0.0; 2]),
        "original-rate sampler remains audible after the grouped audio ends"
    );
    let group = h.run(Request::GroupClips {
        project_generation: 0,
        revision: before.state.revision,
        clips: vec![clip, second_clip],
    });
    (applied, _) = h.applied(applied);
    assert_eq!(
        group
            .state
            .project
            .track(audio_track)
            .unwrap()
            .clips()
            .len(),
        1
    );
    assert_eq!(replay(&mut h), expected);
    h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    assert_eq!(replay(&mut h), expected);
    h.run(Request::Redo);
    h.applied(applied);
    assert_eq!(replay(&mut h), expected);
}
