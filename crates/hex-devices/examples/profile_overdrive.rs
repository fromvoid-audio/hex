//! Finite, asset-free throughput probe; run with `cargo run --release --locked
//! -p hex-devices --example profile_overdrive`. This does not measure physical
//! device deadlines or establish Ableton sound/performance parity.
use hex_devices::overdrive::{OVERDRIVE_DEFINITION as d, parameter};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = d.default_state()?;
    state = d.patch_parameter(&state, parameter::DRIVE, ParameterValue::Float(1.0))?;
    state = d.patch_parameter(&state, parameter::DRY_WET, ParameterValue::Float(1.0))?;
    for rate in [48_000, 96_000] {
        for block in [64, 256, 1_024] {
            let prepared = d
                .prepare_processor(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES,
                    },
                )?
                .ok_or("missing overdrive processor")?;
            let mut runtime = prepared.create_runtime();
            let source: Vec<_> = (0..block)
                .map(|i| {
                    [
                        (i as f32 * 0.13).sin() * 0.2,
                        (i as f32 * 0.17).cos() * 0.15,
                    ]
                })
                .collect();
            let mut output = source.clone();
            let mut run = || {
                output.copy_from_slice(&source);
                runtime.process_block(black_box(&mut output), &EMPTY_DEVICE_RESOURCES);
                black_box(&output);
            };
            for _ in 0..1_000 {
                run();
            }
            let start = Instant::now();
            for _ in 0..10_000 {
                run();
            }
            let ns_per_block = start.elapsed().as_nanos() as f64 / 10_000.0;
            println!(
                "overdrive os={} arch={} rate={rate} block={block} warmup=1000 blocks=10000 drive=100% mix=100% runtime_bytes={} ns_per_block={ns_per_block:.1} audio_time_ratio={:.6}",
                std::env::consts::OS,
                std::env::consts::ARCH,
                prepared.declared_runtime_bytes(),
                ns_per_block / (1.0e9 * block as f64 / f64::from(rate))
            );
        }
    }
    Ok(())
}
