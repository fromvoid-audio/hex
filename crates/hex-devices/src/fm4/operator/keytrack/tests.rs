use super::*;
use crate::{
    fm4::{Oscillator, PreparedOperatorConfiguration, Routing, SineTable},
    operator_envelope::OperatorEnvelope,
};

fn configuration(
    rate: u32,
    carrier: usize,
    tuning: [OperatorTuning; 4],
) -> PreparedOperatorConfiguration {
    let oscillators = std::array::from_fn(|i| Oscillator::new(i as f64 + 1.0, 1.0, 0.0).unwrap());
    let mut carriers = [0.0; 4];
    carriers[carrier] = 1.0;
    PreparedOperatorConfiguration::new_with_tuning(
        rate,
        oscillators,
        Routing::new([[0.0; 4]; 4], carriers).unwrap(),
        [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.0, 0.0).unwrap(); 4],
        [crate::operator_waveform::OperatorWaveform::Sine; 4],
        tuning,
    )
    .unwrap()
}
fn anchors() -> [OperatorTuning; 4] {
    [OperatorTuning::fixed(125.0, 2.0).unwrap(); 4]
}
fn assert_tone(
    context: &PreparedOperatorConfiguration,
    rate: u32,
    pitch: u8,
    velocity: u8,
    hz: f64,
    tuning: Option<&hex_lattice::TuningTable>,
) {
    let note = context
        .prepare_note_with_tuning(pitch, velocity, tuning)
        .unwrap();
    let step = (hz / f64::from(rate) * crate::PHASE_SCALE).round() as u64;
    let table = SineTable::new();
    for age in 0..256 {
        let expected = (table.sample_phase(step.wrapping_mul(age)) as f64 * f64::from(velocity)
            / 127.0) as f32;
        let actual = context.sample(&table, &note, age, u64::MAX);
        // Independent frequency arithmetic may differ by a final f64 rounding;
        // the resulting sample error must remain below float output precision.
        assert!(
            (actual - expected).abs() < 1e-7,
            "age={age}: {actual} != {expected}"
        );
    }
}

#[test]
fn endpoints_retain_exact_legacy_notes_and_pcm_across_rates() {
    let table = SineTable::new();
    for rate in [8_000, 48_000, 192_000] {
        for (amount, tuning) in [(0.0, anchors()), (100.0, [OperatorTuning::Ratio; 4])] {
            let old = configuration(rate, 0, tuning);
            let new = old
                .clone()
                .with_pitch_keytrack([amount; 4], anchors())
                .unwrap();
            assert_eq!(old.configuration_id(), new.configuration_id());
            assert!(old.retarget_compatible_with(&new));
            for pitch in [0, 57, 69, 127] {
                let before = old.prepare_note(pitch, 127).unwrap();
                let after = new.prepare_note(pitch, 127).unwrap();
                assert_eq!(before, after);
                for age in 0..128 {
                    assert_eq!(
                        old.sample(&table, &before, age, u64::MAX).to_bits(),
                        new.sample(&table, &after, age, u64::MAX).to_bits()
                    );
                }
            }
        }
    }
}

#[test]
fn amounts_are_independent_and_half_tracking_halves_played_intervals() {
    let amounts = [0.0, 25.0, 50.0, 100.0];
    for (carrier, amount) in amounts.into_iter().enumerate() {
        let context = configuration(48_000, carrier, [OperatorTuning::Ratio; 4])
            .with_pitch_keytrack(amounts, anchors())
            .unwrap();
        for pitch in [57, 69, 81] {
            let note_hz = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
            let ratio = carrier as f64 + 1.0;
            let weight = f64::from(amount) / 100.0;
            let expected = 250.0_f64.powf(1.0 - weight) * (note_hz * ratio).powf(weight);
            assert_tone(&context, 48_000, pitch, 127, expected, None);
        }
    }
}

#[test]
fn mts_velocity_quantization_and_fine_hz_keep_their_tuning_order() {
    let tuning = hex_lattice::TuningTable::new([512.0; 128]).unwrap();
    let context = configuration(48_000, 2, [OperatorTuning::Ratio; 4])
        .with_pitch_keytrack([50.0; 4], anchors())
        .unwrap()
        .with_note_modifiers([0.0; 4], [0.25; 4], [true; 4])
        .unwrap()
        .with_fine_offsets([150.0; 4])
        .unwrap();
    let base = (125.0_f64 * 512.0).sqrt();
    let multiplier = ((2.0_f64 * 3.0).sqrt() * 2.0_f64.powf(64.0 / 127.0))
        .round()
        .max(1.0);
    assert_tone(
        &context,
        48_000,
        69,
        64,
        base * multiplier + 150.0,
        Some(&tuning),
    );
}

#[test]
fn invalid_amounts_and_nonfixed_or_malformed_anchors_are_rejected() {
    let context = configuration(48_000, 0, [OperatorTuning::Ratio; 4]);
    for index in 0..4 {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 100.01] {
            let mut amounts = [50.0; 4];
            amounts[index] = bad;
            assert!(
                context
                    .clone()
                    .with_pitch_keytrack(amounts, anchors())
                    .is_err()
            );
        }
        for bad in [
            OperatorTuning::Ratio,
            OperatorTuning::Fixed {
                frequency_hz: f64::NAN,
                multiplier: 1.0,
            },
            OperatorTuning::Fixed {
                frequency_hz: 100.0,
                multiplier: 0.0,
            },
        ] {
            let mut fixed = anchors();
            fixed[index] = bad;
            assert!(
                context
                    .clone()
                    .with_pitch_keytrack([50.0; 4], fixed)
                    .is_err()
            );
        }
    }
}

#[test]
fn changed_tracking_retires_configuration_identity_and_level_only_compatibility() {
    let original = configuration(48_000, 0, [OperatorTuning::Ratio; 4]);
    let changed = original
        .clone()
        .with_pitch_keytrack([50.0; 4], anchors())
        .unwrap();
    assert_ne!(original.configuration_id(), changed.configuration_id());
    assert!(!original.retarget_compatible_with(&changed));
    assert!(!original.identical_to(&changed));
    let same = changed
        .clone()
        .with_pitch_keytrack([50.0; 4], anchors())
        .unwrap();
    assert_eq!(changed.configuration_id(), same.configuration_id());
    assert!(changed.identical_to(&same));
    let reset = same.with_pitch_keytrack([100.0; 4], anchors()).unwrap();
    assert_ne!(changed.configuration_id(), reset.configuration_id());
    assert!(original.retarget_compatible_with(&reset));
}

#[test]
fn out_of_band_tracking_silences_only_the_affected_oscillator() {
    let mut fixed = anchors();
    fixed[0] = OperatorTuning::fixed(20_000.0, 1_000.0).unwrap();
    let table = SineTable::new();
    let silent = configuration(48_000, 0, [OperatorTuning::Ratio; 4])
        .with_pitch_keytrack([50.0; 4], fixed)
        .unwrap();
    let neighbor = configuration(48_000, 1, [OperatorTuning::Ratio; 4])
        .with_pitch_keytrack([50.0; 4], fixed)
        .unwrap();
    let note = silent.prepare_note(69, 127).unwrap();
    for age in 0..128 {
        assert_eq!(silent.sample(&table, &note, age, u64::MAX), 0.0);
    }
    let other = neighbor.prepare_note(69, 127).unwrap();
    assert_ne!(neighbor.sample(&table, &other, 17, u64::MAX), 0.0);
    let below = configuration(48_000, 0, [OperatorTuning::Ratio; 4])
        .with_pitch_keytrack([50.0; 4], anchors())
        .unwrap()
        .with_fine_offsets([-1_000.0, 0.0, 0.0, 0.0])
        .unwrap();
    let note = below.prepare_note(69, 127).unwrap();
    for age in 0..128 {
        assert_eq!(below.sample(&table, &note, age, u64::MAX), 0.0);
    }
}
