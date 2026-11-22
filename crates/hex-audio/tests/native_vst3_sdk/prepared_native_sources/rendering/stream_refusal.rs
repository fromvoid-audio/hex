//! A candidate waiting for verified pages has no authority to dispatch its
//! native endpoint or admit a native epoch renewal.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_streamed_effect_waits_for_pages_without_dispatch_or_epoch_renewal() {
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        retirement,
        binding,
        mut owner,
        project,
        samples,
        ..
    } = prepared_fixture_with_profile(2, false, "processing", 8, false);
    owner.dispose_on_control().unwrap();
    owner.dispose_original_inputs_on_control().unwrap();
    drop(owner);
    control.configure_audio_candidate_streaming(3).unwrap();
    let wav = crate::prepared::eligible_publication::tests::stream_fixture::Wav::with_frames(32);
    let definition = wav.definition(project.assets().next().unwrap().id(), 12);
    let metadata = definition.metadata.clone();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: metadata.key(),
        page_index: 0,
    });
    let mut stream = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
        gate,
    )
    .unwrap();
    stream.wait_until_ready(Duration::from_secs(2)).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let ticket = replacement_preparation_inner(
        &mut control,
        &[binding],
        &project,
        &samples,
        None,
        vec![domain],
        Some(metadata),
    );
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[9.0; 2]; 8];
    let mut host = [9.0_f32; 16];
    allocation_free(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(8).unwrap();
            let rendered = callback.render(&mut output);
            assert!(rendered.is_ok());
            assert!(crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut output,
                2,
                &rendered,
                0,
                Instant::now()
            ));
        })
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);
    assert!(control.status.publication_in_flight());
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.status.applied_audio_source(), None);
    let root = control.native_candidate_root();
    assert_eq!(
        crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
            &root, binding
        ),
        Some(0)
    );
    assert_eq!(
        control.request_native_epoch_fence(AppliedAudioSource {
            output_identity: root.root_id(),
            audio_revision: 1,
            context: hex_engine::AudioSourceContext {
                project_generation: 41,
                editor_revision: 1
            },
        }),
        Err("native epoch renewal requires resident playback")
    );
    assert!(!control.status.stopped_edit_pending());
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    drop(release);
    let mut retirement = close_backend(data, errors, retirement);
    let returned = crate::prepared::candidate_tests::recover(&mut control, &ticket);
    assert_eq!(
        returned.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(returned);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|t| t.blocks == 0 && t.frames == 0))
    );
    drop(control);
    finish(&mut retirement);
}
