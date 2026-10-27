//! Release-mode headless EQ workload comparison.
//!
//! Run with `cargo run --release -p hex-devices --example profile_eq`. This is
//! not a physical audio callback deadline, allocation, listening, or external-
//! product parity test.
use hex_devices::{eq, eq8};
use hex_lattice::{
    NativeDeviceDefinition, NativeInstrumentClock, PreparedNativeProcessor, StereoFrame,
};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState, ParameterValue};
use std::{error::Error, hint::black_box, time::Instant};

const RATE: u32 = 48_000;
const WARMUP_BLOCKS: u64 = 100;
const MEASURED_BLOCKS: u64 = 2_000;
const TRIALS: usize = 3;
const AUTOMATION_END_BEAT: u64 = 128 << 32;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Clock {
    frame: u64,
}

impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.arrangement_beat_at_offset(offset)
    }

    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        let frames = u128::from(self.frame).checked_add(offset as u128)?;
        let beat = frames
            .checked_mul(2)?
            .checked_mul(1_u128 << 32)?
            .checked_div(u128::from(RATE))?;
        u64::try_from(beat).ok()
    }

    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.frame.checked_add(offset as u64)
    }

    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
}

struct Scenario {
    label: &'static str,
    prepared: Box<dyn PreparedNativeProcessor>,
    clocked: bool,
    active_bands: usize,
    automation_lanes: usize,
    automation_points: usize,
}

fn center_frequency(index: usize, count: usize) -> f32 {
    let position = if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32
    };
    80.0 * (16_000.0_f32 / 80.0).powf(position)
}

fn legacy_eight_bells() -> Result<NativeDeviceState> {
    let mut state = eq8::EQ8_DEFINITION.default_state()?;
    for index in 0..8 {
        for (id, value) in [
            (
                eq8::parameter::enabled(0, index),
                ParameterValue::Bool(true),
            ),
            (eq8::parameter::kind(0, index), ParameterValue::Choice(0)),
            (
                eq8::parameter::frequency(0, index),
                ParameterValue::Float(center_frequency(index, 8)),
            ),
            (
                eq8::parameter::gain(0, index),
                ParameterValue::Float(if index % 2 == 0 { 0.75 } else { -0.75 }),
            ),
            (eq8::parameter::q(0, index), ParameterValue::Float(0.9)),
        ] {
            state = eq8::EQ8_DEFINITION.patch_parameter(&state, id, value)?;
        }
    }
    Ok(state)
}

fn new_eq(nodes: usize, automated: bool) -> Result<eq::EqState> {
    let mut state = eq::EqState::default();
    for index in 0..nodes {
        let id = eq::NodeId::new((index + 1) as u64)?;
        let mut node = eq::Node::new(
            id,
            center_frequency(index, nodes),
            if index % 2 == 0 { 0.75 } else { -0.75 },
            0,
        )?;
        node.q = 0.9;
        state.add(node)?;
        if automated {
            state.set_lane(eq::AutomationLane {
                node: id,
                parameter: eq::NodeParameter::Frequency,
                points: vec![
                    eq::AutomationPoint {
                        beat: 0,
                        normalized: 0.08,
                    },
                    eq::AutomationPoint {
                        beat: AUTOMATION_END_BEAT,
                        normalized: 0.92,
                    },
                ],
            })?;
        }
    }
    Ok(state)
}

fn scenarios() -> Result<Vec<Scenario>> {
    let legacy_state = legacy_eight_bells()?;
    let legacy = eq8::EQ8_DEFINITION
        .prepare_processor(
            &legacy_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: RATE,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )?
        .ok_or("legacy EQ8 did not prepare a processor")?;
    let static_eight_state = new_eq(8, false)?.encode()?;
    let static_eight = eq::EQ_DEFINITION
        .prepare_processor(
            &static_eight_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: RATE,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )?
        .ok_or("8-band EQ did not prepare a processor")?;
    let static_max_state = new_eq(64, false)?.encode()?;
    let static_max = eq::EQ_DEFINITION
        .prepare_processor(
            &static_max_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: RATE,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )?
        .ok_or("64-band static EQ did not prepare a processor")?;
    let automated_state = new_eq(64, true)?.encode()?;
    let automated_max = eq::EQ_DEFINITION
        .prepare_processor(
            &automated_state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: RATE,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )?
        .ok_or("64-band automated EQ did not prepare a processor")?;
    Ok(vec![
        Scenario {
            label: "legacy_eq8_8_bells_static",
            prepared: legacy,
            clocked: false,
            active_bands: 8,
            automation_lanes: 0,
            automation_points: 0,
        },
        Scenario {
            label: "eq_8_bells_static",
            prepared: static_eight,
            clocked: false,
            active_bands: 8,
            automation_lanes: 0,
            automation_points: 0,
        },
        Scenario {
            label: "eq_64_bands_static",
            prepared: static_max,
            clocked: false,
            active_bands: 64,
            automation_lanes: 0,
            automation_points: 0,
        },
        Scenario {
            label: "eq_64_bands_frequency_automation_128_points",
            prepared: automated_max,
            clocked: true,
            active_bands: 64,
            automation_lanes: 64,
            automation_points: 128,
        },
    ])
}

fn input_template(frames: usize, seed: u64) -> Vec<StereoFrame> {
    (0..frames)
        .map(|offset| {
            let t = (seed + offset as u64) as f32;
            [
                0.025 * (t * 0.013).sin() + 0.009 * (t * 0.031).cos(),
                0.021 * (t * 0.017).cos() - 0.008 * (t * 0.027).sin(),
            ]
        })
        .collect()
}

fn measured_input(block_frames: usize) -> Result<Vec<StereoFrame>> {
    let total_frames = block_frames
        .checked_mul(usize::try_from(MEASURED_BLOCKS)?)
        .ok_or("measured input frame count overflow")?;
    let mut workload = Vec::new();
    workload.try_reserve_exact(total_frames)?;
    for block in 0..MEASURED_BLOCKS {
        let start = block
            .checked_mul(block_frames as u64)
            .ok_or("measured input seed overflow")?;
        workload.extend(input_template(block_frames, start));
    }
    Ok(workload)
}

fn check_output(frames: &[StereoFrame]) -> Result<(f64, f32)> {
    let mut checksum = 0.0_f64;
    let mut peak = 0.0_f32;
    for frame in frames {
        for sample in frame {
            if !sample.is_finite() {
                return Err("EQ profile produced nonfinite output".into());
            }
            checksum += f64::from(*sample);
            peak = peak.max(sample.abs());
        }
    }
    Ok((checksum, peak))
}

fn profile(scenario: &Scenario, block_frames: usize) -> Result<()> {
    let template = input_template(block_frames, 0);
    let block_duration = block_frames as f64 / f64::from(RATE);
    let mut all_checksums = 0.0_f64;
    let mut means = Vec::with_capacity(TRIALS);
    let mut peak = 0.0_f32;

    for _trial in 0..TRIALS {
        // Runtime construction and all input preparation stay outside timing.
        let mut runtime = scenario.prepared.create_runtime();
        let mut frames = template.clone();
        let mut workload = measured_input(block_frames)?;
        let mut clock = Clock { frame: 0 };
        let mut frame_origin = 0_u64;
        for _ in 0..WARMUP_BLOCKS {
            frames.copy_from_slice(&template);
            clock.frame = frame_origin;
            if scenario.clocked {
                runtime.process_block_with_clock(
                    &mut frames,
                    Some(&clock),
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
            } else {
                runtime.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            }
            frame_origin = frame_origin
                .checked_add(block_frames as u64)
                .ok_or("warmup frame counter overflow")?;
        }
        let (warm_checksum, warm_peak) = check_output(&frames)?;
        all_checksums += warm_checksum;
        peak = peak.max(warm_peak);

        let start = Instant::now();
        for block in black_box(&mut workload).chunks_exact_mut(block_frames) {
            clock.frame = frame_origin;
            if scenario.clocked {
                runtime.process_block_with_clock(
                    block,
                    Some(black_box(&clock)),
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
            } else {
                runtime.process_block(block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            }
            black_box(block);
            frame_origin = frame_origin
                .checked_add(block_frames as u64)
                .ok_or("measured frame counter overflow")?;
        }
        let elapsed = start.elapsed();
        let (checksum, trial_peak) = check_output(&workload)?;
        all_checksums += checksum;
        peak = peak.max(trial_peak);
        means.push(elapsed.as_secs_f64() * 1e9 / MEASURED_BLOCKS as f64);
        if !all_checksums.is_finite() {
            return Err("EQ profile checksum became nonfinite".into());
        }
    }

    let mean_ns = means.iter().sum::<f64>() / means.len() as f64;
    let realtime_ratio = mean_ns * 1e-9 / block_duration;
    println!(
        "profile={} active_bands={} automation_lanes={} automation_points={} automation_end_beat={} sample_rate={RATE} block_frames={block_frames} warmup_blocks={WARMUP_BLOCKS} measured_blocks_per_trial={MEASURED_BLOCKS} trials={TRIALS} clocked={} declared_runtime_bytes={} mean_ns_per_block={mean_ns:.1} realtime_ratio={realtime_ratio:.6} peak={peak:.6} checksum={all_checksums:.9}",
        scenario.label,
        scenario.active_bands,
        scenario.automation_lanes,
        scenario.automation_points,
        if scenario.clocked {
            AUTOMATION_END_BEAT >> 32
        } else {
            0
        },
        scenario.clocked,
        scenario.prepared.declared_runtime_bytes(),
    );
    Ok(())
}

fn legacy_pcm_difference(scenarios: &[Scenario]) -> Result<f32> {
    let legacy = scenarios
        .first()
        .ok_or("missing legacy scenario")?
        .prepared
        .create_runtime();
    let eq = scenarios
        .get(1)
        .ok_or("missing new EQ scenario")?
        .prepared
        .create_runtime();
    let mut legacy_frames = input_template(8192, 0);
    let mut eq_frames = legacy_frames.clone();
    let mut legacy = legacy;
    let mut eq = eq;
    legacy.process_block(&mut legacy_frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    eq.process_block(&mut eq_frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let _ = check_output(&legacy_frames)?;
    let _ = check_output(&eq_frames)?;
    Ok(legacy_frames
        .iter()
        .zip(eq_frames.iter())
        .flat_map(|(old, new)| old.iter().zip(new.iter()))
        .map(|(old, new)| (old - new).abs())
        .fold(0.0_f32, f32::max))
}

fn main() -> Result<()> {
    let scenarios = scenarios()?;
    println!(
        "eq release profile os={} arch={} rate={RATE} warmup_blocks={WARMUP_BLOCKS} measured_blocks_per_trial={MEASURED_BLOCKS} trials={TRIALS}; node_freqs=log-spaced-80..16000-hz node_q=0.9 node_gain=alternating-+/-0.75-db automation_normalized=0.08..0.92 automation_end_beat=128-at-120-bpm signal_l=0.025*sin(frame*0.013)+0.009*cos(frame*0.031) signal_r=0.021*cos(frame*0.017)-0.008*sin(frame*0.027); realtime_ratio=mean_block_time/block_duration; no physical deadline claim",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    println!(
        "legacy_eq8_vs_eq_8_bells_static max_abs_sample_difference={:.9}",
        legacy_pcm_difference(&scenarios)?
    );
    for block_frames in [128, 256] {
        for scenario in &scenarios {
            profile(scenario, block_frames)?;
        }
    }
    Ok(())
}
