//! Optimized provider cost only; excludes host twins, graph work and native I/O.
use hex_devices::reverb::{REVERB_DEFINITION, parameter};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = REVERB_DEFINITION.patch_parameter(
        &REVERB_DEFINITION.default_state()?,
        parameter::DECAY,
        ParameterValue::Float(2.5),
    )?;
    for density in [0, 2] {
        let state = REVERB_DEFINITION.patch_parameter(
            &state,
            parameter::DENSITY,
            ParameterValue::Choice(density),
        )?;
        for rate in [48_000, 192_000] {
            let p = REVERB_DEFINITION
                .prepare_processor(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES,
                    },
                )?
                .ok_or("missing processor")?;
            for count in [1, 16] {
                let mut runtimes: Vec<_> = (0..count).map(|_| p.create_runtime()).collect();
                let input: Vec<_> = (0..128)
                    .map(|i| {
                        [
                            (i as f32 * 0.041).sin() * 0.2,
                            (i as f32 * 0.073).cos() * 0.2,
                        ]
                    })
                    .collect();
                let mut block = input.clone();
                let mut times = Vec::with_capacity(1000);
                for iteration in 0..1200 {
                    let start = Instant::now();
                    for runtime in &mut runtimes {
                        block.copy_from_slice(&input);
                        runtime.process_block(&mut block, &EMPTY_DEVICE_RESOURCES);
                        std::hint::black_box(&block);
                    }
                    if iteration >= 200 {
                        times.push(start.elapsed().as_nanos());
                    }
                }
                times.sort_unstable();
                println!(
                    "rate={rate} instances={count} frames=128 decay=2.5s wet=55% density={density} warmups=200 samples=1000 input_copy=included construction=excluded runtime_bytes={} p50_us={:.2} p95_us={:.2} max_us={:.2}",
                    p.declared_runtime_bytes(),
                    times[500] as f64 / 1000.0,
                    times[950] as f64 / 1000.0,
                    times[999] as f64 / 1000.0
                );
            }
        }
    }
    Ok(())
}
