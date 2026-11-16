//! Bounded optimized headless DSP/transaction and history-transfer observations.
//! Includes chain begin/process/commit, excludes construction and output scanning.
//! No physical callback deadline or mixed-project latency claim.
use hex_devices::delay::{DEFINITION, FEEDBACK, LEFT_SYNC, LFO_DELAY, LFO_FILTER, TIME};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, NativeRuntimeChain,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::{error::Error, hint::black_box, sync::Arc, time::Instant};
fn chain(rate: u32, count: u64) -> Result<NativeRuntimeChain, Box<dyn Error>> {
    let mut state = DEFINITION.default_state()?;
    state = DEFINITION.patch_parameter(&state, TIME, ParameterValue::Float(1000.0))?;
    state = DEFINITION.patch_parameter(&state, FEEDBACK, ParameterValue::Float(0.85))?;
    state = DEFINITION.patch_parameter(&state, LEFT_SYNC, ParameterValue::Bool(false))?;
    state = DEFINITION.patch_parameter(&state, LFO_DELAY, ParameterValue::Float(0.2))?;
    state = DEFINITION.patch_parameter(&state, LFO_FILTER, ParameterValue::Float(0.2))?;
    let context = DevicePreparationContext {
        sample_rate: rate,
        tuning: &TuningTable::default(),
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let mut entries = Vec::new();
    for id in 1..=count {
        entries.push(PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: id,
            device_type: DEFINITION.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: rate,
            configuration: Some(Arc::new(state.clone())),
            definition: DEFINITION.prepare_runtime_factory(&state, &context)?,
        });
    }
    Ok(NativeRuntimeChain::new_prepared(entries)?)
}
fn main() -> Result<(), Box<dyn Error>> {
    for (rate, count) in [(48000, 1), (48000, 8), (192000, 1), (192000, 2)] {
        for block in [64, 257] {
            let mut chain = chain(rate, count)?;
            let mut frames = vec![[0.05, -0.025]; block];
            for _ in 0..(rate as usize).div_ceil(block) * 17 {
                frames.fill([0.05, -0.025]);
                chain.begin_block();
                chain.process_block(&mut frames)?;
                chain.commit_block();
            }
            let mut samples = [0.0; 3];
            for sample in &mut samples {
                let start = Instant::now();
                for _ in 0..5000 {
                    frames.fill([0.05, -0.025]);
                    chain.begin_block();
                    chain.process_block(black_box(&mut frames))?;
                    chain.commit_block();
                }
                *sample = start.elapsed().as_secs_f64() * 1e6 / 5000.0;
            }
            samples.sort_by(f64::total_cmp);
            let mut receiver = self::chain(rate, count)?;
            if !receiver.can_inherit_from(&chain) {
                return Err("history transfer unexpectedly refused".into());
            }
            let start = Instant::now();
            for _ in 0..100 {
                receiver.transfer_history_from(black_box(&mut chain));
                chain.transfer_history_from(black_box(&mut receiver));
            }
            let transfer_us = start.elapsed().as_secs_f64() * 1e6 / 200.0;
            if !frames.iter().flatten().all(|v| v.is_finite()) {
                return Err("nonfinite output".into());
            }
            println!(
                "rate={rate} devices={count} block={block} pair_bytes={} block_us_median={:.3} runs_us={samples:?} compatible_handoff_us_mean={transfer_us:.3}",
                chain.declared_pair_bytes(),
                samples[1]
            );
        }
    }
    Ok(())
}
