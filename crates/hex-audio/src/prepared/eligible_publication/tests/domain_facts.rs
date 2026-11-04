//! Exact-key/full-facts ambiguity at the closed constructor publication seam.
//! This uses real verified catalogs and original owner recovery, including on
//! the intentionally accepted pre-fix path. No callback graph source is used.
use super::*;
use hex_project::{Command, Created, Editor};

fn measured_callback(
    data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
) {
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
fn measured_silent_render(
    data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
) {
    let mut output = [[9.0; 2]; 8];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, [[0.0; 2]; 8]);
}
fn recovered_sources_match(
    inputs: &hex_engine::OwnedAudioInputs,
    sources: &[hex_engine::StreamedSourceMetadata],
) -> bool {
    inputs.owning_handle_count() == sources.len()
        && sources.iter().all(|source| {
            inputs
                .source_token(source.key().asset())
                .ok()
                .and_then(|token| inputs.source_view(&token).ok())
                .and_then(|view| view.streamed_metadata().cloned())
                .as_ref()
                == Some(source)
        })
}
#[test]
fn closed_constructor_conflicting_pcm_key_facts_refuse_and_preserve_owner_and_domains() {
    let _serial = serial();
    let mut editor = Editor::new(readiness::project_with_frames(8192), 16).unwrap();
    let asset_a = editor.project().assets().next().unwrap().id();
    let track = editor.project().tracks()[0].id();
    let Created::Asset(asset_b) = editor
        .execute(Command::AddAsset {
            path: "required-b.wav".into(),
            sample_rate: 48000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("second source asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset: asset_b,
            start: 8192,
            duration: 8192,
            source_offset: 0,
            source_length: 8192,
            looping: false,
        })
        .unwrap();
    let old_a = stream_fixture::Wav::with_frames(8192);
    let new_a = stream_fixture::Wav::with_samples(8192, |_| 16384);
    let required_b = stream_fixture::Wav::with_frames(8192);
    let old_a_definition = old_a.definition(asset_a, 11);
    let new_a_definition = new_a.definition(asset_a, 11);
    let b_definition = required_b.definition(asset_b, 11);
    assert_eq!(old_a_definition.key, new_a_definition.key);
    assert_ne!(old_a_definition.metadata, new_a_definition.metadata);
    let expected_sources = [
        new_a_definition.metadata.clone(),
        b_definition.metadata.clone(),
    ];
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 17,
    };
    let mut owner =
        stream_fixture::owner_sources(editor.project(), expected_sources.to_vec(), Some(context));
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let mut domains = Vec::new();
    for definitions in [vec![b_definition, old_a_definition], vec![new_a_definition]] {
        let token = control
            .register_audio_candidate_domain_with_readiness(
                stream_fixture::pair_with_pool(definitions, 4),
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 3,
                },
            )
            .unwrap_or_else(|_| panic!("closed conflicting-facts domain registration"));
        measured_callback(&mut data);
        domains.push(token);
    }
    let selected = domains.clone();
    let selected_pointer = selected.as_ptr();
    let selected_capacity = selected.capacity();
    let before = control.submitted_revision();
    let stage = control.stage_eligible_audio_candidate(
        owner,
        selected,
        PublicationPolicy::InheritRuntime,
        context.project_generation,
    );
    let after = control.submitted_revision();
    let (refused, owner_recovered, tokens_preserved) = match stage {
        Err(mut refusal) => {
            let untouched = refusal.owner.identity() == identity
                && refusal.owner.phase() == hex_engine::AudioCandidateDisposalPhase::Owned;
            let tokens_preserved = refusal.domains == domains
                && refusal.domains.as_ptr() == selected_pointer
                && refusal.domains.capacity() == selected_capacity;
            let disposed = refusal.owner.dispose_on_control().unwrap();
            let inputs = refusal.owner.recover_inputs_on_control().unwrap();
            let recovered = untouched
                && disposed.identity() == identity
                && recovered_sources_match(&inputs, &expected_sources);
            drop(inputs);
            drop(refusal);
            (true, recovered, tokens_preserved)
        }
        Ok(ticket) => {
            // The pre-fix acceptance must still return its original constructor
            // and source bank before the final assertion records behavioral red.
            assert_eq!(
                control.cancel_audio_candidate(&ticket),
                crate::AudioCandidateCancellationResult::Cancelled
            );
            measured_silent_render(&mut data);
            let recovery = recover(&mut control, &ticket);
            let recovered = recovery.disposed.identity() == identity
                && recovered_sources_match(&recovery.inputs, &expected_sources);
            drop(recovery);
            (false, recovered, false)
        }
    };
    for token in &domains {
        assert!(control.retire_audio_candidate_domain(*token));
    }
    control.request_shutdown();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut reports = [false; 2];
    while reports.iter().any(|done| !done) {
        measured_silent_render(&mut data);
        control.reclaim();
        for (index, token) in domains.iter().enumerate() {
            if !reports[index]
                && let Some(report) = control.recover_audio_candidate_domain(*token)
            {
                assert!(report.worker_joined && report.accounting_complete);
                assert_eq!(report.page_buffers, 4);
                reports[index] = true;
            }
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
    assert!(
        owner_recovered,
        "both paths must recover the original constructor and both exact source tokens"
    );
    assert!(
        refused,
        "closed constructor must refuse conflicting full PCM facts under the same selected key"
    );
    assert!(
        tokens_preserved,
        "refusal must return the original domain vector intact"
    );
    assert_eq!(
        after, before,
        "refusal must not consume a publication revision"
    );
}
