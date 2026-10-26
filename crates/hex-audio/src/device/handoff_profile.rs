//! Optimized conversion/handoff microbenchmark, not rendering/device deadlines.
use super::*;

#[test]
#[ignore = "optimized output-handoff profile; run explicitly, not a timing correctness gate"]
fn optimized_host_handoff_profile() -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("run this profile with --release");
    }
    let (_control, mut callback) = prepared::channel(48_000).unwrap();
    let mut source = [[0.25, -0.5]; 256];
    let mut host = [0.0_f32; 512];
    let capture = Instant::now();
    let call = |callback: &mut prepared::Callback, source: &mut [Frame], host: &mut [f32]| {
        assert!(finish_host_chunk(
            std::hint::black_box(callback),
            std::hint::black_box(host),
            std::hint::black_box(source),
            2,
            &Ok(()),
            0,
            capture
        ));
    };
    for _ in 0..1000 {
        call(&mut callback, &mut source, &mut host);
    }
    let mut samples = [0.0_f64; 7];
    for sample in &mut samples {
        let started = Instant::now();
        for _ in 0..50_000 {
            call(&mut callback, &mut source, &mut host);
        }
        *sample = started.elapsed().as_nanos() as f64 / 50_000.0;
        std::hint::black_box(&host);
    }
    assert_eq!(host[..2], [0.25, -0.5]);
    samples.sort_by(f64::total_cmp);
    println!(
        "host_handoff_profile={{\"frames\":256,\"channels\":2,\"format\":\"f32\",\"graph\":\"empty callback, conversion/handoff only\",\"warmup\":1000,\"iterations_per_trial\":50000,\"trials\":7,\"median_ns_per_chunk\":{},\"min_ns_per_chunk\":{},\"max_ns_per_chunk\":{}}}",
        samples[3], samples[0], samples[6]
    );
    Ok(())
}
