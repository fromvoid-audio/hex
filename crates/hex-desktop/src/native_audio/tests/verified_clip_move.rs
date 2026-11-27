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

#[test]
fn verified_pure_streaming_held_clip_move_auditions_cancels_commits_and_undoes_without_callback_allocations()
 {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = super::verified_gain::actual_native_harness();
    h.next();
    h.wait = Duration::from_secs(3);
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());

    let path = files.wav(48_000, 96_000);
    let original_bytes = fs::read(&path).unwrap();
    let imported = h.run(Request::Import(path.clone()));
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let added = h.run(Request::Edit(Command::AddTrack {
        name: "destination".into(),
    }));
    let destination_track = added.state.project.tracks()[1].id();
    let looped = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: 8192,
        enabled: true,
    }));
    let canonical = looped.state.canonical_audio_source.as_ref().unwrap();
    assert!(matches!(
        canonical.required_sources().next().unwrap().1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    let context = h.context.unwrap();
    let original_project = looped.state.project.snapshot();
    h.applied(context);
    let domain = h.domain(context).0;

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let is_loop_signal = |h: &Harness, pcm: &[[f32; 2]; 64], level: f32| {
        let end = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let start = (end + 8192 - 64) % 8192;
        pcm.iter().enumerate().all(|(offset, frame)| {
            let pos = (start + offset as u64) % 8192;
            let gain = if pos >= 8192 - 128 {
                (8191 - pos) as f32 / 127.0
            } else {
                1.0
            };
            let expected = level * gain;
            frame.iter().all(|s| (*s - expected).abs() < 1e-5)
        })
    };

    let expect_level = |h: &Harness, level: f32, expected_identity: Option<_>| {
        let until = Instant::now() + h.wait;
        let mut saw_identity = expected_identity.is_none();
        loop {
            for event in h.events.try_iter() {
                match event {
                    EditorAudioEvent::Publication {
                        state: EditorPublicationState::VerifiedClipMoveSubmitted { identity, .. },
                        ..
                    } if expected_identity == Some(identity) => {
                        saw_identity = true;
                    }
                    EditorAudioEvent::Publication {
                        state:
                            EditorPublicationState::Refused(error)
                            | EditorPublicationState::VerifiedClipMoveRefused(error)
                            | EditorPublicationState::Fault(error),
                        ..
                    } => panic!("{error}"),
                    EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
            let pcm = h.render();
            if saw_identity && is_loop_signal(h, &pcm, level) {
                return;
            }
            assert!(
                Instant::now() < until,
                "expected level {level} (saw_identity={saw_identity}), heard {:?}",
                pcm[63]
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    expect_level(&h, 0.25, None);

    let mut target = ArrangementMoveAudition {
        context,
        gesture: 1,
        sequence: 1,
        clips: vec![clip].into(),
        anchor: clip,
        destination: ArrangementMoveDestination::Existing(destination_track),
        position: ClipGroupPosition::Audio(48_000),
        mode: ArrangementMoveMode::Move,
    };
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    expect_level(&h, 0.0, Some(target.identity()));
    assert_eq!(h.domain(context).0, domain);
    assert!(h.service.as_ref().unwrap().try_event().is_err());

    let measured = allocation_counter::measure(|| {
        for _ in 0..4 {
            let pcm = h.render();
            assert!(pcm.iter().all(|f| *f == [0.0; 2]));
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert_eq!(measured.bytes_total, 0);
    assert_eq!(measured.bytes_current, 0);

    // Coalesce rapid retargets while held back into the loop window on the destination track.
    for (sequence, position) in [(2, 24_000), (3, 48_000), (4, 0)] {
        target.sequence = sequence;
        target.position = ClipGroupPosition::Audio(position);
        assert!(
            h.service
                .as_ref()
                .unwrap()
                .audition_arrangement_move(Some(target.clone()))
        );
    }
    expect_level(&h, 0.25, Some(target.identity()));
    assert_eq!(h.domain(context).0, domain);

    // Move out of loop window again, then cancel (Escape) and verify canonical audio returns.
    target.sequence = 5;
    target.position = ClipGroupPosition::Audio(48_000);
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(target.clone()))
    );
    expect_level(&h, 0.0, Some(target.identity()));
    assert!(!h.service.as_ref().unwrap().audition_arrangement_move(None));
    expect_level(&h, 0.25, None);
    assert_eq!(h.domain(context).0, domain);

    // Second gesture: audition move to 48_000, hold through release, commit once, then undo.
    let commit_target = ArrangementMoveAudition {
        gesture: 2,
        sequence: 1,
        destination: ArrangementMoveDestination::Existing(track),
        position: ClipGroupPosition::Audio(48_000),
        ..target
    };
    assert!(
        h.service
            .as_ref()
            .unwrap()
            .audition_arrangement_move(Some(commit_target.clone()))
    );
    expect_level(&h, 0.0, Some(commit_target.identity()));
    h.service
        .as_ref()
        .unwrap()
        .finish_arrangement_move_audition();
    let release_wait = Instant::now() + Duration::from_millis(45);
    while Instant::now() < release_wait {
        assert!(h.render().iter().all(|f| *f == [0.0; 2]));
        std::thread::sleep(Duration::from_millis(1));
    }
    let committed = h.run(Request::MoveExistingClips {
        project_generation: context.project_generation,
        revision: context.editor_revision,
        command: Command::MoveClipWithOverwrite {
            clip,
            track,
            start: 48_000,
        },
    });
    assert!(committed.error.is_none());
    h.service
        .as_ref()
        .unwrap()
        .retire_arrangement_move_audition();
    h.applied(h.context.unwrap());
    expect_level(&h, 0.0, None);

    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project.snapshot(), original_project);
    h.applied(h.context.unwrap());
    expect_level(&h, 0.25, None);
    assert_eq!(fs::read(path).unwrap(), original_bytes);
}
