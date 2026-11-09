//! Isolated modulation transaction probe, not native callback integration.
use hex_lattice::{
    ModulationRoute, ModulationRuntime, ModulationSourceBank, ModulationSourceId, ModulationTarget,
    PreparedModulation, PreparedModulationSource,
};
use hex_lattice_contract::{
    DeviceContractError, ParameterAccess, ParameterAutomation, ParameterChangeImpact,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterModulation, ParameterNormalization,
    ParameterPresentation, ParameterSemantics, ParameterSmoothing, ParameterStepping,
    ParameterValue,
};
use std::hint::black_box;

fn prepared() -> ModulationRuntime {
    let id = ParameterId::from_static(1);
    let target = ModulationTarget {
        descriptor: ParameterDescriptor {
            id,
            label: "gain",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        semantics: ParameterSemantics {
            access: ParameterAccess::ReadWrite,
            normalization: ParameterNormalization::Linear,
            stepping: ParameterStepping::Continuous,
            automation: ParameterAutomation::Unsupported,
            modulation: ParameterModulation::Unsupported,
            smoothing: ParameterSmoothing::None,
            change_impact: ParameterChangeImpact::ValueOnly,
        },
        base: ParameterValue::Float(0.5),
    };
    let program = PreparedModulation::new(
        &[target],
        &[ModulationRoute {
            source: 0,
            target: id,
            amount: 0.1,
            range: [-1.0, 1.0],
        }],
        1,
    )
    .unwrap();
    let source =
        PreparedModulationSource::free(ModulationSourceId::new(1).unwrap(), 48_000.0, 7.0, 0.0)
            .unwrap();
    ModulationRuntime::new(ModulationSourceBank::new(&[source], 128).unwrap(), program).unwrap()
}

#[test]
fn chunk_transactions_do_not_allocate_or_free() {
    let mut runtime = prepared();
    let mut next = prepared();
    let id = ModulationSourceId::new(1).unwrap();
    let rate = PreparedModulationSource::free(id, 48_000.0, 14.0, 0.0).unwrap();
    let counts = allocation_counter::measure(|| {
        for _ in 0..32 {
            runtime.reset(id, 0.0).unwrap();
            runtime.begin();
            runtime.retarget(rate).unwrap();
            runtime
                .render_chunk(128, None, |block| {
                    for index in 0..128 {
                        black_box(block.value_at(ParameterId::from_static(1), index));
                    }
                    Ok(())
                })
                .unwrap();
            runtime.commit().unwrap();
            next.inherit_committed(&runtime).unwrap();
            next.begin();
            assert!(
                next.render_chunk(128, None, |_| Err(
                    DeviceContractError::InvalidParameterValue
                ))
                .is_err()
            );
            next.rollback();
        }
    });
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

#[test]
fn utility_graph_modulation_transactions_do_not_allocate_or_free() {
    probe_utility_graph(false);
}

#[test]
fn utility_graph_with_lfo_rate_modulation_does_not_allocate_or_free() {
    probe_utility_graph(true);
}

fn probe_utility_graph(routed_rate: bool) {
    use hex_devices::utility::{UTILITY_DEFINITION, UTILITY_TYPE_ID, UtilityDefinition, parameter};
    use hex_lattice::PreparedNativeProcessor;
    use hex_lattice::{
        LfoWaveform, NativeRuntimeChain, PreparedModulationState, PreparedProcessorFactory,
        PreparedRuntimeInstance, ProcessorChainOwner, SourceModulationParameter,
        SourceModulationRoute, StereoMatrix,
    };
    use hex_lattice_contract::NativeDeviceMetadata;
    use std::sync::Arc;
    let build = |waveform| {
        let state = UTILITY_DEFINITION.default_state().unwrap();
        let id = parameter::GAIN;
        let definition = UtilityDefinition::prepare_owned_automated_filter(&state, 48_000).unwrap();
        let target = ModulationTarget {
            descriptor: *UTILITY_DEFINITION
                .descriptors()
                .iter()
                .find(|d| d.id == id)
                .unwrap(),
            semantics: definition.parameter_modulation_semantics(id).unwrap(),
            base: UTILITY_DEFINITION.read_parameter(&state, id).unwrap(),
        };
        let program = PreparedModulation::new(
            &[target],
            &[0, 1, 2].map(|source| ModulationRoute {
                source,
                target: id,
                amount: 0.01,
                range: [-1.0, 1.0],
            }),
            3,
        )
        .unwrap();
        let oscillator = ModulationSourceId::new(1).unwrap();
        let source = if routed_rate {
            PreparedModulationSource::free_with_waveform(
                oscillator, 48_000.0, 12_000.0, 0.0, waveform,
            )
        } else {
            PreparedModulationSource::tempo_with_waveform(
                oscillator, 48_000.0, 120.0, 4.0, 0.0, waveform,
            )
        }
        .unwrap();

        let follower = PreparedModulationSource::envelope_follower(
            ModulationSourceId::new(2).unwrap(),
            48_000,
            0.005,
            0.05,
        )
        .unwrap();
        let manual = PreparedModulationSource::macro_value(
            ModulationSourceId::new(3).unwrap(),
            48_000,
            0.25,
        )
        .unwrap();
        let mut source_routes = vec![SourceModulationRoute {
            source: 0,
            target: 2,
            parameter: SourceModulationParameter::MacroValue,
            amount: 0.2,
            range: [0.0, 1.0],
        }];
        if routed_rate {
            source_routes.push(SourceModulationRoute {
                source: 1,
                target: 0,
                parameter: SourceModulationParameter::FreeLfoRate,
                amount: 0.2,
                range: [-1.0, 1.0],
            });
        }
        let prepared = PreparedRuntimeInstance {
            modulation: Some(
                PreparedModulationState::with_source_routes(
                    vec![source, follower, manual],
                    program,
                    17,
                    &source_routes,
                )
                .unwrap(),
            ),
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: UTILITY_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state)),
            definition: Some(PreparedProcessorFactory::Legacy(Arc::new(definition))),
        };
        let chain = NativeRuntimeChain::new_prepared(vec![prepared.clone()]).unwrap();
        (prepared, chain)
    };
    let (prepared, mut first) = build(LfoWaveform::Sine);
    let (_, mut second) = build(LfoWaveform::Square);
    let mut frames = [[0.1, -0.1]; 128];
    struct Tempo;
    impl hex_lattice::NativeInstrumentClock for Tempo {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
            Some(if offset.is_multiple_of(2) {
                120_000_000
            } else {
                180_000_000
            })
        }
    }
    let counts = allocation_counter::measure(|| {
        for _ in 0..8 {
            assert!(
                first
                    .storage_view()
                    .entries()
                    .all(|entry| entry.matches_prepared_runtime_storage(&prepared))
            );
            first.begin_block();
            first
                .process_range_with_clock(0..1, black_box(&mut frames), Some(&Tempo), Some(&Tempo))
                .unwrap();
            first.rollback_block();
            first.begin_block();
            first
                .process_range_with_clock(0..1, black_box(&mut frames), Some(&Tempo), Some(&Tempo))
                .unwrap();
            first.commit_block();
            second.inherit_from(&first);
            second.transfer_history_from(&mut first);
            second.begin_block();
            second
                .process_range_with_clock(0..1, black_box(&mut frames), Some(&Tempo), Some(&Tempo))
                .unwrap();
            second.rollback_block();
            second.reset();
        }
    });
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
