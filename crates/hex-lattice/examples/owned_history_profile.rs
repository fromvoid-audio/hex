//! Control-side long-history spike, explicitly outside native admission.
use hex_lattice::{
    NativeDeviceRegistry, PreparedProcessorFactory, StereoFrame, conformance::owned_delay,
};
use std::{hint::black_box, time::Instant};
fn percentile(values: &mut [u128], percent: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * percent / 100]
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "fixture=owned-delay original test provider; native admission remains unchanged; stereo; block=128; warmup=100; measured=4000"
    );
    println!(
        "whole-copy baseline copies both complete histories per block; dirty interval copies only written frames at commit; neither run measures native deadlines"
    );
    for rate in [44_100u32, 48_000, 96_000, 192_000] {
        let length = rate as usize * 2;
        let providers: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
        let registry = NativeDeviceRegistry::new(&providers)?;
        let state = owned_delay::state(length as u32)?;
        let prepared = registry.prepare_runtime_device(&state, rate)?;
        let Some(PreparedProcessorFactory::Owned(factory)) = prepared.processor else {
            return Err("owned factory missing".into());
        };
        let mut owned = factory.create_runtime()?;
        let mut committed = vec![[0.0; 2]; length];
        let mut scratch = committed.clone();
        let mut next = 0usize;
        let mut owned_times = Vec::with_capacity(4000);
        let mut full_times = Vec::with_capacity(4000);
        let mut digest = 0u64;
        for block in 0..4100 {
            let mut actual: [[f32; 2]; 128] = std::array::from_fn(|i| {
                let v = ((block * 128 + i) % 257) as f32 / 256.0;
                [v, -v]
            });
            let mut expected = actual;
            let start = Instant::now();
            owned.begin_block();
            owned.process_block(black_box(&mut actual));
            owned.commit_block();
            let own_ns = start.elapsed().as_nanos();
            let start = Instant::now();
            scratch.copy_from_slice(black_box(&committed));
            for frame in &mut expected {
                std::mem::swap(frame, &mut scratch[next]);
                next = (next + 1) % length;
            }
            committed.copy_from_slice(black_box(&scratch));
            let full_ns = start.elapsed().as_nanos();
            if actual != expected {
                return Err("PCM differs from independent shift oracle".into());
            }
            for frame in actual {
                for sample in frame {
                    digest = digest.rotate_left(5) ^ u64::from(sample.to_bits());
                }
            }
            if block >= 100 {
                owned_times.push(own_ns);
                full_times.push(full_ns);
            }
        }
        let own50 = percentile(&mut owned_times, 50);
        let own99 = percentile(&mut owned_times, 99);
        let full50 = percentile(&mut full_times, 50);
        let full99 = percentile(&mut full_times, 99);
        let frames: [StereoFrame; 128] = [[0.0; 2]; 128];
        black_box(frames);
        println!(
            "rate={rate} history_frames={length} pair_bytes={} dirty_copy_bytes_per_block={} whole_copy_bytes_per_block={} dirty_p50_ns={own50} dirty_p99_ns={own99} whole_p50_ns={full50} whole_p99_ns={full99} pcm_equal=true digest={digest:016x} large_live_inheritance={}",
            factory.declared_pair_bytes(),
            128 * std::mem::size_of::<StereoFrame>(),
            2 * length * std::mem::size_of::<StereoFrame>(),
            owned.can_inherit_from(owned.as_ref())
        );
    }
    Ok(())
}
