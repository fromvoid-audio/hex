//! PCM and route-clock oracles for a prepared latency edit that straddles
//! an already admitted quantized action. Each runtime starts independently;
//! the shifted candidate is a counterfactual, never an installed graph.
use super::*;
use hex_engine::{LaunchQuantization, SessionPlaybackCommand, SessionRouteTarget};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

mod aligned_power;
mod analytic_power;
#[cfg(feature = "allocation-probe")]
mod device_move;
#[cfg(feature = "allocation-probe")]
mod instrument_move;
#[cfg(feature = "allocation-probe")]
mod reorder;
#[cfg(feature = "allocation-probe")]
mod return_removal;
mod reverb_power;
#[cfg(feature = "allocation-probe")]
mod short_main_pass;
#[cfg(feature = "allocation-probe")]
mod track_removal;

const RATE: u32 = 48_000;
const DELAY: usize = 72;
const BLOCK: usize = 257;
const SIGNAL: Frame = [0.25; 2];

pub(super) fn revisions() -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    latency_revisions(vec![SIGNAL; 8192])
}

fn latency_revisions(
    pcm: Vec<Frame>,
) -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    let limiter = &hex_devices::limiter::DEFINITION;
    let state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(0),
        )
        .unwrap();
    let ([dry, delayed], track) = main_append_revisions(None, state, pcm);
    assert_eq!(dry.arrangement().main_latency_frames(), 0);
    assert_eq!(delayed.arrangement().main_latency_frames() as usize, DELAY);
    ([dry, delayed], track)
}

fn insert_revisions(
    state: hex_lattice_contract::NativeDeviceState,
) -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    main_append_revisions(None, state, vec![SIGNAL; 8192])
}

fn main_append_revisions(
    prefix: Option<hex_lattice_contract::NativeDeviceState>,
    state: hex_lattice_contract::NativeDeviceState,
    pcm: Vec<Frame>,
) -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    let (mut editor, track, prepare) = resident_input(pcm);
    if let Some(state) = prefix {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
    }
    let before = prepare(editor.project());
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let after = prepare(editor.project());
    ([before, after], track)
}

fn resident_input(
    pcm: Vec<Frame>,
) -> (
    Editor,
    hex_project::TrackId,
    impl Fn(&Project) -> Arc<hex_engine::PreparedRevision>,
) {
    resident_input_at_rate(pcm, RATE)
}

fn resident_input_at_rate(
    pcm: Vec<Frame>,
    sample_rate: u32,
) -> (
    Editor,
    hex_project::TrackId,
    impl Fn(&Project) -> Arc<hex_engine::PreparedRevision>,
) {
    let mut editor = Editor::new(Project::new(sample_rate).unwrap(), 8).unwrap();
    let frames = pcm.len() as u64;
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "resident input".into(),
            path: "memory/resident.wav".into(),
            source_sample_rate: sample_rate,
            source_frames: frames,
            playback_frames: frames,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let media = BTreeMap::from([(asset, hex_engine::Audio::new(sample_rate, pcm).unwrap())]);
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = move |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    (editor, track, prepare)
}

pub(super) fn begin_insert(
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
    begin_insert_with_pending_track(old, next, Some(track), position)
}

pub(super) fn begin_insert_with_pending_track(
    old: Arc<hex_engine::PreparedRevision>,
    next: Arc<hex_engine::PreparedRevision>,
    track: Option<hex_project::TrackId>,
    position: usize,
) -> (
    Control,
    Callback,
    AppliedAudioSource,
    CancellablePublicationReceipt,
) {
    begin_quantized_insert(old, next, track, position, LaunchQuantization::ThirtySecond)
}

fn begin_quantized_insert(
    old: Arc<hex_engine::PreparedRevision>,
    next: Arc<hex_engine::PreparedRevision>,
    track: Option<hex_project::TrackId>,
    position: usize,
    quantization: LaunchQuantization,
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
    let (mut control, mut callback) = channel(old.arrangement().sample_rate()).unwrap();
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
    if let Some(track) = track {
        let ticket = control
            .action_with_ticket(PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
                track,
                quantization,
            }))
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    }
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
    let sample = |frame: usize| [0.125 + frame as f32 / 65_536.0; 2];
    let (revisions, track) = latency_revisions((0..8192).map(sample).collect());
    for (old, next, position, delay, audible_prefix) in
        [(0, 1, 2990, 0, 10), (1, 0, 2980, DELAY, 92)]
    {
        for partition in [1, BLOCK] {
            let (control, mut callback, predecessor, receipt) = begin_insert(
                Arc::clone(&revisions[old]),
                Arc::clone(&revisions[next]),
                track,
                position,
            );
            let mut actual = [[0.0; 2]; BLOCK];
            let original_generation = receipt.retained_route_generation();
            assert!(
                control
                    .offer_resident_device_insert_consent(&receipt)
                    .is_none()
            );
            let mut render = || {
                callback.begin_block();
                assert_eq!(receipt.state(), PublicationReceiptState::Pending);
                assert_eq!(
                    callback.deferred.as_ref().unwrap().scheduled_cut_frame,
                    Some((position + audible_prefix + 1) as u64),
                );
                for block in actual.chunks_mut(partition) {
                    callback.begin_block();
                    callback.render(block).unwrap();
                }
                callback.begin_block();
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
            let routes = control
                .status
                .session_routes(&mut [SessionRouteEntry::default(); 1])
                .unwrap();
            assert_ne!(routes.route_state_generation, original_generation);
            assert_eq!(
                receipt.retained_route_generation(),
                routes.route_state_generation
            );
            assert_eq!(
                control.status.performance_frame.load(Acquire),
                (position + BLOCK) as u64
            );
            for (offset, frame) in actual[..audible_prefix].iter().enumerate() {
                assert_eq!(*frame, sample(position - delay + offset));
            }
            assert!(
                actual[audible_prefix..]
                    .iter()
                    .all(|frame| *frame == [0.0; 2])
            );
            let consent = control
                .offer_resident_device_insert_consent(&receipt)
                .unwrap();
            assert_eq!(consent.predecessor(), predecessor);
        }
    }
}

#[test]
fn device_insert_keeps_a_non_imminent_action_when_no_bounded_cut_is_available() {
    let (revisions, track) = revisions();
    for (old, next) in [(0, 1), (1, 0)] {
        for partition in [1, BLOCK] {
            let (control, mut callback, predecessor, receipt) = begin_quantized_insert(
                Arc::clone(&revisions[old]),
                Arc::clone(&revisions[next]),
                Some(track),
                512,
                LaunchQuantization::Quarter,
            );
            let generation = receipt.retained_route_generation();
            callback.begin_block();
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(
                    PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
                )
            );
            assert!(callback.deferred.is_none());
            let mut pcm = [[0.0; 2]; BLOCK];
            for output in pcm.chunks_mut(partition) {
                callback.begin_block();
                callback.render(output).unwrap();
            }
            assert_eq!(pcm, [SIGNAL; BLOCK]);
            let mut routes = [SessionRouteEntry::default(); 1];
            let snapshot = control.status.session_routes(&mut routes).unwrap();
            assert_eq!(snapshot.route_state_generation, generation);
            assert_eq!(routes[0].pending.unwrap().performance_frame, 24_000);
            assert_eq!(control.status.recording_source(), Some(predecessor));
            assert!(
                control
                    .offer_resident_device_insert_consent(&receipt)
                    .is_none()
            );
        }
    }
}

#[test]
fn cancel_stop_and_a_new_command_win_while_an_insert_waits_for_the_audible_cut() {
    let (revisions, track) = revisions();
    for action in 0..3 {
        let (mut control, mut callback, predecessor, receipt) = begin_insert(
            Arc::clone(&revisions[0]),
            Arc::clone(&revisions[1]),
            track,
            2990,
        );
        callback.begin_block();
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        match action {
            0 => assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled),
            1 => control.stop(),
            _ => {
                control.command(PlaybackCommand::Play).unwrap();
            }
        }
        callback.begin_block();
        let mut pcm = [[0.0; 2]; BLOCK];
        callback.render(&mut pcm).unwrap();
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert_eq!(
            receipt.state(),
            if action == 0 {
                PublicationReceiptState::Cancelled
            } else {
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            }
        );
        if action == 1 {
            assert_eq!(pcm, [[0.0; 2]; BLOCK]);
            assert_eq!(control.status.position.load(Acquire), 0);
        } else {
            assert_eq!(pcm[..10], [SIGNAL; 10]);
            assert_eq!(pcm[10..], [[0.0; 2]; BLOCK - 10]);
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
fn resident_native_removal_installs_with_an_ordinary_stream_workspace() {
    let ([dry, wet], _) = insert_revisions(
        hex_devices::reverb::REVERB_DEFINITION
            .default_state()
            .unwrap(),
    );
    let (mut control, mut callback) = channel(RATE).unwrap();
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 5,
        editor_revision: 7,
    };
    control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(wet).unwrap(),
            PublicationPolicy::InstallFreshRuntime,
            context,
            |_| Ok::<(), ()>(()),
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; BLOCK]).unwrap();
    let predecessor = control.status.recording_source().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let next_context = hex_engine::AudioSourceContext {
        editor_revision: 8,
        ..context
    };
    let revision = control
        .publish_device_insert_with_receipt_on_control(
            Playback::try_from_prepared_with_session(dry).unwrap(),
            PublicationPolicy::DeviceInsert { predecessor, fence },
            next_context,
            Instant::now() + std::time::Duration::from_secs(10),
            |_| {},
            |_| Ok::<(), ()>(()),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(revision).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; BLOCK]).unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(control.status.recording_source(), Some(predecessor));
    callback.begin_block();
    let mut output = [[0.0; 2]; BLOCK];
    callback.render(&mut output).unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    assert_eq!(output, [SIGNAL; BLOCK]);
    assert_eq!(
        control.status.recording_source(),
        Some(AppliedAudioSource {
            context: next_context,
            audio_revision: revision,
            ..predecessor
        })
    );
}

#[test]
fn removing_matrix_utility_fades_track_return_and_main_at_the_receipt_boundary() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};

    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    const FADE: usize = RATE.div_ceil(200) as usize;
    for location in 0..3 {
        let (mut editor, track, prepare) = resident_input(vec![SIGNAL; 2048]);
        let add = match location {
            0 => Command::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            },
            1 => {
                let Created::ReturnTrack(bus) = editor
                    .execute(Command::AddReturnTrack {
                        name: "return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return");
                };
                editor
                    .execute(Command::SetReturnSend {
                        track,
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
                Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: state.clone(),
                }
            }
            _ => Command::InsertMainDevice {
                before: None,
                state: state.clone(),
            },
        };
        let Created::Device(device) = editor.execute(add).unwrap() else {
            panic!("utility");
        };
        let wet = prepare(editor.project());
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let bypassed = prepare(editor.project());
        editor.execute(Command::RemoveDevice(device)).unwrap();
        let dry = prepare(editor.project());
        let (initial, neutral) = if location == 1 {
            ([0.0, 0.5], [0.5; 2])
        } else {
            ([-0.25, 0.25], [0.25; 2])
        };
        for (old, needs_fade) in [(wet, true), (bypassed, false)] {
            let (control, mut callback, predecessor, receipt) =
                begin_insert_with_pending_track(old, Arc::clone(&dry), None, 64);
            if needs_fade {
                let mut offset = 0;
                for count in [1, 31, FADE - 32] {
                    let mut output = [[0.0; 2]; FADE];
                    callback.begin_block();
                    callback.render(&mut output[..count]).unwrap();
                    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
                    assert_eq!(control.status.recording_source(), Some(predecessor));
                    for (index, frame) in output[..count].iter().enumerate() {
                        let mix = 1.0 - (offset + index + 1) as f32 / FADE as f32;
                        for channel in 0..2 {
                            let expected =
                                neutral[channel] + (initial[channel] - neutral[channel]) * mix;
                            assert!(
                                (frame[channel] - expected).abs() < 1e-6,
                                "location {location}"
                            );
                        }
                    }
                    offset += count;
                }
            }
            let mut after = [[0.0; 2]; 32];
            callback.begin_block();
            callback.render(&mut after).unwrap();
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
            assert_eq!(
                control.status.recording_source(),
                Some(AppliedAudioSource {
                    context: hex_engine::AudioSourceContext {
                        editor_revision: 8,
                        ..predecessor.context
                    },
                    audio_revision: receipt.revision(),
                    ..predecessor
                })
            );
            assert_eq!(after, [neutral; 32]);
        }
    }
}

#[test]
fn removing_matrix_utility_finishes_when_the_session_route_is_silent() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};

    let (mut editor, _, prepare) = resident_input(vec![SIGNAL; 1024]);
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "silent".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    INVERT_LEFT,
                    ParameterValue::Bool(true),
                )
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("utility");
    };
    let old = prepare(editor.project());
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let next = prepare(editor.project());
    let (control, mut callback, predecessor, receipt) =
        begin_quantized_insert(old, next, Some(track), 64, LaunchQuantization::Immediate);
    let mut output = [[0.0; 2]; RATE.div_ceil(200) as usize];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(output, [SIGNAL; RATE.div_ceil(200) as usize]);
    callback.begin_block();
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    assert_ne!(control.status.recording_source(), Some(predecessor));
}

#[test]
fn removing_settled_off_main_effect_preserves_the_downstream_delay_history() {
    use hex_devices::delay::{
        DEFINITION, DRY_WET, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, TIME,
    };

    const CUT: usize = 384;
    const TOTAL: usize = 1_152;
    let mut source = vec![[0.0; 2]; 2_048];
    source[0] = [0.125, -0.0625];
    let (mut editor, _, prepare) = resident_input(source);
    let Created::Device(removed) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("powered-off Main effect");
    };
    editor
        .execute(Command::SetDeviceBypass {
            device: removed,
            bypassed: true,
        })
        .unwrap();

    let mut echo = DEFINITION.default_state().unwrap();
    for parameter in [LEFT_SYNC, FILTER, HIGH_QUALITY] {
        echo = DEFINITION
            .patch_parameter(&echo, parameter, ParameterValue::Bool(false))
            .unwrap();
    }
    for (parameter, value) in [(TIME, 10.0), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
        echo = DEFINITION
            .patch_parameter(&echo, parameter, ParameterValue::Float(value))
            .unwrap();
    }
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: echo,
        })
        .unwrap();
    let old = prepare(editor.project());
    editor.execute(Command::RemoveDevice(removed)).unwrap();
    let next = prepare(editor.project());

    let mut reference = Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap();
    reference.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; TOTAL];
    reference.render(&mut expected).unwrap();
    assert_eq!(
        expected.iter().position(|sample| *sample != [0.0; 2]),
        Some(480)
    );
    assert!(expected[960..].iter().any(|sample| *sample != [0.0; 2]));

    for partition in [1, BLOCK] {
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&next), None, CUT);
        let mut actual = [[0.0; 2]; TOTAL - CUT];
        let render = || {
            for part in actual.chunks_mut(partition) {
                callback.begin_block();
                callback.render(part).unwrap();
                assert_eq!(receipt.state(), PublicationReceiptState::Installed);
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(render);
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
        {
            let mut render = render;
            render();
        }
        assert_eq!(actual, expected[CUT..]);
        assert_eq!(control.status.position.load(Acquire), TOTAL as u64);
        assert!(control.status.playing.load(Acquire));
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                context: receipt.context().unwrap(),
                audio_revision: receipt.revision(),
                ..predecessor
            })
        );
        assert_eq!(control.reclaim(), 1);
    }
}

#[test]
fn approved_main_limiter_append_preserves_a_buffered_delay_repeat() {
    assert_approved_limiter_append_preserves_repeat(LimiterAppendTarget::Main);
}

#[test]
fn approved_return_limiter_append_preserves_buffered_repeats_on_both_returns() {
    assert_approved_limiter_append_preserves_repeat(LimiterAppendTarget::Return);
}

#[test]
fn approved_track_limiter_append_preserves_buffered_repeats_on_both_tracks() {
    assert_approved_limiter_append_preserves_repeat(LimiterAppendTarget::Track);
}

enum LimiterAppendTarget {
    Main,
    Return,
    Track,
}

fn feedback_delay(time_ms: f32) -> hex_lattice_contract::NativeDeviceState {
    use hex_devices::delay::{
        DEFINITION, DRY_WET, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, TIME,
    };

    let mut delay = DEFINITION.default_state().unwrap();
    for parameter in [LEFT_SYNC, FILTER, HIGH_QUALITY] {
        delay = DEFINITION
            .patch_parameter(&delay, parameter, ParameterValue::Bool(false))
            .unwrap();
    }
    for (parameter, value) in [(TIME, time_ms), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
        delay = DEFINITION
            .patch_parameter(&delay, parameter, ParameterValue::Float(value))
            .unwrap();
    }
    delay
}

fn assert_approved_limiter_append_preserves_repeat(target: LimiterAppendTarget) {
    let delay = feedback_delay(10.0);
    let limiter = hex_devices::limiter::DEFINITION.default_state().unwrap();
    let mut source = vec![[0.0; 2]; 2048];
    source[0] = [0.125, -0.0625];
    let ([before, after], old_latency) = if matches!(target, LimiterAppendTarget::Main) {
        (main_append_revisions(Some(delay), limiter, source).0, 0)
    } else {
        let (mut editor, track, prepare) = resident_input(source);
        let asset = editor.project().tracks()[0].clips()[0].asset();
        let Created::Track(other) = editor
            .execute(Command::AddTrack {
                name: "independent delay".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        for command in [
            Command::PlaceClip {
                track: other,
                asset,
                start: 31,
                duration: 1024,
                source_offset: 0,
                source_length: 1024,
                looping: false,
            },
            Command::InsertMainDevice {
                before: None,
                state: limiter.clone(),
            },
        ] {
            editor.execute(command).unwrap();
        }
        let append_limiter = if matches!(target, LimiterAppendTarget::Return) {
            let mut add_delayed_return = |track, name: &str, state| {
                let Created::ReturnTrack(bus) = editor
                    .execute(Command::AddReturnTrack { name: name.into() })
                    .unwrap()
                else {
                    panic!("return");
                };
                editor
                    .execute(Command::SetReturnSend {
                        track,
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
                editor
                    .execute(Command::InsertReturnDevice {
                        return_track: bus,
                        before: None,
                        state,
                    })
                    .unwrap();
                bus
            };
            let bus = add_delayed_return(track, "edited return", delay.clone());
            add_delayed_return(other, "independent return", delay);
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: limiter,
            }
        } else {
            for track in [track, other] {
                editor
                    .execute(Command::InsertDevice {
                        track,
                        before: None,
                        state: delay.clone(),
                    })
                    .unwrap();
            }
            Command::InsertDevice {
                track,
                before: None,
                state: limiter,
            }
        };
        let before = prepare(editor.project());
        editor.execute(append_limiter).unwrap();
        ([before, prepare(editor.project())], DELAY)
    };
    assert_eq!(
        before.arrangement().main_latency_frames() as usize,
        old_latency
    );
    assert_eq!(
        after.arrangement().main_latency_frames() as usize,
        old_latency + DELAY
    );
    assert_approved_latency_cut(before, after, None, |partition, actual, oracle| {
        if !matches!(target, LimiterAppendTarget::Return) {
            assert!(oracle[..CUT].iter().all(|pcm| *pcm == [0.0; 2]));
        }
        let onset = CUT
            + oracle[CUT..]
                .iter()
                .position(|pcm| *pcm != [0.0; 2])
                .unwrap();
        assert!(onset >= CUT + QUANTUM);
        assert_eq!(onset, 480 + old_latency + DELAY);
        if !matches!(target, LimiterAppendTarget::Main) {
            assert_ne!(oracle[onset + 31], [0.0; 2], "independent repeat");
        }
        assert_eq!(
            actual.iter().position(|pcm| *pcm != [0.0; 2]),
            Some(onset - CUT),
            "delayed impulse should survive the approved cut, partition {partition}"
        );
        if !matches!(target, LimiterAppendTarget::Main) {
            assert_eq!(
                actual[onset + 31 - CUT],
                oracle[onset + 31],
                "independent repeat should survive the approved cut, partition {partition}"
            );
        }
        assert_eq!(actual, &oracle[CUT..], "partition {partition}");
    });
}

#[test]
fn approved_latency_cut_preserves_an_independent_native_instrument_voice() {
    assert_approved_cut_preserves_held_native_note(false, InstrumentLatencyEdit::OtherTrack);
}

#[test]
fn approved_latency_cut_preserves_a_held_native_session_note() {
    assert_approved_cut_preserves_held_native_note(true, InstrumentLatencyEdit::OtherTrack);
}

#[test]
fn approved_latency_append_preserves_the_native_arrangement_voice_and_its_running_delay() {
    assert_approved_cut_preserves_held_native_note(false, InstrumentLatencyEdit::InstrumentTrack);
}

#[test]
fn approved_latency_append_preserves_the_native_session_voice_and_its_running_delay() {
    assert_approved_cut_preserves_held_native_note(true, InstrumentLatencyEdit::InstrumentTrack);
}

enum InstrumentLatencyEdit {
    OtherTrack,
    InstrumentTrack,
}

fn assert_approved_cut_preserves_held_native_note(session: bool, edit: InstrumentLatencyEdit) {
    use hex_project::BeatTime;

    let limiter = hex_devices::limiter::DEFINITION.default_state().unwrap();
    let (mut editor, audio_track, prepare) = resident_input(vec![[0.0; 2]; 2048]);
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "held native voice".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_devices::fm4::device::FM4_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    let (clip, launch) = if session {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "held clip".into(),
            })
            .unwrap()
        else {
            panic!("scene");
        };
        let length = BeatTime::from_raw(BeatTime::ONE.raw() / 30);
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "held note".into(),
                source_length: length,
            })
            .unwrap()
        else {
            panic!("session clip");
        };
        editor
            .execute(Command::SetSessionMidiClipSource {
                clip,
                source_length: length,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: length,
                looping: true,
            })
            .unwrap();
        (
            clip,
            Some(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            }),
        )
    } else {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("arrangement clip");
        };
        (clip, None)
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() / 32),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: limiter.clone(),
        })
        .unwrap();
    let (target, effect_delay) = match edit {
        InstrumentLatencyEdit::OtherTrack => (audio_track, 0),
        InstrumentLatencyEdit::InstrumentTrack => {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: feedback_delay(5.0),
                })
                .unwrap();
            (track, 240)
        }
    };
    let before = prepare(editor.project());
    editor
        .execute(Command::InsertDevice {
            track: target,
            before: None,
            state: limiter,
        })
        .unwrap();
    let after = prepare(editor.project());
    assert_eq!(before.arrangement().main_latency_frames() as usize, DELAY);
    assert_eq!(
        after.arrangement().main_latency_frames() as usize,
        2 * DELAY
    );

    assert_approved_latency_cut(before, after, launch, |partition, actual, oracle| {
        // The approved cut may drain and refill compensation and fade in.
        // Compare PCM through the held note and its subsequent release.
        let settled = CUT + 2 * DELAY + QUANTUM;
        // A 1/32-beat gate at 120 bpm and 48 kHz lasts 750 frames.
        let note_off = 750 + 2 * DELAY + effect_delay;
        // The Session clip repeats after 800 frames. Check the release
        // before that wrap as well as the next iteration at its original phase.
        let loop_onset = if session {
            800 + 2 * DELAY + effect_delay
        } else {
            TOTAL
        };
        for range in [settled..note_off, note_off..loop_onset, loop_onset..TOTAL] {
            if range.is_empty() {
                continue;
            }
            assert!(oracle[range.clone()].iter().any(|pcm| *pcm != [0.0; 2]));
            let mismatch = actual[range.start - CUT..range.end - CUT]
                .iter()
                .zip(&oracle[range.clone()])
                .position(|(actual, expected)| actual != expected);
            assert_eq!(mismatch, None, "partition {partition}, frames {range:?}");
        }
    });
}

const QUANTUM: usize = 128;
const CUT: usize = 3 * QUANTUM;
const TOTAL: usize = 1408;

fn assert_approved_latency_cut(
    before: Arc<hex_engine::PreparedRevision>,
    after: Arc<hex_engine::PreparedRevision>,
    launch: Option<SessionPlaybackCommand>,
    check: impl Fn(usize, &[Frame], &[Frame]),
) {
    let mut original = Playback::try_from_prepared_with_session(Arc::clone(&before)).unwrap();
    if let Some(launch) = launch {
        original
            .try_apply_action(PlaybackAction::Session(launch))
            .unwrap();
    }
    original.apply(PlaybackCommand::Play);
    let mut before_approval = [[0.0; 2]; 2 * QUANTUM];
    original.render(&mut before_approval).unwrap();
    let mut uninterrupted = Playback::try_from_prepared_with_session(Arc::clone(&after)).unwrap();
    if let Some(launch) = launch {
        uninterrupted
            .try_apply_action(PlaybackAction::Session(launch))
            .unwrap();
    }
    uninterrupted.apply(PlaybackCommand::Play);
    let mut oracle = [[0.0; 2]; TOTAL];
    uninterrupted.render(&mut oracle).unwrap();

    for partition in [1, BLOCK] {
        let (mut control, mut callback) = channel(RATE).unwrap();
        control.enable_native_custody().unwrap();
        control
            .configure_device_resource_publication_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
            )
            .unwrap();
        let context = hex_engine::AudioSourceContext {
            project_generation: 5,
            editor_revision: 7,
        };
        let next_context = hex_engine::AudioSourceContext {
            editor_revision: 8,
            ..context
        };
        control
            .publish_with_admission_and_source_on_control(
                Playback::try_from_prepared_with_session(Arc::clone(&before)).unwrap(),
                PublicationPolicy::InstallFreshRuntime,
                context,
                |_| Ok::<(), ()>(()),
            )
            .unwrap();
        let mut block = |pcm: &mut [Frame]| {
            let mut render = || {
                callback.begin_block();
                callback.render(pcm).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let counts = allocation_counter::measure(&mut render);
                assert_eq!(
                    (
                        counts.count_total,
                        counts.bytes_total,
                        counts.count_current,
                        counts.bytes_current,
                    ),
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            render();
        };
        if let Some(launch) = launch {
            control.session_command_with_ticket(launch).unwrap();
        }
        control.command(PlaybackCommand::Play).unwrap();
        let mut scratch = [[0.0; 2]; QUANTUM];
        block(&mut scratch);
        assert_eq!(scratch, before_approval[..QUANTUM]);
        let predecessor = control.status.recording_source().unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_device_insert_with_receipt_on_control(
                Playback::try_from_prepared_with_session(Arc::clone(&after)).unwrap(),
                PublicationPolicy::DeviceInsert { predecessor, fence },
                next_context,
                Instant::now() + std::time::Duration::from_secs(10),
                |_| {},
                |_| Ok::<(), ()>(()),
            )
            .unwrap();
        block(&mut scratch);
        assert_eq!(scratch, before_approval[QUANTUM..]);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert_eq!(control.status.position.load(Acquire), (2 * QUANTUM) as u64);
        let refused = control.fresh_runtime_receipt(revision).unwrap();
        assert_eq!(
            refused.state(),
            PublicationReceiptState::Refused(
                PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
            )
        );
        let offer = control
            .offer_resident_device_insert_consent(&refused)
            .unwrap();
        assert_eq!(control.reclaim(), 1);
        let revision = control
            .publish_confirmed_device_insert_with_receipt_on_control(
                Playback::try_from_prepared_with_session(Arc::clone(&after)).unwrap(),
                offer,
                Instant::now() + std::time::Duration::from_secs(10),
                |_| {},
                |_| Ok::<(), ()>(()),
            )
            .unwrap();
        block(&mut scratch);
        assert_eq!(control.status.position.load(Acquire), CUT as u64);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        let approved = control.fresh_runtime_receipt(revision).unwrap();
        assert_eq!(approved.state(), PublicationReceiptState::Applying);
        assert_eq!(control.reclaim(), 0);
        let mut actual = [[0.0; 2]; TOTAL - CUT];
        for pcm in actual.chunks_mut(partition) {
            block(pcm);
        }
        assert_eq!(approved.state(), PublicationReceiptState::Installed);
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                audio_revision: revision,
                context: next_context,
                ..predecessor
            })
        );
        assert_eq!(control.status.position.load(Acquire), TOTAL as u64);
        assert_eq!(control.reclaim(), 1);
        check(partition, &actual, &oracle);
    }
}

#[test]
fn seamless_device_insert_keeps_the_pcm_of_an_independently_rendered_held_note() {
    use hex_project::BeatTime;

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "held note".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(
                project,
                &BTreeMap::new(),
                Arc::clone(&table),
                None,
            )
            .unwrap(),
        )
    };
    let old = prepare(editor.project());
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    let next = prepare(editor.project());
    let cut = 512;
    let mut reference = [[0.0; 2]; BLOCK];
    at_cut(&old, cut).render(&mut reference).unwrap();
    assert!(reference.iter().any(|frame| frame[0].abs() > 0.01));

    for partition in [1, BLOCK] {
        let (control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&next), None, cut);
        let mut actual = [[0.0; 2]; BLOCK];
        for output in actual.chunks_mut(partition) {
            callback.begin_block();
            callback.render(output).unwrap();
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                context: receipt.context().unwrap(),
                audio_revision: receipt.revision(),
                ..predecessor
            }),
        );
        assert_eq!(actual, reference);
    }
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
            Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap(),
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

    let consent = control.offer_device_insert_consent(&receipt).unwrap();
    let position = control.status.position.load(Acquire);
    control.reclaim();
    let mut approved_receipt = None;
    let approved = control
        .publish_confirmed_device_insert_with_receipt_on_control(
            Playback::try_from_prepared_with_session(next).unwrap(),
            consent,
            Instant::now() + std::time::Duration::from_secs(10),
            |receipt| approved_receipt = Some(receipt.clone()),
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let approved_receipt = approved_receipt.unwrap();
    assert_eq!(approved_receipt.revision(), approved);
    let check_fade = || {
        let first = render();
        assert_eq!(first[0], [SIGNAL[0] * 127.0 / 128.0; 2]);
        assert_eq!(first[63], [SIGNAL[0] / 2.0; 2]);
        assert_eq!(approved_receipt.state(), PublicationReceiptState::Applying);
        let second = render();
        assert_eq!(second[63], [0.0; 2]);
        assert_eq!(control.status.position.load(Acquire), position + 128);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        render();
        assert_eq!(approved_receipt.state(), PublicationReceiptState::Installed);
        assert_eq!(control.status.position.load(Acquire), position + 192);
        assert_eq!(
            control.status.recording_source(),
            Some(AppliedAudioSource {
                context: consent.candidate_context(),
                audio_revision: approved,
                ..predecessor
            }),
        );
        assert!(control.status.playing.load(Acquire));
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(check_fade);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current,
            ),
            (0, 0, 0, 0),
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut check_fade = check_fade;
        check_fade();
    }
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

#[test]
fn device_insert_expires_at_callback_without_control_polling() {
    let ([old, next], track) = revisions();
    let (control, mut callback, predecessor, receipt) = begin_insert(old, next, track, 2990);
    callback.begin_block();
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    // Withhold both owners. The next callback must enforce the install
    // deadline independently of a worker which might be occupied by Save.
    std::thread::sleep(std::time::Duration::from_millis(280));
    let mut pcm = [[0.0; 2]; BLOCK];
    let mut render = || {
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
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

    assert_eq!(control.status.recording_source(), Some(predecessor));
    assert_eq!(receipt.state(), PublicationReceiptState::Expired);
    assert!(
        control
            .offer_resident_device_insert_consent(&receipt)
            .is_none()
    );
    assert!(pcm[..10].iter().all(|frame| *frame == SIGNAL));
    assert!(pcm[10..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn resident_auto_pan_power_installs_and_restarts_at_the_callback_boundary() {
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};

    let (mut editor, media) = {
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "resident input".into(),
                path: "memory/resident.wav".into(),
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
        (editor, media)
    };
    let auto_pan = &hex_devices::auto_pan::AUTO_PAN_DEFINITION;
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: auto_pan
                .patch_parameter(
                    &auto_pan.default_state().unwrap(),
                    hex_devices::auto_pan::parameter::RATE,
                    ParameterValue::Float(20.0),
                )
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("Auto Pan insertion");
    };
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: hex_devices::auto_pan::parameter::AMOUNT,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(1.0).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();

    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let on = prepare(editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off = prepare(editor.project());

    let publish = |control: &mut Control, prepared, editor_revision, policy| {
        let context = hex_engine::AudioSourceContext {
            project_generation: 5,
            editor_revision,
        };
        let revision = control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(prepared).unwrap(),
                policy,
                context.project_generation,
                true,
                false,
                Some(context),
            )
            .unwrap();
        (context, revision)
    };
    let (mut control, mut callback) = channel(RATE).unwrap();
    publish(
        &mut control,
        Arc::clone(&on),
        1,
        PublicationPolicy::InstallFreshRuntime,
    );
    control.command(PlaybackCommand::Play).unwrap();

    const FADE: usize = 240; // 5 ms at 48 kHz.
    let mut initial = [[0.0; 2]; 2 * BLOCK];
    callback.begin_block();
    callback.render(&mut initial).unwrap();
    assert_ne!(initial[BLOCK], SIGNAL);

    for (prepared, revision) in [(off, 2), (on, 3)] {
        let (context, audio_revision) = publish(
            &mut control,
            prepared,
            revision,
            PublicationPolicy::InheritRuntime,
        );
        let start = control.status.position.load(Acquire);
        let mut actual = [[0.0; 2]; 2 * BLOCK];
        let mut render = || {
            for block in actual.chunks_mut(BLOCK) {
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
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();

        let applied = control.status.recording_source().unwrap();
        assert_eq!(
            (applied.context, applied.audio_revision),
            (context, audio_revision)
        );
        assert_eq!(
            control.status.position.load(Acquire),
            start + actual.len() as u64
        );
        if revision == 2 {
            assert_eq!(actual[FADE..], [SIGNAL; 2 * BLOCK - FADE]);
        } else {
            assert_eq!(actual[FADE..], initial[FADE..]);
        }
        assert_eq!(control.reclaim(), 1);
    }
}

#[test]
fn resident_gate_power_restarts_closed_and_installs_at_the_callback_boundary() {
    use hex_devices::gate::{GATE_DEFINITION, parameter};

    const QUIET: Frame = [0.025; 2];
    const CLOSED: Frame = [0.000_025; 2]; // Default floor is -60 dB.
    const FADE: usize = 240; // 5 ms at 48 kHz.
    let mut pcm = vec![QUIET; 8192];
    pcm[..BLOCK].fill(SIGNAL);
    let (mut editor, _, prepare) = resident_input(pcm);
    let mut state = GATE_DEFINITION.default_state().unwrap();
    for (parameter, db) in [(parameter::THRESHOLD, -18.0), (parameter::RETURN, -24.0)] {
        state = GATE_DEFINITION
            .patch_parameter(&state, parameter, ParameterValue::Float(db))
            .unwrap();
    }
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("Gate insertion");
    };
    let on = prepare(editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off = prepare(editor.project());

    let (mut control, mut callback) = channel(RATE).unwrap();
    let cases = [
        (
            Arc::clone(&on),
            PublicationPolicy::InstallFreshRuntime,
            SIGNAL,
        ),
        (off, PublicationPolicy::InheritRuntime, QUIET),
        (on, PublicationPolicy::InheritRuntime, CLOSED),
    ];
    for (index, (prepared, policy, settled_frame)) in cases.into_iter().enumerate() {
        let context = hex_engine::AudioSourceContext {
            project_generation: 5,
            editor_revision: index as u64 + 1,
        };
        let audio_revision = control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(prepared).unwrap(),
                policy,
                context.project_generation,
                true,
                false,
                Some(context),
            )
            .unwrap();
        if index == 0 {
            control.command(PlaybackCommand::Play).unwrap();
        }
        let mut block = [[0.0; 2]; BLOCK];
        let mut render = || {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current,
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        let applied = control.status.recording_source().unwrap();
        assert_eq!(
            (applied.context, applied.audio_revision),
            (context, audio_revision)
        );
        assert_eq!(
            control.status.position.load(Acquire),
            ((index + 1) * BLOCK) as u64
        );
        assert_eq!(control.reclaim(), usize::from(index > 0));
        if index == 1 {
            assert_eq!(block[FADE..], [QUIET; BLOCK - FADE]);
        }
        for actual in &block[FADE..] {
            assert!(
                actual
                    .iter()
                    .zip(settled_frame)
                    .all(|(value, expected)| { (value - expected).abs() < 1.0e-9 }),
                "revision {index}: {actual:?}, expected {settled_frame:?}"
            );
        }
    }
}

#[test]
fn resident_eq8_power_reverses_wet_or_restarts_silent_at_the_callback_boundary() {
    use hex_devices::eq8::{EQ8_DEFINITION, parameter};

    let mut state = EQ8_DEFINITION.default_state().unwrap();
    for (parameter, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::frequency(0, 0), ParameterValue::Float(40.0)),
        (parameter::gain(0, 0), ParameterValue::Float(12.0)),
        (parameter::q(0, 0), ParameterValue::Float(8.0)),
    ] {
        state = EQ8_DEFINITION
            .patch_parameter(&state, parameter, value)
            .unwrap();
    }
    resident_filter_power_reverses_wet_or_restarts_silent(state, None);
}

#[test]
fn resident_flexible_eq_power_reverses_wet_or_restarts_silent_at_the_callback_boundary() {
    use hex_devices::eq::{EqState, Node, NodeId};

    let mut state = EqState::default();
    let mut node = Node::new(NodeId::new(1).unwrap(), 40.0, 12.0, 0).unwrap();
    node.q = 8.0;
    state.add(node).unwrap();
    resident_filter_power_reverses_wet_or_restarts_silent(state.encode().unwrap(), None);
}

#[test]
fn resident_utility_dc_filter_power_reverses_wet_or_restarts_silent() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};

    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    resident_filter_power_reverses_wet_or_restarts_silent(state, None);
}

#[test]
fn resident_utility_automated_dc_filter_power_reverses_wet_or_restarts_silent() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};

    resident_filter_power_reverses_wet_or_restarts_silent(
        UTILITY_DEFINITION.default_state().unwrap(),
        Some(parameter::DC_REMOVE),
    );
}

fn resident_filter_power_reverses_wet_or_restarts_silent(
    state: hex_lattice_contract::NativeDeviceState,
    automate_enabled: Option<hex_lattice_contract::ParameterId>,
) {
    const SILENCE: Frame = [0.0; 2];
    const FADE: usize = 240;
    let mut pcm = vec![SILENCE; 8192];
    pcm[..BLOCK].fill(SIGNAL);
    let (mut editor, _, prepare) = resident_input(pcm);
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("filter insertion");
    };
    if let Some(parameter) = automate_enabled {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points: vec![hex_project::AutomationPoint {
                    beat: hex_project::BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(1.0).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    let on = prepare(editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off = prepare(editor.project());

    for off_frames in [64, BLOCK] {
        let (mut control, mut callback) = channel(RATE).unwrap();
        let mut position = 0;
        for (index, (revision, frames)) in [(&on, BLOCK), (&off, off_frames), (&on, BLOCK)]
            .into_iter()
            .enumerate()
        {
            let context = hex_engine::AudioSourceContext {
                project_generation: 5,
                editor_revision: index as u64 + 1,
            };
            let policy = if index == 0 {
                PublicationPolicy::InstallFreshRuntime
            } else {
                PublicationPolicy::InheritRuntime
            };
            let audio_revision = control
                .publish_unaccounted_pcm_obligation(
                    Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap(),
                    policy,
                    context.project_generation,
                    true,
                    false,
                    Some(context),
                )
                .unwrap();
            if index == 0 {
                control.command(PlaybackCommand::Play).unwrap();
            }
            let mut buffer = [SILENCE; BLOCK];
            let block = &mut buffer[..frames];
            let mut render = || {
                callback.begin_block();
                callback.render(block).unwrap();
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
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            render();
            let applied = control.status.recording_source().unwrap();
            assert_eq!(
                (applied.context, applied.audio_revision),
                (context, audio_revision)
            );
            position += frames as u64;
            assert_eq!(control.status.position.load(Acquire), position);
            assert_eq!(control.reclaim(), usize::from(index > 0));
            let has_tail = |samples: &[Frame]| samples.iter().any(|frame| frame[0].abs() > 1.0e-5);
            match index {
                0 => assert_ne!(block[BLOCK - 1], SIGNAL),
                1 => {
                    assert!(has_tail(&block[..frames.min(FADE - 1)]));
                    if frames > FADE {
                        assert_eq!(block[FADE..], [SILENCE; BLOCK - FADE]);
                    }
                }
                _ if off_frames < FADE => assert!(
                    has_tail(&block[FADE..]),
                    "reversal discarded filter history"
                ),
                _ => assert_eq!(
                    block, [SILENCE; BLOCK],
                    "settled filter power kept its old history"
                ),
            }
        }
    }
}
