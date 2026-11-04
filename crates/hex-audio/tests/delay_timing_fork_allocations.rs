//! Thread-local probe of the Delay timing-only fork; not a physical callback test.
#![cfg(feature = "allocation-probe")]

use allocation_counter::measure;
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeRuntimeChain,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix, TuningTable,
};
use hex_lattice_contract::NativeDeviceMetadata;
use std::sync::Arc;

#[test]
fn delay_timing_fork_has_no_allocation_or_deallocation() {
    let state = hex_devices::delay::DEFINITION.default_state().unwrap();
    let factory = hex_devices::delay::DEFINITION
        .prepare_runtime_factory(
            &state,
            &DevicePreparationContext {
                sample_rate: 8000,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .unwrap();
    let state = Arc::new(state);
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 8000,
            configuration: Some(Arc::clone(&state)),
            definition: Some(factory.clone()),
        }])
        .unwrap()
    };
    let mut old = make(false);
    let mut successor = make(true);
    old.begin_block();
    let mut input = [[0.2, -0.1]; 128];
    old.process_block(&mut input).unwrap();
    old.commit_block();
    let mut accepted = false;
    let info = measure(|| {
        accepted = successor.can_fork_timing_without_signal_history_at(&old, 0)
            && successor
                .fork_timing_without_signal_history_at(&old, 0)
                .is_ok();
    });
    assert!(accepted);
    let mut refused = false;
    let refusal_info = measure(|| {
        refused = successor
            .fork_timing_without_signal_history_at(&old, 0)
            .is_err()
            && successor
                .fork_timing_without_signal_history_at(&old, 1)
                .is_err();
    });
    assert!(refused);
    assert_eq!(
        (
            refusal_info.count_total,
            refusal_info.bytes_total,
            refusal_info.count_current,
            refusal_info.bytes_current
        ),
        (0, 0, 0, 0)
    );
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
