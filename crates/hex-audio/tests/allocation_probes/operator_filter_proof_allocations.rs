//! Standalone private kernel probe using the existing allowed test-only counter.
//! No audio callback, provider, publication or native deadline coverage.
#[path = "../../../hex-devices/src/operator_filter_lowpass.rs"]
mod filter;
use filter::{Error, Lowpass, PreparedLowpass};

#[test]
fn standalone_process_copy_reset_and_refusal_have_zero_total_and_net_allocations() {
    let prepared = PreparedLowpass::prepare(1000.0, 48_000).unwrap();
    assert!(prepared.coefficients().iter().all(|x| x.is_finite()));
    let mut source = Lowpass::new(prepared);
    let mut copy = Lowpass::new(prepared);
    let mut incompatible = Lowpass::new(PreparedLowpass::prepare(1000.0, 44_100).unwrap());
    let mut overloaded = Lowpass::new(PreparedLowpass::prepare(12_000.0, 48_000).unwrap());
    for _ in 0..2 {
        overloaded.process_frame([0.0, f32::MAX]).unwrap();
    }
    let counts = allocation_counter::measure(|| {
        for _ in 0..128 {
            for frame in [[0.1, -0.1], [0.0; 2]] {
                source.process_frame(frame).unwrap();
            }
            copy.copy_history_from(&source).unwrap();
            assert_eq!(
                incompatible.copy_history_from(&source),
                Err(Error::IncompatibleHistory)
            );
            assert_eq!(
                source.process_frame([0.0, f32::NAN]),
                Err(Error::InvalidSample)
            );
            assert_eq!(
                overloaded.process_frame([0.0, f32::MAX]),
                Err(Error::OutputOverflow)
            );
            copy.reset();
            std::hint::black_box((source, copy, incompatible, overloaded));
        }
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.bytes_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(counts.bytes_current, 0);
}
