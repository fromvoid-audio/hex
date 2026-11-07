//! Private math allocation probe only; not native callback integration evidence.

// Compile the single private precursor source in the existing approved probe
// harness. No public DSP API, production dependency or allocator policy change.
#[path = "../../../hex-devices/src/signed_envelope.rs"]
mod signed_envelope;

use allocation_counter::measure;
use signed_envelope::{Error, SignedEnvelope};
use std::hint::black_box;

#[test]
fn signed_envelope_prepare_sample_copy_and_errors_allocate_nothing() {
    let info = measure(|| {
        for _ in 0..32 {
            let envelope =
                SignedEnvelope::new([-1.0, 1.0, -0.25, 0.75], [0.005, 0.01, 0.02]).unwrap();
            let prepared = envelope.prepare(48_000).unwrap();
            let copy = black_box(prepared);
            for age in 0..2048 {
                black_box(copy.level(age, Some(123)));
            }
            assert_eq!(
                black_box(copy.release_end(u64::MAX)),
                Err(Error::FrameOverflow)
            );
            assert_eq!(black_box(envelope.prepare(0)), Err(Error::InvalidRate));
            assert_eq!(
                black_box(SignedEnvelope::new([f64::NAN; 4], [0.0; 3])),
                Err(Error::InvalidLevel)
            );
            assert_eq!(
                black_box(SignedEnvelope::new([0.0; 4], [0.0, 0.0, -1.0])),
                Err(Error::InvalidDuration)
            );
        }
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
}
