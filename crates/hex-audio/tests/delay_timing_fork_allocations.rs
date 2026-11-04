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

#[test]
fn delay_tail_split_prepare_render_and_retire_are_allocation_free() {
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
    for old_bypassed in [false, true] {
        let mut old = make(old_bypassed);
        let mut next = make(!old_bypassed);
        old.begin_block();
        let mut warm = [[0.2, -0.1]; 128];
        old.process_block(&mut warm).unwrap();
        old.commit_block();
        old.set_metering(true);
        next.set_metering(true);
        let input = [[0.2, -0.1]; 64];
        let mut old_output = [[0.0; 2]; 64];
        let mut new_output = [[0.0; 2]; 64];
        let mut checked = false;
        let preflight = measure(|| {
            checked = next.tail_bypass_change_at(&old, 0)
                && next.can_prepare_tail_split_at(&old, 0)
                && !next.can_prepare_tail_split_at(&old, 1);
        });
        assert!(checked);
        assert_eq!(
            (
                preflight.count_total,
                preflight.bytes_total,
                preflight.count_current,
                preflight.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let mut accepted = false;
        let info = measure(|| {
            if let Ok(mut split) = next.prepare_tail_split_at(&old, 0) {
                old.begin_block();
                next.begin_block();
                if split.begin_block().is_ok()
                    && split
                        .process(
                            &mut old,
                            &mut next,
                            &input,
                            &mut old_output,
                            &mut new_output,
                            0,
                            None,
                        )
                        .is_ok()
                {
                    old.commit_block();
                    next.commit_block();
                    accepted = split.commit_block().is_ok()
                        && split.finish_retained_tail(&mut old).is_ok();
                }
            }
        });
        assert!(accepted);
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
}

#[test]
fn delay_tail_split_control_and_observation_are_allocation_free() {
    use hex_lattice::{
        NativeTransientInput, NativeTransientOperation, NativeTransientProcessing,
        NativeTransientValue, ProcessorInstanceIdentity,
    };
    use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
    let state = Arc::new(hex_devices::delay::DEFINITION.default_state().unwrap());
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
    let mut automation = hex_lattice::PreparedParameterAutomation::default();
    automation
        .arm_host_gain_input(DeviceAutomationTarget::HostOutputGain)
        .unwrap();
    let make = |bypassed| {
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            automation: automation.clone(),
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
    let mut old = make(true);
    let mut next = make(false);
    let mut split = next.prepare_tail_split_at(&old, 0).unwrap();
    old.set_metering(true);
    next.set_metering(true);
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 1).unwrap();
    let operation =
        NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(HostGain::ZERO));
    let mut context = NativeTransientProcessing::new(NativeTransientInput {
        identity,
        operation,
    });
    let mut old_out = [[0.0; 2]; 64];
    let mut next_out = old_out;
    let mut observations = 0;
    let mut accepted = false;
    let info = measure(|| {
        old.begin_block();
        next.begin_block();
        if split.begin_block().is_ok()
            && split
                .process_with_controls(
                    &mut old,
                    &mut next,
                    &[[0.2, -0.1]; 64],
                    &mut old_out,
                    &mut next_out,
                    0,
                    None,
                    Some(&mut context),
                    None,
                    Some(&mut |_, _, _, _, _, _| observations += 1),
                )
                .is_ok()
        {
            old.commit_block();
            next.commit_block();
            accepted = split.commit_block().is_ok() && split.finish_retained_tail(&mut old).is_ok();
        }
    });
    assert!(accepted);
    assert_eq!(observations, 2);
    assert_eq!(context.applied.unwrap().identity, identity);
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
