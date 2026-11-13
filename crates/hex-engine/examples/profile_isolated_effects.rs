//! Bounded serial study of separate one-track owners. This does not admit a
//! 100-track graph or measure physical callbacks, handoffs, or retained tails.
use hex_devices::{compressor, gate, native_registry, saturator};
use hex_engine::{Audio, Renderer, compile};
use hex_lattice::NativeProcessorRuntime;
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const SOURCE: usize = 512;
const GAIN: f32 = 0.01;
const TOLERANCE: f32 = 2.0e-6;
const WARMUP: usize = 100;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> Result<NativeDeviceState> {
    Ok(native_registry()?.patch_parameter(state, id, ParameterValue::Float(value))?)
}
fn states(count: usize) -> Result<Vec<NativeDeviceState>> {
    let gate = patch(
        &gate::GATE_DEFINITION.default_state()?,
        gate::parameter::THRESHOLD,
        -24.0,
    )?;
    let comp = patch(
        &compressor::COMPRESSOR_DEFINITION.default_state()?,
        compressor::parameter::THRESHOLD,
        -24.0,
    )?;
    let sat = patch(
        &saturator::SATURATOR_DEFINITION.default_state()?,
        saturator::parameter::DRIVE,
        18.0,
    )?;
    let sat = patch(&sat, saturator::parameter::OUTPUT, -12.0)?;
    Ok([gate, comp, sat].into_iter().cycle().take(count).collect())
}
fn source() -> Vec<[f32; 2]> {
    (0..SOURCE)
        .map(|i| {
            let level = if i < SOURCE / 2 { 0.2 } else { 0.002 };
            let sign = if i % 32 < 16 { 1.0 } else { -1.0 };
            [level * sign, -level * sign * 0.65]
        })
        .collect()
}
fn fixture(
    states: &[NativeDeviceState],
    block: usize,
    seconds: usize,
    samples: &[[f32; 2]],
) -> Result<hex_engine::Project> {
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let Created::Asset(asset) = editor.execute(Command::AddAsset {
        path: "memory/isolated-effect-profile.wav".into(),
        sample_rate: RATE,
        frames: SOURCE as u64,
    })?
    else {
        return Err("missing profile asset".into());
    };
    let Created::Track(track) = editor.execute(Command::AddTrack {
        name: "isolated effect profile".into(),
    })?
    else {
        return Err("missing profile track".into());
    };
    editor.execute(Command::Gain { track, gain: GAIN })?;
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: (WARMUP * block + seconds * RATE as usize) as u64,
        source_offset: 0,
        source_length: SOURCE as u64,
        looping: true,
    })?;
    for state in states {
        editor.execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })?;
    }
    Ok(compile::project(
        editor.project(),
        &BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec())?)]),
    )?)
}
fn processors(states: &[NativeDeviceState]) -> Result<Vec<Box<dyn NativeProcessorRuntime>>> {
    states
        .iter()
        .map(|state| {
            let factory = native_registry()?
                .prepare_processor(
                    state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: RATE,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                    },
                )?
                .ok_or("missing registered processor")?;
            Ok(factory.create_runtime())
        })
        .collect()
}
fn digest(hash: &mut u64, output: &[[f32; 2]]) {
    for sample in output.iter().flatten() {
        *hash = (*hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
    }
}
fn verify(actual: &[[f32; 2]], expected: &[[f32; 2]], tracks: usize) -> Result<f32> {
    if actual.len() != expected.len() {
        return Err("composition frame count mismatch".into());
    }
    let mut max_error = 0.0_f32;
    for (actual, expected) in actual.iter().zip(expected) {
        for channel in 0..2 {
            let mut sum = 0.0_f32;
            for _ in 0..tracks {
                sum += expected[channel] * GAIN;
            }
            let error = (actual[channel] - sum).abs();
            if !error.is_finite() || error > TOLERANCE {
                return Err(format!(
                    "isolated composition mismatch actual={} expected={sum} error={error}",
                    actual[channel]
                )
                .into());
            }
            max_error = max_error.max(error);
        }
    }
    Ok(max_error)
}
fn run(tracks: usize, count: usize, block: usize, seconds: usize) -> Result<()> {
    if !(1..=100).contains(&tracks)
        || ![1, 4, 16].contains(&count)
        || ![128, 512].contains(&block)
        || !(1..=60).contains(&seconds)
    {
        return Err("isolated profile bounds exceeded".into());
    }
    let samples = source();
    let states = states(count)?;
    let project = fixture(&states, block, seconds, &samples)?;
    let mut owners: Vec<_> = (0..tracks).map(|_| Renderer::new(&project)).collect();
    for owner in &mut owners {
        owner.play();
    }
    let mut standalone = processors(&states)?;
    let mut scratch = vec![[0.0; 2]; block];
    let mut actual = scratch.clone();
    let mut expected = scratch.clone();
    let total = WARMUP * block + seconds * RATE as usize;
    let mut timings = Vec::with_capacity((seconds * RATE as usize).div_ceil(block));
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let mut max_error = 0.0_f32;
    let mut window_peak = 0.0_f32;
    let mut windows = 0;
    for offset in (0..total).step_by(block) {
        let frames = block.min(total - offset);
        actual[..frames].fill([0.0; 2]);
        let started = Instant::now();
        for owner in &mut owners {
            owner.render(black_box(&mut scratch[..frames]))?;
            for (sum, frame) in actual[..frames].iter_mut().zip(&scratch[..frames]) {
                sum[0] += frame[0];
                sum[1] += frame[1];
            }
        }
        let elapsed = started.elapsed().as_nanos();
        for (index, frame) in expected[..frames].iter_mut().enumerate() {
            *frame = samples[(offset + index) % SOURCE];
        }
        for processor in &mut standalone {
            processor.process_block(
                &mut expected[..frames],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
        }
        max_error = max_error.max(verify(&actual[..frames], &expected[..frames], tracks)?);
        if offset >= WARMUP * block {
            timings.push((frames, elapsed));
            digest(&mut hash, &actual[..frames]);
            for (index, frame) in actual[..frames].iter().enumerate() {
                for value in frame {
                    window_peak = window_peak.max(value.abs());
                }
                if (offset - WARMUP * block + index + 1).is_multiple_of(RATE as usize) {
                    if window_peak < 1.0e-6 {
                        return Err("silent measured second".into());
                    }
                    windows += 1;
                    window_peak = 0.0;
                }
            }
        }
    }
    if windows != seconds {
        return Err("incomplete measured activity windows".into());
    }
    let late = timings
        .iter()
        .filter(|&&(frames, ns)| ns * u128::from(RATE) > frames as u128 * 1_000_000_000)
        .count();
    timings.sort_unstable_by_key(|&(_, ns)| ns);
    let p = |n: usize| timings[(timings.len() - 1) * n / 100].1 as f64 / 1000.0;
    println!(
        "isolated_owners={tracks} effects_per_owner={count} total_effects={} rate={RATE} block={block} seconds={seconds} warmup_blocks={WARMUP} measured_blocks={} activity_windows={windows} max_oracle_error={max_error:e} pcm_digest={hash:016x} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} beyond_period={late}; separate owners, no unified admission, no callback or native deadline",
        tracks * count,
        timings.len(),
        p(50),
        p(95),
        p(99),
        p(100)
    );
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "usage: profile_isolated_effects <effects:1|4|16> <block:128|512> <seconds:1..60>"
                .into(),
        );
    }
    run(100, args[0].parse()?, args[1].parse()?, args[2].parse()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_owners_match_standalone_composition_and_keep_activity() -> Result<()> {
        run(2, 4, 128, 1)?;
        run(2, 4, 512, 1)
    }
    #[test]
    fn oracle_rejects_effect_reordering() -> Result<()> {
        let samples = source();
        let chain = states(4)?;
        let project = fixture(&chain, 512, 1, &samples)?;
        let mut render = Renderer::new(&project);
        render.play();
        let mut actual = [[0.0; 2]; SOURCE];
        render.render(&mut actual)?;
        let mut expected = samples;
        let mut reversed = chain;
        reversed.reverse();
        for processor in &mut processors(&reversed)? {
            processor.process_block(&mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        }
        assert!(verify(&actual, &expected, 1).is_err());
        Ok(())
    }
    #[test]
    fn oracle_rejects_missing_track_and_nonfinite_pcm() {
        assert!(verify(&[[0.01, 0.01]], &[[1.0, 1.0]], 2).is_err());
        assert!(verify(&[[f32::NAN, 0.0]], &[[0.0, 0.0]], 1).is_err());
        assert!(verify(&[], &[[0.0, 0.0]], 1).is_err());
    }
}
