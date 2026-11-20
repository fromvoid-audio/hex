//! Resource-free Off uses the ordinary memoryless transaction.
use super::*;

fn node() -> PreparedRuntimeInstance {
    let kind = DeviceTypeId::from_static("hex.vst3");
    PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Main,
        device_id: 7,
        device_type: kind,
        matrix: StereoMatrix::identity(),
        bypassed: true,
        sample_rate: 48_000,
        configuration: Some(std::sync::Arc::new(
            NativeDeviceState::new(kind, 1, vec![7])
                .unwrap()
                .with_input_gain_db(6.0)
                .unwrap(),
        )),
        definition: Some(PreparedProcessorFactory::PoweredOff),
    }
}

#[test]
fn powered_off_is_dry_without_latency_history_or_processor_owners() {
    for partition in [1, 257] {
        let mut chain = NativeRuntimeChain::new_prepared(vec![node()]).unwrap();
        assert_eq!(chain.stateful_count(), 0);
        assert_eq!(chain.declared_pair_bytes(), 0);
        assert_eq!(chain.algorithmic_latency_frames_for_entry(0), Ok(Some(0)));
        assert_eq!(
            chain.zero_input_drain_for_range(0..1),
            Some(OutputTail::Finite { max_tail_frames: 0 })
        );
        let storage = chain.storage_view().entries().next().unwrap();
        assert!(storage.is_powered_off());
        assert_eq!(storage.runtime_box_identities(), (None, None));
        assert!(storage.foreign_processor().is_none());
        assert!(storage.reserved_dry_storage().iter().all(Option::is_none));
        let expected: Vec<_> = (0..514)
            .map(|i| [i as f32 / 1024.0, -(i as f32) / 1024.0])
            .collect();
        let mut actual = expected.clone();
        for block in actual.chunks_mut(partition) {
            chain.process_block(block).unwrap();
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn powered_off_rejects_active_wrong_owner_and_unknown_configuration() {
    for index in 0..8 {
        let mut value = node();
        match index {
            0 => value.bypassed = false,
            1 => value.owner = ProcessorChainOwner::Track(1),
            2 => value.owner = ProcessorChainOwner::Return(1),
            3 => value.configuration = None,
            4 => value.device_type = DeviceTypeId::from_static("hex.utility"),
            5 => value.matrix = StereoMatrix::new([[0.5, 0.0], [0.0, 0.5]]).unwrap(),
            6 => value
                .automation
                .arm_host_gain_input(hex_lattice_contract::DeviceAutomationTarget::HostOutputGain)
                .unwrap(),
            _ => {
                value.modulation = Some(
                    PreparedModulationState::new(
                        vec![
                            PreparedModulationSource::free(
                                ModulationSourceId::new(1).unwrap(),
                                48_000.0,
                                12_000.0,
                                0.0,
                            )
                            .unwrap(),
                        ],
                        PreparedModulation::new(&[], &[], 1).unwrap(),
                        2,
                    )
                    .unwrap(),
                )
            }
        }
        assert!(NativeRuntimeChain::new_prepared(vec![value]).is_err());
    }
}
