//! Isolated modulation transaction probe, not native callback integration.
use hex_lattice::{
    ModulationLfoBank, ModulationRoute, ModulationRuntime, ModulationSourceId, ModulationTarget,
    PreparedLfoSource, PreparedModulation,
};
use hex_lattice_contract::{
    DeviceContractError, ParameterAccess, ParameterAutomation, ParameterChangeImpact,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterModulation, ParameterNormalization,
    ParameterPresentation, ParameterSemantics, ParameterSmoothing, ParameterStepping,
    ParameterValue,
};
use std::hint::black_box;

fn prepared() -> ModulationRuntime {
    let id = ParameterId::from_static(1);
    let target = ModulationTarget {
        descriptor: ParameterDescriptor {
            id,
            label: "gain",
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
    let source =
        PreparedLfoSource::free(ModulationSourceId::new(1).unwrap(), 48_000.0, 7.0, 0.0).unwrap();
    ModulationRuntime::new(ModulationLfoBank::new(&[source], 128).unwrap(), program).unwrap()
}

#[test]
fn chunk_transactions_do_not_allocate_or_free() {
    let mut runtime = prepared();
    let mut next = prepared();
    let id = ModulationSourceId::new(1).unwrap();
    let rate = PreparedLfoSource::free(id, 48_000.0, 14.0, 0.0).unwrap();
    let counts = allocation_counter::measure(|| {
        for _ in 0..32 {
            runtime.reset(id, 0.0).unwrap();
            runtime.begin();
            runtime.retarget(rate).unwrap();
            runtime
                .render_chunk(128, None, |block| {
                    for index in 0..128 {
                        black_box(block.value_at(ParameterId::from_static(1), index));
                    }
                    Ok(())
                })
                .unwrap();
            runtime.commit().unwrap();
            next.inherit_committed(&runtime).unwrap();
            next.begin();
            assert!(
                next.render_chunk(128, None, |_| Err(
                    DeviceContractError::InvalidParameterValue
                ))
                .is_err()
            );
            next.rollback();
        }
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
}
