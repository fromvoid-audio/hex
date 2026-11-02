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
