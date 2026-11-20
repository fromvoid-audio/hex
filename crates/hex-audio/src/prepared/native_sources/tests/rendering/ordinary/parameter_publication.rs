//! The callback admits a parameter successor only for its captured source.
use super::*;
use crate::plugin_effects::{AuthoredSelection, Hosting};
use crate::plugin_worker::NativeWorkerStateAfterGuardedWrite;
use crate::prepared::{NativeParameterContinuationCapture, NativeParameterContinuationRequest};

pub(super) fn assert_guarded_continuation(
    f: &mut OrdinaryFixture,
    before: &NativeDeviceState,
    fact: &NativeWorkerStateAfterGuardedWrite,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let after = loop {
        match f
            .control
            .approve_native_guarded_capture_on_control(f.binding, fact)
        {
            Ok(state) => break state,
            Err("native preparation is busy") => {}
            Err(reason) => panic!("guarded capture provenance: {reason}"),
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert!(after.processing_eq(fact.state()));
    assert!(after.shares_payload_storage_with(fact.state()));
    assert_eq!(
        f.control
            .native_source_approves_processing_state_on_control(f.binding, &after),
        Some(false),
        "a capture alone does not grant processing approval"
    );
    let (source, transport_epoch) = f.control.status.native_parameter_source(true).unwrap();
    let successor = hex_engine::AudioSourceContext {
        editor_revision: source.context.editor_revision + 1,
        ..source.context
    };
    let stale_binding = Binding {
        epoch: f.binding.epoch + 1,
        ..f.binding
    };
    assert!(
        f.control
            .approve_native_guarded_capture_on_control(stale_binding, fact)
            .is_err()
    );

    let (build, _, preparation) = crate::candidate_custody::native_source_test_fixture::input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    );
    let host = Hosting::from_test_build(build, preparation.limits);
    let class = Hosting::saved_identity_on_control(&after).unwrap().class;
    let selection = AuthoredSelection::for_no_parameters(class.bytes()).unwrap();
    let mut query = loop {
        match host.begin_canonical_bases_capture_on_control(
            &mut f.control,
            source,
            f.binding,
            &after,
            &selection,
            true,
        ) {
            Ok(query) => break query,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) => {}
            Err(reason) => panic!("guarded continuation preparation: {reason}"),
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    let facts = loop {
        if let Some(result) = query.poll_on_control(&mut f.control, &mut || false) {
            break result.expect("guarded continuation canonical bases");
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    let track = &f.editor.project().tracks()[0];
    let device = track.devices()[0].id();
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.id().value()),
        device.value(),
    )
    .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: after.clone(),
        })
        .unwrap();
    let revision = loop {
        match Hosting::prepare_with_canonical_bases_on_control(
            &f.control,
            &[f.binding],
            &[],
            std::slice::from_ref(&facts),
            &mut |effects| {
                effects.prepare_registered_revision(
                    f.editor.project(),
                    &f.media,
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                    &hex_engine::audio_transform::AudioTransformPreparation::default(),
                    &f.resources,
                    1_000_000,
                    &AtomicBool::new(false),
                    None,
                )
            },
        ) {
            Ok(revision) => break revision,
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native effect preparation is busy" | "native effect selection is busy"
                ) => {}
            Err(reason) => panic!("guarded continuation compiler: {reason}"),
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    };
    let playback =
        Playback::try_from_prepared_with_device_resources(Arc::new(revision), &f.resources)
            .unwrap();
    let request = NativeParameterContinuationRequest {
        source,
        transport_epoch,
        playing: true,
        successor,
        identity,
        binding: f.binding,
        previous_state: before,
        capture: NativeParameterContinuationCapture::Guarded {
            fact,
            state: &after,
        },
    };
    let unrelated_state = NativeParameterContinuationRequest {
        capture: NativeParameterContinuationCapture::Guarded {
            fact,
            state: before,
        },
        ..request
    };
    let Err(AdmittedPublishError::Publication(refused)) = f
        .control
        .publish_native_parameter_continuation_on_control::<()>(playback, unrelated_state, |_| {
            panic!("unrelated state reached admission");
        })
    else {
        panic!("guarded capture must refuse unrelated state");
    };
    assert_eq!(
        refused.reason,
        "native continuation source or input differs"
    );
    let mut playback = refused.playback;
    let revision =
        loop {
            match f.control.publish_native_parameter_continuation_on_control(
                playback,
                request,
                |_| Ok::<_, ()>(()),
            ) {
                Ok(revision) => break revision,
                Err(AdmittedPublishError::Publication(refused))
                    if matches!(
                        refused.reason,
                        "native preparation is busy"
                            | "native graph preparation is busy"
                            | "native source is busy"
                    ) =>
                {
                    playback = refused.playback
                }
                Err(AdmittedPublishError::Publication(refused)) => {
                    panic!("guarded continuation publication: {}", refused.reason);
                }
                Err(_) => panic!("guarded continuation admission"),
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
    for _ in 0..8 {
        block(f);
        if f.control.status.applied_revision.load(Acquire) == revision {
            break;
        }
    }
    assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        f.control.status.recording_source().unwrap().context,
        successor
    );
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn gesture_publication_accepts_current_source_and_preserves_queued_pause() {
    let _serial = serial();
    for pause in [false, true] {
        let mut f = fixture_with_quantum("processing", 0, 256);
        let initial = compile(&f, true);
        publish(&mut f.control, initial, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        physical_block::<256>(&mut f);
        let (source, transport_epoch) = f.control.status.native_parameter_source(true).unwrap();
        let successor = hex_engine::AudioSourceContext {
            editor_revision: source.context.editor_revision + 1,
            ..source.context
        };
        let candidate = compile(&f, true);
        let revision = publish_in_context(
            &mut f.control,
            candidate,
            PublicationPolicy::InheritUnderNativeParameter {
                source,
                transport_epoch,
                playing: true,
            },
            successor,
        );
        if pause {
            f.control.command(PlaybackCommand::Pause).unwrap();
        }
        for _ in 0..8 {
            physical_block::<256>(&mut f);
            if f.control.status.applied_revision.load(Acquire) == revision
                || f.control
                    .status
                    .rejected_revision()
                    .is_some_and(|(r, _)| r == revision)
            {
                break;
            }
        }
        if pause {
            assert!(!f.control.status.playing.load(Acquire));
            assert_eq!(
                f.control.status.applied_revision.load(Acquire),
                source.audio_revision
            );
            assert_eq!(
                f.control.status.rejected_revision(),
                Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
            );
        } else {
            assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
            assert_eq!(
                f.control.status.recording_source().unwrap().context,
                successor
            );
        }
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some())
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn gesture_publication_rejects_stale_witness_and_priority_stop() {
    let _serial = serial();
    for case in ["output", "revision", "epoch", "successor", "stop"] {
        let mut f = fixture_with_quantum("processing", 0, 256);
        let initial = compile(&f, true);
        publish(&mut f.control, initial, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        physical_block::<256>(&mut f);
        let (source, epoch) = f.control.status.native_parameter_source(true).unwrap();
        let mut witness = source;
        let mut witness_epoch = epoch;
        let mut successor = hex_engine::AudioSourceContext {
            editor_revision: source.context.editor_revision + 1,
            ..source.context
        };
        match case {
            "output" => witness.output_identity += 1,
            "revision" => witness.audio_revision += 1,
            "epoch" => witness_epoch += 1,
            "successor" => successor.editor_revision += 1,
            "stop" => {}
            _ => unreachable!(),
        }
        let candidate = compile(&f, true);
        let revision = publish_in_context(
            &mut f.control,
            candidate,
            PublicationPolicy::InheritUnderNativeParameter {
                source: witness,
                transport_epoch: witness_epoch,
                playing: true,
            },
            successor,
        );
        if case == "stop" {
            f.control.stop();
        }
        for _ in 0..8 {
            physical_block::<256>(&mut f);
            if f.control
                .status
                .rejected_revision()
                .is_some_and(|(r, _)| r == revision)
            {
                break;
            }
        }
        assert_eq!(
            f.control.status.applied_revision.load(Acquire),
            source.audio_revision,
            "{case}"
        );
        assert_eq!(
            f.control.status.rejected_revision(),
            Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged)),
            "{case}"
        );
        if case == "stop" {
            assert!(!f.control.status.playing.load(Acquire));
        }
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some()),
            "{case}"
        );
    }
}
