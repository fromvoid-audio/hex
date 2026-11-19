//! Retry a verified graph while an unrelated native source is unsealed.
use super::*;

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
    assert!(control.retire_native_source_preparation(binding));
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
fn unsealed_native_source_refusal_keeps_built_verified_candidate_for_retry() {
    let _native = native_fixture();
    let policy = crate::audio_policy::AudioPolicy::new().unwrap();
    let (mut output, mut backend) = hex_audio::worker_fixture::open(48_000).unwrap();
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
