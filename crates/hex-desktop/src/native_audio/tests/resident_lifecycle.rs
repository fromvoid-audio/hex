//! Real current-format document swaps retain one admitted graph through actual output teardown.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{BeatTime, Command, Created};

#[test]
fn resident_open_after_new_accepts_before_obsolete_observation_delivery() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_event_capacity(8192, Arc::new(AtomicBool::new(true)), 1);
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    h.run(Request::Edit(Command::AddTrack {
        name: "saved".into(),
    }));
    h.applied(applied);
    let path = files.0.join("backpressured.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    let new = h.run(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced);
    h.release_relinquished_output();
    // Keep New's actual Opened Client in the one-row channel. Its real
    // ResidentSubmitted observation must remain backpressured on the worker.
    // No fabricated status, sleeps-as-witness or waiting for New CURRENT.
    let end = Instant::now() + Duration::from_secs(10);
    while h.pending_observation_generation.load(Acquire) != 2 {
        assert!(
            Instant::now() < end,
            "New did not reach observation backpressure"
        );
        std::thread::yield_now();
    }
    let terminal_without_audio_delivery = |service: &Service| loop {
        if let Ok(event) = service.try_event() {
            break event;
        }
        assert!(
            Instant::now() < end,
            "Open did not produce its real terminal"
        );
        std::thread::yield_now();
    };
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(Request::Open(path.clone())).unwrap();
    let refused = terminal_without_audio_delivery(&h.service);
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert!(!refused.project_replaced);
    assert_eq!(refused.state.project, new.state.project);
    assert_eq!(refused.state.revision, new.state.revision);
    assert_eq!(h.pending_observation_generation.load(Acquire), 2);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    h.service.submit(Request::Open(path)).unwrap();
    let opened = terminal_without_audio_delivery(&h.service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.project_replaced && opened.open_complete);
    assert_eq!(opened.state.project, saved.state.project);
    // Resume UI delivery, closing the obsolete real Client, then require a
    // fresh endpoint and exact generation/revision to reach the real callback.
    h.pump();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    let (_, metadata) = h.applied(0);
    assert_eq!(metadata.context.project_generation, 2);
    assert_eq!(metadata.context.editor_revision, opened.state.revision);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

fn finish_replacement(h: &mut ResidentHarness, event: &Event, old_output: u64) -> u64 {
    assert!(event.project_replaced && event.playback.is_none());
    assert!(h.submitted.is_none_or(|(_, metadata)| {
        metadata.context.project_generation
            != event
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation
    }));
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(old_output)
    );
    // Neither pending acceptance nor closing the old publication is callback retirement. Release
    // both actual headless carriers and the old Client, then wait for the real exclusive Lease.
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    // The worker named the retired endpoint before opening the fresh one.
    let generation = event
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    assert_eq!(h.released.last(), Some(&(generation, old_output)));
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .worker_owns_graph_publication()
    );
    let (audio_revision, metadata) = h.applied(0);
    assert_eq!(
        metadata.context.project_generation,
        event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation
    );
    assert_eq!(metadata.context.editor_revision, event.state.revision);
    audio_revision
}

#[test]
fn resident_new_saved_open_and_journal_recovery_replay_exact_original_with_atomic_refusal() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .worker_owns_graph_publication()
    );
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let sampler = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let path = files.0.join("normal.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(saved.save_complete && !saved.state.unsaved);
    let persisted = hex_storage::load(&path).unwrap();
    let asset = persisted.assets().next().unwrap();
    assert!(asset.original_source().is_some());
    assert_eq!(asset.source_frames(), 8192);
    assert_eq!(persisted.track(track).unwrap().instrument(), Some(&sampler));
    let journal = files.0.join("recover.hex-journal");
    hex_storage::Journal::open(journal.clone())
        .unwrap()
        .checkpoint(&persisted)
        .unwrap();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render(), [[0.25; 2]; 64]);

    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert!(!refused.project_replaced);
    assert_eq!(refused.state.project, saved.state.project);
    assert_eq!(refused.state.revision, saved.state.revision);
    assert_eq!(
        refused
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        saved
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation
    );
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(old_output)
    );
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
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);

    let new = h.run(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::Resident,
        reservation: None,
    });
    assert_eq!(
        new.state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        1
    );
    finish_replacement(&mut h, &new, old_output);
    assert!(new.state.project.tracks().is_empty());
    assert_eq!(h.render(), [[0.0; 2]; 64]);

    let empty_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(path.clone()));
    assert!(opened.open_complete && !opened.state.unsaved);
    assert_eq!(opened.state.project.as_ref(), &persisted);
    assert_eq!(
        opened.state.project.track(track).unwrap().instrument(),
        Some(&sampler)
    );
    applied = finish_replacement(&mut h, &opened, empty_output);
    let reopened_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.service
        .submit(Request::Open(files.0.join("missing.hex")))
        .unwrap();
    let missing = h.next();
    assert!(missing.error.is_some() && !missing.project_replaced);
    assert_eq!(missing.state.project, opened.state.project);
    assert_eq!(missing.state.revision, opened.state.revision);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(reopened_output)
    );
    assert_eq!(h.render(), [[0.25; 2]; 64]);

    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service.submit(Request::Open(path)).unwrap();
    let refused_open = h.next();
    assert!(
        refused_open
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert_eq!(refused_open.state.project, opened.state.project);
    assert_eq!(refused_open.state.revision, opened.state.revision);
    assert_eq!(refused_open.state.document_path, opened.state.document_path);
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
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);

    let recovered = h.run(Request::OpenWith {
        source: hex_session::service::ProjectSource::Journal(journal),
        relinks: Default::default(),
    });
    assert!(recovered.state.unsaved);
    assert_eq!(recovered.state.project.as_ref(), &persisted);
    finish_replacement(&mut h, &recovered, reopened_output);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render(), [[0.25; 2]; 64]);
}

#[test]
fn resident_offline_source_replay_surfaces_refusal_and_restart_keeps_exact_extent() {
    let _native = native_fixture();
    let files = Files::new();
    let gate = Arc::new(AtomicBool::new(false));
    // Exact original extent: current/captured/retired configuration handles must deduplicate
    // the same physical bank, rather than charging another 8192 frames during output teardown.
    let mut h = ResidentHarness::with_open_gate(8192, Arc::clone(&gate));
    let initial = h.next();
    assert!(initial.error.is_none());
    let deadline = Instant::now() + Duration::from_secs(1);
    while h.unavailable.is_empty() {
        h.pump();
        assert!(Instant::now() < deadline, "open refusal was not delivered");
        std::thread::yield_now();
    }
    assert!(h.unavailable[0].contains("fixture output unavailable"));
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    let last = h.run(Request::Edit(Command::Gain { track, gain: 0.5 }));
    assert!(h.audio.is_none() && h.submitted.is_none());
    assert!(h.pending_present.load(Acquire));
    assert!(!h.unavailable.is_empty());
    gate.store(true, std::sync::atomic::Ordering::Release);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::RestartAudio {
            project_generation: 0,
            request_id: 1,
        })
        .unwrap();
    let retry_refused = h.next();
    assert!(
        retry_refused
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert_eq!(retry_refused.state.project, last.state.project);
    h.wait_fresh_output(0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h
        .publication_refusals
        .iter()
        .any(|reason| reason.contains("final admission refusal"))
    {
        h.pump();
        assert!(
            Instant::now() < deadline,
            "retained replay refusal was not surfaced"
        );
        std::thread::yield_now();
    }
    assert!(h.pending_present.load(Acquire));
    assert!(h.submitted.is_none());
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        0
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let (applied, metadata) = h.applied(0);
    assert_eq!(metadata.context.editor_revision, last.state.revision);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    let armed = h.run(Request::ArmGainRecording {
        project_generation: 0,
        revision: last.state.revision,
        track,
    });
    assert_eq!(armed.state.project, last.state.project);
    let (_, metadata) = h.applied(applied);
    assert_eq!(metadata.gain_recording_track, Some(track));
    for request_id in [2, 3] {
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let restarted = h.run(Request::RestartAudio {
            project_generation: 0,
            request_id,
        });
        assert_eq!(restarted.state.project, last.state.project);
        assert_eq!(restarted.state.revision, last.state.revision);
        assert_eq!(restarted.state.can_undo, last.state.can_undo);
        assert_eq!(
            restarted.state.project.track(track).unwrap().instrument(),
            loaded.state.project.track(track).unwrap().instrument()
        );
        h.release_relinquished_output();
        h.wait_fresh_output(old_output);
        let (_, metadata) = h.applied(0);
        assert_eq!(metadata.context.editor_revision, last.state.revision);
        assert_eq!(metadata.context.project_generation, 0);
        assert_eq!(metadata.gain_recording_track, Some(track));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::playback::PlaybackCommand::Play)
            .unwrap();
        assert_eq!(h.render(), [[0.125; 2]; 64]);
    }
}

#[test]
fn resident_pending_survives_refused_verified_transition_and_retires_on_accepted_mode_change() {
    let _native = native_fixture();
    let files = Files::new();
    let gate = Arc::new(AtomicBool::new(false));
    let mut h = ResidentHarness::with_open_gate(8192, Arc::clone(&gate));
    assert!(
        h.next().error.is_none(),
        "the editor opens while audio is pending"
    );
    assert!(h.audio.is_none(), "the gate still holds the output closed");
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert!(h.pending_present.load(Acquire));
    h.budget_refuse
        .store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some() && !refused.project_replaced);
    assert_eq!(refused.state.project, loaded.state.project);
    assert_eq!(refused.state.revision, loaded.state.revision);
    assert!(h.pending_present.load(Acquire));
    assert!(h.audio.is_none() && h.submitted.is_none());
    h.budget_refuse
        .store(false, std::sync::atomic::Ordering::Release);
    gate.store(true, std::sync::atomic::Ordering::Release);
    let verified = h.run(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(verified.project_replaced);
    assert!(
        !h.pending_present.load(Acquire),
        "only the obsolete unpublished resident owner must be retired"
    );
    h.wait_fresh_output(0);
    assert_eq!(
        h.audio.as_ref().unwrap().mode,
        hex_session::DocumentAudioMode::VerifiedStreaming
    );
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .control
            .worker_owns_graph_publication()
    );
    let context = verified
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    h.wait_constructor_applied(context);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let resident = h.run(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::Resident,
        reservation: None,
    });
    let mut applied = finish_replacement(&mut h, &resident, old_output);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "fresh sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 2,
        project_generation: 2,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let (_, metadata) = h.applied(applied);
    assert_eq!(metadata.context.project_generation, 2);
    assert_eq!(metadata.context.editor_revision, loaded.state.revision);
}

#[test]
fn production_default_verified_gestures_rebase_barriers_and_restore_use_verified_protocol() {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    use hex_session::tuning_gesture::{TuningGestureKey, TuningGestureRequest};
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
    h.applied(0);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let empty = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .control
            .worker_owns_graph_publication()
    );
    h.wait_constructor_applied(
        empty
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context(),
    );
    let imported = h.run(Request::Import(files.wav(48_000, 240_000)));
    let track = imported.state.project.tracks()[0].id();
    let context = |event: &Event| {
        event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
    };
    let generation = context(&imported).project_generation;
    h.wait_constructor_applied(context(&imported));
    let class = hex_engine::NativeDeviceClass::Effect;
    let utility = hex_engine::native_device_catalog_for_class(class)
        .unwrap()
        .into_iter()
        .find(|entry| entry.type_id.as_str() == Ok("hex.utility"))
        .unwrap()
        .type_id;
    let added = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state_for_class(class, utility).unwrap(),
    }));
    h.wait_constructor_applied(context(&added));
    let device = added.state.project.track(track).unwrap().devices()[0].id();
    let parameter = hex_engine::native_device_view_for_class(
        class,
        added.state.project.device(device).unwrap().state(),
    )
    .unwrap()
    .parameters
    .into_iter()
    .find(|parameter| {
        parameter.descriptor.label == "gain"
            && parameter.descriptor.id != hex_engine::DEVICE_OUTPUT_GAIN.id
            && parameter.descriptor.id != hex_engine::DEVICE_INPUT_GAIN.id
    })
    .unwrap()
    .descriptor
    .id;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    // A changed static device matrix ramps over `rate.div_ceil(200)` frames
    // (Pulse #4101); 8 blocks of 64 frames cover that ramp up to 96 kHz.
    let assert_pcm = |h: &mut ResidentHarness, sample: f32| {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ramp_blocks = 0;
        loop {
            let pcm = h.render();
            if pcm.iter().flatten().any(|value| *value != 0.0) {
                if pcm == [[sample; 2]; 64] {
                    return;
                }
                ramp_blocks += 1;
                assert!(
                    ramp_blocks <= 8,
                    "output did not settle at {sample}: {pcm:?}"
                );
                continue;
            }
            // Verified source identity/admission ACK precedes asynchronous page readiness.
            assert!(
                Instant::now() < deadline,
                "verified source pages did not become audible"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    assert_pcm(&mut h, 0.25);

    let key = GestureKey {
        generation,
        token: 1,
    };
    let begin = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::DeviceParameter { device, parameter },
    }));
    assert!(
        begin.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        begin.gesture_ack
    );
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    assert!(preview.gesture_ack.unwrap().accepted);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            class,
            preview
                .device_preview
                .unwrap()
                .state
                .project
                .device(device)
                .unwrap()
                .state(),
            parameter,
        )
        .unwrap(),
        ParameterValue::Float(0.5)
    );
    // Independent canonical edits rebase using the existing Verified preview protocol.
    let rebased = h.run(Request::Edit(Command::MasterGain { gain: 0.5 }));
    assert!(rebased.active_device_gesture.is_some());
    assert!(rebased.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(
        rebased
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .master_gain(),
        0.5
    );
    h.wait_constructor_applied(context(&rebased));
    let path = files.0.join("verified-gestures.hex");
    let committed = h.run(Request::SaveNew(path));
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert!(committed.active_device_gesture.is_none() && committed.save_complete);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            class,
            committed.state.project.device(device).unwrap().state(),
            parameter,
        )
        .unwrap(),
        ParameterValue::Float(0.5)
    );
    h.wait_constructor_applied(context(&committed));
    assert_pcm(&mut h, 0.0625);

    let key = TuningGestureKey {
        generation,
        token: 1,
    };
    let tuning = super::sampler::tuning(880.0);
    let begin = h.run(Request::TuningGesture(TuningGestureRequest::Begin { key }));
    assert!(begin.tuning_gesture_ack.unwrap().accepted);
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 1,
        tuning,
    }));
    assert!(preview.tuning_gesture_ack.unwrap().accepted);
    assert_eq!(
        preview.tuning_preview.unwrap().state.project.tuning(),
        tuning
    );
    let rebased = h.run(Request::Edit(Command::MasterGain { gain: 0.25 }));
    assert!(rebased.active_tuning_gesture.is_some());
    assert!(rebased.tuning_gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(
        rebased
            .tuning_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .master_gain(),
        0.25
    );
    h.wait_constructor_applied(context(&rebased));
    let committed = h.run(Request::SaveCurrent);
    assert!(committed.tuning_gesture_ack.as_ref().unwrap().accepted);
    assert!(committed.active_tuning_gesture.is_none() && committed.save_complete);
    assert_eq!(committed.state.project.tuning(), tuning);
    h.wait_constructor_applied(context(&committed));
    assert_pcm(&mut h, 0.03125);
    for request in [
        Request::RestoreStoppedPlayback,
        Request::RestorePlayback { generation: 9 },
    ] {
        let restored = h.run(request);
        assert_eq!(restored.state.project, committed.state.project);
        assert_eq!(restored.state.revision, committed.state.revision);
    }
    h.service
        .submit(Request::ArmGainRecording {
            project_generation: generation,
            revision: committed.state.revision,
            track,
        })
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .unwrap()
            .contains("gain recording is unavailable for verified streaming")
    );
    assert_eq!(refused.state.project, committed.state.project);
    assert_pcm(&mut h, 0.03125);
}

#[test]
fn ordinary_default_drop_grows_beyond_pcm_allowance_and_reaches_real_native_publication() {
    let _native = native_fixture();
    let files = Files::new();
    let first_path = files.wav(48_000, 4);
    let second_path = files.0.join("second.wav");
    std::fs::copy(&first_path, &second_path).unwrap();
    let mut h = ResidentHarness::with_frame_limit(4);
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let first = h.run(Request::Import(first_path));
    let (first_audio, _) = h.applied(initial);
    assert_eq!(first.state.project.assets().count(), 1);
    // This uses production ordinary defaults: no VerifiedStreaming command or
    // source-mode opt-in is sent by the fixture.
    let second = h.run(Request::Import(second_path));
    assert_eq!(second.state.project.assets().count(), 2);
    assert!(
        second
            .state
            .project
            .assets()
            .all(|asset| asset.playback_frames() == 4)
    );
    assert_eq!(second.state.waveforms.len(), 2);
    for waveform in second.state.waveforms.values() {
        assert_eq!(waveform.source_frames, 4);
        assert_eq!(waveform.bins.len(), 4);
        assert!(
            waveform
                .bins
                .iter()
                .flatten()
                .all(|peak| peak.min == 0.25 && peak.max == 0.25)
        );
    }
    let snapshot = second.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(
        snapshot.audio_mode(),
        hex_session::DocumentAudioMode::Resident
    );
    assert!(
        snapshot
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let (second_audio, metadata) = h.applied(first_audio);
    assert_eq!(metadata.context.editor_revision, second.state.revision);
    let refused_path = files.0.join("refused.wav");
    std::fs::copy(files.0.join("second.wav"), &refused_path).unwrap();
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::Import(refused_path.clone()))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("fixture final admission refusal")
    );
    assert_eq!(refused.state.project, second.state.project);
    assert_eq!(refused.state.revision, second.state.revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        second_audio
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    // Refusal left a genuinely verified but unused cache domain beside the
    // active one. A different file must evict that cache, not retire the active
    // domain and wait forever for its own not-yet-prepared successor.
    let third_path = files.0.join("third.wav");
    std::fs::copy(files.0.join("second.wav"), &third_path).unwrap();
    let (batch, batch_control) = Request::audio_batch(
        vec![third_path, refused_path],
        hex_session::import_batch::AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        hex_session::import_batch_decode::AudioBatchContext {
            project_generation: 0,
            revision: second.state.revision,
            active_gesture: false,
        },
    )
    .unwrap();
    let fourth = h.run(batch);
    let (fourth_audio, _) = h.applied(second_audio);
    assert_eq!(
        batch_control.progress().phase,
        hex_session::import_batch_decode::AudioBatchPhase::Completed
    );
    assert_eq!(fourth.state.project.assets().count(), 4);
    assert!(
        fourth
            .state
            .project
            .assets()
            .all(|asset| asset.playback_frames() == 4)
    );
    // Exercise simultaneous page protection and catalog growth through the
    // actual native boundary, beyond the small N2 cache-turnover reproduction.
    let more: Vec<_> = (0..12)
        .map(|index| {
            let path = files.0.join(format!("expanded-{index}.wav"));
            std::fs::copy(files.0.join("second.wav"), &path).unwrap();
            path
        })
        .collect();
    let (batch, control) = Request::audio_batch(
        more,
        hex_session::import_batch::AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        hex_session::import_batch_decode::AudioBatchContext {
            project_generation: 0,
            revision: fourth.state.revision,
            active_gesture: false,
        },
    )
    .unwrap();
    let expanded = h.run(batch);
    assert_eq!(
        control.progress().phase,
        hex_session::import_batch_decode::AudioBatchPhase::Completed
    );
    let (expanded_audio, _) = h.applied(fourth_audio);
    assert_eq!(expanded.state.project.assets().count(), 16);
    assert!(
        expanded
            .state
            .project
            .assets()
            .all(|asset| asset.playback_frames() == 4)
    );
    h.run(Request::Edit(Command::MasterGain { gain: 0.25 }));
    h.applied(expanded_audio);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut expected = [[0.0; 2]; 64];
    expected[..4].fill([1.0; 2]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(pcm, [[0.0; 2]; 64]);
            assert_eq!(status.position.load(Acquire), 0);
            assert_eq!(status.performance_frame.load(Acquire), 0);
            assert!(
                Instant::now() < deadline,
                "ordinary source pages did not become ready"
            );
            std::thread::yield_now();
            continue;
        }
        assert_eq!(pcm, expected);
        assert_eq!(status.position.load(Acquire), 64);
        break;
    }
    let saved_path = files.0.join("growing.hex");
    let saved = h.run(Request::SaveNew(saved_path.clone()));
    assert!(saved.save_complete);
    let persisted = hex_storage::load(&saved_path).unwrap();
    assert_eq!(persisted.assets().count(), 16);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved_path));
    assert!(reopened.open_complete);
    assert_eq!(reopened.state.project.as_ref(), &persisted);
    assert_eq!(reopened.state.waveforms.len(), 16);
    assert!(reopened.state.waveforms.values().all(|waveform| {
        waveform.source_frames == 4
            && waveform
                .bins
                .iter()
                .flatten()
                .all(|peak| peak.min == 0.25 && peak.max == 0.25)
    }));
    finish_replacement(&mut h, &reopened, old_output);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(pcm, [[0.0; 2]; 64]);
            assert_eq!(status.position.load(Acquire), 0);
            assert_eq!(status.performance_frame.load(Acquire), 0);
            assert!(
                Instant::now() < deadline,
                "reopened source pages did not become ready"
            );
            std::thread::yield_now();
        } else {
            assert_eq!(pcm, expected);
            assert_eq!(status.position.load(Acquire), 64);
            break;
        }
    }
    // Actual carrier destruction and Root cleanup must release the exclusive
    // native lease, including an active ordinary graph with installed pages.
    drop(h);
    let mut fresh = ResidentHarness::with_frame_limit(4);
    assert!(fresh.next().error.is_none());
    fresh.applied(0);
}

#[test]
fn ordinary_file_backed_seek_publishes_exact_native_pcm() {
    let _native = native_fixture();
    for (sampler, section) in [(false, false), (true, false), (true, true)] {
        ordinary_file_seek_case(sampler, section);
    }
}
fn ordinary_file_seek_case(sampler: bool, section: bool) {
    let files = Files::new();
    let sampler_files = Files::new();
    let path = files.wav(48_000, 1024);
    let mut h = ResidentHarness::with_frame_limit(if sampler { 8192 } else { 4 });
    assert!(h.next().error.is_none());
    let (mut initial, _) = h.applied(0);
    if sampler {
        let added = h.run(Request::Edit(Command::AddMidiTrack {
            name: "sampler".into(),
        }));
        let Some(Created::Track(track)) = added.created else {
            panic!("track")
        };
        (initial, _) = h.applied(initial);
        h.run(Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: added.state.revision,
            track,
            empty_sampler: None,
            path: sampler_files.wav(48_000, 8192),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        (initial, _) = h.applied(initial);
        let placed = h.run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }));
        let Some(Created::Clip(clip)) = placed.created else {
            panic!("clip")
        };
        (initial, _) = h.applied(initial);
        h.run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        }));
        (initial, _) = h.applied(initial);
    }
    let imported = h.run(Request::ImportToNewTrack {
        path: path.clone(),
        before_track: None,
        start: 0,
    });
    let (before_seek, _) = h.applied(initial);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let intent = |h: &ResidentHarness, id| {
        let mut intent = h.seek_intent(id, imported.state.revision, None, None);
        if section {
            intent.section = Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(129, 513, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Immediate,
            });
        }
        intent
    };
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit_resident_arrangement_seek(intent(&h, 1))
        .unwrap();
    assert!(h.next().error.is_none());
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("fixture final admission refusal")
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        before_seek
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    h.service
        .submit_resident_arrangement_seek(intent(&h, 2))
        .unwrap();
    assert!(h.next().error.is_none());
    let result = h.seek_result();
    let hex_session::service::ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
        result.result.unwrap()
    else {
        panic!("actual native submission required")
    };
    let (applied, metadata) = h.applied(before_seek);
    assert_eq!(applied, audio_revision);
    assert_eq!(metadata.seek_request, Some(2));
    assert_eq!(h.render(), [[if sampler { 0.5 } else { 0.25 }; 2]; 64]);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.performance_frame.load(Acquire) >= 193);
    if section {
        assert!((129..513).contains(&status.position.load(Acquire)));
    }

    // Captured source facts cannot silently refresh after a successful seek.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[44..].fill(0);
    std::fs::write(&path, bytes).unwrap();
    h.service
        .submit_resident_arrangement_seek(intent(&h, 3))
        .unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.unwrap_err().contains("media"));
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

#[test]
fn ordinary_mixed_original_sampler_and_file_sources_save_reopen_without_decoding_file_pcm() {
    let _native = native_fixture();
    let files = Files::new();
    let sampler_files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(16_384);
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: sampler_files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    let imported = h.run(Request::ImportToNewTrack {
        path: files.wav(48_000, 12_288),
        before_track: None,
        start: 0,
    });
    h.applied(applied);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let saved_path = files.0.join("mixed-source-project.hex");
    assert!(h.run(Request::SaveNew(saved_path.clone())).save_complete);
    let expected = hex_storage::load(&saved_path).unwrap();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved_path));
    assert!(reopened.open_complete);
    assert_eq!(reopened.state.project.as_ref(), &expected);
    assert_eq!(reopened.state.waveforms.len(), 1);
    assert_eq!(
        reopened
            .state
            .waveforms
            .values()
            .next()
            .unwrap()
            .source_frames,
        12_288
    );
    assert!(
        reopened
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    finish_replacement(&mut h, &reopened, output);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if !status.stream_buffering.load(Acquire) {
            assert_eq!(pcm, [[0.5; 2]; 64]);
            break;
        }
        assert_eq!(pcm, [[0.0; 2]; 64]);
        assert!(
            Instant::now() < deadline,
            "reopened file pages did not become ready"
        );
        std::thread::yield_now();
    }
}

#[test]
fn resident_offline_media_opens_silent_and_relinks_in_place_with_undo() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(32_768);
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let source = files.wav(48_000, 8_192);
    let imported = h.run(Request::Import(source.clone()));
    let asset = imported.state.project.assets().next().unwrap().id();
    h.applied(applied);
    let path = files.0.join("offline.hex");
    h.run(Request::SaveNew(path.clone()));
    let moved = files.0.join("moved.wav");
    std::fs::rename(&source, &moved).unwrap();

    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(path));
    assert!(opened.state.project.asset_offline(asset));
    let applied = finish_replacement(&mut h, &opened, old_output);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(hex_engine::playback::PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render(), [[0.0; 2]; 64], "offline clips are silent");

    let relinked = h.run(Request::RelinkMedia(std::collections::BTreeMap::from([(
        asset,
        moved.clone(),
    )])));
    assert!(!relinked.state.project.asset_offline(asset));
    assert_eq!(relinked.state.project.asset(asset).unwrap().path(), moved);
    let (applied, metadata) = h.applied(applied);
    assert_eq!(metadata.context.editor_revision, relinked.state.revision);
    assert!(
        h.render().iter().any(|frame| *frame != [0.0; 2]),
        "relinked media plays once admitted"
    );

    let undone = h.run(Request::Undo);
    assert!(undone.state.project.asset_offline(asset));
    h.applied(applied);
    assert_eq!(
        h.render(),
        [[0.0; 2]; 64],
        "undo returns the clip to silence"
    );
}
