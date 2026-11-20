//! Exact saved metadata yields no source or DSP factory owner.
use super::*;
use hex_lattice::{
    NativeDeviceRegistry, NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner,
    TuningTable,
};
use std::sync::Arc;

fn saved() -> NativeDeviceState {
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(&[7, 8, 9]),
        controller: Some(&[10, 11]),
    })
    .unwrap()
}

#[test]
fn exact_main_off_definition_preserves_opaque_state_and_renders_dry_without_source() {
    let state = saved();
    let definition = NativeEffectDefinition::powered_off(&[PoweredOffEffect {
        owner: ProcessorChainOwner::Main,
        device_id: 7,
        sample_rate: 48_000,
        state: &state,
    }])
    .unwrap();
    let defs: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&defs).unwrap();
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 7).unwrap();
    let selected = registry
        .prepare_runtime_instance(identity, &state, &context)
        .unwrap();
    assert!(matches!(
        selected.processor,
        Some(PreparedProcessorFactory::PoweredOff)
    ));
    assert!(
        selected
            .processor
            .as_ref()
            .unwrap()
            .native_factory_arc_bytes()
            .is_none()
    );
    let node = PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: state.type_id(),
        matrix: selected.matrix,
        bypassed: true,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state.clone())),
        definition: selected.processor,
    };
    assert!(
        definition
            .prepared_processor_on_control(&node)
            .unwrap()
            .is_none()
    );
    assert!(
        definition
            .foreign_automation_source(identity, &state, &context)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        registry
            .prepare_instance_state(identity, &state, &context)
            .unwrap(),
        state
    );
    assert!(registry.prepare_runtime_device(&state, &context).is_err());
    for partition in [1, 257] {
        let mut chain = NativeRuntimeChain::new_prepared(vec![node.clone()]).unwrap();
        let expected = [[0.25, -0.5]; 514];
        let mut actual = expected;
        let render = || {
            for block in actual.chunks_mut(partition) {
                chain.process_block(block).unwrap();
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let count = allocation_counter::measure(render);
            assert_eq!(
                (
                    count.count_total,
                    count.bytes_total,
                    count.count_current,
                    count.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn wrong_identity_rate_state_and_malformed_off_selection_refuse() {
    let state = saved();
    let entry = PoweredOffEffect {
        owner: ProcessorChainOwner::Main,
        device_id: 7,
        sample_rate: 48_000,
        state: &state,
    };
    let definition = NativeEffectDefinition::powered_off(&[entry]).unwrap();
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    for identity in [
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 8).unwrap(),
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 7).unwrap(),
    ] {
        assert!(
            definition
                .prepare_instance_runtime_factory(identity, &state, &context)
                .is_err()
        );
    }
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 7).unwrap();
    assert!(
        definition
            .prepare_instance_runtime_factory(
                identity,
                &state,
                &DevicePreparationContext {
                    sample_rate: 44_100,
                    ..context
                }
            )
            .is_err()
    );
    let malformed = NativeDeviceState::new(state.type_id(), 1, vec![1, 2]).unwrap();
    assert!(
        NativeEffectDefinition::powered_off(&[PoweredOffEffect {
            state: &malformed,
            ..entry
        }])
        .is_err()
    );
    assert!(
        NativeEffectDefinition::powered_off(&[PoweredOffEffect {
            owner: ProcessorChainOwner::Return(1),
            ..entry
        }])
        .is_err()
    );
    assert!(
        NativeEffectDefinition::powered_off(&[PoweredOffEffect {
            sample_rate: 0,
            ..entry
        }])
        .is_err()
    );
    assert!(NativeEffectDefinition::powered_off(&[entry, entry]).is_err());
    assert!(
        definition
            .prepare_instance_runtime_factory(identity, &malformed, &context)
            .is_err()
    );
    let captured = hex_plugin_state::decode(&state).unwrap();
    let changed = hex_plugin_state::encode(hex_plugin_state::State {
        identity: captured.identity,
        component: Some(&[99]),
        controller: captured.controller,
    })
    .unwrap();
    assert!(
        definition
            .prepare_instance_runtime_factory(identity, &changed, &context)
            .is_err()
    );
}
