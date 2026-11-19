use super::*;
use crate::prepared::eligible_publication::tests::stream_fixture;
use hex_engine::{AudioCandidateEffects, AudioSourceContext, Frame};
use hex_project::{Command, Editor, Project};
use std::time::{Duration, Instant};

#[test]
fn verified_device_insert_preserves_audio_before_approval_and_commits_at_the_held_cut() {
    check_insert(Insert::Confirmed);
}

#[test]
fn verified_device_insert_proves_a_seamless_handoff_and_acknowledges_the_graph() {
    check_insert(Insert::Seamless);
}

#[test]
fn verified_device_insert_yields_to_priority_stop_during_the_fade() {
    check_insert(Insert::StopAtFade);
}

#[test]
fn verified_device_insert_refuses_an_outgoing_fade_larger_than_the_cache() {
    check_insert(Insert::PageCapacity);
}

enum Insert {
    Seamless,
    Confirmed,
    StopAtFade,
    PageCapacity,
}

fn prepare(
    control: &mut Control,
    project: &Project,
    metadata: &hex_engine::StreamedSourceMetadata,
    domain: crate::AudioCandidateDomainToken,
    context: AudioSourceContext,
) -> crate::AudioCandidatePreparationToken {
    AudioCandidateEffects::with_host_definitions(&[], |effects| {
        let quote = effects.quote(project, 1).unwrap();
        let token = control
            .reserve_audio_candidate_preparation(
                crate::AudioCandidatePreparationReservation {
                    source_capacity: quote.source_capacity,
                    storage: quote.storage_reservation(0).unwrap(),
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                },
                vec![domain],
            )
            .unwrap_or_else(|error| panic!("{}", error.reason));
        assert_eq!(
            control
                .prepare_audio_candidate_on_control(token, |cell| {
                    cell.add_streamed_source_on_control(metadata.clone())?;
                    effects.prepare(cell, project, metadata.key().generation(), context, None)
                })
                .unwrap(),
            crate::AudioCandidatePreparationProgress::Ready
        );
        Ok(token)
    })
    .unwrap()
}

fn check_insert(insert: Insert) {
    let _serial = serial();
    let needs_confirmation = !matches!(insert, Insert::Seamless);
    let stop = matches!(insert, Insert::StopAtFade);
    let small_cache = matches!(insert, Insert::PageCapacity);
    let device = if needs_confirmation {
        "hex.limiter"
    } else {
        "hex.utility"
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let project = super::readiness::project_with_frames(32_768);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(32_768);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static(device),
    )
    .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let quote = AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    let mut active_context = AudioSourceContext {
        project_generation: 2,
        editor_revision: 7,
    };
    let mut first =
        stream_fixture::owner_sources(&project, vec![metadata.clone()], Some(active_context));
    let mut limits = reservation(&mut first);
    // The initial graph has no effects. Reserve space for a later real Limiter.
    limits.member_rows += quote.member_rows * 3;
    limits.transfer_rows += quote.transfer_rows * 3;
    limits.handle_rows += quote.handle_rows * 3;
    limits.data_rows = limits.data_rows.max(quote.data_rows);
    limits.max_retained_data_bytes += storage.max_prepared_data_bytes * 3;
    limits.max_construction_peak_data_bytes += storage.max_construction_data_bytes * 3;
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![definition], if small_cache { 2 } else { 4 }),
            hex_stream::StreamReadinessReservation {
                extents_per_role: if small_cache { 1 } else { 2 },
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let first_ticket = control
        .stage_eligible_audio_candidate(
            first,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            active_context.project_generation,
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.command(PlaybackCommand::Play).unwrap();
    let mut block = || -> [Frame; 64] {
        let mut pcm = [[0.0; 2]; 64];
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
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
        pcm
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while block() != [[0.25; 2]; 64] {
        assert!(Instant::now() < deadline, "predecessor did not play");
        std::thread::yield_now();
    }
    let predecessor = control.status.applied_audio_source().unwrap();
    assert_eq!(predecessor.context, active_context);
    active_context.editor_revision += 1;
    let token = prepare(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        active_context,
    );
    let (mut ticket, receipt) = control
        .stage_verified_device_insert(token, active_context, predecessor, None)
        .unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    let deadline = Instant::now() + Duration::from_secs(2);
    while receipt.state() == PublicationReceiptState::Pending {
        assert_eq!(block(), [[0.25; 2]; 64]);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let receipt = if needs_confirmation {
        assert_eq!(
            receipt.state(),
            PublicationReceiptState::Refused(
                PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
            )
        );
        let offer = control
            .offer_resident_device_insert_consent(&receipt)
            .unwrap();
        assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        assert_eq!(block(), [[0.25; 2]; 64]);
        drop(recover(&mut control, &ticket));
        let token = prepare(
            &mut control,
            editor.project(),
            &metadata,
            domain,
            active_context,
        );
        if small_cache {
            // The next 64-frame render fits on page zero. The complete
            // 128-frame fade also needs page one, which cannot coexist.
            let cut = u64::try_from(hex_engine::STREAM_PAGE_FRAMES).unwrap() - 64;
            while control.status.position.load(Acquire) < cut {
                assert_eq!(block(), [[0.25; 2]; 64]);
            }
            assert_eq!(control.status.position.load(Acquire), cut);
        }
        let approved = control
            .stage_verified_device_insert(token, active_context, predecessor, Some(offer))
            .unwrap();
        ticket = approved.0;
        let receipt = approved.1;
        if small_cache {
            let position = control.status.position.load(Acquire);
            assert_eq!(block(), [[0.25; 2]; 64]);
            assert_eq!(control.status.position.load(Acquire), position + 64);
        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            let fade_start = loop {
                let position = control.status.position.load(Acquire);
                let pcm = block();
                if pcm[0] == [0.25 * 127.0 / 128.0; 2] {
                    break position;
                }
                assert_eq!(pcm, [[0.25; 2]; 64]);
                assert!(
                    Instant::now() < deadline,
                    "fade did not start: {:?}",
                    receipt.state()
                );
                std::thread::yield_now();
            };
            assert_eq!(receipt.state(), PublicationReceiptState::Applying);
            if stop {
                control.command(PlaybackCommand::Stop).unwrap();
                block();
                assert_eq!(control.status.applied_audio_source(), Some(predecessor));
                assert_eq!(block()[63], [0.0; 2]);
                assert_eq!(control.status.position.load(Acquire), 0);
                assert_eq!(block(), [[0.0; 2]; 64]);
            } else {
                assert_eq!(block()[63], [0.0; 2]);
                assert_eq!(control.status.position.load(Acquire), fade_start + 128);
                assert_eq!(control.status.applied_audio_source(), Some(predecessor));
                block();
                assert_eq!(control.status.position.load(Acquire), fade_start + 192);
            }
        }
        receipt
    } else {
        receipt
    };
    let (expected_state, expected_source) = if small_cache {
        (
            PublicationReceiptState::Refused(PlaybackHandoffRejection::StreamedPageCapacity),
            predecessor,
        )
    } else if stop {
        (
            PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged),
            predecessor,
        )
    } else {
        (
            PublicationReceiptState::Installed,
            AppliedAudioSource {
                context: active_context,
                audio_revision: receipt.revision(),
                output_identity: predecessor.output_identity,
            },
        )
    };
    assert_eq!(receipt.state(), expected_state);
    assert_eq!(control.status.applied_audio_source(), Some(expected_source));
    assert_eq!(control.status.playing.load(Acquire), !stop);
    crate::lifecycle::test_dispose_backend(
        &mut Some(()),
        &mut false,
        &mut false,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    drop(recover(&mut control, &first_ticket));
    drop(recover(&mut control, &ticket));
    drop(control);
    finish(&mut retirement);
}
