//! Borrowed compositor probe; this does not exercise a native audio callback.
use hex_lattice::{
    ModulationRoute, ModulationTarget, NativeParameterAutomation, PreparedModulation,
};
use hex_lattice_contract::{
    ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterDescriptor, ParameterId,
    ParameterKind, ParameterModulation, ParameterNormalization, ParameterPresentation,
    ParameterSemantics, ParameterSmoothing, ParameterStepping, ParameterValue,
};
use std::hint::black_box;

#[test]
fn borrowed_source_composition_has_no_allocations_or_frees() {
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
            amount: 0.5,
            range: [-1.0, 1.0],
        }],
        1,
    )
    .unwrap();
    let samples = [0.2; 128];
    let sources = [&samples[..]];
    let mut output = [None];
    let counts = allocation_counter::measure(|| {
        let block = program
            .block(None, black_box(&sources), samples.len())
            .unwrap();
        for offset in 0..samples.len() {
            block.values_at(offset, black_box(&mut output)).unwrap();
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
