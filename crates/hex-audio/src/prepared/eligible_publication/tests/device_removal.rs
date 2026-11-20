use super::*;
use crate::prepared::eligible_publication::tests::stream_fixture;
use hex_engine::{AudioCandidateEffects, AudioSourceContext, Frame};
use hex_project::{Command, Editor};
use std::time::{Duration, Instant};

#[test]
fn verified_native_removal_declicks_before_installing() {
    check_native_removal(RemovalEnd::Install);
}

#[test]
fn verified_native_removal_restores_the_stage_on_cancellation() {
    check_native_removal(RemovalEnd::Cancel);
}

#[test]
fn verified_native_removal_restores_the_stage_on_priority_stop() {
    check_native_removal(RemovalEnd::Stop);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn verified_native_removal_restores_the_stage_on_a_cold_render_window() {
    check_native_removal(RemovalEnd::ColdWindow);
}

#[derive(Clone, Copy)]
enum RemovalEnd {
    Install,
    Cancel,
    Stop,
    #[cfg(feature = "stream-test-support")]
    ColdWindow,
}

fn check_native_removal(end: RemovalEnd) {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::Created;

    let _serial = serial();
    const FADE: usize = 240;
    const WET: Frame = [-0.25, 0.25];
    const DRY: Frame = [0.25; 2];
    let (mut control, mut data, errors, mut retirement) = anchored();
    let project = super::readiness::project_with_frames(32_768);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(32_768);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let mut editor = Editor::new(project, 8).unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("utility");
    };
    let context = AudioSourceContext {
        project_generation: 2,
        editor_revision: 7,
    };
    let mut first =
        stream_fixture::owner_with_effects(editor.project(), metadata.clone(), Some(context));
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let quote = AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    let mut limits = reservation(&mut first);
    limits.member_rows += quote.member_rows;
    limits.transfer_rows += quote.transfer_rows;
    limits.handle_rows += quote.handle_rows;
    limits.data_rows = limits.data_rows.max(quote.data_rows);
    limits.max_retained_data_bytes += storage.max_prepared_data_bytes;
    limits.max_construction_peak_data_bytes += storage.max_construction_data_bytes;
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let register = |control: &mut Control, definition| {
        control
            .register_audio_candidate_domain_with_readiness(
                stream_fixture::pair_with_pool(vec![definition], 4),
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                },
            )
            .unwrap_or_else(|error| panic!("{}", error.reason))
    };
    #[cfg(feature = "stream-test-support")]
    let (domain, gate_release) = if matches!(end, RemovalEnd::ColdWindow) {
        let (gate, entered, release) =
            hex_stream::test_support::PageGate::new(hex_stream::PageKey {
                source: metadata.key(),
                page_index: 1,
            });
        let domain = control
            .begin_audio_candidate_domain_preparation_with_gate(
                hex_stream::StreamConfig::with_pool_pages(4),
                vec![definition],
                hex_stream::StreamReadinessReservation {
                    extents_per_role: 2,
                },
                gate,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while control
            .poll_audio_candidate_domain_preparation(domain)
            .unwrap()
            != crate::AudioCandidateDomainPreparationProgress::Ready
        {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        (domain, Some((entered, release)))
    } else {
        (register(&mut control, definition), None)
    };
    #[cfg(not(feature = "stream-test-support"))]
    let domain = register(&mut control, definition);
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
    control.command(PlaybackCommand::Play).unwrap();
    let mut block = |frames: usize| {
        let mut pcm = [[0.0; 2]; FADE];
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut pcm[..frames]).unwrap();
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
    while block(64)[..64] != [WET; 64] {
        assert!(Instant::now() < deadline, "predecessor did not play");
        std::thread::yield_now();
    }
    #[cfg(feature = "stream-test-support")]
    if gate_release.is_some() {
        let cut = hex_engine::STREAM_PAGE_FRAMES as u64 - 48;
        while let Some(remaining) = cut
            .checked_sub(control.status.position.load(Acquire))
            .filter(|remaining| *remaining != 0)
        {
            let count = remaining.min(FADE as u64) as usize;
            assert!(block(count)[..count].iter().all(|frame| *frame == WET));
            assert_eq!(
                control.status.position.load(Acquire),
                cut - remaining + count as u64
            );
        }
        assert_eq!(control.status.position.load(Acquire), cut);
    }
    let predecessor = control.status.applied_audio_source().unwrap();
    assert_eq!(predecessor.context, context);
    let next_context = AudioSourceContext {
        editor_revision: context.editor_revision + 1,
        ..context
    };
    let token = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        next_context,
    );
    let (ticket, receipt) = control
        .stage_verified_device_insert(
            token,
            next_context,
            predecessor,
            None,
            Instant::now() + Duration::from_secs(2),
            |_| {},
        )
        .unwrap();
    let start = control.status.position.load(Acquire);
    let mut elapsed = 0;
    let blocks = if matches!(end, RemovalEnd::Install) {
        &[1, 31, FADE - 32][..]
    } else {
        &[1, 31][..]
    };
    for &frames in blocks {
        let pcm = block(frames);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        for (index, frame) in pcm[..frames].iter().enumerate() {
            let expected = -0.25 + 0.5 * (elapsed + index + 1) as f32 / FADE as f32;
            assert!((frame[0] - expected).abs() < 1e-6, "removal ramp");
            assert_eq!(frame[1], 0.25);
        }
        elapsed += frames;
    }
    match end {
        RemovalEnd::Install => {
            assert_eq!(block(32)[..32], [DRY; 32]);
            assert_eq!(
                control.status.position.load(Acquire),
                start + FADE as u64 + 32
            );
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
            assert_eq!(
                control.status.applied_audio_source(),
                Some(AppliedAudioSource {
                    context: next_context,
                    audio_revision: receipt.revision(),
                    ..predecessor
                })
            );
        }
        RemovalEnd::Cancel => {
            use crate::prepared::FreshRuntimeCancellationResult;
            assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
            let restored = block(FADE);
            assert!(restored[0][0] < -0.25 + 0.5 * elapsed as f32 / FADE as f32);
            assert_eq!(restored[FADE - 1], WET);
            assert_eq!(block(32)[..32], [WET; 32]);
            assert_eq!(
                control.status.position.load(Acquire),
                start + (elapsed + FADE + 32) as u64
            );
            assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
            assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        }
        RemovalEnd::Stop => {
            control.stop();
            assert_eq!(block(32)[..32], [[0.0; 2]; 32]);
            assert_eq!(control.status.position.load(Acquire), 0);
            assert!(!control.status.playing.load(Acquire));
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            );
            control.command(PlaybackCommand::Play).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while block(FADE) != [WET; FADE] {
                assert!(Instant::now() < deadline, "replayed stage was not restored");
                std::thread::yield_now();
            }
            assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        }
        #[cfg(feature = "stream-test-support")]
        RemovalEnd::ColdWindow => {
            let (entered, release) = gate_release.unwrap();
            block(64);
            entered.recv_timeout(Duration::from_millis(250)).unwrap();
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(PlaybackHandoffRejection::StreamedPagesNotReady)
            );
            assert_eq!(control.status.applied_audio_source(), Some(predecessor));
            release.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while block(FADE) != [WET; FADE] {
                assert!(
                    Instant::now() < deadline,
                    "the original stage did not recover"
                );
                std::thread::yield_now();
            }
            assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        }
    }
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
