//! Sustained effect-only headless profiles, not physical callback deadlines or
//! representative mixed-media/export acceptance. A standalone registered chain
//! validates every sample before timing; the timed run must match its full digest.
//! CLI bounds limit profiling storage/work without changing runtime budgets.
//! `--seconds` counts measured audio frames exactly, including a partial final
//! block; warmup is additional. Per-call render timing excludes digest/activity
//! scans, while loop elapsed time includes them. Fixture/compile timing includes
//! canonical authorship, not the separate Renderer runtime construction.
//! Run with `--help` for the bounded rate, block, duration and density controls.
#[path = "profile_lattice_effects/activity.rs"]
mod activity;
#[path = "profile_lattice_effects/config.rs"]
mod config;
use config::Config;
#[cfg(test)]
#[path = "profile_lattice_effects/tests.rs"]
mod tests;
use hex_devices::{compressor, gate, native_registry, saturator};
use hex_engine::{Audio, Renderer, compile};
use hex_lattice::NativeProcessorRuntime;
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, error::Error, hint::black_box, time::Instant};

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
        (
            "gate-compressor-saturator",
            vec![gate.clone(), comp.clone(), sat.clone()],
        ),
        (
            "mixed-4",
            [gate.clone(), comp.clone(), sat.clone()]
                .into_iter()
                .cycle()
                .take(4)
                .collect(),
        ),
        (
            "mixed-16",
            [gate, comp, sat].into_iter().cycle().take(16).collect(),
        ),
    ])
}

fn fixture(
    config: Config,
    states: &[NativeDeviceState],
) -> Fallible<(hex_engine::Project, Vec<[f32; 2]>)> {
    config.validate()?;
    let samples: Vec<_> = (0..SOURCE)
        .map(|i| {
            let level = if i < SOURCE / 2 { 0.2 } else { 0.002 };
            let sign = if i % 32 < 16 { 1.0 } else { -1.0 };
            [level * sign, -level * sign * 0.65]
        })
        .collect();
    let mut editor = Editor::new(Project::new(config.rate)?, 1)?;
    let Created::Asset(asset) = editor.execute(Command::AddAsset {
        path: "memory/lattice-profile.wav".into(),
        sample_rate: config.rate,
        frames: SOURCE as u64,
    })?
    else {
        return Err("expected profile asset".into());
    };
    for i in 0..config.tracks {
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
            duration: (config.warmup_frames() + config.measured_frames()) as u64,
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
    let media = BTreeMap::from([(asset, Audio::new(config.rate, samples.clone())?)]);
    Ok((compile::project(editor.project(), &media)?, samples))
}

fn digest_frames(digest: &mut u64, frames: &[[f32; 2]]) {
    for sample in frames.iter().flatten() {
        *digest ^= u64::from(sample.to_bits());
        *digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
}
const DIGEST_START: u64 = 0xcbf2_9ce4_8422_2325;

fn oracle(
    config: Config,
    project: &hex_engine::Project,
    samples: &[[f32; 2]],
    states: &[NativeDeviceState],
) -> Fallible<(f32, u64, activity::Summary)> {
    config.validate()?;
    if samples.len() != SOURCE {
        return Err("profile source length changed".into());
    }
    let mut processors: Vec<Box<dyn NativeProcessorRuntime>> = Vec::new();
    for state in states {
        let prepared = native_registry()?
            .prepare_processor(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: config.rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )?
            .ok_or("expected registered processor")?;
        processors.push(prepared.create_runtime());
    }
    let mut renderer = Renderer::new(project);
    renderer.play();
    let mut actual = vec![[0.0; 2]; config.block];
    let mut expected = actual.clone();
    let mut max_error = 0.0_f32;
    let mut digest = DIGEST_START;
    let mut activity = activity::Activity::new(config.rate);
    let total = config.warmup_frames() + config.measured_frames();
    for offset in (0..total).step_by(config.block) {
        let count = config.block.min(total - offset);
        for (i, frame) in expected[..count].iter_mut().enumerate() {
            *frame = samples[(offset + i) % SOURCE];
        }
        for processor in &mut processors {
            processor.process_block(&mut expected[..count], &hex_lattice::EMPTY_DEVICE_RESOURCES);
        }
        renderer.render(&mut actual[..count])?;
        if offset >= config.warmup_frames() {
            digest_frames(&mut digest, &actual[..count]);
            activity.observe(&actual[..count])?;
        }
        for (frame_index, (actual, expected)) in
            actual[..count].iter().zip(&expected[..count]).enumerate()
        {
            for channel in 0..2 {
                let mut sum = 0.0_f32;
                for _ in 0..config.tracks {
                    sum += expected[channel] * GAIN;
                }
                let error = (actual[channel] - sum).abs();
                if !actual[channel].is_finite() || !sum.is_finite() || error > TOLERANCE {
                    return Err(format!("whole-run oracle mismatch frame={} channel={channel} actual={} expected={sum} error={error}", offset + frame_index, actual[channel]).into());
                }
                max_error = max_error.max(error);
            }
        }
    }
    if renderer.position() != total as u64 {
        return Err("profile renderer frame count differs".into());
    }
    Ok((max_error, digest, activity.finish()?))
}

fn profile(config: Config, name: &str, states: &[NativeDeviceState]) -> Fallible<()> {
    config.validate()?;
    let processors = config.tracks * states.len();
    let preparation = Instant::now();
    let (project, samples) = match fixture(config, states) {
        Ok(fixture) => fixture,
        // Only the existing specific dense-count refusal is an expected result.
        // Configuration, asset, schema, provider and lower-count failures stay errors.
        Err(error)
            if processors > 256
                && error.to_string() == "stateful device runtime exceeds its bounded budget" =>
        {
            println!(
                "case={name} processors={processors} admission=refused fixture_compile_ms={:.3} error={error}",
                preparation.elapsed().as_secs_f64() * 1000.0
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let fixture_compile_ms = preparation.elapsed().as_secs_f64() * 1000.0;
    let (max_error, oracle_digest, oracle_activity) = oracle(config, &project, &samples, states)?;
    let mut renderer = Renderer::new(&project);
    renderer.set_effect_metering(config.meters);
    renderer.play();
    let mut output = vec![[0.0; 2]; config.block];
    for _ in 0..config.warmup {
        renderer.render(black_box(&mut output))?;
    }
    let mut timings = Vec::with_capacity(config.blocks());
    let mut digest = DIGEST_START;
    let mut activity = activity::Activity::new(config.rate);
    let measured = config.measured_frames();
    let wall = Instant::now();
    for offset in (0..measured).step_by(config.block) {
        let count = config.block.min(measured - offset);
        let start = Instant::now();
        renderer.render(black_box(&mut output[..count]))?;
        timings.push((count, start.elapsed().as_nanos()));
        digest_frames(&mut digest, &output[..count]);
        activity.observe(&output[..count])?;
    }
    let elapsed_ms = wall.elapsed().as_secs_f64() * 1000.0;
    let activity = activity.finish()?;
    if digest != oracle_digest || activity.windows != oracle_activity.windows {
        return Err("timed whole-run PCM differs from validated oracle run".into());
    }
    if renderer.position() != (config.warmup_frames() + measured) as u64 {
        return Err("timed renderer frame count differs".into());
    }
    let total: u128 = timings.iter().map(|(_, ns)| ns).sum();
    let late = timings
        .iter()
        .filter(|&&(frames, ns)| ns > frames as u128 * 1_000_000_000 / u128::from(config.rate))
        .count();
    timings.sort_unstable_by_key(|(_, ns)| *ns);
    let percentile =
        |percent: usize| timings[(timings.len() - 1) * percent / 100].1 as f64 / 1_000.0;
    println!(
        "case={name} processors={processors} admission=accepted fixture_compile_ms={fixture_compile_ms:.3} measured_frames={measured} audio_seconds={:.6} blocks={} activity_windows={} min_window_peak={:.9} final_window_peak={:.9} oracle_max_error={max_error:.9} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} render_total_ms={:.3} loop_elapsed_ms={elapsed_ms:.3} beyond_period={late} pcm_digest={digest:016x}",
        measured as f64 / f64::from(config.rate),
        timings.len(),
        activity.windows,
        activity.min_peak,
        activity.final_peak,
        percentile(50),
        percentile(95),
        percentile(99),
        timings[timings.len() - 1].1 as f64 / 1_000.0,
        total as f64 / 1_000_000.0,
    );
    Ok(())
}

fn main() -> Fallible<()> {
    if std::env::args().any(|arg| arg == "--help") {
        println!(
            "sustained resident effect-only profile · --rate 44100|48000|96000 --block 128|512 --tracks 1..100 --seconds 1..300 --warmup 0..1000 --devices 0|1|2|3|4|16 --meters; defaults: 48000, 128, 100, 60, 100, all densities, meters off"
        );
        return Ok(());
    }
    let config = Config::parse(std::env::args().skip(1))?;
    println!(
        "lattice effects: {}-{} entrypoint=compile-project-renderer fixture=homogeneous-resident-loop rate={} block={} tracks={} warmup_blocks={} measured_frames={} source_frames={SOURCE} meters={} nominal_full_period_us={:.3}; standalone whole-run composition oracle precedes timing; numeric activity is not listening acceptance; other host load uncontrolled; not physical callback deadlines",
        std::env::consts::OS,
        std::env::consts::ARCH,
        config.rate,
        config.block,
        config.tracks,
        config.warmup,
        config.measured_frames(),
        config.meters,
        config.block as f64 / f64::from(config.rate) * 1_000_000.0
    );
    for (name, states) in states()? {
        if config.devices.is_none_or(|count| states.len() == count) {
            profile(config, name, &states)?;
        }
    }
    Ok(())
}
