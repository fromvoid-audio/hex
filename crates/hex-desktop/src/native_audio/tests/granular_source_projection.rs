//! Real sealed native raw domain cannot authorize a granular derivative.
use super::*;
fn receive(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match service.try_event() {
            Ok(event) => {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(error) => panic!("service closed: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn sealed_raw_domain_does_not_cover_granular_snapshot() {
    let _native = native_fixture();
    let files = Files::new();
    let service = Service::new(48_000, 16, 0).unwrap();
    receive(&service);
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    receive(&service);
    service
        .submit(Request::Import(files.wav(48_000, 8192)))
        .unwrap();
    let imported = receive(&service);
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        }))
        .unwrap();
    let neutral = receive(&service);
    let plain = neutral.state.canonical_audio_source.as_ref().unwrap();
    let raw = DomainSources::capture(plain, 11).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error) if error == "audio output is open or still shutting down" => {}
            Err(error) => panic!("fixture unavailable: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    };
    let policy = AudioPolicy::new().unwrap();
    let control = output.publication.control().unwrap();
    policy.configure(control).unwrap();
    let token = control
        .begin_audio_candidate_domain_preparation(
            policy.stream,
            raw.definitions_for_snapshot(plain).unwrap(),
            policy.readiness,
        )
        .unwrap();
    loop {
        match control.poll_audio_candidate_domain_preparation(token) {
            Ok(hex_audio::AudioCandidateDomainPreparationProgress::Ready) => break,
            Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying) => {}
            other => panic!("raw domain did not seal: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    // Receive the published endpoint through the real data carrier before
    // asking Root to retire it; an unread endpoint still has live custody.
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    assert!(raw.covers(plain, token, control));
    let warp = hex_project::default_audio_warp(
        neutral.state.project.clip(clip).unwrap(),
        neutral.state.project.tempo_map(),
        hex_project::WarpMode::Tones { grain_ms: 32 },
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        }))
        .unwrap();
    let edited = receive(&service);
    let snapshot = edited.state.canonical_audio_source.as_ref().unwrap();
    let granular = DomainSources::capture(snapshot, 11).unwrap();
    assert!(
        !granular.covers(snapshot, token, control),
        "actual raw catalog has no granular worker source"
    );
    assert!(control.retire_audio_candidate_domain(token));
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        control.reclaim();
        if control.recover_audio_candidate_domain(token).is_some() {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
