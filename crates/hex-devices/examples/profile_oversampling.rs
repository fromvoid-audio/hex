//! Release benchmark for oversampled native effects (8x saturator/overdrive,
//! 2x rattle, true-peak limiter, and 16-track stereo mix).
//!
//! Run before and after hot-path changes under matched conditions:
//! `cargo run -p hex-devices --release --locked --example profile_oversampling`
use hex_devices::{limiter, overdrive, rattle, saturator};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeDeviceDefinition,
    PreparedOwnedProcessorRuntime, StereoFrame, TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::hint::black_box;
use std::time::Instant;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_SIZE: usize = 256;
const WARMUP_BLOCKS: usize = 64;
const MEASURED_BLOCKS: usize = 2_000;

fn make_signal() -> [StereoFrame; BLOCK_SIZE] {
    std::array::from_fn(|i| {
        let t = i as f32 / SAMPLE_RATE as f32;
        let l = 0.6 * (std::f32::consts::TAU * 440.0 * t).sin()
            + 0.3 * (std::f32::consts::TAU * 1_760.0 * t).sin();
        let r = 0.55 * (std::f32::consts::TAU * 554.37 * t).sin()
            + 0.35 * (std::f32::consts::TAU * 2_217.46 * t).sin();
        [l, r]
    })
}

fn bench_single(
    name: &str,
    def: &dyn NativeDeviceDefinition,
    signal: &[StereoFrame; BLOCK_SIZE],
    state: Option<hex_lattice_contract::NativeDeviceState>,
) {
    let state = state.unwrap_or_else(|| def.default_state().unwrap());
    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: SAMPLE_RATE,
        resources: &EMPTY_DEVICE_RESOURCES,
        tuning: &tuning,
    };
    let prepared = def.prepare_processor(&state, &ctx).unwrap().unwrap();
    let mut runtime = prepared.create_runtime();
    let mut buf: [StereoFrame; BLOCK_SIZE];
    for _ in 0..WARMUP_BLOCKS {
        buf = *signal;
        runtime.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
    }
    let start = Instant::now();
    let mut sink = 0.0_f32;
    for _ in 0..MEASURED_BLOCKS {
        buf = *signal;
        runtime.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
        sink += buf[0][0] + buf[BLOCK_SIZE - 1][1];
    }
    let elapsed = start.elapsed();
    black_box(sink);
    let total_frames = MEASURED_BLOCKS * BLOCK_SIZE;
    let ns_per_frame = elapsed.as_nanos() as f64 / total_frames as f64;
    let rt_pct = (elapsed.as_secs_f64() / (total_frames as f64 / SAMPLE_RATE as f64)) * 100.0;
    println!(
        "{name:24} {:7.2} ms  {:7.1} ns/frame  {:5.2}% of realtime",
        elapsed.as_secs_f64() * 1e3,
        ns_per_frame,
        rt_pct
    );
}

fn bench_limiter_true_peak(signal: &[StereoFrame; BLOCK_SIZE]) {
    let state = limiter::DEFINITION
        .patch_parameter(
            &limiter::DEFINITION.default_state().unwrap(),
            limiter::parameter::MODE,
            ParameterValue::Choice(2),
        )
        .unwrap();
    let prepared = limiter::LimiterDefinition::prepare_owned(&state, SAMPLE_RATE, &[]).unwrap();
    let mut runtime = prepared.create_runtime().unwrap();
    let mut buf: [StereoFrame; BLOCK_SIZE];
    for _ in 0..WARMUP_BLOCKS {
        buf = *signal;
        runtime.begin_block();
        runtime.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
    }
    let start = Instant::now();
    let mut sink = 0.0_f32;
    for _ in 0..MEASURED_BLOCKS {
        buf = *signal;
        runtime.begin_block();
        runtime.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        sink += buf[0][0] + buf[BLOCK_SIZE - 1][1];
    }
    let elapsed = start.elapsed();
    black_box(sink);
    let total_frames = MEASURED_BLOCKS * BLOCK_SIZE;
    let ns_per_frame = elapsed.as_nanos() as f64 / total_frames as f64;
    let rt_pct = (elapsed.as_secs_f64() / (total_frames as f64 / SAMPLE_RATE as f64)) * 100.0;
    println!(
        "{:24} {:7.2} ms  {:7.1} ns/frame  {:5.2}% of realtime",
        "limiter_true_peak",
        elapsed.as_secs_f64() * 1e3,
        ns_per_frame,
        rt_pct
    );
}

fn bench_multitrack(signal: &[StereoFrame; BLOCK_SIZE]) {
    let tuning = TuningTable::default();
    let ctx = DevicePreparationContext {
        sample_rate: SAMPLE_RATE,
        resources: &EMPTY_DEVICE_RESOURCES,
        tuning: &tuning,
    };
    let sat_drive = saturator::SATURATOR_DEFINITION
        .patch_parameter(
            &saturator::SATURATOR_DEFINITION.default_state().unwrap(),
            saturator::parameter::DRIVE,
            ParameterValue::Float(18.0),
        )
        .unwrap();
    let defs: [(
        &dyn NativeDeviceDefinition,
        Option<hex_lattice_contract::NativeDeviceState>,
    ); 3] = [
        (&saturator::SATURATOR_DEFINITION, Some(sat_drive)),
        (&overdrive::OVERDRIVE_DEFINITION, None),
        (&rattle::RATTLE_DEFINITION, None),
    ];
    let mut runtimes: Vec<_> = (0..16)
        .map(|track| {
            let (def, ref st) = defs[track % defs.len()];
            let state = st.clone().unwrap_or_else(|| def.default_state().unwrap());
            def.prepare_processor(&state, &ctx)
                .unwrap()
                .unwrap()
                .create_runtime()
        })
        .collect();
    let mut buf: [StereoFrame; BLOCK_SIZE];
    for _ in 0..16 {
        for rt in &mut runtimes {
            buf = *signal;
            rt.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
        }
    }
    let blocks = 500;
    let start = Instant::now();
    let mut sink = 0.0_f32;
    for _ in 0..blocks {
        for rt in &mut runtimes {
            buf = *signal;
            rt.process_block(&mut buf, &EMPTY_DEVICE_RESOURCES);
            sink += buf[0][0];
        }
    }
    let elapsed = start.elapsed();
    black_box(sink);
    let total_frames = blocks * BLOCK_SIZE;
    let ns_per_frame = elapsed.as_nanos() as f64 / total_frames as f64;
    let rt_pct = (elapsed.as_secs_f64() / (total_frames as f64 / SAMPLE_RATE as f64)) * 100.0;
    println!(
        "{:24} {:7.2} ms  {:7.1} ns/frame  {:5.2}% of realtime (16 tracks)",
        "multitrack_16",
        elapsed.as_secs_f64() * 1e3,
        ns_per_frame,
        rt_pct
    );
}

fn main() {
    let signal = make_signal();
    println!(
        "profile_oversampling: rate={SAMPLE_RATE} Hz, block={BLOCK_SIZE}, warmup={WARMUP_BLOCKS}, measured={MEASURED_BLOCKS}"
    );
    let sat_drive = saturator::SATURATOR_DEFINITION
        .patch_parameter(
            &saturator::SATURATOR_DEFINITION.default_state().unwrap(),
            saturator::parameter::DRIVE,
            ParameterValue::Float(18.0),
        )
        .unwrap();
    let sat_digital = saturator::SATURATOR_DEFINITION
        .patch_parameter(
            &sat_drive,
            saturator::parameter::CURVE,
            ParameterValue::Choice(1),
        )
        .unwrap();
    bench_single(
        "saturator_soft_8x",
        &saturator::SATURATOR_DEFINITION,
        &signal,
        Some(sat_drive),
    );
    bench_single(
        "saturator_digital_8x",
        &saturator::SATURATOR_DEFINITION,
        &signal,
        Some(sat_digital),
    );
    bench_single(
        "overdrive_8x",
        &overdrive::OVERDRIVE_DEFINITION,
        &signal,
        None,
    );
    bench_single("rattle_2x", &rattle::RATTLE_DEFINITION, &signal, None);
    bench_limiter_true_peak(&signal);
    bench_multitrack(&signal);
}
