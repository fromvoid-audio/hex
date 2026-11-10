use hex_devices::rattle::{RATTLE_DEFINITION, RATTLE_TYPE_ID, RattleDefinition, parameter::DRIVE};
use hex_lattice::{
    ModulationRoute, ModulationSourceId, ModulationTarget, NativeRuntimeChain, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, PreparedNativeProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::{hint::black_box, sync::Arc};

#[test]
fn rattle_modulation_and_latency_aligned_retry_do_not_allocate_or_free() {
    let state = RATTLE_DEFINITION.default_state().unwrap();
    let factory = RattleDefinition::prepare_owned(&state, 48_000, &[DRIVE]).unwrap();
    let target = ModulationTarget {
        descriptor: factory.parameter_automation_descriptor(DRIVE).unwrap(),
        semantics: factory.parameter_modulation_semantics(DRIVE).unwrap(),
        base: RATTLE_DEFINITION.read_parameter(&state, DRIVE).unwrap(),
    };
    let program = PreparedModulation::new(
        &[target],
        &[ModulationRoute {
            source: 0,
            target: DRIVE,
            amount: 0.15,
            range: [-1.0, 1.0],
        }],
        1,
    )
    .unwrap();
    let source = PreparedModulationSource::free(
        ModulationSourceId::new(1).unwrap(),
        48_000.0,
        12_000.0,
        0.0,
    )
    .unwrap();
    let modulation = PreparedModulationState::new(vec![source], program, 17).unwrap();
    let mut chain = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: Some(modulation),
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: RATTLE_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: true,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(factory))),
    }])
    .unwrap();
    let mut first = [[0.2, -0.15]; 263];
    let mut retry = first;
    let counts = allocation_counter::measure(|| {
        chain.begin_block();
        chain
            .process_range_with_clock(0..1, black_box(&mut first), None, None)
            .unwrap();
        chain.rollback_block();
        chain.begin_block();
        chain
            .process_range_with_clock(0..1, black_box(&mut retry), None, None)
            .unwrap();
        chain.commit_block();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert_eq!(first, retry);
    let latency = hex_devices::rattle::LATENCY_FRAMES as usize;
    assert_eq!(
        retry[..latency],
        [[0.0; 2]; hex_devices::rattle::LATENCY_FRAMES as usize]
    );
    assert_eq!(retry[latency], [0.2, -0.15]);
}
