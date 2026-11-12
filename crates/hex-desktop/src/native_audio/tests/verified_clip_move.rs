use super::*;
use hex_project::{ClipGroupPosition, Command};
use hex_session::service::{
    ArrangementMoveAudition, ArrangementMoveDestination, ArrangementMoveMode,
};

#[test]
fn verified_worker_delivers_metadata_and_exact_terminal_without_a_callback_claim() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = Harness::new();
    let initial = h.next();
    let wrong_source = Arc::clone(initial.state.canonical_audio_source.as_ref().unwrap());
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let imported = h.run(Request::Import(files.wav(48_000, 128)));
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let event = h.run(Request::Edit(Command::AddTrack {
        name: "destination".into(),
    }));
    let canonical = event.state.canonical_audio_source.as_ref().unwrap();
    let before = Arc::clone(&event.state.project);
    let mut target = ArrangementMoveAudition {
        context: canonical.context(),
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(event.state.project.tracks()[1].id()),
        position: ClipGroupPosition::Audio(32),
        mode: ArrangementMoveMode::Move,
    };
    use hex_session::service::{VerifiedClipMoveSource, VerifiedClipMoveUpdate};
    let mut control = super::super::verified_clip_move::VerifiedClipControl::default();
    let first =
        Arc::new(VerifiedClipMoveSource::capture(Arc::clone(canonical), target.clone()).unwrap());
    assert!(!control.receive(
        Some(&wrong_source),
        VerifiedClipMoveUpdate::Preview(Arc::clone(&first))
    ));
    assert!(control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Preview(Arc::clone(&first))
    ));
    assert!(!control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Preview(Arc::clone(&first))
    ));
    let mut newer = target.clone();
    newer.sequence = 2;
    newer.position = ClipGroupPosition::Audio(64);
    let second =
        Arc::new(VerifiedClipMoveSource::capture(Arc::clone(canonical), newer.clone()).unwrap());
    assert!(control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Preview(Arc::clone(&second))
    ));
    assert!(!control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Cancelled {
            canonical: Arc::clone(canonical),
            identity: first.identity()
        }
    ));
    assert!(control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Released {
            canonical: Arc::clone(canonical),
            identity: newer.identity()
        }
    ));
    assert!(!control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Preview(Arc::clone(&second))
    ));
    assert!(control.receive(
        Some(canonical),
        VerifiedClipMoveUpdate::Cancelled {
            canonical: Arc::clone(canonical),
            identity: newer.identity()
        }
    ));
    assert!(!control.receive(Some(canonical), VerifiedClipMoveUpdate::Preview(first)));
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .observation
        .lock()
        .unwrap()
        .verified_clip_updates
        .is_empty()
    {
        assert!(Instant::now() < deadline, "clip intent was not delivered");
        std::thread::yield_now();
    }
    assert_eq!(
        h.observation.lock().unwrap().verified_clip_updates[0],
        (target.identity(), "preview")
    );
    target.sequence = 2;
    target.position = ClipGroupPosition::Audio(64);
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_move_audition();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h.observation.lock().unwrap().verified_clip_updates.len() < 2 {
        assert!(Instant::now() < deadline, "clip release was not delivered");
        std::thread::yield_now();
    }
    assert_eq!(
        h.observation.lock().unwrap().verified_clip_updates[1],
        (target.identity(), "released")
    );
    h.service
        .as_ref()
        .unwrap()
        .retire_arrangement_move_audition();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h.observation.lock().unwrap().verified_clip_updates.len() < 3 {
        assert!(
            Instant::now() < deadline,
            "clip cancellation was not delivered"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        h.observation.lock().unwrap().verified_clip_updates[2].1,
        "cancelled"
    );
    assert_eq!(event.state.project.as_ref(), before.as_ref());
    assert!(h.service.as_ref().unwrap().try_event().is_err());
}

#[test]
fn verified_control_accepts_worker_proven_comparison_authoring_revision() {
    use hex_project::{DeviceComparisonAction, DeviceVariant};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = Harness::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let imported = h.run(Request::Import(files.wav(48_000, 128)));
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let inserted = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.utility",
        ))
        .unwrap(),
    }));
    let device = inserted.state.project.tracks()[0].devices()[0].id();
    let source = Arc::clone(inserted.state.canonical_audio_source.as_ref().unwrap());
    let mut target = ArrangementMoveAudition {
        context: source.context(),
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(32),
        mode: ArrangementMoveMode::Move,
    };
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .observation
        .lock()
        .unwrap()
        .verified_clip_updates
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "initial preview was not delivered"
        );
        std::thread::yield_now();
    }
    let changed = h.run(Request::CompareDevice {
        owner: hex_session::service::preset::Owner {
            project_generation: source.context().project_generation,
            revision: inserted.state.revision,
            device,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: hex_engine::DeviceTypeId::from_static("hex.utility"),
        },
        expected: DeviceVariant::A,
        action: DeviceComparisonAction::Switch,
    });
    assert!(Arc::ptr_eq(
        changed.state.canonical_audio_source.as_ref().unwrap(),
        &source
    ));
    target.context = hex_engine::AudioSourceContext {
        project_generation: source.context().project_generation,
        editor_revision: changed.state.revision,
    };
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .observation
        .lock()
        .unwrap()
        .verified_clip_updates
        .iter()
        .all(|update| *update != (target.identity(), "preview"))
    {
        assert!(
            Instant::now() < deadline,
            "comparison preview was not delivered"
        );
        std::thread::yield_now();
    }
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_move_audition();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .observation
        .lock()
        .unwrap()
        .verified_clip_updates
        .iter()
        .all(|update| *update != (target.identity(), "released"))
    {
        assert!(
            Instant::now() < deadline,
            "comparison release was not delivered"
        );
        std::thread::yield_now();
    }
    assert!(h.service.as_ref().unwrap().try_event().is_err());
}
