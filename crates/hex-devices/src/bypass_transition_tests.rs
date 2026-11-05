//! Live host transitions use the actual provider state, including bypass history.
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeDeviceRegistry,
    NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner,
};
use std::sync::Arc;

fn chain(definition: &'static dyn NativeDeviceDefinition, bypassed: bool) -> NativeRuntimeChain {
    let state = definition
        .default_state()
        .unwrap()
        .with_output_gain_db(6.0)
        .unwrap();
    let definitions = [definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let prepared = registry
        .prepare_runtime_device(
            &state,
            &DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: state.type_id(),
        matrix: prepared.matrix,
        bypassed,
        sample_rate: 48_000,
        configuration: Some(Arc::new(state)),
        definition: prepared.processor,
    }])
    .unwrap()
}

fn render(chain: &mut NativeRuntimeChain, count: usize, block: usize) -> Vec<[f32; 2]> {
    let mut frames = vec![[0.25, -0.25]; count];
    for part in frames.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(part).unwrap();
        chain.commit_block();
    }
    frames
}

fn check(definition: &'static dyn NativeDeviceDefinition) {
    for old_bypassed in [false, true] {
        let mut wet_reference = chain(definition, false);
        render(&mut wet_reference, 1024, 64);
        let wet = render(&mut wet_reference, 300, 1);
        for block in [1, 7, 64, 256] {
            let mut old = chain(definition, old_bypassed);
            render(&mut old, 1024, 64);
            let mut next = chain(definition, !old_bypassed);
            assert!(next.configuration_matches_range(&old, 0..1));
            assert!(next.can_inherit_from(&old));
            next.transfer_history_from(&mut old);
            let actual = render(&mut next, 300, block);
            for (index, (frame, wet)) in actual.iter().zip(&wet).enumerate() {
                let progress = (index + 1).min(240) as f32 / 240.0;
                let mix = if old_bypassed {
                    progress
                } else {
                    1.0 - progress
                };
                for channel in 0..2 {
                    let dry = if channel == 0 { 0.25 } else { -0.25 };
                    let expected = dry + (wet[channel] - dry) * mix;
                    assert!(
                        (frame[channel] - expected).abs() < 1e-5,
                        "block {block} frame {index}: {frame:?} expected {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn auto_pan_bypass_transition_preserves_real_modulator_history() {
    check(&crate::auto_pan::AUTO_PAN_DEFINITION);
}
#[test]
fn gate_bypass_transition_preserves_real_detector_history() {
    check(&crate::gate::GATE_DEFINITION);
}
#[test]
fn compressor_bypass_transition_preserves_real_detector_history() {
    check(&crate::compressor::COMPRESSOR_DEFINITION);
}
