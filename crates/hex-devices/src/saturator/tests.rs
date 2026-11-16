use super::*;
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, NativeProcessorRuntime, NativeRuntimeChain,
    OutputTail, PreparedNativeProcessor, PreparedProcessorInstance, ProcessorChainOwner,
    StereoFrame,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

static DEFINITIONS: [&dyn NativeDeviceDefinition; 1] = [&SATURATOR_DEFINITION];

fn registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFINITIONS).expect("valid saturator registry")
}

fn patch(
    state: &NativeDeviceState,
    parameter: ParameterId,
    value: ParameterValue,
) -> NativeDeviceState {
    SATURATOR_DEFINITION
        .patch_parameter(state, parameter, value)
        .expect("valid parameter patch")
}

fn runtime(state: &NativeDeviceState) -> Box<dyn NativeProcessorRuntime> {
    SATURATOR_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("valid preparation")
        .expect("nonlinear processor")
        .create_runtime()
}

fn render(state: &NativeDeviceState, frames: &mut [StereoFrame]) {
    runtime(state).process_block(frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
}

/// Frames for a held input to fill the oversampler and dry delay.
const SETTLE_FRAMES: usize = 32;

/// The output once `frame` has been held long enough to reach the static
/// transfer.
fn settle(state: &NativeDeviceState, frame: StereoFrame) -> StereoFrame {
    let mut frames = [frame; SETTLE_FRAMES];
    render(state, &mut frames);
    frames[SETTLE_FRAMES - 1]
}

fn assert_near(actual: StereoFrame, expected: StereoFrame) {
    for (a, e) in actual.into_iter().zip(expected) {
        assert!((a - e).abs() <= 1.0e-6, "{actual:?} vs {expected:?}");
    }
}

#[test]
fn default_codec_and_descriptors_round_trip_through_the_generic_registry() {
    let registry = registry();
    let state = registry
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    assert_eq!(state.state_version(), 1);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(registry.migrate_state(&state).expect("migration"), state);
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&state, parameter::DRIVE),
        Ok(ParameterValue::Float(0.0))
    );
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&state, parameter::OUTPUT),
        Ok(ParameterValue::Float(0.0))
    );
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&state, parameter::DRY_WET),
        Ok(ParameterValue::Float(1.0))
    );
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&state, parameter::CURVE),
        Ok(ParameterValue::Choice(0))
    );
    assert!(
        DESCRIPTORS
            .iter()
            .all(|descriptor| descriptor.validate_descriptor().is_ok())
    );

    let changed = patch(&state, parameter::DRIVE, ParameterValue::Float(12.0));
    let changed = patch(&changed, parameter::OUTPUT, ParameterValue::Float(-3.0));
    let changed = patch(&changed, parameter::DRY_WET, ParameterValue::Float(0.25));
    let changed = patch(&changed, parameter::CURVE, ParameterValue::Choice(1));
    assert_eq!(
        registry.migrate_state(&changed).expect("changed state"),
        changed
    );
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&changed, parameter::DRIVE),
        Ok(ParameterValue::Float(12.0))
    );
    assert_eq!(
        SATURATOR_DEFINITION.read_parameter(&changed, parameter::CURVE),
        Ok(ParameterValue::Choice(1))
    );
}

#[test]
fn malformed_states_and_parameter_values_are_rejected() {
    let registry = registry();
    let state = registry
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    assert_eq!(
        registry.patch_parameter(&state, parameter::DRIVE, ParameterValue::Float(f32::NAN)),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert!(
        registry
            .patch_parameter(&state, parameter::DRIVE, ParameterValue::Float(36.01))
            .is_err()
    );
    assert!(
        registry
            .patch_parameter(&state, parameter::OUTPUT, ParameterValue::Float(-36.01))
            .is_err()
    );
    assert!(
        registry
            .patch_parameter(&state, parameter::DRY_WET, ParameterValue::Float(1.01))
            .is_err()
    );
    assert!(
        registry
            .patch_parameter(&state, parameter::CURVE, ParameterValue::Choice(2))
            .is_err()
    );

    let wrong_type = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.utility"),
        1,
        vec![0; STATE_BYTES],
    )
    .expect("bounded state");
    assert_eq!(
        registry.migrate_state(&wrong_type),
        Err(DeviceContractError::WrongDeviceType)
    );
    let future = NativeDeviceState::new(SATURATOR_TYPE_ID, 2, state.payload().to_vec())
        .expect("future state wrapper");
    assert_eq!(
        registry.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for length in [0, STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed = NativeDeviceState::new(SATURATOR_TYPE_ID, 1, vec![0; length])
            .expect("bounded malformed payload");
        assert_eq!(
            registry.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let mut invalid_choice = state.payload().to_vec();
    invalid_choice[12] = 2;
    let malformed = NativeDeviceState::new(SATURATOR_TYPE_ID, 1, invalid_choice)
        .expect("bounded malformed choice");
    assert_eq!(
        registry.migrate_state(&malformed),
        Err(DeviceContractError::MalformedState)
    );
}

#[test]
fn soft_clip_matches_known_anchors_and_is_symmetric_at_the_join() {
    let state = registry()
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    let frames = [
        [-f32::MAX, -1.0],
        [-0.500_01, -0.5],
        [-0.25, 0.0],
        [0.5, 0.500_01],
        [1.0, f32::MAX],
        [2.0, -2.0],
    ]
    .map(|frame| settle(&state, frame));

    for frame in frames {
        assert!(frame[0].is_finite() && frame[1].is_finite());
        assert!(frame[0].abs() <= 1.0 && frame[1].abs() <= 1.0);
    }
    // The oversampled ADAA stage is exactly odd.
    assert_eq!(frames[1][1], -frames[3][0]);
    assert_eq!(frames[1][0], -frames[3][1]);
    assert_eq!(frames[2][1], 0.0);
    assert_near(frames[2], [-0.25, 0.0]);
    assert_near([frames[3][0], frames[4][0]], [0.5, 0.75]);
    assert_near([frames[4][1], frames[0][0]], [1.0, -1.0]);
    assert_near(frames[5], [0.875, -0.875]);
}

#[test]
fn digital_clip_drive_output_and_wet_have_their_declared_audible_effects() {
    let registry = registry();
    let defaults = registry
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    let digital = patch(&defaults, parameter::CURVE, ParameterValue::Choice(1));
    assert_near(settle(&digital, [-2.0, 0.25]), [-1.0, 0.25]);
    assert_near(settle(&digital, [2.0, -0.25]), [1.0, -0.25]);

    let driven = patch(&defaults, parameter::DRIVE, ParameterValue::Float(12.0));
    let driven_frame = settle(&driven, [0.25, -0.25]);
    assert!(driven_frame[0] > 0.25);
    assert!(driven_frame[1] < -0.25);

    let output = patch(&defaults, parameter::OUTPUT, ParameterValue::Float(-6.0));
    let output_frame = settle(&output, [0.5, -0.5]);
    assert!(output_frame[0] < 0.5);
    assert!(output_frame[1] > -0.5);

    let dry = patch(&defaults, parameter::DRY_WET, ParameterValue::Float(0.0));
    let dry = patch(&dry, parameter::CURVE, ParameterValue::Choice(1));
    let dry = patch(&dry, parameter::DRIVE, ParameterValue::Float(36.0));
    let dry = patch(&dry, parameter::OUTPUT, ParameterValue::Float(6.0));
    // Zero wet is the exact dry signal, delayed by the declared latency;
    // the non-finite channel stays visible immediately.
    let input = [0.25, f32::from_bits(0x7f80_0000)];
    let mut dry_frames = [input; LATENCY_FRAMES + 1];
    render(&dry, &mut dry_frames);
    for (index, frame) in dry_frames.iter().enumerate() {
        let expected = if index < LATENCY_FRAMES {
            0.0
        } else {
            input[0]
        };
        assert_eq!(frame[0].to_bits(), expected.to_bits());
        assert_eq!(frame[1].to_bits(), input[1].to_bits());
    }
}

#[test]
fn linear_utility_and_saturator_observe_the_declared_device_order() {
    use crate::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
    use hex_lattice_contract::NativeDeviceMetadata;

    let utility_state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().expect("utility default"),
            utility_parameter::GAIN,
            ParameterValue::Float(4.0),
        )
        .expect("utility gain");
    let saturator_state = SATURATOR_DEFINITION
        .patch_parameter(
            &SATURATOR_DEFINITION
                .default_state()
                .expect("saturator default"),
            parameter::CURVE,
            ParameterValue::Choice(1),
        )
        .expect("digital clip");
    let catalog = crate::native_registry().expect("built-in catalog");
    let utility = catalog
        .prepare_device(
            &utility_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("prepared linear utility");
    let saturator = catalog
        .prepare_device(
            &saturator_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("prepared nonlinear saturator");
    let saturator_processor: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::from(saturator.processor.expect("nonlinear runtime"));

    let instance = |device_id, device_type, matrix, definition| PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id,
        device_type,
        matrix,
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition,
    };
    let utility_first = vec![
        instance(2, utility_state.type_id(), utility.matrix, None),
        instance(
            3,
            saturator_state.type_id(),
            saturator.matrix,
            Some(std::sync::Arc::clone(&saturator_processor)),
        ),
    ];
    let saturator_first = vec![
        instance(
            3,
            saturator_state.type_id(),
            saturator.matrix,
            Some(saturator_processor),
        ),
        instance(2, utility_state.type_id(), utility.matrix, None),
    ];

    let render = |instances| {
        let mut chain = NativeRuntimeChain::new(instances).expect("runtime chain");
        let mut signal = [[1.0, -1.0]; SETTLE_FRAMES];
        chain.begin_block();
        chain.process_block(&mut signal).unwrap();
        chain.commit_block();
        signal[SETTLE_FRAMES - 1]
    };
    assert_near(render(utility_first), [1.0, -1.0]);
    assert_near(render(saturator_first), [4.0, -4.0]);
}

#[test]
fn nonlinear_processor_refuses_matrix_compilation_and_reports_oversampling_delay_and_tail() {
    let state = registry()
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    assert_eq!(
        SATURATOR_DEFINITION.prepare_matrix(&state),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );
    assert_eq!(
        SATURATOR_DEFINITION
            .prepare_effect_matrix(&state)
            .expect("identity linear placeholder"),
        StereoMatrix::identity()
    );
    let prepared = SATURATOR_DEFINITION
        .prepare_processor(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("processor prepared")
        .expect("stateful contract processor");
    assert_eq!(prepared.algorithmic_latency_frames(), Some(11));
    let OutputTail::Finite { max_tail_frames } = prepared.output_tail() else {
        panic!("saturator tail must be finite");
    };
    let mut runtime = prepared.create_runtime();
    assert!(runtime.is_quiescent());
    // A held signal leaves history; the declared tail drains it exactly.
    let mut signal = [[0.5, -0.25]; 64];
    runtime.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(!runtime.is_quiescent());
    let mut tail = vec![[0.0; 2]; max_tail_frames as usize];
    runtime.process_block(&mut tail, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(
        tail[..LATENCY_FRAMES + 1]
            .iter()
            .any(|frame| *frame != [0.0; 2])
    );
    assert!(runtime.is_quiescent());
    let mut after = [[0.0; 2]; 64];
    runtime.process_block(&mut after, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(after, [[0.0; 2]; 64]);
    runtime.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    runtime.reset();
    assert!(runtime.is_quiescent());
    assert!(
        SATURATOR_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 7_999,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                }
            )
            .is_err()
    );
}

#[test]
fn non_finite_inputs_remain_visible_to_the_audio_fault_boundary() {
    let state = registry()
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    let mut faulted = [[0.25, -0.5]; 2 * SETTLE_FRAMES];
    faulted[3] = [f32::NAN, f32::INFINITY];
    faulted[4] = [f32::NEG_INFINITY, -0.5];
    render(&state, &mut faulted);
    assert!(faulted[3][0].is_nan());
    assert_eq!(faulted[3][1], f32::INFINITY);
    assert_eq!(faulted[4][0], f32::NEG_INFINITY);
    assert!(faulted[4][1].is_finite());
    // Faults never enter the histories: once the faulted frames leave the
    // oversampler, output matches an unfaulted run of the same signal.
    let mut clean = [[0.25, -0.5]; 2 * SETTLE_FRAMES];
    render(&state, &mut clean);
    assert_eq!(
        faulted[SETTLE_FRAMES + 5..],
        clean[SETTLE_FRAMES + 5..],
        "faulted samples leaked into history"
    );
    assert!(faulted.iter().flatten().filter(|v| !v.is_finite()).count() == 3);
}

#[test]
fn transfer_preview_matches_runtime_and_exposes_output_axis_without_clipping() {
    let default = SATURATOR_DEFINITION.default_state().expect("default state");
    for curve in [0, 1] {
        for (drive, output, wet) in [(0.0, 0.0, 1.0), (24.0, 6.0, 0.75), (-12.0, -6.0, 0.0)] {
            let state = patch(&default, parameter::CURVE, ParameterValue::Choice(curve));
            let state = patch(&state, parameter::DRIVE, ParameterValue::Float(drive));
            let state = patch(&state, parameter::OUTPUT, ParameterValue::Float(output));
            let state = patch(&state, parameter::DRY_WET, ParameterValue::Float(wet));
            let preview = transfer_curve(&state).expect("valid preview");
            for (index, [x, y]) in preview.points.iter().enumerate() {
                let input = index as f32 / 64.0 - 1.0;
                let frame = settle(&state, [input, input]);
                assert!((0.0..=1.0).contains(x) && (0.0..=1.0).contains(y));
                let represented = (y * 2.0 - 1.0) * preview.output_peak;
                assert!(
                    (represented - frame[0]).abs() < 1.0e-6,
                    "{index}: {represented} vs {}",
                    frame[0]
                );
            }
            assert_eq!(preview.points[64], [0.5, 0.5]);
            assert!(preview.output_peak >= 1.0);
            if output > 0.0 {
                assert!(preview.output_peak > 1.0);
            }
        }
    }
    let malformed = NativeDeviceState::new(SATURATOR_TYPE_ID, 1, vec![0]).expect("state");
    assert_eq!(
        transfer_curve(&malformed),
        Err(DeviceContractError::MalformedState)
    );
}

#[test]
fn shared_binding_retarget_and_subset_restore_base_with_unchanged_drive() {
    struct Values(Vec<(ParameterId, ParameterValue)>, Vec<ParameterId>);
    impl Values {
        fn new(items: Vec<(ParameterId, ParameterValue)>) -> Self {
            Self(items.clone(), items.iter().map(|v| v.0).collect())
        }
    }
    impl hex_lattice::NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.1
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            self.0.iter().find(|v| v.0 == id).map(|v| v.1)
        }
    }
    let base = SATURATOR_DEFINITION.default_state().unwrap();
    let parameters = [parameter::DRIVE, parameter::OUTPUT];
    let both = Values::new(vec![
        (parameter::DRIVE, ParameterValue::Float(12.0)),
        (parameter::OUTPUT, ParameterValue::Float(-6.0)),
    ]);
    let drive = Values::new(vec![(parameter::DRIVE, ParameterValue::Float(12.0))]);
    let mut previous = SaturatorDefinition::prepare_owned(&base, 48_000, &parameters)
        .unwrap()
        .create_runtime();
    let mut discard = [[0.25, -0.5]; 4];
    previous
        .process_block_with_automation(
            &mut discard,
            None,
            &both,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    for output in [0.0, 3.0] {
        let new_base = patch(&base, parameter::OUTPUT, ParameterValue::Float(output));
        let mut candidate = SaturatorDefinition::prepare_owned(&new_base, 48_000, &parameters)
            .unwrap()
            .create_runtime();
        candidate.copy_history_from(previous.as_ref());
        let expected_state = patch(&new_base, parameter::DRIVE, ParameterValue::Float(12.0));
        let mut actual = [[0.25, -0.5]; 4];
        candidate
            .process_block_with_automation(
                &mut actual,
                None,
                &drive,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        // The candidate inherits the oversampler history the previous runtime
        // built with drive 12, so the reference renders that history first.
        let mut reference = runtime(&expected_state);
        let mut reference_history = [[0.25, -0.5]; 4];
        reference.process_block(&mut reference_history, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut expected = [[0.25, -0.5]; 4];
        reference.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, expected);
        let invalid = Values::new(vec![(parameter::DRIVE, ParameterValue::Float(f32::NAN))]);
        let mut untouched = [[0.25, -0.5]; 4];
        assert!(
            candidate
                .process_block_with_automation(
                    &mut untouched,
                    None,
                    &invalid,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES
                )
                .is_err()
        );
        assert_eq!(untouched, [[0.25, -0.5]; 4]);
        let mut retry = [[0.25, -0.5]; 4];
        candidate
            .process_block_with_automation(
                &mut retry,
                None,
                &drive,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        let mut expected_retry = [[0.25, -0.5]; 4];
        reference.process_block(&mut expected_retry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(retry, expected_retry);
    }
}

/// Pulse #4085: a direct drive/output edit ramps its linear gains over 5 ms at
/// every sample rate, with bounded per-sample steps and block-size invariance.
#[test]
fn direct_gain_edit_ramps_across_handoff_at_every_rate() {
    let old_state = SATURATOR_DEFINITION.default_state().unwrap();
    let new_state = patch(
        &patch(&old_state, parameter::DRIVE, ParameterValue::Float(12.0)),
        parameter::OUTPUT,
        ParameterValue::Float(-6.0),
    );
    let prepare = |state: &NativeDeviceState, rate| {
        SaturatorDefinition::prepare_owned(state, rate, &[])
            .unwrap()
            .create_runtime()
    };
    let gains = |runtime: &dyn NativeProcessorRuntime| {
        let runtime = runtime.as_any().downcast_ref::<SaturatorRuntime>().unwrap();
        (runtime.drive_gain, runtime.output_gain)
    };
    let resources = &hex_lattice::EMPTY_DEVICE_RESOURCES;
    for rate in [8_000, 44_100, 48_000, 192_000] {
        let old = prepare(&old_state, rate);
        let (old_drive, old_output) = gains(old.as_ref());
        let target = SaturatorDefinition::prepare_owned(&new_state, rate, &[]).unwrap();
        let ramp = rate.div_ceil(200) as usize;
        assert!(
            (ramp as f64 / f64::from(rate) - 0.005).abs() <= 1.0 / f64::from(rate),
            "rate={rate}: {ramp} frames"
        );
        let mut runtime = target.create_runtime();
        runtime.copy_history_from(old.as_ref());
        let mut audio = vec![[0.01_f32, -0.02]; ramp + 8];
        // Steps are measured from the gains the previous runtime last used.
        let (mut drive, mut output) = (old_drive, old_output);
        let (mut largest_drive, mut largest_output) = (0.0_f64, 0.0_f64);
        for frame in audio.iter_mut() {
            runtime.process_block(std::slice::from_mut(frame), resources);
            let (next_drive, next_output) = gains(runtime.as_ref());
            largest_drive = largest_drive.max((next_drive - drive).abs());
            largest_output = largest_output.max((next_output - output).abs());
            (drive, output) = (next_drive, next_output);
        }
        assert_eq!((drive, output), (target.drive_gain, target.output_gain));
        let tolerance = 1.0 + 1.0e-9;
        assert!(
            largest_drive <= (target.drive_gain - old_drive).abs() / ramp as f64 * tolerance,
            "rate={rate}: drive step {largest_drive}"
        );
        assert!(
            largest_output <= (target.output_gain - old_output).abs() / ramp as f64 * tolerance,
            "rate={rate}: output step {largest_output}"
        );
        // Block partitions and scratch-twin handoffs follow the same trajectory.
        let mut chunked = target.create_runtime();
        chunked.copy_history_from(old.as_ref());
        let mut chunked_audio = vec![[0.01_f32, -0.02]; ramp + 8];
        let mut offset = 0;
        for size in [1_usize, 7, 64, 128].into_iter().cycle() {
            if offset == chunked_audio.len() {
                break;
            }
            let end = (offset + size).min(chunked_audio.len());
            let mut twin = target.create_runtime();
            twin.copy_history_from(chunked.as_ref());
            twin.process_block(&mut chunked_audio[offset..end], resources);
            chunked = twin;
            offset = end;
        }
        assert_eq!(audio, chunked_audio, "rate={rate}");
        // Reset jumps straight to the targets.
        let mut reset = target.create_runtime();
        reset.copy_history_from(old.as_ref());
        reset.reset();
        assert_eq!(
            gains(reset.as_ref()),
            (target.drive_gain, target.output_gain)
        );
    }
}

#[test]
fn direct_gain_edits_ramp_even_when_wet_is_automated() {
    struct Wet;
    impl hex_lattice::NativeParameterAutomation for Wet {
        fn parameter_ids(&self) -> &[ParameterId] {
            std::slice::from_ref(&parameter::DRY_WET)
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(1.0))
        }
    }
    let old = SATURATOR_DEFINITION.default_state().unwrap();
    let new = patch(&old, parameter::DRIVE, ParameterValue::Float(12.0));
    let prepare = |state| {
        SaturatorDefinition::prepare_owned(state, 48_000, &[parameter::DRY_WET])
            .unwrap()
            .create_runtime()
    };
    let mut previous = prepare(&old);
    previous
        .process_block_with_automation(
            &mut [[0.1, 0.1]],
            None,
            &Wet,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let old_gain = previous
        .as_any()
        .downcast_ref::<SaturatorRuntime>()
        .unwrap()
        .drive_gain;
    let mut candidate = prepare(&new);
    candidate.copy_history_from(previous.as_ref());
    candidate
        .process_block_with_automation(
            &mut [[0.1, 0.1]],
            None,
            &Wet,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let first = candidate
        .as_any()
        .downcast_ref::<SaturatorRuntime>()
        .unwrap()
        .drive_gain;
    let target = amplitude_from_db(12.0).unwrap();
    assert!(
        first > old_gain && first < target,
        "old={old_gain} first={first} target={target}"
    );
}

#[test]
fn automated_drive_stays_exact_while_direct_output_ramps_across_partitions() {
    struct Drive;
    impl hex_lattice::NativeParameterAutomation for Drive {
        fn parameter_ids(&self) -> &[ParameterId] {
            std::slice::from_ref(&parameter::DRIVE)
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(6.0))
        }
    }
    let old = SATURATOR_DEFINITION.default_state().unwrap();
    let new = patch(&old, parameter::OUTPUT, ParameterValue::Float(-6.0));
    let prepare = |state| {
        SaturatorDefinition::prepare_owned(state, 48_000, &[parameter::DRIVE])
            .unwrap()
            .create_runtime()
    };
    let mut previous = prepare(&old);
    previous
        .process_block_with_automation(
            &mut [[0.1, 0.1]],
            None,
            &Drive,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let mut whole = prepare(&new);
    whole.copy_history_from(previous.as_ref());
    let mut one = [[0.1, -0.1]; 248];
    whole
        .process_block_with_automation(&mut one, None, &Drive, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let state = whole.as_any().downcast_ref::<SaturatorRuntime>().unwrap();
    assert_eq!(state.drive_gain, amplitude_from_db(6.0).unwrap());
    assert_eq!(state.output_gain, amplitude_from_db(-6.0).unwrap());
    let mut chunked = prepare(&new);
    chunked.copy_history_from(previous.as_ref());
    let mut split = [[0.1, -0.1]; 248];
    for chunk in split.chunks_mut(7) {
        let mut twin = prepare(&new);
        twin.copy_history_from(chunked.as_ref());
        twin.process_block_with_automation(
            chunk,
            None,
            &Drive,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
        chunked = twin;
    }
    assert_eq!(one, split);
    let mut bypassed = prepare(&new);
    bypassed.copy_history_from(previous.as_ref());
    bypassed
        .process_bypassed_block_with_automation(
            &mut [[0.1, 0.1]],
            None,
            &Drive,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let state = bypassed
        .as_any()
        .downcast_ref::<SaturatorRuntime>()
        .unwrap();
    assert_eq!(state.drive_gain, amplitude_from_db(6.0).unwrap());
    assert_eq!(state.output_gain, amplitude_from_db(-6.0).unwrap());
}

/// Pulse #4146: both curves at +36 db drive keep summed non-harmonic
/// (aliased) energy of a 7 kHz sine at 48 kHz below -60 db relative to the
/// fundamental. Measured: soft clip -64.3 db, digital clip -62.2 db. The
/// former memoryless base-rate law measured about -14 db for its largest
/// single aliased component (prototype, peak metric).
#[test]
fn full_drive_7khz_sine_aliasing_stays_below_minus_60_db_for_both_curves() {
    use crate::oversampling::spectrum::inharmonic_energy_db;
    let rate = 48_000_u32;
    let count = (1 << 16) + 2048;
    for curve in [0, 1] {
        let state = patch(
            &patch(
                &SATURATOR_DEFINITION.default_state().unwrap(),
                parameter::DRIVE,
                ParameterValue::Float(36.0),
            ),
            parameter::CURVE,
            ParameterValue::Choice(curve),
        );
        let mut frames: Vec<StereoFrame> = (0..count)
            .map(|frame| {
                let value =
                    (std::f64::consts::TAU * 7_000.0 * frame as f64 / f64::from(rate)).sin() as f32;
                [value, value]
            })
            .collect();
        render(&state, &mut frames);
        let energy = inharmonic_energy_db(&frames, 7_000.0, rate);
        assert!(
            energy < -60.0,
            "curve {curve}: inharmonic energy {energy:.1} db"
        );
    }
}

/// A bypassed block advances every history exactly once, whether or not it
/// carries automation, and exactly as an audible block would.
#[test]
fn bypassed_blocks_advance_histories_once_with_or_without_automation() {
    struct Wet;
    impl hex_lattice::NativeParameterAutomation for Wet {
        fn parameter_ids(&self) -> &[ParameterId] {
            std::slice::from_ref(&parameter::DRY_WET)
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(1.0))
        }
    }
    let state = patch(
        &SATURATOR_DEFINITION.default_state().unwrap(),
        parameter::DRIVE,
        ParameterValue::Float(18.0),
    );
    let prepare = || {
        SaturatorDefinition::prepare_owned(&state, 48_000, &[parameter::DRY_WET])
            .unwrap()
            .create_runtime()
    };
    let resources = &hex_lattice::EMPTY_DEVICE_RESOURCES;
    let first: Vec<StereoFrame> = (0..37)
        .map(|frame| {
            [
                (frame as f32 * 0.37).sin() * 0.8,
                (frame as f32 * 0.23).cos() * 0.6,
            ]
        })
        .collect();
    // A bypassed block shorter than the histories exposes a repeated advance.
    let bypassed_input = [[0.7_f32, -0.3], [-0.6, 0.9], [0.2, -0.8]];
    let resumed = |prime: &dyn Fn(&mut dyn NativeProcessorRuntime, &mut [StereoFrame])| {
        let mut runtime = prepare();
        let mut warm = first.clone();
        runtime.process_block(&mut warm, resources);
        let mut bypassed = bypassed_input;
        prime(runtime.as_mut(), &mut bypassed);
        let mut next = [[0.25_f32, -0.5]; 48];
        runtime
            .process_block_with_automation(&mut next, None, &Wet, resources)
            .unwrap();
        (bypassed, next)
    };
    let (automated_dry, automated) = resumed(&|runtime, frames| {
        runtime
            .process_bypassed_block_with_automation(frames, None, &Wet, resources)
            .unwrap()
    });
    let (plain_dry, plain) =
        resumed(&|runtime, frames| runtime.process_bypassed_block(frames, resources));
    let (_, audible) = resumed(&|runtime, frames| runtime.process_block(frames, resources));
    assert_eq!(automated_dry, bypassed_input, "bypass keeps its input");
    assert_eq!(plain_dry, bypassed_input, "bypass keeps its input");
    assert_eq!(automated, plain);
    assert_eq!(automated, audible);
}

#[test]
fn retained_gain_eligibility_preserves_manual_ramps_and_transactional_twins() {
    struct Values([Option<f32>; 2], Vec<ParameterId>);
    impl hex_lattice::NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.1
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            let index = if id == parameter::DRIVE { 0 } else { 1 };
            self.0[index].map(ParameterValue::Float)
        }
    }
    let ids = [parameter::DRIVE, parameter::OUTPUT];
    let resources = &hex_lattice::EMPTY_DEVICE_RESOURCES;
    let old = SATURATOR_DEFINITION.default_state().unwrap();
    let new = patch(
        &patch(&old, parameter::DRIVE, ParameterValue::Float(-6.0)),
        parameter::OUTPUT,
        ParameterValue::Float(-12.0),
    );
    let prepare = |state, admitted: &[ParameterId]| {
        SaturatorDefinition::prepare_owned(state, 48_000, admitted)
            .unwrap()
            .create_runtime()
    };
    let mut previous = prepare(&old, &ids);
    previous
        .process_block_with_automation(
            &mut [[0.2, -0.3]; 128],
            None,
            &Values([Some(18.0), Some(-3.0)], ids.to_vec()),
            resources,
        )
        .unwrap();
    for new in [&old, &new] {
        for active in [[None, None], [Some(18.0), None], [None, Some(-3.0)]] {
            let admitted: Vec<_> = ids
                .into_iter()
                .zip(active)
                .filter_map(|(id, v)| v.map(|_| id))
                .collect();
            let mut oracle = prepare(new, &admitted);
            oracle.copy_history_from(previous.as_ref());
            let mut actual = prepare(new, &ids);
            actual.copy_history_from(previous.as_ref());
            let mut expected = [[0.2, -0.3]; 248];
            oracle
                .process_block_with_automation(
                    &mut expected,
                    None,
                    &Values(active, admitted.clone()),
                    resources,
                )
                .unwrap();
            let mut output = [[0.2, -0.3]; 248];
            for chunk in output.chunks_mut(7) {
                let mut twin = prepare(new, &ids);
                twin.copy_history_from(actual.as_ref());
                twin.process_block_with_automation(
                    chunk,
                    None,
                    &Values(active, ids.to_vec()),
                    resources,
                )
                .unwrap();
                actual = twin;
            }
            assert_eq!(output, expected, "active={active:?}");
            let mut resumed = [[0.2, -0.3]; 32];
            actual
                .process_block_with_automation(
                    &mut resumed,
                    None,
                    &Values([Some(18.0), Some(-3.0)], ids.to_vec()),
                    resources,
                )
                .unwrap();
            let state = actual.as_any().downcast_ref::<SaturatorRuntime>().unwrap();
            assert_eq!(state.drive_gain, amplitude_from_db(18.0).unwrap());
            assert_eq!(state.output_gain, amplitude_from_db(-3.0).unwrap());
        }
    }
}

#[test]
fn shared_lfo_matches_independent_saturator_control_frames() {
    use hex_lattice::{
        ModulationRoute, ModulationTarget, NativeParameterAutomation, PreparedModulation,
    };
    struct Expected;
    impl NativeParameterAutomation for Expected {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[parameter::DRIVE, parameter::DRY_WET]
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            let lfo = [0.0, 1.0, 0.0, -1.0][offset % 4];
            match id {
                parameter::DRIVE => Some(ParameterValue::Float(lfo * 7.2)),
                parameter::DRY_WET => Some(ParameterValue::Float(
                    (1.0 + lfo * 0.25_f32).clamp(0.0, 1.0),
                )),
                _ => None,
            }
        }
    }
    let state = SATURATOR_DEFINITION.default_state().unwrap();
    let ids = [parameter::DRIVE, parameter::DRY_WET];
    let factory = SaturatorDefinition::prepare_owned(&state, 48_000, &ids).unwrap();
    let targets = ids.map(|id| ModulationTarget {
        descriptor: *DESCRIPTORS.iter().find(|d| d.id == id).unwrap(),
        semantics: factory.parameter_modulation_semantics(id).unwrap(),
        base: SATURATOR_DEFINITION.read_parameter(&state, id).unwrap(),
    });
    let program = PreparedModulation::new(
        &targets,
        &[
            ModulationRoute {
                source: 0,
                target: ids[0],
                amount: 0.1,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 0,
                target: ids[1],
                amount: 0.25,
                range: [-1.0, 1.0],
            },
        ],
        1,
    )
    .unwrap();
    let mut lfo =
        hex_dsp::ModulationLfo::new(hex_dsp::LfoRate::free(48_000.0, 12_000.0).unwrap(), 0.0)
            .unwrap();
    let mut samples = [0.0; 64];
    lfo.render(&mut samples);
    let sources = [&samples[..]];
    let block = program.block(None, &sources, samples.len()).unwrap();

    let mut actual_runtime = factory.create_runtime();
    let mut expected_runtime = factory.create_runtime();
    let mut actual = [[0.2, -0.1]; 64];
    let mut expected = actual;
    actual_runtime
        .process_block_with_automation(
            &mut actual,
            None,
            &block,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    expected_runtime
        .process_block_with_automation(
            &mut expected,
            None,
            &Expected,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    for (a, b) in actual.into_iter().zip(expected) {
        assert_near(a, b);
    }
    assert_eq!(SATURATOR_DEFINITION.default_state().unwrap(), state);
}
