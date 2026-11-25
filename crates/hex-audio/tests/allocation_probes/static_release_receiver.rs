//! Sampler power through actual prepared publication, with independent dry
//! and wet references for the receiver, a second source, and a later onset.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Frame, NodeSignalPhase, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    },
    playback::{Playback, PlaybackCommand},
};
use hex_lattice::{DevicePreparationContext, EMPTY_DEVICE_RESOURCES, TuningTable};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata, NativeDeviceState,
    ParameterValue,
};
use hex_project::{BeatTime, Command, Created, DeviceId, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

fn prepare(
    project: &Project,
    store: &ResidentDeviceAssets,
) -> (Arc<PreparedRevision>, PreparedDeviceResources) {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 20_000_000, &cancel).unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
        project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        resources.compiler_scan_work_on_control(project).unwrap(),
        &cancel,
    )
    .unwrap();
    (Arc::new(revision), resources)
}

#[test]
fn static_receiver_power_fade_and_future_onset_have_no_callback_allocations() {
    for case in 0..3 {
        for block in [1, 7, 64, 512] {
            check(case, block);
        }
    }
}

#[test]
fn gate_suffix_power_cycle_retains_one_running_detector() {
    for block in [1, 7, 64, 512] {
        check(3, block);
    }
}

#[test]
fn compressor_power_cycle_retains_one_running_detector() {
    check(4, 17);
}

#[test]
fn sampler_power_fade_preserves_its_return_send_and_other_feeds() {
    for block in [1, 7, 64, 512] {
        check(5, block);
    }
}

#[test]
fn unproved_compensated_bypass_refuses_without_a_dropout() {
    check(6, 17);
}

fn check(case: usize, block: usize) {
    let static_receiver = matches!(case, 0..=2 | 5);
    let stateful_receiver = matches!(case, 3 | 4);
    const CUT: usize = 129;
    const FADE: usize = 40;
    let mut editor = Editor::new(Project::new(8000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "memory/release.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let mut sampler = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: sampler.region,
    };
    sampler.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.1,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let utility = &hex_devices::utility::UTILITY_DEFINITION;
    use hex_devices::utility::parameter as p;
    let mut state = utility.default_state().unwrap();
    if case == 1 {
        state = utility
            .patch_parameter(&state, p::WIDTH, ParameterValue::Float(2.0))
            .unwrap();
        state = utility
            .patch_parameter(&state, p::BALANCE, ParameterValue::Float(0.5))
            .unwrap();
    } else if case == 3 {
        state = hex_engine::default_native_device_state(hex_devices::gate::GATE_TYPE_ID).unwrap();
    } else if case == 4 {
        state =
            hex_engine::default_native_device_state(hex_devices::compressor::COMPRESSOR_TYPE_ID)
                .unwrap();
    } else if case == 2 {
        state = utility
            .patch_parameter(&state, p::INVERT_LEFT, ParameterValue::Bool(true))
            .unwrap();
    }
    let receiver_state = state.clone();
    let Created::Device(receiver_effect) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("receiver effect");
    };
    if case == 2 {
        let state = utility
            .patch_parameter(
                &utility.default_state().unwrap(),
                p::WIDTH,
                ParameterValue::Float(0.5),
            )
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
    }
    let Created::Track(unrelated) = editor
        .execute(Command::AddMidiTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("unrelated");
    };
    editor
        .execute(Command::Gain {
            track: unrelated,
            gain: 0.25,
        })
        .unwrap();
    if case == 5 {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "shared return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        for (track, gain) in [(track, 0.5), (unrelated, 0.25)] {
            editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain,
                })
                .unwrap();
        }
    }
    if case == 6 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
    }
    let Created::Clip(other_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: unrelated,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: other_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
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
        panic!("clip");
    };
    let mut oracle_editor = Editor::new(editor.project().clone(), 32).unwrap();
    let mut fresh_editor = (static_receiver || stateful_receiver)
        .then(|| Editor::new(editor.project().clone(), 32).unwrap());
    let off = editor
        .project()
        .tempo_map()
        .frame_to_beat(CUT as u64)
        .unwrap();
    let future = editor.project().tempo_map().frame_to_beat(300).unwrap();
    let duration = editor.project().tempo_map().frame_to_beat(16).unwrap();
    let oracle_gate = if static_receiver || stateful_receiver || case == 6 {
        BeatTime::ONE
    } else {
        off
    };
    for (target, gate) in [
        (&mut editor, BeatTime::ONE),
        (&mut oracle_editor, oracle_gate),
    ] {
        target
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: gate,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        target
            .execute(Command::AddMidiNote {
                clip,
                start: future,
                duration,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    if let Some(fresh) = &mut fresh_editor {
        fresh
            .execute(Command::AddMidiNote {
                clip,
                start: future,
                duration,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(
                    8000,
                    vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]],
                )
                .ok()
                .unwrap(),
            ),
            0,
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            4,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let (revision, resources) = prepare(editor.project(), &store);
    let (oracle_revision, oracle_resources) = prepare(oracle_editor.project(), &store);
    let mut oracle =
        SessionPlayback::new_with_device_resources(oracle_revision, &oracle_resources).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let stateful_reference = stateful_receiver.then(|| {
        let fresh_editor = fresh_editor.as_ref().unwrap();
        let mut original =
            SessionPlayback::new_with_device_resources(Arc::clone(&revision), &resources).unwrap();
        let (fresh_revision, fresh_resources) = prepare(fresh_editor.project(), &store);
        let mut fresh =
            SessionPlayback::new_with_device_resources(fresh_revision, &fresh_resources).unwrap();
        stateful_receiver_reference(
            &mut original,
            &mut fresh,
            receiver_effect,
            &receiver_state,
            CUT,
            FADE,
        )
    });
    let mut fresh = fresh_editor
        .as_ref()
        .filter(|_| static_receiver)
        .map(|editor| {
            let (revision, resources) = prepare(editor.project(), &store);
            let mut oracle =
                SessionPlayback::new_with_device_resources(revision, &resources).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.render(&mut [[0.0; 2]; CUT]).unwrap();
            oracle
        });
    let (mut control, mut callback) = prepared::channel(8000).unwrap();
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
        .publish(Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    let mut position = 0;
    let mut fresh_pcm = actual;
    render(
        &mut callback,
        &mut oracle,
        &mut actual[..CUT],
        &mut expected[..CUT],
    );
    assert_eq!(&actual[..CUT], &expected[..CUT]);
    position += CUT;
    let first_off = if static_receiver || stateful_receiver {
        FADE + 24
    } else {
        13
    };
    for (bypassed, count) in [(true, first_off), (false, 13), (true, 7), (false, 13)] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let (revision, resources) = prepare(editor.project(), &store);
        let previous_revision = control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire);
        let applied = control
            .publish(
                Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap(),
            )
            .unwrap();
        let mut done = 0;
        while done < count {
            let n = (count - done).min(block);
            render(
                &mut callback,
                &mut oracle,
                &mut actual[..n],
                &mut expected[..n],
            );
            if let Some(reference) = &stateful_reference {
                check_stateful_reference(
                    &actual[..n],
                    &reference[position..position + n],
                    case,
                    position,
                );
            } else if let Some(fresh) = fresh.as_mut() {
                fresh.render(&mut fresh_pcm[..n]).unwrap();
                for i in 0..n {
                    let gain = FADE.saturating_sub(position + i - CUT) as f32 / FADE as f32;
                    for channel in 0..2 {
                        let background = fresh_pcm[i][channel];
                        let target = background + (expected[i][channel] - background) * gain;
                        assert!(
                            (actual[i][channel] - target).abs() < 0.000_001,
                            "case={case} block={block} frame={}",
                            position + i
                        );
                    }
                }
            } else {
                assert_eq!(
                    &actual[..n],
                    &expected[..n],
                    "case={case} block={block} bypassed={bypassed}"
                );
            }
            done += n;
            position += n;
            assert_eq!(control.status.recovery_position(), position as u64);
        }
        if case == 6 && bypassed {
            assert_eq!(
                control.status.rejected_revision(),
                Some((
                    applied,
                    hex_engine::PlaybackHandoffRejection::SourceHistoryUnavailable
                ))
            );
            assert_eq!(
                control
                    .status
                    .applied_revision
                    .load(std::sync::atomic::Ordering::Acquire),
                previous_revision
            );
            assert_eq!(control.reclaim(), 1, "refused owner returns to control");
            // A terminal compensated-source refusal must not obstruct
            // priority Stop or chase the later onset from a fresh kernel.
            control.stop();
            oracle.apply(SessionPlaybackCommand::Stop).unwrap();
            for frames in [1, 7, 64, 512, 512, 64] {
                render(
                    &mut callback,
                    &mut oracle,
                    &mut actual[..frames],
                    &mut expected[..frames],
                );
                assert_eq!(&actual[..frames], &expected[..frames]);
            }
            assert!(actual[..64].iter().all(|frame| *frame == [0.0; 2]));
            control.request_shutdown();
            zero(measure(|| callback.begin_block()));
            assert_eq!(control.reclaim(), 1);
            return;
        }
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            applied
        );
        assert!(control.status.rejected_revision().is_none());
        control.reclaim();
    }
    let mut oracle = fresh.unwrap_or(oracle);
    while position < 5000 {
        let n = (5000 - position).min(block);
        render(
            &mut callback,
            &mut oracle,
            &mut actual[..n],
            &mut expected[..n],
        );
        if let Some(reference) = &stateful_reference {
            check_stateful_reference(
                &actual[..n],
                &reference[position..position + n],
                case,
                position,
            );
        } else {
            assert_eq!(
                &actual[..n],
                &expected[..n],
                "case={case} block={block} frame={position}: release, future onset or unrelated source changed"
            );
        }
        position += n;
    }
    assert_eq!(
        actual[0], [0.0; 2],
        "all notes end without a chased/stuck gate"
    );
    control.request_shutdown();
    zero(measure(|| {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    }));
    control.reclaim();
}

// Measure both input signals independently from the public prepared graph
// taps. The wet reference applies a single uninterrupted receiver to the
// authored input, the old source fade, and the later fresh note in that order.
fn stateful_receiver_reference(
    original: &mut SessionPlayback,
    fresh: &mut SessionPlayback,
    effect: DeviceId,
    state: &NativeDeviceState,
    cut: usize,
    fade: usize,
) -> Vec<Frame> {
    const FRAMES: usize = 5000;
    let trace = |playback: &mut SessionPlayback| {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut input = vec![[0.0; 2]; FRAMES];
        let mut wet = input.clone();
        let mut mixed = input.clone();
        let mut seen_input = 0;
        let mut seen_output = 0;
        playback
            .render_with_node_signal_observer(
                &mut mixed,
                Some(&mut |node: hex_engine::PostNodeIdentity,
                           phase: NodeSignalPhase,
                           offset: usize,
                           frames: &[hex_engine::Frame]| {
                    if node.device_id == effect.value() {
                        let (target, seen) = match phase {
                            NodeSignalPhase::Input => (&mut input, &mut seen_input),
                            NodeSignalPhase::Output => (&mut wet, &mut seen_output),
                        };
                        assert_eq!(*seen, offset);
                        *seen += frames.len();
                        target[offset..*seen].copy_from_slice(frames);
                    }
                }),
            )
            .unwrap();
        assert_eq!((seen_input, seen_output), (FRAMES, FRAMES));
        (input, wet, mixed)
    };
    let (mut input, _, _) = trace(original);
    let (new_notes, new_wet, mut expected) = trace(fresh);
    assert!(
        input[cut..cut + fade]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.05)
    );
    for (offset, frame) in input[cut..cut + fade].iter_mut().enumerate() {
        let gain = (fade - offset) as f32 / fade as f32;
        *frame = frame.map(|sample| sample * gain);
    }
    input[cut + fade..].copy_from_slice(&new_notes[cut + fade..]);
    let context = DevicePreparationContext {
        sample_rate: 8000,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let prepared = hex_devices::native_registry()
        .unwrap()
        .prepare_runtime_device(state, &context)
        .unwrap();
    match prepared.processor.as_ref().expect("processor factory") {
        hex_lattice::PreparedProcessorFactory::Legacy(factory) => {
            assert_eq!(factory.algorithmic_latency_frames(), Some(0));
            let mut original_effect = factory.create_runtime();
            for block in input.chunks_mut(256) {
                original_effect.process_block(block, &EMPTY_DEVICE_RESOURCES);
            }
        }
        hex_lattice::PreparedProcessorFactory::Owned(factory) => {
            assert_eq!(factory.algorithmic_latency_frames(), Some(0));
            let mut original_effect = factory.create_runtime().unwrap();
            for block in input.chunks_mut(256) {
                original_effect.begin_block();
                original_effect.process_block(block, &EMPTY_DEVICE_RESOURCES);
                original_effect.commit_block();
            }
        }
        _ => panic!("expected native processor factory"),
    }
    for ((whole, replaced), receiver) in expected.iter_mut().zip(new_wet).zip(input) {
        for ((whole, replaced), receiver) in whole.iter_mut().zip(replaced).zip(receiver) {
            *whole += receiver - replaced;
        }
    }
    expected
}

fn check_stateful_reference(actual: &[Frame], expected: &[Frame], case: usize, position: usize) {
    for (offset, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 0.000_002,
                "case={case} frame={}: {actual} vs {expected}",
                position + offset,
            );
        }
    }
}

fn render(
    callback: &mut prepared::Callback,
    oracle: &mut SessionPlayback,
    actual: &mut [[f32; 2]],
    expected: &mut [[f32; 2]],
) {
    oracle.render(expected).unwrap();
    zero(measure(|| {
        callback.begin_block();
        callback.render(actual).unwrap();
    }));
}
fn zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
