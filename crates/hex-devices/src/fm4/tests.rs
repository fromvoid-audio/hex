use super::*;
use std::f64::consts::TAU;

fn oscillator(ratio: f64, level: f64, phase: f64) -> Oscillator {
    Oscillator::new(ratio, level, phase).unwrap()
}

#[test]
fn fractional_phase_matches_euclidean_wrap_at_feedback_and_float_boundaries() {
    let expected = |turns: f64| (turns.rem_euclid(1.0) * PHASE_SCALE).round() as u64;
    for numerator in -8192..=8192 {
        let turns = f64::from(numerator) / 257.0;
        assert_eq!(phase_from_turns(turns), expected(turns), "{turns}");
    }
    for boundary in [
        0.0_f64,
        f64::MIN_POSITIVE,
        f64::EPSILON,
        1.0,
        32.0,
        2.0_f64.powi(52),
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
    ] {
        for turns in [boundary.next_down(), boundary, boundary.next_up()] {
            for signed in [turns, -turns] {
                assert_eq!(phase_from_turns(signed), expected(signed), "{signed}");
            }
        }
    }
}

#[test]
fn packed_analytic_note_matches_standalone_kernel_through_release() {
    let table = SineTable::new();
    let oscillators = [
        oscillator(1.0, 0.7, 0.125),
        oscillator(2.0, 0.45, 0.0),
        oscillator(0.5, 0.3, 0.25),
        oscillator(3.0, 0.2, 0.5),
    ];
    let mut modulation = [[0.0; 4]; 4];
    modulation[3][1] = 0.17;
    modulation[1][0] = -0.08;
    modulation[0][2] = 0.04;
    let routing = Routing::new(modulation, [0.8, 0.0, 0.25, 0.0]).unwrap();
    let prepared = PreparedFm4::new(
        48_000,
        57,
        93,
        oscillators,
        routing,
        Envelope::new(0.003, 0.02, 0.63, 0.04).unwrap(),
    )
    .unwrap();
    let analytic = prepared.into_analytic_note();
    assert_eq!(analytic.release_frames(), prepared.release_frames());
    assert_eq!(std::mem::size_of_val(&analytic), 352);
    // Include zero/early gates during attack and decay, as well as sustain.
    for gate in [0, 1, 72, 500, 2_400] {
        for age in [
            0,
            1,
            143,
            144,
            1_103,
            gate,
            gate + 1,
            gate + prepared.release_frames() - 1,
            gate + prepared.release_frames(),
        ] {
            let expected = prepared.sample(&table, age, gate);
            assert_eq!(
                analytic.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                [expected, expected],
                "age={age}, gate={gate}"
            );
        }
    }
}

fn sustained() -> Envelope {
    Envelope::new(0.0, 0.0, 1.0, 0.01).unwrap()
}

fn note(oscillators: [Oscillator; 4], matrix: [[f64; 4]; 4], carriers: [f64; 4]) -> PreparedFm4 {
    PreparedFm4::new(
        48_000,
        69,
        127,
        oscillators,
        Routing::new(matrix, carriers).unwrap(),
        sustained(),
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn previous_full_evaluation(
    table: &SineTable,
    sample_rate: u32,
    pitch: u8,
    velocity: u8,
    oscillators: [Oscillator; 4],
    routing: Routing,
    envelope: Envelope,
    age: u64,
    gate: u64,
) -> f32 {
    let base_frequency = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
    let mut steps = [0_u64; 4];
    let mut phases = [0_u64; 4];
    for (index, oscillator) in oscillators.iter().enumerate() {
        let frequency = base_frequency * oscillator.ratio;
        steps[index] = (frequency / f64::from(sample_rate) * PHASE_SCALE).round() as u64;
        phases[index] = phase_from_turns(oscillator.phase_turns);
    }
    let mut signals = [0.0; 4];
    for &target in &routing.order {
        let modulation: f64 = signals
            .iter()
            .enumerate()
            .map(|(source, signal)| signal * routing.modulation[source][target])
            .sum();
        let phase = steps[target]
            .wrapping_mul(age)
            .wrapping_add(phases[target])
            .wrapping_add(phase_from_turns(modulation));
        signals[target] = table.sample_phase(phase) as f64 * oscillators[target].level;
    }
    let output: f64 = signals
        .iter()
        .zip(routing.carriers)
        .map(|(signal, weight)| signal * weight)
        .sum();
    let frames = |seconds: f64| (seconds * f64::from(sample_rate)).round() as u64;
    let amplitude = (PreparedEnvelope {
        attack: frames(envelope.attack_seconds),
        decay: frames(envelope.decay_seconds),
        sustain: envelope.sustain,
        release: frames(envelope.release_seconds),
    })
    .level(age, gate)
        * (f64::from(velocity) / 127.0);
    if amplitude == 0.0 {
        return 0.0;
    }
    (output * amplitude) as f32
}

fn assert_matches_previous_full_evaluation(
    oscillators: [Oscillator; 4],
    matrix: [[f64; 4]; 4],
    carriers: [f64; 4],
    expected_audible: [bool; 4],
) {
    let sample_rate = 48_000;
    let pitch = 61;
    let envelope = Envelope::new(0.002, 0.03, 0.41, 0.025).unwrap();
    let routing = Routing::new(matrix, carriers).unwrap();
    let table = SineTable::new();
    for velocity in [1, 91, 127] {
        let prepared =
            PreparedFm4::new(sample_rate, pitch, velocity, oscillators, routing, envelope).unwrap();
        assert_eq!(
            prepared.core.configuration.levels.map(|level| level != 0.0),
            expected_audible
        );
        let packed = prepared.into_analytic_note();
        for gate in [0, 1, 37, 713, 2_147] {
            for age in [0, 1, 37, 96, 712, 713, 714, 1_000, 2_147, 3_347] {
                let previous = previous_full_evaluation(
                    &table,
                    sample_rate,
                    pitch,
                    velocity,
                    oscillators,
                    routing,
                    envelope,
                    age,
                    gate,
                );
                assert_eq!(
                    prepared.sample(&table, age, gate).to_bits(),
                    previous.to_bits()
                );
                assert_eq!(
                    packed.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)[0].to_bits(),
                    previous.to_bits()
                );
                assert_eq!(
                    packed.sample(age, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)[1].to_bits(),
                    previous.to_bits()
                );
            }
        }
    }
}

#[test]
fn skips_only_non_audible_operator_ancestors_and_preserves_full_evaluation_bits() {
    let oscillators = [
        oscillator(1.0, 0.7, 0.13),
        oscillator(2.0, 0.4, 0.21),
        oscillator(3.0, 0.6, 0.07),
        oscillator(4.0, 0.8, 0.19),
    ];

    // A single carrier leaves three unconnected oscillators inaudible.
    assert_matches_previous_full_evaluation(
        oscillators,
        [[0.0; 4]; 4],
        [1.0, 0.0, 0.0, 0.0],
        [true, false, false, false],
    );

    // A serial chain retains every ancestor of its sole carrier.
    let mut serial = [[0.0; 4]; 4];
    serial[0][1] = 0.11;
    serial[1][2] = -0.19;
    serial[2][3] = 0.23;
    assert_matches_previous_full_evaluation(oscillators, serial, [0.0, 0.0, 0.0, 1.0], [true; 4]);

    // Multiple carriers retain each audible branch and preserve signed mixing.
    assert_matches_previous_full_evaluation(
        oscillators,
        [[0.0; 4]; 4],
        [0.75, -0.5, 0.25, -0.0],
        [true, true, true, false],
    );

    // Both incoming branches of carrier 4 stay active; disconnected op 3 is skipped.
    let mut branch = [[0.0; 4]; 4];
    branch[0][3] = 0.31;
    branch[1][3] = -0.17;
    branch[2][1] = -0.0;
    assert_matches_previous_full_evaluation(
        oscillators,
        branch,
        [0.0, 0.0, 0.0, 0.8],
        [true, true, false, true],
    );

    // A zero-level middle modulator cuts off its otherwise nonzero parent.
    // Exercise both signed zeros while the serial path and carrier are active.
    for middle_level in [0.0, -0.0] {
        let mut silent_middle = oscillators;
        silent_middle[1].level = middle_level;
        let mut serial_to_carrier = [[0.0; 4]; 4];
        serial_to_carrier[0][1] = 0.37;
        serial_to_carrier[1][3] = -0.29;
        assert_matches_previous_full_evaluation(
            silent_middle,
            serial_to_carrier,
            [0.0, 0.0, 0.0, 0.8],
            [false, false, false, true],
        );
    }

    // Signed-zero carrier weights do not create an audible seed.
    assert_matches_previous_full_evaluation(
        oscillators,
        [[0.0; 4]; 4],
        [-0.0, 0.0, -0.0, 0.0],
        [false; 4],
    );
}

#[test]
fn unreachable_oscillators_still_undergo_nyquist_validation() {
    let oscillators = [
        oscillator(1.0, 1.0, 0.0),
        oscillator(2.0, 1.0, 0.0),
        oscillator(3.0, 1.0, 0.0),
        oscillator(128.0, 1.0, 0.0),
    ];
    let routing = Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).unwrap();
    assert!(PreparedFm4::new(48_000, 69, 127, oscillators, routing, sustained()).is_err());
}

#[test]
fn each_carrier_has_independent_frequency_level_and_phase() {
    let table = SineTable::new();
    for carrier in 0..4 {
        let oscillators =
            std::array::from_fn(|i| oscillator((i + 1) as f64, 0.25 * (i + 1) as f64, 0.125));
        let mut carriers = [0.0; 4];
        carriers[carrier] = 1.0;
        let voice = note(oscillators, [[0.0; 4]; 4], carriers);
        let mut crossings = 0;
        let mut previous = voice.sample(&table, 0, 48_001);
        for age in 1..=48_000 {
            let actual = voice.sample(&table, age, 48_001);
            let expected = (TAU * (440.0 * (carrier + 1) as f64 * age as f64 / 48_000.0 + 0.125))
                .sin()
                * 0.25
                * (carrier + 1) as f64;
            assert!((f64::from(actual) - expected).abs() < 0.000_002);
            crossings += usize::from(previous <= 0.0 && actual > 0.0);
            previous = actual;
        }
        assert_eq!(crossings, 440 * (carrier + 1));
    }
}

#[test]
fn additive_output_and_two_stage_modulation_match_independent_formulas() {
    let table = SineTable::new();
    let operators = [
        oscillator(1.0, 0.6, 0.0),
        oscillator(2.0, 0.35, 0.0),
        oscillator(3.0, 0.0, 0.0),
        oscillator(4.0, 0.0, 0.0),
    ];
    let additive = note(operators, [[0.0; 4]; 4], [1.0, -0.5, 0.0, 0.0]);
    let mut matrix = [[0.0; 4]; 4];
    matrix[1][0] = 0.2;
    let fm = note(operators, matrix, [1.0, 0.0, 0.0, 0.0]);
    let mut difference = 0.0;
    for age in 0..2048 {
        let phase = TAU * 440.0 * age as f64 / 48_000.0;
        let modulator = 0.35 * (2.0 * phase).sin();
        let expected = 0.6 * (phase + TAU * 0.2 * modulator).sin();
        assert!((f64::from(fm.sample(&table, age, 4096)) - expected).abs() < 0.000_004);
        assert!(
            (f64::from(additive.sample(&table, age, 4096)) - (0.6 * phase.sin() - 0.5 * modulator))
                .abs()
                < 0.000_002
        );
        difference += (f64::from(fm.sample(&table, age, 4096)) - 0.6 * phase.sin()).abs();
    }
    assert!(
        difference > 20.0,
        "modulator must change the carrier, not mix alongside it"
    );
}

#[test]
fn arbitrary_operator_order_and_four_stage_graph_are_resolved_before_sampling() {
    let table = SineTable::new();
    let operators = [oscillator(1.0, 0.5, 0.0); 4];
    let mut matrix = [[0.0; 4]; 4];
    matrix[0][3] = 0.1;
    matrix[3][1] = -0.2;
    matrix[1][2] = 0.3;
    let voice = note(operators, matrix, [0.0, 0.0, 1.0, 0.0]);
    for age in 0..1024 {
        let phase = TAU * 440.0 * age as f64 / 48_000.0;
        let a = 0.5 * phase.sin();
        let d = 0.5 * (phase + TAU * 0.1 * a).sin();
        let b = 0.5 * (phase - TAU * 0.2 * d).sin();
        let c = 0.5 * (phase + TAU * 0.3 * b).sin();
        assert!((f64::from(voice.sample(&table, age, 2048)) - c).abs() < 0.000_005);
    }
}

#[test]
fn cycles_and_non_finite_or_unbounded_routing_are_refused() {
    for vertex in 0..4 {
        let mut matrix = [[0.0; 4]; 4];
        matrix[vertex][vertex] = 0.1;
        assert!(Routing::new(matrix, [1.0; 4]).is_err());
    }
    let mut cycle = [[0.0; 4]; 4];
    cycle[0][1] = 1.0;
    cycle[1][2] = 1.0;
    cycle[2][0] = 1.0;
    assert!(Routing::new(cycle, [0.0; 4]).is_err());
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 8.01, -8.01] {
        let mut matrix = [[0.0; 4]; 4];
        matrix[1][0] = bad;
        assert!(Routing::new(matrix, [0.0; 4]).is_err());
    }
    for bad in [f64::NAN, f64::INFINITY, 1.01, -1.01] {
        assert!(Routing::new([[0.0; 4]; 4], [bad, 0.0, 0.0, 0.0]).is_err());
    }
}

fn envelope_voice(envelope: Envelope) -> PreparedFm4 {
    PreparedFm4::new(
        8000,
        69,
        127,
        [oscillator(100.0 / 440.0, 1.0, 0.25); 4],
        Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).unwrap(),
        envelope,
    )
    .unwrap()
}

fn assert_envelope(voice: &PreparedFm4, gate: u64, expected: &[f64]) {
    let table = SineTable::new();
    for (age, &level) in expected.iter().enumerate() {
        let tone = (TAU * 100.0 * age as f64 / 8000.0).cos();
        assert!(
            (f64::from(voice.sample(&table, age as u64, gate)) - level * tone).abs() < 0.000_002,
            "age {age}"
        );
    }
}

#[test]
fn adsr_sample_boundaries_and_early_note_off_are_continuous() {
    let voice =
        envelope_voice(Envelope::new(4.0 / 8000.0, 2.0 / 8000.0, 0.5, 4.0 / 8000.0).unwrap());
    assert_eq!(voice.release_frames(), 4);
    assert_envelope(
        &voice,
        8,
        &[
            0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.5, 0.5, 0.375, 0.25, 0.125, 0.0, 0.0,
        ],
    );
    assert_envelope(&voice, 2, &[0.0, 0.25, 0.5, 0.375, 0.25, 0.125, 0.0, 0.0]);
    assert_envelope(
        &voice,
        5,
        &[0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5625, 0.375, 0.1875, 0.0],
    );
}

#[test]
fn zero_duration_stages_and_empty_gate_have_defined_silence() {
    let voice = envelope_voice(Envelope::new(0.0, 0.0, 0.5, 0.0).unwrap());
    assert_envelope(&voice, 2, &[0.5, 0.5, 0.0, 0.0]);
    assert_envelope(&voice, 0, &[0.0; 5]);
    let silent = envelope_voice(Envelope::new(0.0, 0.0, 0.0, 0.5).unwrap());
    assert_envelope(&silent, 3, &[0.0; 8]);
}

#[test]
fn numerical_validation_covers_configuration_rate_pitch_and_velocity() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(Oscillator::new(bad, 1.0, 0.0).is_err());
        assert!(Oscillator::new(1.0, bad, 0.0).is_err());
        assert!(Oscillator::new(1.0, 1.0, bad).is_err());
        assert!(Envelope::new(bad, 0.0, 1.0, 0.0).is_err());
        assert!(Envelope::new(0.0, bad, 1.0, 0.0).is_err());
        assert!(Envelope::new(0.0, 0.0, bad, 0.0).is_err());
        assert!(Envelope::new(0.0, 0.0, 1.0, bad).is_err());
    }
    assert!(Oscillator::new(129.0, 1.0, 0.0).is_err());
    assert!(Oscillator::new(1.0, 8.01, 0.0).is_err());
    assert!(Oscillator::new(1.0, 1.0, 1.0).is_err());
    assert!(Envelope::new(120.01, 0.0, 1.0, 0.0).is_err());
    assert!(Envelope::new(0.0, 0.0, 1.01, 0.0).is_err());
    let graph = Routing::new([[0.0; 4]; 4], [1.0; 4]).unwrap();
    let operators = [oscillator(1.0, 1.0, 0.0); 4];
    for (rate, pitch, velocity) in [
        (0, 69, 127),
        (7999, 69, 127),
        (192001, 69, 127),
        (48000, 128, 127),
        (48000, 69, 0),
        (48000, 69, 128),
        (8000, 127, 127),
    ] {
        assert!(PreparedFm4::new(rate, pitch, velocity, operators, graph, sustained()).is_err());
    }
    assert!(
        PreparedFm4::new(
            48000,
            69,
            127,
            [oscillator(128.0, 1.0, 0.0); 4],
            graph,
            sustained()
        )
        .is_err()
    );
}

#[test]
fn partitions_seek_release_and_large_ages_preserve_deterministic_samples() {
    let table = SineTable::new();
    let mut matrix = [[0.0; 4]; 4];
    matrix[3][2] = 0.1;
    matrix[2][1] = 0.2;
    matrix[1][0] = 0.3;
    let voice = note([oscillator(1.0, 0.5, 0.1); 4], matrix, [1.0, 0.0, 0.0, 0.0]);
    let expected: Vec<_> = (0..1500)
        .map(|age| voice.sample(&table, age, 1000))
        .collect();
    for block in [1, 64, 128, 256, 513] {
        let mut actual = Vec::new();
        for start in (0..1500).step_by(block) {
            actual.extend(
                (start..(start + block).min(1500))
                    .map(|age| voice.sample(&table, age as u64, 1000)),
            );
        }
        assert_eq!(actual, expected);
    }
    assert_eq!(voice.sample(&table, 1117, 1000), expected[1117]);
    assert_eq!(voice.sample(&table, 1480, 1000), 0.0);
    assert!(voice.sample(&table, u64::MAX, u64::MAX - 1).is_finite());
    let quiet = PreparedFm4::new(
        48000,
        69,
        64,
        [oscillator(1.0, 0.5, 0.1); 4],
        Routing::new(matrix, [1.0, 0.0, 0.0, 0.0]).unwrap(),
        sustained(),
    )
    .unwrap();
    assert!(
        (quiet.sample(&table, 17, 1000) - voice.sample(&table, 17, 1000) * 64.0 / 127.0).abs()
            < 0.000_001
    );
}
