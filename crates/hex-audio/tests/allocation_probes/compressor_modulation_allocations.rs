//! Closed native Compressor paths use preallocated provider and source histories.
use hex_devices::compressor::{COMPRESSOR_DEFINITION, COMPRESSOR_TYPE_ID, CompressorDefinition};
use hex_lattice::{
    ModulationRoute, ModulationSourceId, ModulationTarget, NativeRuntimeChain, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, PreparedNativeProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner, StereoAuxiliaryBlock,
    StereoMatrix,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::{hint::black_box, sync::Arc};

#[test]
fn compressor_modulation_and_keyed_retry_do_not_allocate_or_free() {
    let state = COMPRESSOR_DEFINITION.default_state().unwrap();
    let ids: Vec<_> = COMPRESSOR_DEFINITION
        .descriptors()
        .iter()
        .map(|d| d.id)
        .collect();
    let factory = CompressorDefinition::prepare_owned(&state, 48_000, &ids).unwrap();
    let targets: Vec<_> = ids
        .iter()
        .map(|&id| ModulationTarget {
            descriptor: factory.parameter_automation_descriptor(id).unwrap(),
            semantics: factory.parameter_modulation_semantics(id).unwrap(),
            base: COMPRESSOR_DEFINITION.read_parameter(&state, id).unwrap(),
        })
        .collect();
    let routes: Vec<_> = ids
        .iter()
        .map(|&target| ModulationRoute {
            source: 0,
            target,
            amount: 0.1,
            range: [-1.0, 1.0],
        })
        .collect();
    let program = PreparedModulation::new(&targets, &routes, 1).unwrap();
    let source = PreparedModulationSource::free(
        ModulationSourceId::new(1).unwrap(),
        48_000.0,
        12_000.0,
        0.0,
    )
    .unwrap();
    let modulation = PreparedModulationState::new(vec![source], program, 17).unwrap();
    let mut runtime = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: Some(modulation),
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: COMPRESSOR_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(factory))),
    }])
    .unwrap();
    let dry = [[0.12, -0.08]; 263];
    let key = [[0.9, -0.6]; 263];
    let mut local = dry;
    let mut keyed = dry;
    let counts = allocation_counter::measure(|| {
        runtime.begin_block();
        runtime
            .process_range_with_clock(0..1, black_box(&mut local), None, None)
            .unwrap();
        runtime.rollback_block();

        runtime.begin_block();
        runtime
            .process_auxiliary_stage(
                0,
                StereoAuxiliaryBlock::new(
                    black_box(&mut keyed),
                    Some(black_box(&key)),
                    &mut [],
                    key.len(),
                )
                .unwrap(),
                0,
                None,
                None,
                None,
                None,
                &mut |_, _, _, _, _, _| {},
            )
            .unwrap();
        runtime.commit_block();
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
    assert_ne!(keyed, local);
}
