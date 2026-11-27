#![deny(unsafe_code)]

use hex_device_abi_qual::{
    ContainedError, ContainedRunner, InProcessModule, StaticSine, StaticUtility, compile_c_fixture,
};
use hex_lattice_contract::package_abi::{
    AbiNoteEvent, AbiParamEvent, AbiStatus, CAP_AUDIO_EFFECT, CAP_INSTRUMENT, CAP_ZERO_LATENCY,
    NOTE_EVENT_OFF, NOTE_EVENT_ON,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hex-abi-qual-{tag}-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn runner_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hex-device-runner"))
}

fn build_fixture(tmp: &TempDir, name: &str, source_file: &str, defines: &[&str]) -> PathBuf {
    let root = crate_root();
    compile_c_fixture(
        &root.join("fixtures").join(source_file),
        &root.join("include"),
        &tmp.path().join(format!("lib{name}.dylib")),
        defines,
    )
    .expect("compile detached C99 fixture")
}

fn max_abs_diff(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .flat_map(|(x, y)| [(x[0] - y[0]).abs(), (x[1] - y[1]).abs()])
        .fold(0.0_f32, f32::max)
}

#[test]
fn detached_utility_and_sine_match_independent_pcm_oracles_in_both_boundaries() {
    let tmp = TempDir::new("oracles");
    let utility_dylib = build_fixture(&tmp, "utility_v1", "utility_v1.c", &[]);
    let sine_dylib = build_fixture(&tmp, "sine_v1", "sine_v1.c", &[]);

    // 1. Utility descriptor, parameter metadata, state round-trip, and PCM oracle.
    let util_mod = InProcessModule::load(&utility_dylib).expect("load utility");
    let util_desc = util_mod.descriptor();
    let (util_id, util_versions) = util_desc.validate().expect("valid utility descriptor");
    assert_eq!(util_id.as_str().unwrap(), "hex.pkg.utility");
    assert_eq!(
        (
            util_versions.package_major,
            util_versions.state_version,
            util_versions.abi_version,
            util_versions.ui_version
        ),
        (1, 1, 1, 1)
    );
    assert_ne!(util_desc.capabilities & CAP_AUDIO_EFFECT, 0);
    assert_ne!(util_desc.capabilities & CAP_ZERO_LATENCY, 0);
    assert_eq!(util_desc.reported_latency_frames, 0);
    assert_eq!(util_desc.parameter_count, 5);
    assert_eq!(
        util_mod
            .parameter_descriptor(0)
            .expect("gain param")
            .validate()
            .unwrap(),
        "gain"
    );

    let mut util_inproc = util_mod.create_instance(48_000).expect("create utility");
    let mut state_buf = [0u8; 64];
    let state_len = util_inproc.get_state(&mut state_buf).expect("get state");
    assert_eq!(state_len, 20);
    util_inproc
        .set_state(&state_buf[..state_len])
        .expect("restore valid state");
    assert_eq!(
        util_inproc.set_state(&[0u8; 3]),
        Err(AbiStatus::InvalidState)
    );

    let mut util_runner =
        ContainedRunner::spawn(&runner_bin(), &utility_dylib).expect("spawn utility runner");
    assert_eq!(
        util_runner
            .describe(Duration::from_secs(2))
            .expect("describe"),
        "hex.pkg.utility"
    );
    let util_ipc_id = util_runner
        .create(48_000, Duration::from_secs(2))
        .expect("create ipc utility");

    let frames = 128usize;
    let mut input = vec![[0.0f32; 2]; frames];
    for (i, sample) in input.iter_mut().enumerate() {
        let t = i as f32 / 128.0;
        *sample = [(t * 6.0).sin() * 0.5, (t * 9.0).cos() * 0.4];
    }
    let param_events = [
        AbiParamEvent {
            sample_offset: 0,
            parameter_id: 1,
            value: 1.5,
            reserved: 0,
        },
        AbiParamEvent {
            sample_offset: 32,
            parameter_id: 2,
            value: -0.4,
            reserved: 0,
        },
        AbiParamEvent {
            sample_offset: 64,
            parameter_id: 3,
            value: 0.5,
            reserved: 0,
        },
        AbiParamEvent {
            sample_offset: 96,
            parameter_id: 4,
            value: 1.0,
            reserved: 0,
        },
        AbiParamEvent {
            sample_offset: 112,
            parameter_id: 5,
            value: 1.0,
            reserved: 0,
        },
    ];
    let mut out_oracle = vec![[0.0f32; 2]; frames];
    let mut out_inproc = vec![[0.0f32; 2]; frames];
    let mut out_ipc = vec![[0.0f32; 2]; frames];

    let mut oracle_util = StaticUtility::default();
    oracle_util.process(&input, &mut out_oracle, &param_events);
    util_inproc
        .process(&input, &mut out_inproc, &param_events, &[])
        .expect("inproc utility process");
    util_runner
        .process(
            util_ipc_id,
            48_000,
            &input,
            &mut out_ipc,
            &param_events,
            &[],
            Duration::from_secs(2),
        )
        .expect("ipc utility process");

    assert!(max_abs_diff(&out_oracle, &out_inproc) < 1e-6);
    assert!(max_abs_diff(&out_oracle, &out_ipc) < 1e-6);
    assert_eq!(out_inproc[112], [0.0, 0.0]);
    util_runner
        .destroy(util_ipc_id, Duration::from_secs(2))
        .expect("destroy ipc utility");
    util_runner
        .shutdown_bounded(Duration::from_secs(2))
        .expect("shutdown ipc utility");

    // 2. Sine instrument descriptor and sample-accurate note-offset PCM oracle.
    let sine_mod = InProcessModule::load(&sine_dylib).expect("load sine");
    let sine_desc = sine_mod.descriptor();
    let (sine_id, _) = sine_desc.validate().expect("valid sine descriptor");
    assert_eq!(sine_id.as_str().unwrap(), "hex.pkg.sine");
    assert_ne!(sine_desc.capabilities & CAP_INSTRUMENT, 0);

    let mut sine_inproc = sine_mod.create_instance(48_000).expect("create sine");
    let mut sine_runner =
        ContainedRunner::spawn(&runner_bin(), &sine_dylib).expect("spawn sine runner");
    let sine_ipc_id = sine_runner
        .create(48_000, Duration::from_secs(2))
        .expect("create ipc sine");

    let silence = vec![[0.0f32; 2]; frames];
    let sine_params = [AbiParamEvent {
        sample_offset: 0,
        parameter_id: 1,
        value: 0.75,
        reserved: 0,
    }];
    let note_events = [
        AbiNoteEvent {
            sample_offset: 13,
            kind: NOTE_EVENT_ON,
            note: 69,
            channel: 0,
            reserved0: 0,
            velocity: 0.8,
            detune_cents: 0.0,
            reserved1: 0,
        },
        AbiNoteEvent {
            sample_offset: 37,
            kind: NOTE_EVENT_ON,
            note: 76,
            channel: 0,
            reserved0: 0,
            velocity: 0.5,
            detune_cents: 12.5,
            reserved1: 0,
        },
        AbiNoteEvent {
            sample_offset: 100,
            kind: NOTE_EVENT_OFF,
            note: 69,
            channel: 0,
            reserved0: 0,
            velocity: 0.0,
            detune_cents: 0.0,
            reserved1: 0,
        },
    ];
    let mut sine_oracle = StaticSine::new(48_000);
    sine_oracle.process(&mut out_oracle, &sine_params, &note_events);
    sine_inproc
        .process(&silence, &mut out_inproc, &sine_params, &note_events)
        .expect("inproc sine process");
    sine_runner
        .process(
            sine_ipc_id,
            48_000,
            &silence,
            &mut out_ipc,
            &sine_params,
            &note_events,
            Duration::from_secs(2),
        )
        .expect("ipc sine process");

    assert_eq!(out_inproc[12], [0.0, 0.0]);
    assert!(out_inproc[14][0].abs() > 0.01);
    assert!(max_abs_diff(&out_oracle, &out_inproc) < 1e-6);
    assert!(max_abs_diff(&out_oracle, &out_ipc) < 1e-6);
    sine_runner
        .shutdown_bounded(Duration::from_secs(2))
        .expect("shutdown ipc sine");
}

#[test]
fn malformed_abi_and_invalid_output_are_rejected_without_corrupting_host() {
    let tmp = TempDir::new("malformed");
    let bad_magic = build_fixture(&tmp, "fault_magic", "faults_v1.c", &["FAULT_MODE=1"]);
    let bad_size = build_fixture(&tmp, "fault_size", "faults_v1.c", &["FAULT_MODE=2"]);
    let future_ver = build_fixture(&tmp, "fault_ver", "faults_v1.c", &["FAULT_MODE=3"]);
    let nan_out = build_fixture(&tmp, "fault_nan", "faults_v1.c", &["FAULT_MODE=4"]);

    assert_eq!(
        InProcessModule::load(&bad_magic).err(),
        Some(AbiStatus::InvalidArg)
    );
    assert_eq!(
        InProcessModule::load(&bad_size).err(),
        Some(AbiStatus::SizeMismatch)
    );
    assert_eq!(
        InProcessModule::load(&future_ver).err(),
        Some(AbiStatus::UnsupportedAbi)
    );

    let nan_mod = InProcessModule::load(&nan_out).expect("load nan fixture");
    let mut inst = nan_mod.create_instance(48_000).expect("create nan fixture");
    let input = [[0.25f32; 2]; 32];
    let mut output = [[1.0f32; 2]; 32];
    assert_eq!(
        inst.process(&input, &mut output, &[], &[]),
        Err(AbiStatus::InvalidOutput)
    );
    assert_eq!(output, [[0.0; 2]; 32]);

    let mut runner = ContainedRunner::spawn(&runner_bin(), &nan_out).expect("spawn nan runner");
    let id = runner
        .create(48_000, Duration::from_secs(2))
        .expect("create nan ipc");
    let mut ipc_out = [[1.0f32; 2]; 32];
    assert_eq!(
        runner.process(
            id,
            48_000,
            &input,
            &mut ipc_out,
            &[],
            &[],
            Duration::from_secs(2)
        ),
        Err(ContainedError::Abi(AbiStatus::InvalidOutput))
    );
    runner
        .shutdown_bounded(Duration::from_secs(2))
        .expect("shutdown nan runner");
}

#[test]
fn in_process_callback_allocates_zero_bytes_and_zero_net_count() {
    let tmp = TempDir::new("alloc");
    let utility_dylib = build_fixture(&tmp, "utility_alloc", "utility_v1.c", &[]);
    let sine_dylib = build_fixture(&tmp, "sine_alloc", "sine_v1.c", &[]);

    let util_mod = InProcessModule::load(&utility_dylib).expect("load utility");
    let sine_mod = InProcessModule::load(&sine_dylib).expect("load sine");
    let mut util = util_mod.create_instance(48_000).expect("create utility");
    let mut sine = sine_mod.create_instance(48_000).expect("create sine");

    let input = [[0.25f32, -0.25f32]; 64];
    let mut mid = [[0.0f32; 2]; 64];
    let mut output = [[0.0f32; 2]; 64];
    let note_on = [AbiNoteEvent {
        sample_offset: 0,
        kind: NOTE_EVENT_ON,
        note: 60,
        channel: 0,
        reserved0: 0,
        velocity: 0.7,
        detune_cents: 0.0,
        reserved1: 0,
    }];
    let param_ev = [AbiParamEvent {
        sample_offset: 16,
        parameter_id: 1,
        value: 0.9,
        reserved: 0,
    }];

    // Warm up once outside measured scope.
    sine.process(&input, &mut mid, &[], &note_on).unwrap();
    util.process(&mid, &mut output, &param_ev, &[]).unwrap();

    let measured = allocation_counter::measure(|| {
        for _ in 0..512 {
            sine.process(&input, &mut mid, &[], &[]).unwrap();
            util.process(&mid, &mut output, &param_ev, &[]).unwrap();
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn contained_runner_isolates_crashed_hung_and_teardown_stalled_modules() {
    let tmp = TempDir::new("faults");
    let crash_dylib = build_fixture(&tmp, "fault_crash", "faults_v1.c", &["FAULT_MODE=5"]);
    let hang_dylib = build_fixture(&tmp, "fault_hang", "faults_v1.c", &["FAULT_MODE=6"]);
    let teardown_dylib = build_fixture(&tmp, "fault_teardown", "faults_v1.c", &["FAULT_MODE=7"]);
    let input = [[0.1f32; 2]; 32];
    let mut output = [[0.0f32; 2]; 32];

    // 1. Crashed module (abort) contained without crashing host.
    let mut crash_runner =
        ContainedRunner::spawn(&runner_bin(), &crash_dylib).expect("spawn crash runner");
    let crash_id = crash_runner
        .create(48_000, Duration::from_secs(2))
        .expect("create crash instance");
    assert_eq!(
        crash_runner.process(
            crash_id,
            48_000,
            &input,
            &mut output,
            &[],
            &[],
            Duration::from_millis(200)
        ),
        Err(ContainedError::Crashed)
    );

    // 2. Hung process call timed out and killed cleanly.
    let mut hang_runner =
        ContainedRunner::spawn(&runner_bin(), &hang_dylib).expect("spawn hang runner");
    let hang_id = hang_runner
        .create(48_000, Duration::from_secs(2))
        .expect("create hang instance");
    let start = Instant::now();
    assert_eq!(
        hang_runner.process(
            hang_id,
            48_000,
            &input,
            &mut output,
            &[],
            &[],
            Duration::from_millis(80)
        ),
        Err(ContainedError::Timeout)
    );
    assert!(start.elapsed() < Duration::from_secs(2));

    // 3. Hung teardown/destroy terminated cleanly within bounded timeout.
    let mut teardown_runner =
        ContainedRunner::spawn(&runner_bin(), &teardown_dylib).expect("spawn teardown runner");
    let td_id = teardown_runner
        .create(48_000, Duration::from_secs(2))
        .expect("create teardown instance");
    teardown_runner
        .process(
            td_id,
            48_000,
            &input,
            &mut output,
            &[],
            &[],
            Duration::from_millis(200),
        )
        .expect("teardown fixture processes cleanly before destroy");
    let td_start = Instant::now();
    assert_eq!(
        teardown_runner.shutdown_bounded(Duration::from_millis(80)),
        Err(ContainedError::Timeout)
    );
    assert!(td_start.elapsed() < Duration::from_secs(2));
}

#[test]
#[ignore = "release measurement matrix for #2990; run with --release --ignored --nocapture"]
fn measure_boundary_comparison_matrix_and_write_raw_json() {
    let tmp = TempDir::new("bench");
    let utility_dylib = build_fixture(&tmp, "utility_bench", "utility_v1.c", &[]);
    let sine_dylib = build_fixture(&tmp, "sine_bench", "sine_v1.c", &[]);

    // Control preparation costs
    let t0 = Instant::now();
    let util_mod = InProcessModule::load(&utility_dylib).unwrap();
    let inproc_load_ns = t0.elapsed().as_nanos() as u64;

    let t1 = Instant::now();
    let _inst = util_mod.create_instance(48_000).unwrap();
    let inproc_create_ns = t1.elapsed().as_nanos() as u64;

    let t2 = Instant::now();
    let mut ipc_runner = ContainedRunner::spawn(&runner_bin(), &utility_dylib).unwrap();
    let _ = ipc_runner.describe(Duration::from_secs(2)).unwrap();
    let ipc_spawn_describe_ns = t2.elapsed().as_nanos() as u64;

    let t3 = Instant::now();
    let ipc_id = ipc_runner.create(48_000, Duration::from_secs(2)).unwrap();
    let ipc_create_ns = t3.elapsed().as_nanos() as u64;
    ipc_runner.destroy(ipc_id, Duration::from_secs(2)).unwrap();

    let sine_mod = InProcessModule::load(&sine_dylib).unwrap();
    let mut rows = Vec::new();

    for &rate in &[44_100u32, 48_000, 96_000] {
        for &frames in &[32usize, 64, 128, 512] {
            let quantum_ns = ((frames as f64) * 1_000_000_000.0 / (rate as f64)) as u64;
            let input = vec![[0.25f32, -0.25f32]; frames];
            let mut out_a = vec![[0.0f32; 2]; frames];
            let mut out_b = vec![[0.0f32; 2]; frames];

            for &instances in &[1usize, 32, 100] {
                let iters = if instances == 100 { 200 } else { 500 };

                // Static Utility baseline
                let mut static_utils = vec![StaticUtility::default(); instances];
                for u in &mut static_utils {
                    u.process(&input, &mut out_a, &[]);
                }
                let s0 = Instant::now();
                for _ in 0..iters {
                    for u in &mut static_utils {
                        u.process(&input, &mut out_a, &[]);
                    }
                }
                let static_ns = (s0.elapsed().as_nanos() / iters as u128) as u64;

                // In-process external C ABI Utility
                let mut inproc_utils: Vec<_> = (0..instances)
                    .map(|_| util_mod.create_instance(rate).unwrap())
                    .collect();
                for u in &mut inproc_utils {
                    u.process(&input, &mut out_b, &[], &[]).unwrap();
                }
                let p0 = Instant::now();
                for _ in 0..iters {
                    for u in &mut inproc_utils {
                        u.process(&input, &mut out_b, &[], &[]).unwrap();
                    }
                }
                let inproc_ns = (p0.elapsed().as_nanos() / iters as u128) as u64;

                // Contained-runner IPC (1/32/100 instances sequential exchanges over one runner)
                let ipc_iters = if instances == 1 { 100 } else { 10 };
                let ipc_ids: Vec<u32> = (0..instances)
                    .map(|_| ipc_runner.create(rate, Duration::from_secs(2)).unwrap())
                    .collect();
                let c0 = Instant::now();
                for _ in 0..ipc_iters {
                    for &id in &ipc_ids {
                        ipc_runner
                            .process(
                                id,
                                rate,
                                &input,
                                &mut out_b,
                                &[],
                                &[],
                                Duration::from_secs(2),
                            )
                            .unwrap();
                    }
                }
                let ipc_ns = (c0.elapsed().as_nanos() / ipc_iters as u128) as u64;
                for &id in &ipc_ids {
                    ipc_runner.destroy(id, Duration::from_secs(2)).unwrap();
                }

                rows.push(format!(
                    "    {{\"kind\":\"utility_batch\",\"sample_rate\":{rate},\"frames\":{frames},\"instances\":{instances},\"quantum_ns\":{quantum_ns},\"static_ns\":{static_ns},\"in_process_c_abi_ns\":{inproc_ns},\"contained_ipc_ns\":{ipc_ns}}}"
                ));
            }

            // 8-device sequential chain (1 Sine -> 7 Utilities)
            let chain_iters = 200u128;
            let mut static_sine = StaticSine::new(rate);
            let note_on = [AbiNoteEvent {
                sample_offset: 0,
                kind: NOTE_EVENT_ON,
                note: 69,
                channel: 0,
                reserved0: 0,
                velocity: 0.7,
                detune_cents: 0.0,
                reserved1: 0,
            }];
            static_sine.process(&mut out_a, &[], &note_on);
            let mut static_chain = vec![StaticUtility::default(); 7];
            let s0 = Instant::now();
            for _ in 0..chain_iters {
                static_sine.process(&mut out_a, &[], &[]);
                for u in &mut static_chain {
                    u.process(&out_a, &mut out_b, &[]);
                    std::mem::swap(&mut out_a, &mut out_b);
                }
            }
            let static_chain_ns = (s0.elapsed().as_nanos() / chain_iters) as u64;

            let mut inproc_sine = sine_mod.create_instance(rate).unwrap();
            inproc_sine
                .process(&input, &mut out_a, &[], &note_on)
                .unwrap();
            let mut inproc_chain: Vec<_> = (0..7)
                .map(|_| util_mod.create_instance(rate).unwrap())
                .collect();
            let p0 = Instant::now();
            for _ in 0..chain_iters {
                inproc_sine.process(&input, &mut out_a, &[], &[]).unwrap();
                for u in &mut inproc_chain {
                    u.process(&out_a, &mut out_b, &[], &[]).unwrap();
                    std::mem::swap(&mut out_a, &mut out_b);
                }
            }
            let inproc_chain_ns = (p0.elapsed().as_nanos() / chain_iters) as u64;

            let mut sine_runner = ContainedRunner::spawn(&runner_bin(), &sine_dylib).unwrap();
            let ipc_sine_id = sine_runner.create(rate, Duration::from_secs(2)).unwrap();
            sine_runner
                .process(
                    ipc_sine_id,
                    rate,
                    &input,
                    &mut out_a,
                    &[],
                    &note_on,
                    Duration::from_secs(2),
                )
                .unwrap();
            let ipc_util_ids: Vec<u32> = (0..7)
                .map(|_| ipc_runner.create(rate, Duration::from_secs(2)).unwrap())
                .collect();
            let ipc_chain_iters = 20u128;
            let c0 = Instant::now();
            for _ in 0..ipc_chain_iters {
                sine_runner
                    .process(
                        ipc_sine_id,
                        rate,
                        &input,
                        &mut out_a,
                        &[],
                        &[],
                        Duration::from_secs(2),
                    )
                    .unwrap();
                for &id in &ipc_util_ids {
                    ipc_runner
                        .process(
                            id,
                            rate,
                            &out_a,
                            &mut out_b,
                            &[],
                            &[],
                            Duration::from_secs(2),
                        )
                        .unwrap();
                    std::mem::swap(&mut out_a, &mut out_b);
                }
            }
            let ipc_chain_ns = (c0.elapsed().as_nanos() / ipc_chain_iters) as u64;
            for &id in &ipc_util_ids {
                ipc_runner.destroy(id, Duration::from_secs(2)).unwrap();
            }
            sine_runner
                .shutdown_bounded(Duration::from_secs(2))
                .unwrap();

            rows.push(format!(
                "    {{\"kind\":\"sequential_chain_8\",\"sample_rate\":{rate},\"frames\":{frames},\"instances\":8,\"quantum_ns\":{quantum_ns},\"static_ns\":{static_chain_ns},\"in_process_c_abi_ns\":{inproc_chain_ns},\"contained_ipc_ns\":{ipc_chain_ns}}}"
            ));
        }
    }

    ipc_runner.shutdown_bounded(Duration::from_secs(2)).unwrap();

    let json = format!(
        "{{\n  \"ticket\": 2990,\n  \"target\": \"arm64-apple-darwin\",\n  \"profile\": \"release\",\n  \"declared_added_latency_frames\": {{\"static\": 0, \"in_process_c_abi\": 0, \"contained_ipc_blocking_sync\": 0, \"contained_ipc_rt_bridge_required\": \"2*quantum\"}},\n  \"callback_allocations_in_process_c_abi\": {{\"count_total\": 0, \"bytes_total\": 0, \"count_net\": 0, \"bytes_net\": 0}},\n  \"control_preparation_ns\": {{\"in_process_dlopen_describe\": {inproc_load_ns}, \"in_process_create\": {inproc_create_ns}, \"contained_ipc_spawn_describe\": {ipc_spawn_describe_ns}, \"contained_ipc_create\": {ipc_create_ns}}},\n  \"measurements\": [\n{}\n  ]\n}}\n",
        rows.join(",\n")
    );
    println!("{json}");
    if let Ok(path) = std::env::var("HEX_WRITE_QUAL_JSON") {
        fs::write(&path, &json).expect("write qualification json");
    }
}
