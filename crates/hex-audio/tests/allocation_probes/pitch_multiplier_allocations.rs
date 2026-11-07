//! Standalone control-math probe, not native callback or pitch admission.
#[path = "../../../hex-devices/src/pitch_multiplier.rs"]
mod pitch;
use pitch::{Error, Multiplier};

#[test]
fn multiplier_prepare_frequency_copy_and_refusal_have_zero_total_and_net_allocations() {
    let count = allocation_counter::measure(|| {
        for _ in 0..128 {
            let neutral = Multiplier::prepare(0.0).unwrap();
            assert_eq!(neutral.ratio(), 1.0);
            let mut destination = neutral.frequency(440.0, 48_000).unwrap();
            for semitones in [-48.0, -12.0, 0.0, 6.0, 12.0, 48.0] {
                let multiplier = Multiplier::prepare(semitones).unwrap();
                multiplier
                    .prepare_into(440.0, 48_000, &mut destination)
                    .unwrap();
                std::hint::black_box(destination);
            }
            let before = destination;
            assert_eq!(
                neutral.prepare_into(24_000.0, 48_000, &mut destination),
                Err(Error::Nyquist)
            );
            assert_eq!(destination, before);
            assert_eq!(Multiplier::prepare(f64::NAN), Err(Error::InvalidSemitones));
            assert_eq!(
                Multiplier::prepare(f64::MAX),
                Err(Error::UnrepresentableMultiplier)
            );
            assert_eq!(neutral.frequency(0.0, 48_000), Err(Error::InvalidFrequency));
            assert_eq!(neutral.frequency(440.0, 0), Err(Error::InvalidRate));
            assert_eq!(
                neutral.frequency(f64::from_bits(1), 48_000),
                Err(Error::UnrepresentablePhase)
            );
            assert_eq!(
                Multiplier::prepare(12.0)
                    .unwrap()
                    .frequency(f64::MAX, 48_000),
                Err(Error::UnrepresentableFrequency)
            );
        }
    });
    assert_eq!(
        (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
