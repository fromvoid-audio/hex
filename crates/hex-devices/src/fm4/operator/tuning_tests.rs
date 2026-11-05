use super::*;

#[test]
fn review_ratio_mixed_fixed_and_velocity_paths_refuse_zero_phase_quantization() {
    let table = hex_lattice::TuningTable::new([1.0e-25; 128]).unwrap();
    let fixed = OperatorTuning::fixed(125.0, 1.0).unwrap();
    let mut admitted = Vec::new();
    for velocity_amount in [0.0, -1.0] {
        for mixed in [false, true] {
            let tuning = if mixed {
                [OperatorTuning::Ratio, fixed, OperatorTuning::Ratio, fixed]
            } else {
                [OperatorTuning::Ratio; 4]
            };
            let context = configuration(tuning, [OperatorWaveform::Sine; 4])
                .with_note_modifiers([0.0; 4], [velocity_amount; 4], [false; 4])
                .unwrap();
            if context
                .prepare_note_with_tuning(0, 127, Some(&table))
                .is_ok()
            {
                admitted.push((mixed, velocity_amount));
            }
        }
        let fixed_only = configuration([fixed; 4], [OperatorWaveform::Sine; 4])
            .with_note_modifiers([0.0; 4], [velocity_amount; 4], [false; 4])
            .unwrap();
        assert!(
            fixed_only
                .prepare_note_with_tuning(0, 127, Some(&table))
                .is_ok()
        );
    }
    assert!(
        admitted.is_empty(),
        "zero-step ratio oscillators admitted: {admitted:?}"
    );
    let context = configuration([OperatorTuning::Ratio; 4], [OperatorWaveform::Sine; 4]);
    let quantum = 48_000.0 / 2.0_f64.powi(64);
    let first = hex_lattice::TuningTable::new([quantum * 0.5; 128]).unwrap();
    assert!(
        context
            .prepare_note_with_tuning(0, 127, Some(&first))
            .is_ok()
    );
    let nyquist = hex_lattice::TuningTable::new([24_000.0; 128]).unwrap();
    assert!(
        context
            .prepare_note_with_tuning(0, 127, Some(&nyquist))
            .is_ok()
    );
}

fn configuration(
    tuning: [OperatorTuning; 4],
    shapes: [OperatorWaveform; 4],
) -> PreparedOperatorConfiguration {
    PreparedOperatorConfiguration::new_with_tuning(
        48_000,
        [Oscillator::new(1.0, 1.0, 0.0).expect("oscillator"); 4],
        Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).expect("routing"),
        [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.0, 0.0).expect("envelope"); 4],
        shapes,
        tuning,
    )
    .expect("configuration")
}

#[test]
fn fixed_tone_ignores_note_pitch_and_matches_independent_phase_increment() {
    let fixed = OperatorTuning::fixed(125.0, 2.0).expect("fixed");
    let context = configuration([fixed; 4], [OperatorWaveform::Sine; 4]);
    let table = SineTable::new();
    let step = (250.0 / 48_000.0 * crate::PHASE_SCALE).round() as u64;
    for pitch in [0, 36, 69, 127] {
        let note = context.prepare_note(pitch, 127).expect("fixed note");
        for age in (0_u64..4096).chain([u64::MAX / 2, u64::MAX - 1]) {
            assert_eq!(
                context.sample(&table, &note, age, u64::MAX).to_bits(),
                table.sample_phase(step.wrapping_mul(age)).to_bits()
            );
        }
    }
}

#[test]
fn mixed_ratio_and_fixed_notes_match_standalone_and_original_frequency_math() {
    let tuning = [
        OperatorTuning::Ratio,
        OperatorTuning::fixed(200.0, 2.0).expect("fixed"),
        OperatorTuning::Ratio,
        OperatorTuning::Ratio,
    ];
    let oscillators = [Oscillator::new(1.0, 1.0, 0.0).expect("oscillator"); 4];
    let mut matrix = [[0.0; 4]; 4];
    matrix[1][0] = 0.25;
    let routing = Routing::new(matrix, [1.0, 0.0, 0.0, 0.0]).expect("routing");
    let envelopes = [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.0, 0.0).expect("envelope"); 4];
    let context = PreparedOperatorConfiguration::new_with_tuning(
        48_000,
        oscillators,
        routing,
        envelopes,
        [OperatorWaveform::Sine; 4],
        tuning,
    )
    .expect("configuration");
    let table = SineTable::new();
    for pitch in [57, 69, 81] {
        let note = context.prepare_note(pitch, 127).expect("note");
        let standalone = PreparedOperatorFm4::new_with_tuning(
            48_000,
            pitch,
            127,
            oscillators,
            routing,
            envelopes,
            [OperatorWaveform::Sine; 4],
            tuning,
        )
        .expect("standalone");
        let carrier_hz = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
        for age in (0_u64..2048).rev() {
            let actual = context.sample(&table, &note, age, 2048);
            assert_eq!(
                actual.to_bits(),
                standalone.sample(&table, age, 2048).to_bits()
            );
            let time = age as f64 / 48_000.0;
            let expected = (std::f64::consts::TAU
                * (carrier_hz * time + 0.25 * (std::f64::consts::TAU * 400.0 * time).sin()))
            .sin();
            assert!(
                (f64::from(actual) - expected).abs() < 0.002,
                "pitch{pitch} frame{age}"
            );
        }
    }
}

#[test]
fn fixed_shapes_keep_pitch_independent_caps_and_defensive_validation() {
    for shape in [
        OperatorWaveform::Saw,
        OperatorWaveform::Square,
        OperatorWaveform::Triangle,
    ] {
        let context = configuration(
            [OperatorTuning::fixed(100.0, 1.0).expect("fixed"); 4],
            [shape; 4],
        );
        let low = context.prepare_note(0, 127).expect("low");
        let high = context.prepare_note(127, 127).expect("high");
        let table = SineTable::new();
        for age in 0..1024 {
            assert_eq!(
                context.sample(&table, &low, age, 1024).to_bits(),
                context.sample(&table, &high, age, 1024).to_bits()
            );
        }
        assert!(context.prepare_note(128, 127).is_err());
        assert!(context.prepare_note(69, 0).is_err());
    }
    for (hz, multi) in [
        (f64::NAN, 1.0),
        (100.0, f64::INFINITY),
        (0.0, 1.0),
        (100.0, 0.0),
        (20_001.0, 1.0),
    ] {
        assert!(OperatorTuning::fixed(hz, multi).is_err());
    }
    let invalid = OperatorTuning::Fixed {
        frequency_hz: f64::NAN,
        multiplier: 1.0,
    };
    assert!(
        PreparedOperatorConfiguration::new_with_tuning(
            48_000,
            [Oscillator::new(1.0, 1.0, 0.0).expect("osc"); 4],
            Routing::new([[0.0; 4]; 4], [1.0; 4]).expect("route"),
            [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.0, 0.0).expect("envelope"); 4],
            [OperatorWaveform::Sine; 4],
            [invalid; 4]
        )
        .is_err()
    );
    let nyquist = configuration(
        [OperatorTuning::fixed(12_000.0, 2.0).expect("valid domain"); 4],
        [OperatorWaveform::Sine; 4],
    );
    assert!(nyquist.prepare_note(0, 127).is_ok());
    let below = configuration(
        [OperatorTuning::fixed(11_999.0, 2.0).expect("below"); 4],
        [OperatorWaveform::Sine; 4],
    );
    assert!(below.prepare_note(127, 127).is_ok());
}

/// Pulse #3957: a modulator at or above nyquist is silenced for that note only.
/// The note renders exactly like the same patch with that modulator muted.
#[test]
fn nyquist_modulator_is_silenced_for_that_note_only() {
    let mut modulation = [[0.0; 4]; 4];
    modulation[3][0] = 1.0;
    let routing = Routing::new(modulation, [1.0, 0.0, 0.0, 0.0]).expect("routing");
    let envelopes = [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.0, 0.0).expect("envelope"); 4];
    let oscillators = |modulator_level| {
        let mut oscillators = [Oscillator::new(1.0, 1.0, 0.0).expect("oscillator"); 4];
        oscillators[3] = Oscillator::new(16.0, modulator_level, 0.0).expect("modulator");
        oscillators
    };
    let table = SineTable::new();
    for shapes in [[OperatorWaveform::Sine; 4], [OperatorWaveform::Square; 4]] {
        let prepare = |modulator_level| {
            PreparedOperatorConfiguration::new_with_waveforms(
                44_100,
                oscillators(modulator_level),
                routing,
                envelopes,
                shapes,
            )
            .expect("configuration")
        };
        let (patch, muted) = (prepare(1.0), prepare(0.0));
        let render = |context: &PreparedOperatorConfiguration, pitch| {
            let note = context.prepare_note(pitch, 100).expect("note admitted");
            (0..64)
                .map(|age| context.sample(&table, &note, age, u64::MAX))
                .collect::<Vec<_>>()
        };
        let high = render(&patch, 120);
        assert!(high.iter().any(|sample| *sample != 0.0));
        assert_eq!(high, render(&muted, 120), "{shapes:?}");
        assert_ne!(render(&patch, 60), render(&muted, 60), "{shapes:?}");
        // The silenced note stays valid across the shared-context payload.
        let note = patch.prepare_note(120, 100).expect("note admitted");
        assert!(patch.note_matches_configuration(&note));
        let decoded = PreparedOperatorNote::from_shared_payload(&note.into_shared_payload());
        assert_eq!(decoded, Some(note));
        let lfo = patch.prepare_lfo_note(120, 100).expect("lfo note admitted");
        assert_eq!(
            PreparedOperatorConfiguration::lfo_steps(&lfo)[3],
            super::tuning::SILENCED_STEP
        );
    }
}
