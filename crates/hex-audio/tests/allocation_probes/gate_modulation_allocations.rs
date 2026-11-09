//! Headless rendering of the closed fixed-inline Gate factory.
use hex_devices::gate::{GATE_DEFINITION, GATE_TYPE_ID, GateDefinition, parameter};
use hex_lattice::{
    ModulationRoute, ModulationSourceId, ModulationTarget, NativeRuntimeChain, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, PreparedNativeProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::{hint::black_box, sync::Arc};

#[test]
fn gate_sealed_factory_render_and_rollback_do_not_allocate_or_free() {
    let ids = [
        parameter::FLOOR,
        parameter::ATTACK,
        parameter::HOLD,
        parameter::RELEASE,
    ];
    for parameters in [ids.as_slice(), &[]] {
        let state = GATE_DEFINITION.default_state().unwrap();
        let factory = GateDefinition::prepare_owned(&state, 48_000, parameters).unwrap();
        let modulation = if parameters.is_empty() {
            None
        } else {
            let targets = parameters
                .iter()
                .map(|&id| ModulationTarget {
                    descriptor: factory.parameter_automation_descriptor(id).unwrap(),
                    semantics: factory.parameter_modulation_semantics(id).unwrap(),
                    base: GATE_DEFINITION.read_parameter(&state, id).unwrap(),
                })
                .collect::<Vec<_>>();
            let routes = parameters
                .iter()
                .map(|&target| ModulationRoute {
                    source: 0,
                    target,
                    amount: 0.008,
                    range: [-1.0, 1.0],
                })
                .collect::<Vec<_>>();
            let source = PreparedModulationSource::free(
                ModulationSourceId::new(1).unwrap(),
                48_000.0,
                12_000.0,
                0.0,
            )
            .unwrap();
            let program = PreparedModulation::new(&targets, &routes, 1).unwrap();
            Some(PreparedModulationState::new(vec![source], program, 17).unwrap())
        };
        let mut runtime = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: GATE_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state)),
            definition: Some(PreparedProcessorFactory::Legacy(Arc::new(factory))),
        }])
        .unwrap();
        let mut pcm = [[0.0; 2]; 128];
        let counts = allocation_counter::measure(|| {
            for commit in [false, true] {
                runtime.begin_block();
                pcm.fill([0.15, -0.2]);
                runtime
                    .process_range_with_clock(0..1, black_box(&mut pcm), None, None)
                    .unwrap();
                if commit {
                    runtime.commit_block();
                } else {
                    runtime.rollback_block();
                }
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
        assert!(pcm.iter().any(|frame| *frame != [0.0; 2]));
    }
}
