use super::*;
use hex_lattice::{NativeInstrumentClock, NativeParameterAutomation};
struct Values(Vec<(ParameterId, ParameterValue)>, Vec<ParameterId>);
impl Values {
    fn new(values: Vec<(ParameterId, ParameterValue)>) -> Self {
        let ids = values.iter().map(|v| v.0).collect();
        Self(values, ids)
    }
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.1
    }
    fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
        self.0.iter().find(|v| v.0 == id).map(|v| v.1)
    }
}
fn signal() -> Vec<StereoFrame> {
    (0..257)
        .map(|i| {
            let a = ((i as f32) * 0.17).sin() * 0.6;
            [a, a * 0.23 - 0.07]
        })
        .collect()
}
fn static_render(state: &NativeDeviceState, input: &[StereoFrame]) -> Vec<StereoFrame> {
    let matrix = UTILITY_DEFINITION.prepare_effect_matrix(state).unwrap();
    let mut output = input.to_vec();
    matrix.apply_block(&mut output);
    if let Some(factory) = UtilityDefinition::prepare_owned_filter(state, 48_000).unwrap() {
        factory
            .create_runtime()
            .process_block(&mut output, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    }
    output
}
#[test]
fn every_control_and_combined_configuration_match_the_original_static_law() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let input = signal();
    let chosen: Vec<_> = DESCRIPTORS
        .iter()
        .map(|d| {
            let value = match d.kind {
                ParameterKind::Float { min, max, .. } => {
                    ParameterValue::Float(min + (max - min) * 0.37)
                }
                ParameterKind::Bool => ParameterValue::Bool(true),
                ParameterKind::Choice { labels } => {
                    ParameterValue::Choice((labels.len() - 1) as u32)
                }
            };
            (d.id, value)
        })
        .collect();
    for values in chosen
        .iter()
        .copied()
        .map(|value| vec![value])
        .chain(std::iter::once(chosen.clone()))
    {
        let mut target = base.clone();
        for (id, value) in &values {
            target = UTILITY_DEFINITION
                .patch_parameter(&target, *id, *value)
                .unwrap();
        }
        let factory = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000).unwrap();
        let mut runtime = factory.create_runtime();
        let mut actual = input.clone();
        runtime
            .process_block_with_automation(
                &mut actual,
                None,
                &Values::new(values),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(actual, static_render(&target, &input));
        assert_eq!(
            UTILITY_DEFINITION.default_state().unwrap(),
            base,
            "effective writes do not alter the base"
        );
    }
}
struct Clock;
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        Some(u64::MAX)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64)
    }
}
#[test]
fn borrowed_program_timing_bypass_and_missing_content_origin_are_explicit() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let descriptor = DESCRIPTORS[0];
    let semantics = UTILITY_DEFINITION
        .parameter_semantics_for_state(&base, descriptor.id)
        .unwrap()
        .unwrap();
    let program = hex_lattice::PreparedParameterAutomation::new(
        vec![hex_lattice::ParameterAutomationLane {
            descriptor,
            semantics,
            points: 0..2,
        }],
        vec![
            hex_lattice::ParameterAutomationPoint {
                beat_raw: 0,
                normalized: 0.0,
                step: false,
            },
            hex_lattice::ParameterAutomationPoint {
                beat_raw: 4,
                normalized: 1.0,
                step: true,
            },
        ],
    )
    .unwrap();
    let factory = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000).unwrap();
    let mut runtime = factory.create_runtime();
    let block = program.block(Some(&Clock), 5);
    let mut audible = [[0.01, 0.02]; 5];
    runtime
        .process_block_with_automation(
            &mut audible,
            Some(&Clock),
            &block,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    for (offset, actual) in audible.iter().enumerate() {
        let value = block.value_at(descriptor.id, offset).unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(&base, descriptor.id, value)
            .unwrap();
        assert_eq!(*actual, static_render(&state, &[[0.01, 0.02]])[0]);
    }
    let mut dry = [[0.01, 0.02]; 5];
    runtime
        .process_bypassed_block_with_automation(
            &mut dry,
            Some(&Clock),
            &block,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(dry, [[0.01, 0.02]; 5]);
    let mut restored = [[0.01, 0.02]; 5];
    let missing = program.block(None, 5);
    runtime
        .process_block_with_automation(
            &mut restored,
            None,
            &missing,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(
        restored,
        static_render(&base, &[[0.01, 0.02]; 5]).as_slice()
    );
}
#[test]
fn speculative_copy_retains_effective_filter_settings_and_replays_identically() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let factory = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000).unwrap();
    let mut primary = factory.create_runtime();
    let mut scratch = factory.create_runtime();
    let controls = Values::new(vec![
        (parameter::DC_REMOVE, ParameterValue::Bool(true)),
        (parameter::BASS_MONO, ParameterValue::Bool(true)),
        (parameter::BASS_MONO_CUTOFF, ParameterValue::Float(193.0)),
    ]);
    let mut first = signal();
    primary
        .process_block_with_automation(
            &mut first,
            None,
            &controls,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    scratch.copy_history_from(primary.as_ref());
    let mut speculative = signal();
    scratch
        .process_block_with_automation(
            &mut speculative,
            None,
            &controls,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    scratch.copy_history_from(primary.as_ref());
    let mut retry = signal();
    scratch
        .process_block_with_automation(
            &mut retry,
            None,
            &controls,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(retry, speculative);
    // Ordinary processing after tail detachment freezes the complete effective law.
    let mut frozen = signal();
    primary.process_block(&mut frozen, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(frozen, speculative);
    primary.reset();
    let mut reset = signal();
    primary.process_block(&mut reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(reset, static_render(&base, &signal()));
}

#[test]
fn registry_shared_automation_requires_explicit_factory_and_current_parameter_support() {
    let registry = crate::native_registry().unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let identity =
        hex_lattice::ProcessorInstanceIdentity::new(hex_lattice::ProcessorChainOwner::Track(1), 2)
            .unwrap();
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let ids: Vec<_> = DESCRIPTORS.iter().map(|d| d.id).collect();
    let prepared = registry
        .prepare_runtime_instance_with_automation(identity, &base, &context, &ids)
        .unwrap();
    assert!(prepared.matrix.is_identity());
    let factory = prepared.processor.unwrap();
    for id in ids {
        assert!(factory.supports_parameter_automation(id));
        assert!(factory.parameter_automation_descriptor(id).is_some());
    }
    assert!(
        registry
            .prepare_runtime_instance_with_automation(
                identity,
                &base,
                &context,
                &[ParameterId::from_static(100)]
            )
            .is_err()
    );
    assert!(
        registry
            .prepare_runtime_instance_with_automation(
                identity,
                &base,
                &context,
                &[ParameterId::from_static(u32::MAX)]
            )
            .is_err()
    );
    let unsupported = crate::compressor::COMPRESSOR_DEFINITION
        .default_state()
        .unwrap();
    assert!(
        registry
            .prepare_runtime_instance_with_automation(
                identity,
                &unsupported,
                &context,
                &[crate::compressor::parameter::THRESHOLD]
            )
            .is_err()
    );
}

#[test]
fn scoped_gain_only_filters_skip_safely_and_enableable_lanes_keep_dormant_history() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let gain_factory = UtilityDefinition::prepare_owned_automated_filter_for_parameters(
        &base,
        48_000,
        &[parameter::GAIN],
    )
    .unwrap();
    assert!(gain_factory.supports_parameter_automation(parameter::GAIN));
    assert!(!gain_factory.supports_parameter_automation(parameter::DC_REMOVE));
    assert!(!gain_factory.supports_parameter_automation(parameter::BASS_MONO));
    let unadmitted = Values::new(vec![(parameter::DC_REMOVE, ParameterValue::Bool(true))]);
    assert!(gain_factory.1.unwrap().resolve(&unadmitted).is_err());
    let descriptor = *DESCRIPTORS
        .iter()
        .find(|d| d.id == parameter::DC_REMOVE)
        .unwrap();
    let semantics = UTILITY_DEFINITION
        .parameter_semantics_for_state(&base, descriptor.id)
        .unwrap()
        .unwrap();
    let unadmitted_program = hex_lattice::PreparedParameterAutomation::new(
        vec![hex_lattice::ParameterAutomationLane {
            descriptor,
            semantics,
            points: 0..1,
        }],
        vec![hex_lattice::ParameterAutomationPoint {
            beat_raw: 0,
            normalized: 1.0,
            step: true,
        }],
    )
    .unwrap();
    let scoped = hex_lattice::PreparedRuntimeInstance {
        modulation: None,
        automation: unadmitted_program,
        meter: Default::default(),
        owner: hex_lattice::ProcessorChainOwner::Track(1),
        device_id: 2,
        device_type: UTILITY_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(std::sync::Arc::new(base.clone())),
        definition: Some(hex_lattice::PreparedProcessorFactory::Legacy(
            std::sync::Arc::new(
                UtilityDefinition::prepare_owned_automated_filter_for_parameters(
                    &base,
                    48_000,
                    &[parameter::GAIN],
                )
                .unwrap(),
            ),
        )),
    };
    assert!(matches!(
        hex_lattice::NativeRuntimeChain::new_prepared(vec![scoped]),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
    let mut skipped = gain_factory.create_runtime();
    let mut first = signal();
    let gain = Values::new(vec![(parameter::GAIN, ParameterValue::Float(1.0))]);
    skipped
        .process_block_with_automation(
            &mut first,
            None,
            &gain,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(first, signal());
    let modes = [parameter::DC_REMOVE, parameter::BASS_MONO];
    let mode_factory =
        UtilityDefinition::prepare_owned_automated_filter_for_parameters(&base, 48_000, &modes)
            .unwrap();
    let on = Values::new(
        modes
            .into_iter()
            .map(|id| (id, ParameterValue::Bool(true)))
            .collect(),
    );
    let off = Values::new(
        modes
            .into_iter()
            .map(|id| (id, ParameterValue::Bool(false)))
            .collect(),
    );
    let mut enabled = mode_factory.create_runtime();
    enabled.copy_history_from(skipped.as_ref());
    let mut actual = signal();
    enabled
        .process_block_with_automation(&mut actual, None, &on, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut fresh = mode_factory.create_runtime();
    let mut expected = signal();
    fresh
        .process_block_with_automation(
            &mut expected,
            None,
            &on,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(
        actual, expected,
        "newly enabled modes cannot revive histories from before a skipped span"
    );
    let mut eligible = mode_factory.create_runtime();
    let mut conservative = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000)
        .unwrap()
        .create_runtime();
    for values in [&off, &on, &off, &on] {
        let mut actual = signal();
        let mut expected = signal();
        eligible
            .process_block_with_automation(
                &mut actual,
                None,
                values,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        conservative
            .process_block_with_automation(
                &mut expected,
                None,
                values,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(
            actual, expected,
            "admitted enable lanes retain their complete dormant history"
        );
    }
}

#[test]
fn binding_preserves_disabled_mid_side_balance_and_automation_uses_latent_value() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let latent = UTILITY_DEFINITION
        .patch_parameter(
            &base,
            parameter::MID_SIDE_BALANCE,
            ParameterValue::Float(0.75),
        )
        .unwrap();
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&latent, parameter::MID_SIDE_ENABLED)
            .unwrap(),
        ParameterValue::Bool(false)
    );
    assert_eq!(
        UTILITY_DEFINITION
            .read_parameter(&latent, parameter::MID_SIDE_BALANCE)
            .unwrap(),
        ParameterValue::Float(0.75)
    );
    let mut runtime = UtilityDefinition::prepare_owned_automated_filter(&latent, 48_000)
        .unwrap()
        .create_runtime();
    let enabled = Values::new(vec![(
        parameter::MID_SIDE_ENABLED,
        ParameterValue::Bool(true),
    )]);
    let input = signal();
    let mut actual = input.clone();
    runtime
        .process_block_with_automation(
            &mut actual,
            None,
            &enabled,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    let expected = UTILITY_DEFINITION
        .patch_parameter(
            &latent,
            parameter::MID_SIDE_ENABLED,
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert_eq!(actual, static_render(&expected, &input));
}
#[test]
fn copied_effective_cache_never_hides_changed_base_or_removed_lane() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let input = signal();
    let both = Values::new(vec![
        (parameter::GAIN, ParameterValue::Float(2.0)),
        (parameter::WIDTH, ParameterValue::Float(3.0)),
    ]);
    let gain = Values::new(vec![(parameter::GAIN, ParameterValue::Float(2.0))]);
    let mut previous = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000)
        .unwrap()
        .create_runtime();
    let mut old = input.clone();
    previous
        .process_block_with_automation(&mut old, None, &both, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    for changed_base in [false, true] {
        let new_base = if changed_base {
            UTILITY_DEFINITION
                .patch_parameter(&base, parameter::WIDTH, ParameterValue::Float(2.0))
                .unwrap()
        } else {
            base.clone()
        };
        let mut candidate = UtilityDefinition::prepare_owned_automated_filter(&new_base, 48_000)
            .unwrap()
            .create_runtime();
        candidate.copy_history_from(previous.as_ref());
        let mut actual = input.clone();
        candidate
            .process_block_with_automation(
                &mut actual,
                None,
                &gain,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        let expected = UTILITY_DEFINITION
            .patch_parameter(&new_base, parameter::GAIN, ParameterValue::Float(2.0))
            .unwrap();
        assert_eq!(actual, static_render(&expected, &input));
    }
}
#[test]
fn failed_derived_configuration_keeps_controls_cache_atomic_for_same_value_retry() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let mut automation = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000)
        .unwrap()
        .1
        .unwrap();
    let values = Values::new(vec![(parameter::GAIN, ParameterValue::Float(2.0))]);
    let mut resolved = automation.resolve(&values).unwrap();
    assert!(
        automation
            .configuration(Some((&values, &mut resolved)), 0, 0, None)
            .is_err()
    );
    assert_eq!(
        automation.controls.effective().settings(),
        decode_state(&base).unwrap()
    );
    let (matrix, _) = automation
        .configuration(Some((&values, &mut resolved)), 0, 48_000, None)
        .unwrap();
    let expected = UTILITY_DEFINITION
        .patch_parameter(&base, parameter::GAIN, ParameterValue::Float(2.0))
        .unwrap();
    assert_eq!(
        matrix,
        UTILITY_DEFINITION.prepare_effect_matrix(&expected).unwrap()
    );
}

#[test]
fn bypass_advances_the_same_filter_and_automated_control_history() {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let factory = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000).unwrap();
    let values = |muted| {
        Values::new(vec![
            (parameter::MUTE, ParameterValue::Bool(muted)),
            (parameter::DC_REMOVE, ParameterValue::Bool(true)),
            (parameter::BASS_MONO, ParameterValue::Bool(true)),
        ])
    };
    let unmuted = values(false);
    for muted in [false, true] {
        let values = values(muted);
        let mut active = factory.create_runtime();
        let mut bypassed = factory.create_runtime();
        let input = signal();
        // Seed an audible residual before the automation can mute the output.
        for runtime in [&mut active, &mut bypassed] {
            let mut warmup = input.clone();
            runtime
                .process_block_with_automation(
                    &mut warmup,
                    None,
                    &unmuted,
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                )
                .unwrap();
            assert!(!runtime.is_quiescent());
        }
        let mut wet = input.clone();
        let mut dry = input.clone();
        active
            .process_block_with_automation(
                &mut wet,
                None,
                &values,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        bypassed
            .process_bypassed_block_with_automation(
                &mut dry,
                None,
                &values,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(dry, input);
        let mut after_active = signal();
        let mut after_bypass = after_active.clone();
        active
            .process_block_with_automation(
                &mut after_active,
                None,
                &unmuted,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        bypassed
            .process_block_with_automation(
                &mut after_bypass,
                None,
                &unmuted,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(after_bypass, after_active);
    }
}

#[test]
fn shared_lfo_drives_two_utility_controls_on_each_sample() {
    use hex_lattice::{
        ModulationRoute, ModulationSourceId, ModulationTarget, PreparedLfoSource,
        PreparedModulation,
    };
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let factory = UtilityDefinition::prepare_owned_automated_filter(&base, 48_000).unwrap();
    let ids = [parameter::GAIN, parameter::BALANCE];
    let targets = ids.map(|id| ModulationTarget {
        descriptor: *DESCRIPTORS.iter().find(|d| d.id == id).unwrap(),
        semantics: factory.parameter_modulation_semantics(id).unwrap(),
        base: UTILITY_DEFINITION.read_parameter(&base, id).unwrap(),
    });
    let program = PreparedModulation::new(
        &targets,
        &[
            ModulationRoute {
                source: 0,
                target: ids[0],
                amount: 0.01,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 0,
                target: ids[1],
                amount: 0.2,
                range: [-1.0, 1.0],
            },
        ],
        1,
    )
    .unwrap();
    let source_id = ModulationSourceId::new(1).unwrap();
    let source = PreparedLfoSource::tempo(source_id, 48_000.0, 120.0, 1.0 / 6_000.0, 0.0).unwrap();

    let declaration = hex_lattice::PreparedModulationState::new(vec![source], program, 2).unwrap();
    let instance = hex_lattice::PreparedRuntimeInstance {
        modulation: Some(declaration),
        automation: Default::default(),
        meter: Default::default(),
        owner: hex_lattice::ProcessorChainOwner::Track(1),
        device_id: 2,
        device_type: UTILITY_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(std::sync::Arc::new(base.clone())),
        definition: Some(hex_lattice::PreparedProcessorFactory::Legacy(
            std::sync::Arc::new(factory),
        )),
    };
    let mut wrong_rate = instance.clone();
    wrong_rate.sample_rate = 44_100;
    assert!(matches!(
        hex_lattice::NativeRuntimeChain::new_prepared(vec![wrong_rate]),
        Err(DeviceContractError::InvalidParameterValue)
    ));
    let mut undeclared = instance.clone();
    let mut invalid_targets = targets;
    invalid_targets[0].semantics.modulation =
        hex_lattice_contract::ParameterModulation::Unsupported;
    undeclared.modulation = Some(
        hex_lattice::PreparedModulationState::new(
            vec![source],
            PreparedModulation::new(&invalid_targets, &[], 1).unwrap(),
            2,
        )
        .unwrap(),
    );
    assert!(matches!(
        hex_lattice::NativeRuntimeChain::new_prepared(vec![undeclared]),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
    let mut chain = hex_lattice::NativeRuntimeChain::new_prepared(vec![instance.clone()]).unwrap();
    chain.begin_block();
    let input = [[0.25, -0.1]; 5];
    let mut output = input;
    chain.process_block(&mut output).unwrap();
    for (index, lfo) in [0.0, 1.0, 0.0, -1.0, 0.0].into_iter().enumerate() {
        let gain = (1.0 + lfo * 0.01 * MAX_UTILITY_GAIN).clamp(0.0, MAX_UTILITY_GAIN);
        let balance = lfo * 0.4;
        let state = UTILITY_DEFINITION
            .patch_parameter(&base, ids[0], ParameterValue::Float(gain))
            .unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(&state, ids[1], ParameterValue::Float(balance))
            .unwrap();
        let expected = static_render(&state, &input[index..index + 1])[0];
        for channel in 0..2 {
            assert!((output[index][channel] - expected[channel]).abs() < 1e-5);
        }
    }
    chain.rollback_block();
    chain.begin_block();
    let mut retry = input;
    chain.process_block(&mut retry).unwrap();
    assert_eq!(retry, output);
    chain.commit_block();
    let mut replacement = hex_lattice::NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
    replacement.inherit_from(&chain);
    chain.begin_block();
    replacement.begin_block();
    let mut continued = input;
    let mut inherited = input;
    chain.process_block(&mut continued).unwrap();
    replacement.process_block(&mut inherited).unwrap();
    assert_eq!(continued, inherited);
    assert_ne!(continued, output);
    replacement.commit_block();
    replacement.reset();
    replacement.begin_block();
    let mut reset = input;
    replacement.process_block(&mut reset).unwrap();
    assert_eq!(reset, output);
    struct StepTempo;
    impl hex_lattice::NativeInstrumentClock for StepTempo {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
            Some(if offset < 3 { 120_000_000 } else { 60_000_000 })
        }
    }
    replacement.reset();
    replacement.begin_block();
    let mut tempo_output = input;
    replacement
        .process_range_with_clock(0..1, &mut tempo_output, Some(&StepTempo), Some(&StepTempo))
        .unwrap();
    for (index, lfo) in [0.0, 1.0, 0.0, -1.0, -std::f32::consts::FRAC_1_SQRT_2]
        .into_iter()
        .enumerate()
    {
        let gain = (1.0 + lfo * 0.01 * MAX_UTILITY_GAIN).clamp(0.0, MAX_UTILITY_GAIN);
        let state = UTILITY_DEFINITION
            .patch_parameter(&base, ids[0], ParameterValue::Float(gain))
            .unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(&state, ids[1], ParameterValue::Float(lfo * 0.4))
            .unwrap();
        let expected = static_render(&state, &input[index..index + 1])[0];
        for channel in 0..2 {
            assert!((tempo_output[index][channel] - expected[channel]).abs() < 1e-5);
        }
    }

    assert_eq!(UTILITY_DEFINITION.default_state().unwrap(), base);
}
