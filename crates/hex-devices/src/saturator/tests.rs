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
        .prepare_processor(state, 48_000)
        .expect("valid preparation")
        .expect("nonlinear processor")
        .create_runtime()
}

fn render(state: &NativeDeviceState, frames: &mut [StereoFrame]) {
    runtime(state).process_block(frames);
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
    let mut frames = [
        [-f32::MAX, -1.0],
        [-0.500_01, -0.5],
        [-0.25, 0.0],
        [0.5, 0.500_01],
        [1.0, f32::MAX],
        [2.0, -2.0],
    ];
    render(&state, &mut frames);

    for frame in frames {
        assert!(frame[0].is_finite() && frame[1].is_finite());
        assert!(frame[0].abs() <= 1.0 && frame[1].abs() <= 1.0);
    }
    assert_eq!(frames[1][1], -frames[3][0]);
    assert_eq!(frames[1][0], -frames[3][1]);
    assert_eq!(frames[2][0], -0.25);
    assert_eq!(frames[2][1], 0.0);
    assert_eq!(frames[3][0], 0.5);
    assert_eq!(frames[4][0], 0.75);
    assert_eq!(frames[4][1], 1.0);
    assert_eq!(frames[5], [0.875, -0.875]);
}

#[test]
fn digital_clip_drive_output_and_wet_have_their_declared_audible_effects() {
    let registry = registry();
    let defaults = registry
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    let digital = patch(&defaults, parameter::CURVE, ParameterValue::Choice(1));
    let mut frames = [[-2.0, 0.25], [2.0, -0.25]];
    render(&digital, &mut frames);
    assert_eq!(frames[0], [-1.0, 0.25]);
    assert_eq!(frames[1], [1.0, -0.25]);

    let driven = patch(&defaults, parameter::DRIVE, ParameterValue::Float(12.0));
    let mut driven_frame = [[0.25, -0.25]];
    render(&driven, &mut driven_frame);
    assert!(driven_frame[0][0] > 0.25);
    assert!(driven_frame[0][1] < -0.25);

    let output = patch(&defaults, parameter::OUTPUT, ParameterValue::Float(-6.0));
    let mut output_frame = [[0.5, -0.5]];
    render(&output, &mut output_frame);
    assert!(output_frame[0][0] < 0.5);
    assert!(output_frame[0][1] > -0.5);

    let dry = patch(&defaults, parameter::DRY_WET, ParameterValue::Float(0.0));
    let dry = patch(&dry, parameter::CURVE, ParameterValue::Choice(1));
    let dry = patch(&dry, parameter::DRIVE, ParameterValue::Float(36.0));
    let dry = patch(&dry, parameter::OUTPUT, ParameterValue::Float(6.0));
    let mut dry_frame = [[0.25, f32::from_bits(0x7f80_0000)]];
    let untouched = dry_frame;
    render(&dry, &mut dry_frame);
    assert_eq!(dry_frame[0][0].to_bits(), untouched[0][0].to_bits());
    assert_eq!(dry_frame[0][1].to_bits(), untouched[0][1].to_bits());
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
        .prepare_device(&utility_state, 48_000)
        .expect("prepared linear utility");
    let saturator = catalog
        .prepare_device(&saturator_state, 48_000)
        .expect("prepared nonlinear saturator");
    let saturator_processor: std::sync::Arc<dyn PreparedNativeProcessor> =
        std::sync::Arc::from(saturator.processor.expect("nonlinear runtime"));

    let instance = |device_id, device_type, matrix, definition| PreparedProcessorInstance {
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
        let mut signal = [[1.0, -1.0]];
        chain.begin_block();
        chain.process_block(&mut signal);
        chain.commit_block();
        signal[0]
    };
    assert_eq!(render(utility_first), [1.0, -1.0]);
    assert_eq!(render(saturator_first), [4.0, -4.0]);
}

#[test]
fn nonlinear_processor_refuses_matrix_compilation_and_reports_no_delay_or_tail() {
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
        .prepare_processor(&state, 48_000)
        .expect("processor prepared")
        .expect("stateful contract processor");
    assert_eq!(prepared.algorithmic_latency_frames(), Some(0));
    assert_eq!(
        prepared.output_tail(),
        OutputTail::Finite { max_tail_frames: 0 }
    );
    let mut runtime = prepared.create_runtime();
    assert!(runtime.is_quiescent());
    runtime.reset();
    assert!(runtime.is_quiescent());
    assert!(
        SATURATOR_DEFINITION
            .prepare_processor(&state, 7_999)
            .is_err()
    );
}

#[test]
fn non_finite_inputs_remain_visible_to_the_audio_fault_boundary() {
    let state = registry()
        .default_state(SATURATOR_TYPE_ID)
        .expect("default state");
    let mut frames = [[f32::NAN, f32::INFINITY], [f32::NEG_INFINITY, 0.5]];
    render(&state, &mut frames);
    assert!(frames[0][0].is_nan());
    assert_eq!(frames[0][1], f32::INFINITY);
    assert_eq!(frames[1][0], f32::NEG_INFINITY);
    assert_eq!(frames[1][1], 0.5);
}
