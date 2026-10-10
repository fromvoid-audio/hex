//! Release-only original filter workload, not a native callback deadline claim.
use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prepared = PreparedUtilityFilters::new(48_000, true, true, 200.0, false)?;
    const BLOCK: usize = 128;
    let input: Vec<_> = (0..BLOCK)
        .map(|frame| [(frame as f32 * 0.041).sin(), (frame as f32 * 0.091).cos()])
        .collect();
    let mut block_oracle = input.clone();
    let mut block_processor = UtilityFilterProcessor::new(prepared);
    block_processor.process_block(&mut block_oracle);
    let mut scalar_processor = UtilityFilterProcessor::new(prepared);
    let scalar_oracle: Vec<_> = input
        .iter()
        .map(|frame| scalar_processor.process_frame(*frame))
        .collect();
    if block_oracle != scalar_oracle {
        return Err("filter scalar/block oracle mismatch".into());
    }
    println!(
        "48khz; 128frames; dc and bass mono active; 20 warmups; 1000 samples; track input copy included; construction excluded"
    );
    for tracks in [1, 100] {
        for devices in [1, 4, 16] {
            let mut processors = vec![UtilityFilterProcessor::new(prepared); tracks * devices];
            let mut block = input.clone();
            let mut timings = Vec::with_capacity(1000);
            let mut checksum = 0.0_f64;
            for iteration in 0..1020 {
                let start = Instant::now();
                for chain in processors.chunks_mut(devices) {
                    block.copy_from_slice(&input);
                    for processor in chain {
                        processor.process_block(&mut block);
                    }
                    std::hint::black_box(&block);
                    checksum += f64::from(block[BLOCK - 1][0]);
                }
                let elapsed = start.elapsed();
                if iteration >= 20 {
                    timings.push(elapsed.as_nanos());
                }
            }
            timings.sort_unstable();
            if !checksum.is_finite() {
                return Err("filter benchmark produced non-finite output".into());
            }
            println!(
                "tracks={tracks} devices={devices} p50_us={:.2} p95_us={:.2} max_us={:.2} checksum={checksum:.9}",
                timings[500] as f64 / 1000.0,
                timings[950] as f64 / 1000.0,
                timings[999] as f64 / 1000.0
            );
        }
    }
    Ok(())
}
