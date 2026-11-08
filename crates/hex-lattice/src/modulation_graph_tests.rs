//! Graph-level source state, including devices with no mapped targets.
use super::*;
use hex_lattice_contract::{
    ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterDescriptor, ParameterId,
    ParameterKind, ParameterModulation, ParameterNormalization, ParameterPresentation,
    ParameterSemantics, ParameterSmoothing, ParameterStepping, ParameterValue,
};

fn source() -> PreparedLfoSource {
    PreparedLfoSource::free(ModulationSourceId::new(1).unwrap(), 48_000.0, 12_000.0, 0.0).unwrap()
}
fn chain(bypassed: bool) -> NativeRuntimeChain {
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        modulation: Some(
            PreparedModulationState::new(
                vec![source()],
                PreparedModulation::new(&[], &[], 1).unwrap(),
                2,
            )
            .unwrap(),
        ),
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 2,
        device_type: DeviceTypeId::from_static("test.modulation.graph"),
        matrix: StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap(),
        bypassed,
        sample_rate: 48_000,
        configuration: None,
        definition: None,
    }])
    .unwrap()
}
fn next_value(chain: &NativeRuntimeChain) -> f32 {
    let id = ParameterId::from_static(1);
    let target = ModulationTarget {
        descriptor: ParameterDescriptor {
            id,
            label: "test",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        semantics: ParameterSemantics {
            access: ParameterAccess::ReadWrite,
            normalization: ParameterNormalization::Linear,
            stepping: ParameterStepping::Continuous,
            automation: ParameterAutomation::Unsupported,
            modulation: ParameterModulation::Unsupported,
            smoothing: ParameterSmoothing::None,
            change_impact: ParameterChangeImpact::ValueOnly,
        },
        base: ParameterValue::Float(0.5),
    };
    let program = PreparedModulation::new(
        &[target],
        &[ModulationRoute {
            source: 0,
            target: id,
            amount: 0.1,
        }],
        1,
    )
    .unwrap();
    let mut probe =
        ModulationRuntime::new(ModulationLfoBank::new(&[source()], 2).unwrap(), program).unwrap();
    probe
        .inherit_committed(chain.entries[0].modulation.as_ref().unwrap())
        .unwrap();
    probe.begin();
    let mut result = 0.0;
    probe
        .render_chunk(1, None, |values| {
            let Some(ParameterValue::Float(value)) = values.value_at(id, 0) else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            result = value;
            Ok(())
        })
        .unwrap();
    result
}

#[test]
fn unassigned_sources_advance_transactionally_on_static_and_bypassed_devices() {
    for bypassed in [false, true] {
        let mut chain = chain(bypassed);
        assert!(chain.has_stateful_processors());
        assert!(!chain.static_processing_matches(&chain));
        assert!(!chain.detached_source_history_matches(&chain));
        chain.begin_block();
        let mut frames = [[0.25, -0.125]; 5];
        chain.process_block(&mut frames).unwrap();
        assert_eq!(
            frames,
            if bypassed {
                [[0.25, -0.125]; 5]
            } else {
                [[0.5, -0.25]; 5]
            }
        );
        chain.rollback_block();
        assert_eq!(next_value(&chain), 0.5);
        chain.begin_block();
        chain.process_block(&mut [[0.1; 2]; 5]).unwrap();
        chain.commit_block();
        assert!((next_value(&chain) - 0.6).abs() < 1e-6);
    }
}

#[test]
fn source_phase_survives_processor_version_changes_and_honors_tail_exclusions() {
    let mut old = chain(false);
    old.begin_block();
    old.process_block(&mut [[0.1; 2]; 5]).unwrap();
    old.commit_block();
    for transfer in [false, true] {
        let mut candidate = chain(false);
        // DSP layout versions may differ when adding/removing a mapping. The
        // source belongs to the stable device, independently of that layout.
        candidate.entries[0].runtime_version = 17;
        if transfer {
            candidate.transfer_history_from(&mut old);
        } else {
            candidate.inherit_from(&old);
        }
        assert!((next_value(&candidate) - 0.6).abs() < 1e-6);
        let mut excluded = chain(false);
        excluded
            .inherit_from_excluding_old_range(&old, 0..1)
            .unwrap();
        assert_eq!(next_value(&excluded), 0.5);
        let mut other_device = chain(false);
        other_device.entries[0].device_id = 3;
        other_device.inherit_from(&old);
        assert_eq!(next_value(&other_device), 0.5);
    }
}
