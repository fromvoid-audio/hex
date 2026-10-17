//! Private original math only; no Fine mapping, envelope application or runtime.
mod support;

#[path = "../src/pitch_multiplier.rs"]
mod pitch;
use hex_devices::operator_state::{OPERATOR_METADATA, parameter};
use hex_lattice::NativeInstrumentDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use pitch::{Error, Multiplier};

#[test]
fn hand_octaves_fractional_oracles_and_inverse_ratios_match_existing_math() {
    for (semitones, ratio) in [
        (-48.0, 0.0625),
        (-24.0, 0.25),
        (-12.0, 0.5),
        (0.0, 1.0),
        (12.0, 2.0),
        (24.0, 4.0),
        (48.0, 16.0),
    ] {
        let multiplier = Multiplier::prepare(semitones).unwrap();
        assert_eq!(multiplier.ratio(), ratio);
        assert_eq!(
            multiplier.frequency(440.0, 192_000).unwrap().hz,
            440.0 * ratio
        );
    }
    for (semitones, ratio) in [
        (1.0, 1.0594630943592953),
        (4.0, 1.2599210498948732),
        (6.0, std::f64::consts::SQRT_2),
        (7.0, 1.4983070768766815),
    ] {
        let forward = Multiplier::prepare(semitones).unwrap().ratio();
        let backward = Multiplier::prepare(-semitones).unwrap().ratio();
        assert!((forward - ratio).abs() < 5e-16);
        assert!((forward * backward - 1.0).abs() < 5e-16);
        // Independent analytic identity, distinct from calling the same powf.
        let exponential = (std::f64::consts::LN_2 * semitones / 12.0).exp();
        assert!((forward - exponential).abs() < 5e-16);
    }
    assert!((Multiplier::prepare(1.0).unwrap().ratio().powi(12) - 2.0).abs() < 3e-15);
}

#[test]
fn neutral_preserves_bits_and_phase_matches_actual_registered_note_preparation() {
    let mut state = support::legacy_operator_state();
    for op in 1..=4 {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                parameter::oscillator_ratio(op),
                ParameterValue::Float(1.0),
            )
            .unwrap();
    }
    for rate in [8_000, 11_025, 44_100, 48_000, 192_000] {
        let instrument = OPERATOR_METADATA.prepare_instrument(&state, rate).unwrap();
        let context = instrument
            .prepare_shared_analytic_context()
            .unwrap()
            .unwrap();
        for semitones in [-48, -24, -12, 0, 12] {
            let shifted = Multiplier::prepare(f64::from(semitones))
                .unwrap()
                .frequency(440.0, rate)
                .unwrap();
            let note = context.prepare_note((69 + semitones) as u8, 100).unwrap();
            assert_eq!(&note.payload()[..4], &[shifted.phase_step; 4]);
        }
        for hz in [0.1, 123.456789, 440.0, f64::from(rate) * 0.49] {
            for zero in [0.0, -0.0] {
                assert_eq!(
                    Multiplier::prepare(zero)
                        .unwrap()
                        .frequency(hz, rate)
                        .unwrap()
                        .hz
                        .to_bits(),
                    hz.to_bits()
                );
            }
        }
    }
}

#[test]
fn finite_overflow_underflow_rate_and_nyquist_boundaries_refuse_without_clamping() {
    for semitones in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(Multiplier::prepare(semitones), Err(Error::InvalidSemitones));
    }
    for semitones in [f64::MAX, -f64::MAX, 12_288.0, -12_900.0] {
        assert_eq!(
            Multiplier::prepare(semitones),
            Err(Error::UnrepresentableMultiplier)
        );
    }
    let neutral = Multiplier::prepare(0.0).unwrap();
    assert_eq!(
        Multiplier::prepare(12.0 * 1023.0)
            .unwrap()
            .ratio()
            .to_bits(),
        0x7fe0_0000_0000_0000
    );
    assert_eq!(
        Multiplier::prepare(-12.0 * 1074.0)
            .unwrap()
            .ratio()
            .to_bits(),
        1
    );
    for hz in [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(neutral.frequency(hz, 48_000), Err(Error::InvalidFrequency));
    }
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert_eq!(neutral.frequency(440.0, rate), Err(Error::InvalidRate));
    }
    assert_eq!(
        Multiplier::prepare(12.0)
            .unwrap()
            .frequency(f64::MAX, 48_000),
        Err(Error::UnrepresentableFrequency)
    );
    assert_eq!(
        Multiplier::prepare(-12.0)
            .unwrap()
            .frequency(f64::from_bits(1), 48_000),
        Err(Error::UnrepresentableFrequency)
    );
    for rate in [8_000, 48_000, 192_000] {
        let nyquist = f64::from(rate) * 0.5;
        assert_eq!(neutral.frequency(nyquist, rate), Err(Error::Nyquist));
        assert_eq!(
            neutral.frequency(f64::from_bits(nyquist.to_bits() + 1), rate),
            Err(Error::Nyquist)
        );
        let below = neutral
            .frequency(f64::from_bits(nyquist.to_bits() - 1), rate)
            .unwrap();
        assert!(below.phase_step < 1_u64 << 63);
        let half_step_hz = f64::from(rate) * 2.0_f64.powi(-65);
        assert_eq!(neutral.frequency(half_step_hz, rate).unwrap().phase_step, 1);
        assert_eq!(
            neutral
                .frequency(half_step_hz * 3.0, rate)
                .unwrap()
                .phase_step,
            2
        );
        assert_eq!(
            neutral.frequency(half_step_hz * 0.25, rate),
            Err(Error::UnrepresentablePhase)
        );
        assert_eq!(
            Multiplier::prepare(12.0)
                .unwrap()
                .frequency(nyquist * 0.75, rate),
            Err(Error::Nyquist)
        );
        // This mathematical helper validates final frequency; it chooses no
        // product rule for a fixed-mode base that was already above Nyquist.
        assert_eq!(
            Multiplier::prepare(-24.0)
                .unwrap()
                .frequency(nyquist * 2.0, rate)
                .unwrap()
                .hz,
            nyquist * 0.5
        );
    }
}

#[test]
fn failed_control_value_replacement_preserves_old_value_and_input_bits() {
    let neutral = Multiplier::prepare(0.0).unwrap();
    let mut destination = neutral.frequency(440.0, 48_000).unwrap();
    let original = destination;
    for (multiplier, base, rate) in [
        (neutral, f64::NAN, 48_000),
        (neutral, 440.0, 0),
        (Multiplier::prepare(12.0).unwrap(), 20_000.0, 48_000),
        (
            Multiplier::prepare(-12.0).unwrap(),
            f64::from_bits(1),
            48_000,
        ),
    ] {
        let before = base.to_bits();
        assert!(
            multiplier
                .prepare_into(base, rate, &mut destination)
                .is_err()
        );
        assert_eq!(destination, original);
        assert_eq!(base.to_bits(), before);
    }
    Multiplier::prepare(-12.0)
        .unwrap()
        .prepare_into(440.0, 48_000, &mut destination)
        .unwrap();
    assert_eq!(destination.hz, 220.0);
    assert_ne!(destination, original);
    assert_eq!(std::mem::size_of::<Multiplier>(), 8);
    assert_eq!(std::mem::size_of::<pitch::Frequency>(), 16);
}
