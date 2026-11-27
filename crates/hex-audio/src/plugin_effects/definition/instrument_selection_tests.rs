use super::*;
use hex_lattice::{ProcessorChainOwner, TuningTable};

fn track_identity(owner: u64) -> ProcessorInstanceIdentity {
    ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(owner), 7).unwrap()
}

fn saved_state(component: &[u8]) -> NativeDeviceState {
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(component),
        controller: None,
    })
    .unwrap()
}

#[test]
fn instrument_factory_rejects_other_owners_and_changed_state_or_rate() {
    let identity = track_identity;
    let binding = Binding {
        root: 2,
        instance: 7,
        graph: 3,
        epoch: 4,
    };
    let state = saved_state(b"original");
    let configuration = Configuration {
        identity: track_identity(10),
        binding,
        sample_rate: 48_000,
        state: state.clone(),
        prepared: PreparedForeignProcessor::new(256, 0, 512)
            .unwrap()
            .with_binding(binding)
            .unwrap()
            .with_instrument_profile(),
        authored: Default::default(),
        dense_raw_id: None,
        manual_persistence: Default::default(),
        program_revision: std::num::NonZeroU64::new(1).unwrap(),
        state_verifier: None,
    };
    let definition = NativeEffectDefinition::from_selected([(&configuration, true)]).unwrap();
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: configuration.sample_rate,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    assert!(matches!(
        definition.prepare_instance_runtime_factory(identity(10), &state, &context).unwrap(),
        Some(PreparedProcessorFactory::Foreign(selected)) if selected.binding() == Some(binding)
    ));
    assert!(
        definition
            .prepare_instance_runtime_factory(identity(11), &state, &context)
            .is_err()
    );
    assert!(
        definition
            .prepare_instance_runtime_factory(identity(10), &saved_state(b"changed"), &context)
            .is_err()
    );
    assert!(
        definition
            .prepare_instance_runtime_factory(
                identity(10),
                &state,
                &DevicePreparationContext {
                    sample_rate: 96_000,
                    ..context
                }
            )
            .is_err()
    );
    assert!(
        NativeEffectDefinition::from_selected([(&configuration, true), (&configuration, true)])
            .is_err()
    );
}

#[test]
fn foreign_automation_source_exposes_unpersisted_parameters_from_manual_persistence() {
    let binding = Binding {
        root: 1,
        instance: 7,
        graph: 3,
        epoch: 4,
    };
    let state = saved_state(b"original");
    let class = hex_plugin_state::ClassId::new([1; 16]).unwrap();
    let mut values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
    values[0] = Some((0, 0.5_f64.to_bits()));
    values[1] = Some((1, 0.0_f64.to_bits()));
    let authored = automation::SourceCapabilities::from_canonical(
        class,
        &values,
        &[0; MAX_FOREIGN_AUTOMATION_TARGETS],
    )
    .unwrap();
    let mut manual_persistence = state_approval::ManualPersistenceTracker::default();
    manual_persistence.observe_manual_base(
        &state,
        &authored,
        state_approval::ManualFlushObservation {
            id: 1,
            flushed_normalized: 0.75,
            before_fingerprint: state_approval::state_fingerprint(&state),
            after_fingerprint: state_approval::state_fingerprint(&state),
        },
    );
    let configuration = Configuration {
        identity: track_identity(10),
        binding,
        sample_rate: 48_000,
        state: state.clone(),
        prepared: PreparedForeignProcessor::new(256, 0, 512)
            .unwrap()
            .with_binding(binding)
            .unwrap()
            .with_instrument_profile(),
        authored,
        dense_raw_id: None,
        manual_persistence,
        program_revision: std::num::NonZeroU64::new(1).unwrap(),
        state_verifier: None,
    };
    let definition = NativeEffectDefinition::from_selected([(&configuration, true)]).unwrap();
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: configuration.sample_rate,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let source = definition
        .foreign_automation_source(track_identity(10), &state, &context)
        .unwrap()
        .unwrap();
    assert_eq!(source.unpersisted_parameters, &[1]);
}
