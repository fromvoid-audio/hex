//! Literal source frames across the real page-backed, consented silent cut.
use super::*;
use hex_engine::MediaPages as _;

const FADE: u64 = 128;
const LATENCY: u64 = 72; // Default Limiter: 1.5 ms at 48 kHz.

fn sample(frame: u64) -> i16 {
    // Positive, below the Limiter ceiling, and distinct at neighboring frames.
    i16::try_from(1024 + (frame * 13) % 8191).unwrap()
}

fn pcm(frame: u64) -> f32 {
    f32::from(sample(frame)) / 32768.0
}

#[test]
fn verified_confirmed_cut_keeps_exact_source_frames_across_a_page_boundary() {
    for partition in [1, 257] {
        check_position(partition);
    }
}

fn check_position(partition: usize) {
    let _serial = serial();
    let (mut control, mut data, errors, mut retirement) = anchored();
    let page = hex_engine::STREAM_PAGE_FRAMES as u64;
    let frames = u32::try_from(page * 3).unwrap();
    let project = super::super::readiness::project_with_frames(u64::from(frames));
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_samples(frames, |frame| sample(u64::from(frame)));
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        })
        .unwrap();
    let quote = AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    let context = AudioSourceContext {
        project_generation: 2,
        editor_revision: 7,
    };
    let mut first = stream_fixture::owner_sources(&project, vec![metadata.clone()], Some(context));
    let mut limits = reservation(&mut first);
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
    // Warm actual file pages on control. This witness tests their simultaneous
    // outgoing/current custody, not the separately covered cold-read scheduler.
    let (stream_control, mut stream_pages) = stream_fixture::pair_with_pool(vec![definition], 4);
    let warmup = Instant::now() + Duration::from_millis(400);
    for index in [0, 1] {
        stream_pages.request_page(metadata.key(), index);
        while stream_pages.page(metadata.key(), index).is_none() {
            stream_pages.pump_ready(2);
            assert!(Instant::now() < warmup, "actual file pages did not warm");
            std::thread::yield_now();
        }
    }
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            (stream_control, stream_pages),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
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
            context.project_generation,
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    control
        .command(PlaybackCommand::PlayFrom(page - 1024))
        .unwrap();
    let status = Arc::clone(&control.status);
    let mut saw_pins = false;
    let mut render = |frames: usize| {
        let mut output = [[0.0; 2]; 257];
        let start = status.position.load(Acquire);
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output[..frames]).unwrap();
                if callback.device_insert_fade_pins.is_some() {
                    saw_pins = true;
                    let current = callback
                        .reserve_current_stream_window(frames)
                        .expect("current page loan coexists with outgoing fade")
                        .expect("actual page-backed predecessor");
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
        (start, output)
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while status.position.load(Acquire) < page - 768 {
        render(partition);
        assert!(Instant::now() < deadline, "initial stream did not play");
        std::thread::yield_now();
    }
    let predecessor = status.applied_audio_source().unwrap();
    assert_eq!(predecessor.context, context);
    let candidate_context = AudioSourceContext {
        editor_revision: context.editor_revision + 1,
        ..context
    };
    let token = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        candidate_context,
    );
    let (refused, receipt) = control
        .stage_verified_device_insert(
            token,
            candidate_context,
            predecessor,
            None,
            Instant::now() + Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
    while receipt.state() == PublicationReceiptState::Pending {
        let (start, output) = render(1);
        assert_eq!(output[0], [pcm(start); 2]);
        assert!(Instant::now() < deadline, "unapproved edit did not refuse");
    }
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::DeviceInsertNeedsConfirmation)
    );
    let consent = control
        .offer_resident_device_insert_consent(&receipt)
        .unwrap();
    drop(recover(&mut control, &refused));
    let fade_target = page - FADE / 2;
    while status.position.load(Acquire) < fade_target {
        let remaining = fade_target - status.position.load(Acquire);
        let frames = partition.min(usize::try_from(remaining).unwrap());
        let (start, output) = render(frames);
        for (offset, frame) in output[..frames].iter().enumerate() {
            assert_eq!(*frame, [pcm(start + offset as u64); 2]);
        }
        assert!(
            Instant::now() < deadline,
            "predecessor did not reach page boundary"
        );
    }
    let token = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        candidate_context,
    );
    let (installed_ticket, receipt) = control
        .stage_verified_device_insert(
            token,
            candidate_context,
            predecessor,
            Some(consent),
            Instant::now() + Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
    let mut fade_start = None;
    let mut cut = None;
    let mut resumed_frames = 0_u64;
    while resumed_frames < FADE + LATENCY + 257 {
        let frames = partition;
        let (start, output) = render(frames);
        if receipt.state() == PublicationReceiptState::Installed {
            let held = cut.expect("the exact held predecessor was observed before installation");
            assert_eq!(start, held + resumed_frames);
            for (offset, actual) in output[..frames].iter().enumerate() {
                let elapsed = resumed_frames + offset as u64;
                let gain = ((elapsed + 1) as f32 / FADE as f32).min(1.0);
                let expected = if elapsed < LATENCY {
                    0.0
                } else {
                    pcm(held + elapsed - LATENCY) * gain
                };
                assert!(
                    actual
                        .iter()
                        .all(|sample| (*sample - expected).abs() < 1e-6),
                    "partition={partition}, resumed={elapsed}, held={held}, actual={actual:?}, expected={expected}"
                );
            }
            resumed_frames += frames as u64;
        } else {
            assert_eq!(status.applied_audio_source(), Some(predecessor));
            for (offset, actual) in output[..frames].iter().enumerate() {
                let source = start + offset as u64;
                let expected = if let Some(fade) = fade_start {
                    if source < fade + FADE {
                        pcm(source) * (1.0 - (source - fade + 1) as f32 / FADE as f32)
                    } else {
                        0.0
                    }
                } else if *actual != [pcm(source); 2] {
                    fade_start = Some(source);
                    pcm(source) * (1.0 - 1.0 / FADE as f32)
                } else {
                    pcm(source)
                };
                assert!(
                    actual
                        .iter()
                        .all(|sample| (*sample - expected).abs() < 1e-6)
                );
            }
            if let Some(fade) = fade_start
                && status.position.load(Acquire) == fade + FADE
            {
                cut = Some(fade + FADE);
            }
        }
        assert!(
            Instant::now() < deadline,
            "confirmed edit stalled: {:?}",
            receipt.state()
        );
        std::thread::yield_now();
    }
    assert!(
        saw_pins,
        "the real outgoing page reservation was never retained"
    );
    let fade_start = fade_start.unwrap();
    assert!(fade_start < page && fade_start + FADE > page);
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    let applied = status.applied_audio_source().unwrap();
    assert_eq!(applied.context, candidate_context);
    assert_eq!(applied.audio_revision, receipt.revision());
    assert_eq!(applied.output_identity, predecessor.output_identity);
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
    drop(recover(&mut control, &installed_ticket));
    drop(control);
    finish(&mut retirement);
}
