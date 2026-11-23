//! Fast non-SDK unit tests for `plugin_effects`.
use super::*;
use hex_lattice::ProcessorChainOwner;

#[test]
fn program_revision_reservation_never_wraps_or_changes_processing_state() {
    let binding = Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap();
    let state = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.revision"),
        1,
        vec![1],
    )
    .unwrap();
    let (_control, transport, _worker) = hex_plugin_host::transport::channel(binding, 8).unwrap();
    let mut callback = NativeEffectCallback::for_test(identity, binding, state.clone(), transport);
    assert_eq!(
        callback
            .configuration
            .reserve_program_revision_on_control()
            .unwrap()
            .get(),
        5
    );
    assert_eq!(callback.configuration.state, state);
    callback.configuration.program_revision = std::num::NonZeroU64::MAX;
    assert!(
        callback
            .configuration
            .reserve_program_revision_on_control()
            .is_err()
    );
    assert_eq!(
        callback.configuration.program_revision,
        std::num::NonZeroU64::MAX
    );
    assert_eq!(callback.configuration.state, state);
}

#[test]
fn unreserved_or_wrong_dense_target_refuses_before_any_request() {
    use hex_lattice_contract::DeviceTypeId;
    use hex_plugin_host::transport::channel;

    const BINDING: Binding = Binding {
        root: 41,
        instance: 21,
        graph: 12,
        epoch: 1,
    };
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 21).unwrap();
    let state =
        NativeDeviceState::new(DeviceTypeId::from_static("test.foreign"), 1, vec![]).unwrap();
    for (expected, incoming) in [(None, 0), (Some(0), 1)] {
        let (control, transport, mut worker) = channel(BINDING, 8).unwrap();
        let mut callback =
            NativeEffectCallback::for_test(identity, BINDING, state.clone(), transport);
        callback.configuration.dense_raw_id = expected;
        let config = callback.configuration.clone();
        callback.begin_host_callback(8).unwrap();
        let node = ForeignProcessorNode {
            owner: config.identity.owner(),
            device_id: config.identity.device_id(),
            device_type: config.state.type_id(),
            sample_rate: config.sample_rate,
            configuration: &config.state,
            prepared: config.prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        };
        assert_eq!(
            callback.process_segment_with_modulation(
                node,
                0,
                &mut [[0.25; 2]; 8],
                ForeignModulationSamples {
                    id: incoming,
                    normalized: &[0.5; 8],
                },
                None,
            ),
            Err(DeviceContractError::ForeignProcessingUnavailable)
        );
        assert!(control.is_revoked());
        assert!(matches!(worker.receive_request(), Err(Fault::Revoked)));
    }
}

#[test]
fn renewal_preserves_state_authority_across_latency_and_tail_updates_only() {
    let binding_v1 = Binding {
        root: 1,
        instance: 7,
        graph: 2,
        epoch: 1,
    };
    let binding_v2 = Binding {
        epoch: 2,
        ..binding_v1
    };
    let old = PreparedForeignProcessor::new(256, 0, 512)
        .unwrap()
        .with_output_tail(hex_lattice::OutputTail::Finite {
            max_tail_frames: 512,
        })
        .with_binding(binding_v1)
        .unwrap();
    let latency_updated = PreparedForeignProcessor::new(256, 1024, 512)
        .unwrap()
        .with_output_tail(hex_lattice::OutputTail::Unbounded)
        .with_binding(binding_v2)
        .unwrap();

    // Same SDK instance with updated latency/tail preserves Vst3StateAuthority
    // so previously UI-edited states remain approved for Save, gestures, and compile.
    assert!(renewal_preserves_state_authority(old, latency_updated));
    let authority = hex_lattice_contract::Vst3StateAuthority::new_on_control().unwrap();
    let ui_edited =
        NativeDeviceState::new(hex_lattice_contract::VST3_DEVICE_TYPE_ID, 1, vec![42, 99]).unwrap();
    let approved = authority.approve_on_control(ui_edited).unwrap();
    let preserved = renewal_preserves_state_authority(old, latency_updated).then_some(authority);
    let verifier = preserved
        .expect("latency-only reactivation must preserve Vst3StateAuthority")
        .verifier();
    assert!(verifier.approves(&approved));

    // Changing quantum, bridge latency, or instrument/effect profile requires
    // fresh state qualification.
    let different_quantum = PreparedForeignProcessor::new(128, 0, 512)
        .unwrap()
        .with_binding(binding_v2)
        .unwrap();
    let different_bridge = PreparedForeignProcessor::new(256, 0, 1024)
        .unwrap()
        .with_binding(binding_v2)
        .unwrap();
    let different_profile = PreparedForeignProcessor::new(256, 0, 512)
        .unwrap()
        .with_instrument_profile()
        .with_binding(binding_v2)
        .unwrap();
    assert!(!renewal_preserves_state_authority(old, different_quantum));
    assert!(!renewal_preserves_state_authority(old, different_bridge));
    assert!(!renewal_preserves_state_authority(old, different_profile));
}
