//! PCM and route-clock oracles for a prepared latency edit that straddles
//! an already admitted quantized action. Each runtime starts independently;
//! the shifted candidate is a counterfactual, never an installed graph.
use super::*;
use hex_engine::{LaunchQuantization, SessionPlaybackCommand, SessionRouteTarget};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const DELAY: usize = 72;
const BLOCK: usize = 257;
const SIGNAL: Frame = [0.25; 2];

fn revisions() -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    let limiter = &hex_devices::limiter::DEFINITION;
    let state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(0),
        )
        .unwrap();
    let ([dry, delayed], track) = insert_revisions(state);
    assert_eq!(dry.arrangement().main_latency_frames(), 0);
    assert_eq!(delayed.arrangement().main_latency_frames() as usize, DELAY);
    ([dry, delayed], track)
}

fn insert_revisions(
    state: hex_lattice_contract::NativeDeviceState,
) -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "constant input".into(),
            path: "memory/constant.wav".into(),
            source_sample_rate: RATE,
            source_frames: 8192,
            playback_frames: 8192,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(RATE, vec![SIGNAL; 8192]).unwrap(),
    )]);
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let dry = prepare(editor.project());
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let delayed = prepare(editor.project());
    ([dry, delayed], track)
}

fn begin_insert(
    old: Arc<hex_engine::PreparedRevision>,
    next: Arc<hex_engine::PreparedRevision>,
    track: hex_project::TrackId,
    position: usize,
) -> (
    Control,
    Callback,
    AppliedAudioSource,
    CancellablePublicationReceipt,
) {
    let initial = hex_engine::AudioSourceContext {
        project_generation: 5,
        editor_revision: 7,
    };
    let (mut control, mut callback) = channel(RATE).unwrap();
    control
        .publish_unaccounted_pcm_obligation(
            Playback::try_from_prepared_with_session(old).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            initial.project_generation,
            true,
            false,
            Some(initial),
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut vec![[0.0; 2]; position]).unwrap();
    let ticket = control
        .action_with_ticket(PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::ThirtySecond,
        }))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    let predecessor = control.status.recording_source().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_unaccounted_pcm_obligation(
            Playback::try_from_prepared_with_session(next).unwrap(),
            PublicationPolicy::DeviceInsert { predecessor, fence },
            initial.project_generation,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                editor_revision: 8,
                ..initial
            }),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(revision).unwrap();
    (control, callback, predecessor, receipt)
}

#[test]
fn incompatible_latency_insert_preserves_the_admitted_action_and_pcm() {
    let (revisions, track) = revisions();
    for (old, next, position, audible_prefix) in [(0, 1, 2990, 10), (1, 0, 2980, 92)] {
        for partition in [1, BLOCK] {
            let (control, mut callback, predecessor, receipt) = begin_insert(
                Arc::clone(&revisions[old]),
                Arc::clone(&revisions[next]),
                track,
                position,
            );
            let mut actual = [[0.0; 2]; BLOCK];
            let mut render = || {
                for block in actual.chunks_mut(partition) {
                    callback.begin_block();
                    callback.render(block).unwrap();
                }
            };
            #[cfg(feature = "allocation-probe")]
            {
                let counts = allocation_counter::measure(&mut render);
                assert_eq!(
                    (
                        counts.count_total,
                        counts.bytes_total,
                        counts.count_current,
                        counts.bytes_current
                    ),
                    (0, 0, 0, 0),
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            render();
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(
                    PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
                )
            );
            assert_eq!(control.status.recording_source(), Some(predecessor));
            assert!(
                actual[..audible_prefix]
                    .iter()
                    .all(|frame| *frame == SIGNAL)
            );
            assert!(
                actual[audible_prefix..]
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
        }
    }
}

#[test]
fn zero_latency_insert_installs_with_the_admitted_action_intact() {
    let ([old, next], track) = insert_revisions(
        hex_devices::utility::UTILITY_DEFINITION
            .default_state()
            .unwrap(),
    );
    let (control, mut callback, predecessor, receipt) = begin_insert(old, next, track, 2990);
    callback.begin_block();
    let mut actual = [[0.0; 2]; BLOCK];
    callback.render(&mut actual).unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    assert_eq!(
        control.status.recording_source(),
        Some(AppliedAudioSource {
            context: receipt.context().unwrap(),
            audio_revision: receipt.revision(),
            ..predecessor
        })
    );
    assert_eq!(actual[..10], [SIGNAL; 10]);
    assert_eq!(actual[10..], [[0.0; 2]; BLOCK - 10]);
}

#[test]
fn ordinary_workspace_uses_the_same_insert_continuity_law_during_preflight() {
    use super::candidate_tests::{anchored, finish, serial};
    use hex_engine::AudioStorageLedgerReservation;
    let _serial = serial();
    let ([old, next], _) = revisions();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        })
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(old).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let mut render = || {
        let mut pcm = [[0.0; 2]; 64];
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
        });
        pcm
    };
    render();
    control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(render(), [SIGNAL; 64]);
    let predecessor = control.status.recording_source().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(next).unwrap(),
            PublicationPolicy::DeviceInsert { predecessor, fence },
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(revision).unwrap();
    assert_eq!(render(), [SIGNAL; 64]);
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::DeviceInsertNeedsConfirmation)
    );
    assert_eq!(control.status.recording_source(), Some(predecessor));
    control.request_shutdown();
    render();
    control.reclaim();
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn insert_cancel_and_priority_stop_can_each_preserve_the_predecessor() {
    let (revisions, track) = insert_revisions(
        hex_devices::utility::UTILITY_DEFINITION
            .default_state()
            .unwrap(),
    );
    for stop in [false, true] {
        let (control, mut callback, predecessor, receipt) = begin_insert(
            Arc::clone(&revisions[0]),
            Arc::clone(&revisions[1]),
            track,
            2990,
        );
        let barrier = Arc::new(std::sync::Barrier::new(2));
        if stop {
            callback.test_after_fresh_runtime_apply_decision = Some(Arc::clone(&barrier));
        } else {
            callback.test_before_fresh_runtime_apply = Some(Arc::clone(&barrier));
        }
        std::thread::scope(|scope| {
            let handle = scope.spawn(|| callback.begin_block());
            barrier.wait();
            let action = if stop {
                control.stop();
                FreshRuntimeCancellationResult::Applying
            } else {
                FreshRuntimeCancellationResult::Cancelled
            };
            let cancelled = receipt.cancel();
            barrier.wait();
            handle.join().unwrap();
            assert_eq!(cancelled, action);
        });
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert_eq!(
            receipt.state(),
            if stop {
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            } else {
                PublicationReceiptState::Cancelled
            }
        );
        let mut block = [[0.0; 2]; BLOCK];
        callback.render(&mut block).unwrap();
        if stop {
            assert_eq!(block, [[0.0; 2]; BLOCK]);
        } else {
            assert_eq!(block[..10], [SIGNAL; 10]);
            assert_eq!(block[10..], [[0.0; 2]; BLOCK - 10]);
        }
    }
}

fn at_cut(revision: &Arc<hex_engine::PreparedRevision>, frame: usize) -> Playback {
    let mut runtime = Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap();
    runtime.apply(PlaybackCommand::Play);
    runtime.render(&mut vec![[0.0; 2]; frame]).unwrap();
    assert_eq!(runtime.position(), frame as u64);
    runtime
}

#[test]
fn latency_edit_must_keep_the_admitted_stop_track_interval() {
    let (revisions, track) = revisions();
    // At 120 bpm the 1/32 boundaries are 3,000 frames apart. The wider
    // interval after each command contains both the causal and audible cut.
    for (old, next, host, old_cut, next_cut, difference) in [
        (0, 1, 2900, 3000, 3000, None),
        (0, 1, 2990, 3000, 6000, Some(10)),
        (1, 0, 2900, 3000, 3000, None),
        (1, 0, 3010, 6000, 3000, Some(62)),
    ] {
        for partition in [1, BLOCK] {
            let candidate_frame = (host + next * DELAY) - old * DELAY;
            let command = PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::ThirtySecond,
            });
            let mut previous = at_cut(&revisions[old], host);
            let mut candidate = at_cut(&revisions[next], candidate_frame);
            let render = |runtime: &mut Playback, cut| {
                runtime.try_apply_action(command).unwrap();
                let pending = runtime.session_route_status(0).unwrap().pending.unwrap();
                assert_eq!(pending.target, SessionRouteTarget::Silence);
                assert_eq!(pending.performance_frame, cut);
                let mut output = [[0.0; 2]; BLOCK];
                for block in output.chunks_mut(partition) {
                    runtime.render(block).unwrap();
                }
                output
            };
            let expected = render(&mut previous, old_cut);
            let shifted = render(&mut candidate, next_cut);
            assert_eq!(
                expected.iter().zip(shifted).position(|(a, b)| *a != b),
                difference,
                "old={old}, next={next}, host={host}, partition={partition}",
            );
            let predecessor_signal = |offset| {
                if (host + offset) < old_cut as usize + old * DELAY {
                    SIGNAL
                } else {
                    [0.0; 2]
                }
            };
            assert_eq!(expected, std::array::from_fn(predecessor_signal));
        }
    }
}

#[test]
fn rejecting_an_expired_cut_keeps_the_original_audio_and_action() {
    let ([old, next], track) = revisions();
    let (mut control, mut callback) = channel(RATE).unwrap();
    let installed = control
        .publish(Playback::try_from_prepared_with_session(old).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2990]).unwrap();
    let cut = control.capture_fresh_runtime_fence().unwrap();

    let ticket = control
        .action_with_ticket(PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::ThirtySecond,
        }))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 2990);

    let revision = control
        .publish_with_policy(
            Playback::try_from_prepared_with_session(next).unwrap(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(cut),
        )
        .unwrap();
    callback.begin_block();
    let mut actual = [[0.0; 2]; BLOCK];
    callback.render(&mut actual).unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), installed);
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged)),
    );
    assert_eq!(actual[..10], [SIGNAL; 10]);
    assert_eq!(actual[10..], [[0.0; 2]; BLOCK - 10]);
    assert_eq!(control.status.applied_revision.load(Acquire), installed);
    assert!(control.status.playing.load(Acquire));
}
