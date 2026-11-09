//! Prepared native output gain renders and hands off without realtime allocation.
use hex_devices::utility::{UTILITY_DEFINITION, UTILITY_TYPE_ID};
use hex_lattice::{
    HostOutputModulationRoute, LfoWaveform, ModulationSourceId, NativeRuntimeChain,
    PreparedModulation, PreparedModulationSource, PreparedModulationState, PreparedRuntimeInstance,
    ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::{HostGain, NativeDeviceMetadata};
use std::{hint::black_box, sync::Arc};

#[test]
fn host_output_modulation_handoff_keeps_phase_without_allocation() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 127;
    const DEPTH: f32 = 0.05;
    let dry = [0.125, -0.25];
    let program = PreparedModulation::with_host_output(
        &[],
        &[],
        1,
        Some((
            HostGain::ZERO,
            &[HostOutputModulationRoute {
                source: 0,
                amount: DEPTH,
                range: [-1.0, 1.0],
            }],
        )),
    )
    .unwrap();
    let source = PreparedModulationSource::free_with_waveform(
        ModulationSourceId::new(1).unwrap(),
        f64::from(RATE),
        12_000.0,
        0.0,
        LfoWaveform::Square,
    )
    .unwrap();
    let prepared = PreparedRuntimeInstance {
        modulation: Some(PreparedModulationState::new(vec![source], program, 17).unwrap()),
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: UTILITY_TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: RATE,
        configuration: Some(Arc::new(UTILITY_DEFINITION.default_state().unwrap())),
        definition: None,
    };
    let mut first = NativeRuntimeChain::new_prepared(vec![prepared.clone()]).unwrap();
    let mut replacement = NativeRuntimeChain::new_prepared(vec![prepared]).unwrap();
    let mut retried = [dry; FRAMES];
    let mut resumed = [dry; FRAMES];
    let counts = allocation_counter::measure(|| {
        first.begin_block();
        first
            .process_range_with_clock(0..1, black_box(&mut retried), None, None)
            .unwrap();
        first.rollback_block();
        retried.fill(dry);
        first.begin_block();
        first
            .process_range_with_clock(0..1, black_box(&mut retried), None, None)
            .unwrap();
        first.commit_block();
        replacement.inherit_from(&first);
        replacement.transfer_history_from(&mut first);
        replacement.begin_block();
        replacement
            .process_range_with_clock(0..1, black_box(&mut resumed), None, None)
            .unwrap();
        replacement.commit_block();
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
    for (index, frame) in retried.into_iter().chain(resumed).enumerate() {
        let wave = if index % 4 < 2 { 1.0 } else { -1.0 };
        let normalized = (f64::from(60.0_f32 / 84.0) + f64::from(DEPTH) * wave) as f32;
        let db = (f64::from(normalized) * 84_000.0 - 60_000.0).round() as f32 / 1000.0;
        let gain = 10.0_f32.powf(db / 20.0);
        assert_eq!(frame, dry.map(|sample| sample * gain), "frame {index}");
    }
}
