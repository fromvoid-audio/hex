//! Verified publication with a separate native source building or retiring.
use super::*;
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

fn next_snapshot(service: &Service) -> Arc<CanonicalAudioSourceSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
        }
        assert!(Instant::now() < deadline, "canonical snapshot timed out");
        std::thread::yield_now();
    }
}

fn wait_for_native_facts(
    control: &mut hex_audio::prepared::PublicationControl,
    binding: hex_audio::plugin_effects::EffectBinding,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if matches!(
            control.native_source_preparation_progress(binding),
            Some(hex_audio::NativeSourcePreparationProgress::Facts(_))
        ) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native source did not reach facts"
        );
        std::thread::yield_now();
    }
}

fn recover_native_source(
    control: &mut hex_audio::prepared::PublicationControl,
    binding: hex_audio::plugin_effects::EffectBinding,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        control.reclaim();
        if control.recover_native_source_preparation(binding).is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "native source return timed out");
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn retiring_unsealed_source_keeps_built_verified_candidate_for_retry() {
    let _native = native_fixture();
    let policy = crate::audio_policy::AudioPolicy::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error) if error == "audio output is open or still shutting down" => {}
            Err(error) => panic!("fixture output unavailable: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "prior fixture output did not retire"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let host = host();
    let selected = selection();
    let binding = {
        let control = output.publication.control().unwrap();
        policy.configure(control).unwrap();
        control
            .configure_native_source_preparation(policy.plugins.capacity, policy.plugins.source)
            .unwrap();
        let binding = host
            .begin_on_control(
                control,
                hex_audio::plugin_effects::EffectSelection {
                    module: &selected.module,
                    executable: &selected.executable,
                    cid: selected.cid,
                    executable_sha256: &selected.executable_sha256,
                    info_sha256: &selected.info_sha256,
                },
                PluginEffectDestination::Main.owner(),
                1,
                1,
                48_000,
                hex_audio::plugin_effects::PluginProfile::Effect,
                None,
                None,
            )
            .unwrap();
        wait_for_native_facts(control, binding);
        assert_eq!(control.audio_candidate_control_backing_bytes(), None);
        assert!(
            control
                .with_native_effect_definition(&[binding], |_| Ok(()))
                .is_err()
        );
        let mut other_epoch = binding;
        other_epoch.epoch += 1;
        assert!(
            control
                .with_native_effect_definition(&[other_epoch], |_| Ok(()))
                .is_err()
        );
        // A retiring source has no live Facts-stage publication witness.
        assert!(control.retire_native_source_preparation(binding));
        assert!(
            control
                .with_native_effect_definition(&[binding], |_| Ok(()))
                .is_err()
        );
        binding
    };

    let service = Service::new(48_000, 16, 0).unwrap();
    let _initial = next_snapshot(&service);
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    let snapshot = next_snapshot(&service);
    assert_eq!(snapshot.audio_mode(), DocumentAudioMode::VerifiedStreaming);
    assert_eq!(snapshot.required_sources().count(), 0);

    let (events, _receiver) = mpsc::sync_channel(8);
    let mut sink = NativeAudioSink::new(events);
    sink.policy = Ok(policy);
    sink.desired = Some(Arc::clone(&snapshot));
    sink.output_identity = output.publication.control().unwrap().output_identity();

    let mut silence = [[0.0; 2]; 64];
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        // The callback must receive the domain endpoint before the graph can
        // use the same publication queue.
        backend.render(&mut silence).unwrap();
        sink.drive_verified(&mut output);
        if let Some(EditorAudioEvent::Publication {
            state: EditorPublicationState::Refused(reason),
            ..
        }) = &sink.pending_observation
        {
            panic!("candidate was refused instead of retried: {reason}");
        }
        if sink.preparation.as_ref().is_some_and(|pending| {
            pending.built
                && pending.domain.is_some_and(|domain| {
                    output
                        .publication
                        .control()
                        .unwrap()
                        .audio_candidate_domain_endpoint_is_installed(domain)
                })
        }) && sink.last_state.is_some_and(|(context, tag, revision)| {
            context == snapshot.context() && tag == "busy" && revision.is_none()
        }) {
            break;
        }
        assert!(
            Instant::now() < until,
            "verified candidate did not reach stage refusal"
        );
        std::thread::yield_now();
    }
    let pending = sink.preparation.as_ref().unwrap();
    let token = pending.token;
    let domain = pending.domain;
    let key = pending.snapshot.key();
    assert!(pending.bound && pending.built);
    assert!(Arc::ptr_eq(pending.snapshot.canonical_arc(), &snapshot));
    assert_eq!(sink.verified_blocked, None);
    assert_eq!(
        output.publication.control().unwrap().submitted_revision(),
        0
    );
    assert_eq!(
        output
            .publication
            .control()
            .unwrap()
            .stage_audio_candidate_preparation(token, snapshot.context().project_generation)
            .err(),
        Some("native source construction is not sealed"),
    );

    sink.drive_verified(&mut output);
    let retry = sink.preparation.as_ref().unwrap();
    assert_eq!(
        retry.token, token,
        "transient refusal keeps the same preparation"
    );
    assert_eq!(
        retry.domain, domain,
        "transient refusal keeps the exact domain"
    );
    assert!(retry.bound && retry.built);
    assert_eq!(sink.verified_blocked, None);
    assert_eq!(
        output.publication.control().unwrap().submitted_revision(),
        0
    );

    recover_native_source(output.publication.control().unwrap(), binding);
    let until = Instant::now() + Duration::from_secs(3);
    while sink.submitted.iter().all(Option::is_none) {
        sink.drive_verified(&mut output);
        assert!(
            sink.verified_blocked.is_none(),
            "candidate was blocked after source retirement: {:?}",
            sink.last_state
        );
        assert!(
            Instant::now() < until,
            "verified candidate did not retry after source retirement"
        );
        std::thread::yield_now();
    }
    let submitted = sink.submitted.iter().flatten().next().unwrap();
    assert_eq!(submitted.key, key);
    assert_eq!(
        output.publication.control().unwrap().submitted_revision(),
        1
    );
    assert_eq!(sink.verified_blocked, None);

    wait(|| {
        backend.render(&mut silence).unwrap();
        backend
            .status()
            .unwrap()
            .applied_audio_source()
            .is_some_and(|applied| {
                applied.context == snapshot.context() && applied.audio_revision == 1
            })
    });
    drop(output);
    backend.close();
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn independent_verified_eq_preview_installs_while_native_facts_are_held() {
    let _native = native_fixture();
    let files = Files::new();
    let HeldVerifiedEq {
        mut h,
        operation,
        project,
        key,
        resume_facts: _hold,
        ..
    } = hold_verified_eq_preview(&files);
    assert!(operation.cancel());
    loop {
        let event = h.next_with_audio();
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &operation) && !terminal.accepted);
            assert_eq!(event.state.project, project);
            assert_eq!(event.active_device_gesture.unwrap().key, key);
            break;
        }
    }
}

pub(super) struct HeldVerifiedEq {
    pub(super) h: Harness,
    pub(super) operation: Arc<hex_session::service::PluginEffectControl>,
    pub(super) project: Arc<hex_project::Project>,
    pub(super) device: hex_project::DeviceId,
    pub(super) key: GestureKey,
    pub(super) resume_facts: mpsc::SyncSender<()>,
    pub(super) progress: Arc<Mutex<crate::native_audio::plugins::PreparationObservation>>,
}

pub(super) fn hold_verified_eq_preview(files: &Files) -> HeldVerifiedEq {
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let (reached, facts) = mpsc::sync_channel(1);
    let (resume_facts, resumed) = mpsc::sync_channel(1);
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.facts_gate = Some(crate::native_audio::plugins::FactsGate::new(
            reached, resumed,
        ));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let inserted = h.run_verified_device_insert(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::eq::EqState::default().encode().unwrap(),
    });
    let Some(Created::Device(device)) = inserted.created else {
        panic!("fixture EQ")
    };
    let predecessor = h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    expect_verified_sample(&h, 0.25);
    let input = request(&inserted.state, PluginEffectDestination::Track(track));
    let operation = Arc::clone(&input.control);
    assert!(
        h.run(Request::InsertPluginEffect(input))
            .plugin_effect_complete
            .is_empty()
    );
    facts.recv_timeout(Duration::from_secs(3)).unwrap();

    let parameter = hex_engine::eq::OUTPUT_GAIN;
    let value = ParameterValue::Float(0.5);
    let key = GestureKey {
        generation: predecessor.context.project_generation,
        token: 1,
    };
    let begun = eq_gesture(
        &mut h,
        GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter { device, parameter },
        },
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    let changed = eq_gesture(
        &mut h,
        GestureRequest::Change {
            key,
            sequence: 1,
            value,
        },
    );
    assert!(changed.gesture_ack.unwrap().accepted);
    assert_eq!(changed.state.project, inserted.state.project);
    assert!(changed.device_preview.is_some());
    assert!(
        h.observation
            .lock()
            .unwrap()
            .received_verified_device
            .is_some()
    );
    expect_verified_sample(&h, 0.125);
    HeldVerifiedEq {
        h,
        operation,
        project: Arc::clone(&inserted.state.project),
        device,
        key,
        resume_facts,
        progress,
    }
}

pub(super) fn eq_gesture(h: &mut Harness, request: GestureRequest) -> Event {
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::DeviceGesture(request))
        .unwrap();
    loop {
        let event = h.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.gesture_ack.is_some() {
            break event;
        }
        assert!(event.plugin_effect_complete.is_empty());
    }
}

pub(super) fn expect_verified_sample(h: &Harness, expected: f32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut observation = None;
    loop {
        let pcm = h.render();
        for event in h.events.try_iter() {
            if let EditorAudioEvent::Publication { context, state, .. } = event {
                observation = Some((context, state));
            }
        }
        if (pcm[63][0] - expected).abs() < 1e-5 {
            break;
        }
        if Instant::now() >= deadline {
            let status = h.audio.as_ref().unwrap().status().unwrap();
            panic!(
                "expected {expected}, rendered {:?}, applied {:?}, rejected {:?}, observation {observation:?}",
                pcm[63],
                status.applied_audio_source(),
                status.rejected_revision()
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
