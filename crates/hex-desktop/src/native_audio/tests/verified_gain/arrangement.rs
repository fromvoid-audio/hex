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
    let base = h.run(Request::Import(path.clone()));
    let context = h.context.unwrap();
    let clip = base.state.project.tracks()[0].clips()[0].id();
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
    for _ in 0..4 {
        assert!(matches_gain(h.render()[63], half));
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
