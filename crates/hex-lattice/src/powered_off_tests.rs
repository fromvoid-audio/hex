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
            2 => value.owner = ProcessorChainOwner::Source(1),
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

#[test]
fn return_power_off_and_deletion_retain_the_exact_aligned_owner() {
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 7,
        graph: 3,
        epoch: 4,
    };
    let owner = ProcessorChainOwner::Return(1);
    let mut node = configured_main();
    node.owner = owner;
    let off = NativeRuntimeChain::new_prepared(vec![node.clone()]).unwrap();
    let mut active = node.clone();
    active.bypassed = false;
    active.definition = Some(PreparedProcessorFactory::Foreign(
        PreparedForeignProcessor::new(256, 0, 512)
            .unwrap()
            .with_binding(binding)
            .unwrap(),
    ));
    let mut original = NativeRuntimeChain::new_prepared(vec![active.clone()]).unwrap();
    let cut = off.return_power_off_from(&original, 1).unwrap();
    assert_eq!(cut.foreign_return_pass(), Some(binding));
    assert!(cut.foreign_main_pass().is_none());
    assert!(off.return_power_off_from(&original, 2).is_none());
    assert!(off.main_pass_removal_from(&original).is_none());
    assert!(original.begin_native_removal(cut));
    assert!(original.native_removal_ready(cut));
    original.cancel_native_removal(cut);
    assert!(!original.native_removal_ready(cut));
    assert!(off.return_power_off_from(&original, 1).is_some());

    let trimmed = active
        .configuration
        .as_ref()
        .unwrap()
        .as_ref()
        .clone()
        .with_input_gain_db(6.0)
        .unwrap();
    active.configuration = Some(std::sync::Arc::new(trimmed));
    let trimmed = NativeRuntimeChain::new_prepared(vec![active]).unwrap();
    assert!(off.return_power_off_from(&trimmed, 1).is_none());
    let wrong = NativeRuntimeChain::new_prepared(vec![configured_main()]).unwrap();
    assert!(wrong.return_power_off_from(&original, 1).is_none());
    let deleted = NativeRuntimeChain::new_prepared(Vec::new()).unwrap();
    let removal = deleted.return_power_off_from(&original, 1).unwrap();
    assert_eq!(removal.foreign_return_pass(), Some(binding));
    assert_eq!(removal.foreign_return_pass_frames(), Some(512));
    assert!(deleted.return_power_off_from(&original, 2).is_none());
}

#[test]
fn only_an_exact_off_placeholder_accepts_the_hosted_main_pass_cut() {
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let before = active_main_chain(0, binding);
    let after = NativeRuntimeChain::new_prepared(vec![configured_main()]).unwrap();
    assert_eq!(
        after
            .main_pass_removal_from(&before)
            .unwrap()
            .foreign_main_pass(),
        Some((binding, 512))
    );
    for mismatch in 0..4 {
        let mut candidate = configured_main();
        match mismatch {
            0 => candidate.device_id += 1,
            1 => candidate.sample_rate = 96_000,
            2 => {
                candidate.configuration = Some(std::sync::Arc::new(
                    NativeDeviceState::new(candidate.device_type, 1, vec![8]).unwrap(),
                ))
            }
            _ => candidate.definition = None,
        }
        let candidate = NativeRuntimeChain::new_prepared(vec![candidate]).unwrap();
        assert!(candidate.main_pass_removal_from(&before).is_none());
    }
}

#[test]
fn hosted_main_removal_and_power_off_require_the_exact_latency_and_binding() {
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    };
    let mut before = active_main_chain(7, binding);
    let removed = NativeRuntimeChain::new_prepared(Vec::new()).unwrap();
    let transition = removed.main_pass_removal_from(&before).unwrap();
    assert_eq!(transition.foreign_main_pass(), Some((binding, 519)));
    let off = NativeRuntimeChain::new_prepared(vec![configured_main()]).unwrap();
    assert_eq!(
        off.main_pass_removal_from(&before)
            .unwrap()
            .foreign_main_pass(),
        Some((binding, 519))
    );
    for frames in [6, 8] {
        let mut different = active_main_chain(frames, binding);
        assert_eq!(
            removed
                .main_pass_removal_from(&different)
                .unwrap()
                .foreign_main_pass(),
            Some((binding, 512 + frames))
        );
        assert!(!different.begin_native_removal(transition));
    }
    let mut different = active_main_chain(
        7,
        hex_lattice_contract::ForeignProcessorBinding {
            epoch: binding.epoch + 1,
            ..binding
        },
    );
    assert!(!different.begin_native_removal(transition));
    assert!(before.begin_native_removal(transition));
}

fn configured_main() -> PreparedRuntimeInstance {
    let mut value = node();
    value.configuration = Some(std::sync::Arc::new(
        NativeDeviceState::new(value.device_type, 1, vec![7]).unwrap(),
    ));
    value
}

fn active_main_chain(
    processor_frames: u32,
    binding: hex_lattice_contract::ForeignProcessorBinding,
) -> NativeRuntimeChain {
    let mut active = configured_main();
    active.bypassed = false;
    active.definition = Some(PreparedProcessorFactory::Foreign(
        PreparedForeignProcessor::new(256, processor_frames, 512)
            .unwrap()
            .with_binding(binding)
            .unwrap(),
    ));
    NativeRuntimeChain::new_prepared(vec![active]).unwrap()
}
