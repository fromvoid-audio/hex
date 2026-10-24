//! Opt-in optimized capture-stage measurement, not native deadline evidence.
use super::*;
use std::hint::black_box;

#[test]
#[ignore = "manual optimized spectrum capture profile; not a device deadline test"]
fn spectrum_capture_stage_profile() {
    assert!(!cfg!(debug_assertions), "use an optimized profile");
    let target = spectrum_target::SpectrumTarget {
        token: crate::spectrum_worker::SpectrumTargetId(1),
        identity: PostNodeIdentity::new(
            hex_engine::ProcessorChainOwner::Track(1),
            1,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        ),
        project_generation: 1,
        activation_floor: 1,
        sample_rate_hz: 48_000,
    };
    let input = [[0.25, -0.4]; 256];
    let output = [[0.125, -0.2]; 256];
    let mut stage = SpectrumStage::default();
    let mut quantum = || {
        stage.begin(black_box(target), 1, 256);
        stage.observe_signal(
            target.identity,
            hex_engine::NodeSignalPhase::Input,
            0,
            black_box(&input),
        );
        stage.observe_signal(
            target.identity,
            hex_engine::NodeSignalPhase::Output,
            0,
            black_box(&output),
        );
        assert!(stage.complete());
        black_box(&stage.input_frames);
        black_box(&stage.frames);
        stage.clear();
    };
    for _ in 0..1_000 {
        quantum();
    }
    let mut times = Vec::with_capacity(20);
    for _ in 0..20 {
        let start = Instant::now();
        for _ in 0..10_000 {
            quantum();
        }
        times.push(start.elapsed().as_nanos() as f64 / 10_000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "spectrum capture profile: targets=1 rate=48000 quantum=256 warmup=1000 rounds=20 iterations=10000 min_ns={} median_ns={} p95_ns={}; capture staging only, no DSP/FFT/OS device",
        times[0], times[10], times[18]
    );
}
