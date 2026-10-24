//! Borrowed owned-effect operations only; no physical native custody claim.
use hex_lattice::{
    NativeDeviceRegistry, NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner,
    conformance::owned_delay,
};
use std::sync::Arc;
fn chain(bypassed: bool) -> NativeRuntimeChain {
    let providers: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = NativeDeviceRegistry::new(&providers).unwrap();
    let state = owned_delay::state(4096).unwrap();
    let prepared = registry
        .prepare_runtime_device(
            &state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: owned_delay::TYPE_ID,
        matrix: prepared.matrix,
        bypassed,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: prepared.processor,
    }])
    .unwrap()
}
#[test]
fn owned_effect_active_abort_bypass_inherit_and_reset_allocate_and_free_nothing() {
    let mut active = chain(false);
    let mut candidate = chain(false);
    let mut bypass = chain(true);
    let mut frames = [[0.25, -0.5]; 512];
    let mut dry = frames;
    let mut inherited = false;
    let measurement = allocation_counter::measure(|| {
        for _ in 0..16 {
            frames.fill([0.25, -0.5]);
            active.begin_block();
            active.process_block(&mut frames);
            active.commit_block();
        }
        active.begin_block();
        active.process_block(&mut frames);
        active.process_block(&mut frames);
        active.rollback_block();
        bypass.begin_block();
        bypass.process_block(&mut dry);
        bypass.rollback_block();
        inherited = candidate.can_inherit_from(&active);
        if inherited {
            candidate.inherit_from(&active);
        }
        candidate.begin_block();
        candidate.process_block(&mut frames);
        candidate.commit_block();
        active.reset();
        candidate.reset();
        bypass.reset();
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(inherited);
    assert_eq!(dry, [[0.25, -0.5]; 512]);
    assert!(active.committed_range_is_quiescent(0..1));
}
