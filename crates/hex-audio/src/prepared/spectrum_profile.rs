//! Opt-in optimized capture-stage measurement, not native deadline evidence.
use super::*;
use std::hint::black_box;

#[test]
#[ignore = "manual optimized spectrum capture profile; not a device deadline test"]
fn spectrum_capture_stage_profile() -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("use an optimized profile");
    }
    for count in [1, 4, spectrum_target::MAX_SPECTRUM_TARGETS] {
        let (mut control, _callback) = spectrum_target::channel();
        let requests: Vec<_> = (1..=count as u64)
            .map(|id| spectrum_target::SpectrumTargetRequest {
                identity: PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Track(1),
                    id,
                    hex_engine::DeviceTypeId::from_static("hex.eq8"),
                ),
                project_generation: 1,
                activation_floor: 1,
                sample_rate_hz: 48_000,
            })
            .collect();
        control.set_targets(&requests).unwrap();
        let targets = control.desired_targets();
        let input = [[0.25, -0.4]; 256];
        let output = [[0.125, -0.2]; 256];
        let mut stages = SpectrumStages::new().unwrap();
        let mut quantum = || {
            stages.begin(Some(black_box(targets)), 1, 256, |_| true);
            for (_, target) in targets.iter() {
                stages.observe_signal(
                    Some(targets),
                    black_box(target.identity),
                    hex_engine::NodeSignalPhase::Input,
                    0,
                    black_box(&input),
                );
                stages.observe_signal(
                    Some(targets),
                    black_box(target.identity),
                    hex_engine::NodeSignalPhase::Output,
                    0,
                    black_box(&output),
                );
            }
            assert!(stages.iter().all(|(_, stage)| stage.complete()));
            for (_, stage) in stages.iter() {
                black_box(&stage.input_frames);
                black_box(&stage.frames);
            }
            stages.clear();
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
            "spectrum capture profile: targets={count} rate=48000 quantum=256 warmup=1000 rounds=20 iterations=10000 min_ns={} median_ns={} p95_ns={}; capture staging only, no DSP/FFT/OS device",
            times[0], times[10], times[18]
        );
    }
    Ok(())
}
