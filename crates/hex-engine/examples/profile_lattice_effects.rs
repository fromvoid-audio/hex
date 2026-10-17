//! Bounded 100-track effect profiles with a complete standalone composition oracle.
use hex_devices::{compressor, gate, native_registry, saturator};
use hex_engine::{Audio, Renderer, compile};
use hex_lattice::NativeProcessorRuntime;
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const TRACKS: usize = 100;
const WARMUP: usize = 100;
const MEASURED: usize = 2_000;
const SOURCE: usize = 512;
const GAIN: f32 = 0.01;
const TOLERANCE: f32 = 2.0e-6;
type Fallible<T> = Result<T, Box<dyn Error>>;

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> Fallible<NativeDeviceState> {
    Ok(native_registry()?.patch_parameter(state, id, ParameterValue::Float(value))?)
}

fn states() -> Fallible<Vec<(&'static str, Vec<NativeDeviceState>)>> {
    let sat = patch(
        &saturator::SATURATOR_DEFINITION.default_state()?,
        saturator::parameter::DRIVE,
        18.0,
    )?;
    let sat = patch(&sat, saturator::parameter::OUTPUT, -12.0)?;
    let comp = patch(
        &compressor::COMPRESSOR_DEFINITION.default_state()?,
        compressor::parameter::THRESHOLD,
        -24.0,
    )?;
    let gate = patch(
        &gate::GATE_DEFINITION.default_state()?,
        gate::parameter::THRESHOLD,
        -24.0,
    )?;
    let gate = patch(&gate, gate::parameter::RETURN, -30.0)?;
    let gate = patch(&gate, gate::parameter::HOLD, 1.0)?;
    let gate = patch(&gate, gate::parameter::RELEASE, 1.0)?;
    let gate = patch(&gate, gate::parameter::FLOOR, -36.0)?;
    Ok(vec![
        ("baseline", vec![]),
        ("saturator", vec![sat.clone()]),
        ("compressor", vec![comp.clone()]),
        ("gate", vec![gate.clone()]),
        ("gate-compressor", vec![gate.clone(), comp.clone()]),
        ("gate-compressor-saturator", vec![gate, comp, sat]),
    ])
}

fn fixture(states: &[NativeDeviceState]) -> Fallible<(hex_engine::Project, Vec<[f32; 2]>)> {
    let samples: Vec<_> = (0..SOURCE)
        .map(|i| {
            let level = if i < SOURCE / 2 { 0.2 } else { 0.002 };
            let sign = if i % 32 < 16 { 1.0 } else { -1.0 };
            [level * sign, -level * sign * 0.65]
        })
        .collect();
    let mut editor = Editor::new(Project::new(RATE)?, 1)?;
    let Created::Asset(asset) = editor.execute(Command::AddAsset {
        path: "memory/lattice-profile.wav".into(),
        sample_rate: RATE,
        frames: SOURCE as u64,
    })?
    else {
        return Err("expected profile asset".into());
    };
    for i in 0..TRACKS {
        let Created::Track(track) = editor.execute(Command::AddTrack {
            name: format!("profile {i}"),
        })?
        else {
            return Err("expected profile track".into());
        };
        editor.execute(Command::Gain { track, gain: GAIN })?;
        editor.execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: ((WARMUP + MEASURED) * BLOCK) as u64,
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
    }
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.clone())?)]);
    Ok((compile::project(editor.project(), &media)?, samples))
}

fn oracle(
    project: &hex_engine::Project,
    samples: &[[f32; 2]],
    states: &[NativeDeviceState],
) -> Fallible<(f32, u64)> {
    let mut processors: Vec<Box<dyn NativeProcessorRuntime>> = Vec::new();
    for state in states {
        let prepared = native_registry()?
            .prepare_processor(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: RATE,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )?
            .ok_or("expected registered processor")?;
        processors.push(prepared.create_runtime());
    }
    let mut renderer = Renderer::new(project);
    renderer.play();
    let mut actual = [[0.0; 2]; BLOCK];
    let mut expected = actual;
    let mut max_error = 0.0_f32;
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for block in 0..WARMUP + MEASURED {
        for (i, frame) in expected.iter_mut().enumerate() {
            *frame = samples[(block * BLOCK + i) % SOURCE];
        }
        for processor in &mut processors {
            processor.process_block(&mut expected);
        }
        renderer.render(&mut actual)?;
        if block >= WARMUP {
            for sample in actual.iter().flatten() {
                digest ^= u64::from(sample.to_bits());
                digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        for (frame_index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            for channel in 0..2 {
                let mut sum = 0.0_f32;
                for _ in 0..TRACKS {
                    sum += expected[channel] * GAIN;
                }
                let error = (actual[channel] - sum).abs();
                if !actual[channel].is_finite() || error > TOLERANCE {
                    return Err(format!("whole-run oracle mismatch frame={} channel={channel} actual={} expected={sum} error={error}", block * BLOCK + frame_index, actual[channel]).into());
                }
                max_error = max_error.max(error);
            }
        }
    }
    Ok((max_error, digest))
}

fn profile(name: &str, states: &[NativeDeviceState]) -> Fallible<()> {
    let (project, samples) = match fixture(states) {
        Ok(fixture) => fixture,
        Err(error)
            if name == "gate-compressor-saturator"
                && error.to_string() == "stateful device runtime exceeds its bounded budget" =>
        {
            println!(
                "case={name} processors={} admission=refused error={error}",
                TRACKS * states.len()
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let (max_error, oracle_digest) = oracle(&project, &samples, states)?;
    let mut renderer = Renderer::new(&project);
    renderer.set_effect_metering(std::env::args().any(|argument| argument == "--meters"));
    renderer.play();
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..WARMUP {
        renderer.render(black_box(&mut output))?;
    }
    let mut timings = Vec::with_capacity(MEASURED);
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for _ in 0..MEASURED {
        let start = Instant::now();
        renderer.render(black_box(&mut output))?;
        timings.push(start.elapsed().as_nanos());
        for sample in output.iter().flatten() {
            digest ^= u64::from(sample.to_bits());
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    if digest != oracle_digest {
        return Err("timed whole-run PCM differs from validated oracle run".into());
    }
    let total: u128 = timings.iter().sum();
    let deadline_ns = u128::from(BLOCK as u64) * 1_000_000_000 / u128::from(RATE);
    let late = timings.iter().filter(|&&ns| ns > deadline_ns).count();
    timings.sort_unstable();
    let percentile = |percent| timings[(MEASURED - 1) * percent / 100] as f64 / 1_000.0;
    println!(
        "case={name} processors={} oracle_max_error={max_error:.9} p50_us={:.3} p99_us={:.3} max_us={:.3} total_ms={:.3} beyond_period={late} pcm_digest={digest:016x}",
        TRACKS * states.len(),
        percentile(50),
        percentile(99),
        timings[MEASURED - 1] as f64 / 1_000.0,
        total as f64 / 1_000_000.0
    );
    Ok(())
}

fn main() -> Fallible<()> {
    println!(
        "lattice effects: {}-{} rate={RATE} block={BLOCK} tracks={TRACKS} warmup={WARMUP} measured={MEASURED} source_frames={SOURCE} nominal_period_us={:.3}; standalone whole-run composition oracle precedes timing; other host load uncontrolled",
        std::env::consts::OS,
        std::env::consts::ARCH,
        BLOCK as f64 / f64::from(RATE) * 1_000_000.0
    );
    for (name, states) in states()? {
        profile(name, &states)?;
    }
    Ok(())
}
