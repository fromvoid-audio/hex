use super::*;
use crate::prepared::eligible_publication::tests::stream_fixture;
use hex_engine::{AudioCandidateEffects, AudioSourceContext, Frame};
use hex_project::{Command, Editor};
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

#[test]
fn verified_device_insert_binds_cancellation_before_the_callback_can_install() {
    check_insert(Insert::CancelledAtEnqueue);
}

#[test]
fn verified_device_insert_expires_at_the_preparation_deadline() {
    check_insert(Insert::PreparationDeadline);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn outgoing_device_fade_requests_the_cold_next_file_while_current_pcm_keeps_playing() {
    use hex_project::Created;

    let _serial = serial();
    let project = super::readiness::project_with_frames(512);
    let first_asset = project.assets().next().unwrap().id();
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Asset(next_asset) = editor
        .execute(Command::AddAsset {
            path: "next-fade-source.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("next asset");
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "next".into(),
        })
        .unwrap()
    else {
        panic!("next track");
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset: next_asset,
            start: 96,
            duration: 512,
            source_offset: 0,
            source_length: 512,
            looping: false,
        })
        .unwrap();
    let first = stream_fixture::Wav::with_frames(512);
    let next = stream_fixture::Wav::with_samples(512, |_| 4_096);
    let definitions = vec![
        first.definition(first_asset, 11),
        next.definition(next_asset, 11),
    ];
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: definitions[1].metadata.key(),
        page_index: 0,
    });
    let context = AudioSourceContext {
        project_generation: 0,
        editor_revision: 7,
    };
    let mut owner = stream_fixture::owner_sources(
        editor.project(),
        definitions
            .iter()
            .map(|definition| definition.metadata.clone())
            .collect(),
        Some(context),
    );
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation(&mut owner))
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let mut stream = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(4),
        definitions,
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
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(
            owner,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            context.project_generation,
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control.command(PlaybackCommand::Play).unwrap();

    let mut run = |project_fade: bool, render: bool| {
        let mut output = [[0.0; 2]; 16];
        let mut held = false;
        let allocations = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                if project_fade {
                    let mut active = callback
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                        .unwrap();
                    if let Some(pins) = active
                        .reserve_outgoing_fade(&mut callback.domain_pages, 128)
                        .unwrap()
                    {
                        held = true;
                        pins.release(&mut callback.domain_pages);
                    }
                }
                if render {
                    callback.render(&mut output).unwrap();
                }
            });
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        (held, output)
    };
    let deadline = Instant::now() + Duration::from_millis(600);
    while run(false, true).1 != [[0.25; 2]; 16] {
        assert!(Instant::now() < deadline, "predecessor did not play");
        std::thread::yield_now();
    }
    assert!(!run(true, false).0);
    entered
        .recv_timeout(Duration::from_millis(200))
        .expect("the outgoing projection did not request the cold file");
    assert_eq!(run(true, true), (false, [[0.25; 2]; 16]));
    assert_eq!(control.status.position.load(Acquire), 32);
    release.send(()).unwrap();
    while !run(true, false).0 {
        assert!(
            Instant::now() < deadline,
            "ready file did not join the held span"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.position.load(Acquire), 32);
    control.command(PlaybackCommand::Stop).unwrap();
    for _ in 0..8 {
        run(false, true);
    }
    assert_eq!(control.status.position.load(Acquire), 0);
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
    drop(recover(&mut control, &ticket));
    drop(control);
    finish(&mut retirement);
}

enum Insert {
    Seamless,
    Confirmed,
    StopAtFade,
    PageCapacity,
    CancelledAtEnqueue,
    PreparationDeadline,
}

fn check_insert(insert: Insert) {
    let _serial = serial();
    let cancel = matches!(insert, Insert::CancelledAtEnqueue);
    let expire = matches!(insert, Insert::PreparationDeadline);
    let needs_confirmation = !matches!(
        insert,
        Insert::Seamless | Insert::CancelledAtEnqueue | Insert::PreparationDeadline
    );
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
                if callback.device_insert_fade_pins.is_some() {
                    let current = callback
                        .reserve_current_stream_window(pcm.len())
                        .expect("the old fade must coexist with a current window")
                        .expect("page-backed predecessor");
                    assert!(current.complete());
                    current.release(&mut callback.domain_pages);
                }
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
    let token = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        active_context,
    );
    if expire {
        let late = control.stage_verified_device_insert(
            token,
            active_context,
            predecessor,
            None,
            Instant::now(),
            |_| panic!("expired preparation bound a receipt"),
        );
        assert_eq!(
            late.err(),
            Some("verified graph edit preparation timed out")
        );
        assert_eq!(control.submitted_revision(), predecessor.audio_revision);
    }
    let prepare_until = Instant::now()
        + if expire {
            Duration::from_millis(75)
        } else {
            Duration::from_secs(2)
        };
    let (mut ticket, receipt) = control
        .stage_verified_device_insert(
            token,
            active_context,
            predecessor,
            None,
            prepare_until,
            |receipt| {
                assert_eq!(receipt.state(), PublicationReceiptState::Pending);
                if cancel {
                    assert_eq!(
                        receipt.cancel(),
                        crate::prepared::FreshRuntimeCancellationResult::Cancelled
                    );
                }
            },
        )
        .unwrap();
    if expire {
        std::thread::sleep(prepare_until.saturating_duration_since(Instant::now()));
        let position = control.status.position.load(Acquire);
        assert_eq!(block(), [[0.25; 2]; 64]);
        assert_eq!(control.status.position.load(Acquire), position + 64);
        assert_eq!(receipt.state(), PublicationReceiptState::Expired);
        assert_eq!(
            recover(&mut control, &ticket).return_kind,
            crate::AudioCandidateReturnKind::NeverInstalled
        );
    } else if cancel {
        assert_eq!(block(), [[0.25; 2]; 64]);
    } else {
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    }
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
        let token = stream_fixture::prepare_candidate(
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
            .stage_verified_device_insert(
                token,
                active_context,
                predecessor,
                Some(offer),
                Instant::now() + Duration::from_secs(2),
                |_| {},
            )
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
    let (expected_state, expected_source) = if expire {
        (PublicationReceiptState::Expired, predecessor)
    } else if cancel {
        (PublicationReceiptState::Cancelled, predecessor)
    } else if small_cache {
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
    data.owner_mut().unwrap().run(|callback, _| {
        assert!(callback.device_insert_fade_pins.is_none());
    });
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
    if !expire {
        drop(recover(&mut control, &ticket));
    }
    drop(control);
    finish(&mut retirement);
}
