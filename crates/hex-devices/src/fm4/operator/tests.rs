use super::*;
use std::f64::consts::TAU;

fn config(levels: [f64; 3], times: [u64; 3]) -> OperatorEnvelope {
    OperatorEnvelope::new(
        levels[0],
        levels[1],
        levels[2],
        times[0] as f64 / 8000.0,
        times[1] as f64 / 8000.0,
        times[2] as f64 / 8000.0,
    )
    .expect("valid envelope")
}

fn amplitude(levels: [f64; 3], times: [u64; 3], age: u64, gate: u64) -> f64 {
    let [a, d, r] = times;
    let held = |age: u64| {
        if age < a {
            levels[0] * (1.0 - age as f64 / a as f64) + levels[1] * age as f64 / a as f64
        } else if age - a < d {
            levels[2] + (levels[1] - levels[2]) * 0.00001_f64.powf((age - a) as f64 / d as f64)
        } else {
            levels[2]
        }
    };
    if age < gate {
        held(age)
    } else if age - gate >= r {
        0.0
    } else {
        held(gate) * 0.00001_f64.powf((age - gate) as f64 / r as f64)
    }
}

#[test]
fn independent_envelopes_shape_carriers_and_modulators_before_routing() {
    let table = SineTable::new();
    let levels = [
        [0.2, 0.9, 0.4],
        [0.1, 0.7, 0.2],
        [0.5, 0.3, 0.6],
        [0.0, 1.0, 0.8],
    ];
    let times = [[11, 17, 19], [3, 13, 7], [0, 23, 31], [7, 0, 41]];
    let ratios = [1.0, 2.0, 0.5, 3.0];
    let gains = [0.7, 0.4, 0.8, 0.3];
    let phases = [0.125, 0.0, 0.25, 0.5];
    let oscillators = std::array::from_fn(|i| {
        Oscillator::new(ratios[i], gains[i], phases[i]).expect("valid oscillator")
    });
    let envelopes = std::array::from_fn(|i| config(levels[i], times[i]));
    for variant in 0..4 {
        let mut modulation = [[0.0; 4]; 4];
        let (order, carriers) = match variant {
            0 => ([0, 1, 2, 3], [0.6, 0.0, 0.0, 0.0]),
            1 => ([0, 1, 2, 3], [0.4, -0.2, 0.3, 0.1]),
            2 => {
                modulation[0][1] = 0.17;
                ([0, 1, 2, 3], [0.0, 0.7, 0.0, 0.0])
            }
            _ => {
                modulation[3][1] = 0.11;
                modulation[1][0] = -0.08;
                modulation[0][2] = 0.13;
                ([3, 1, 0, 2], [0.0, 0.0, 0.8, 0.0])
            }
        };
        let note = PreparedOperatorFm4::new(
            8000,
            69,
            93,
            oscillators,
            Routing::new(modulation, carriers).expect("acyclic graph"),
            envelopes,
        )
        .expect("original note");
        assert_eq!(note.release_frames(), 41);
        for gate in [0, 1, 3, 7, 11, 17, 28, 43] {
            for age in 0..100 {
                // Independent continuous-sine and power-law amplitude oracle.
                let mut signals = [0.0; 4];
                for target in order {
                    let phase_modulation: f64 = (0..4)
                        .map(|source| signals[source] * modulation[source][target])
                        .sum();
                    signals[target] = (TAU
                        * (440.0 * ratios[target] * age as f64 / 8000.0
                            + phases[target]
                            + phase_modulation))
                        .sin()
                        * gains[target]
                        * amplitude(levels[target], times[target], age, gate);
                }
                let expected: f64 = signals
                    .iter()
                    .zip(carriers)
                    .map(|(signal, carrier)| signal * carrier)
                    .sum::<f64>()
                    * 93.0
                    / 127.0;
                let actual = f64::from(note.sample(&table, age, gate));
                assert!(
                    (actual - expected).abs() < 0.00005,
                    "variant={variant}, age={age}, gate={gate}: {actual} != {expected}"
                );
                if age >= gate + 41 {
                    assert_eq!(actual, 0.0);
                }
            }
        }
    }
}

#[test]
fn a_silent_modulator_leaves_the_carrier_releasing_without_modulation() {
    let table = SineTable::new();
    let oscillators = [
        Oscillator::new(1.0, 0.6, 0.0).expect("modulator"),
        Oscillator::new(2.0, 0.8, 0.125).expect("carrier"),
        Oscillator::new(1.0, 0.0, 0.0).expect("quiet"),
        Oscillator::new(1.0, 0.0, 0.0).expect("quiet"),
    ];
    let mut matrix = [[0.0; 4]; 4];
    matrix[0][1] = 0.2;
    let envelopes = [
        config([1.0, 1.0, 1.0], [0, 0, 0]),
        config([1.0, 1.0, 1.0], [0, 0, 20]),
        config([0.0; 3], [0; 3]),
        config([0.0; 3], [0; 3]),
    ];
    let note = PreparedOperatorFm4::new(
        8000,
        69,
        127,
        oscillators,
        Routing::new(matrix, [0.0, 1.0, 0.0, 0.0]).expect("routing"),
        envelopes,
    )
    .expect("note");
    // At NoteOff the modulator has zero release, while the carrier releases for
    // twenty frames. The resulting phase must be the unmodulated carrier.
    for age in 7..27 {
        let expected = (TAU * (880.0 * age as f64 / 8000.0 + 0.125)).sin()
            * 0.8
            * 0.00001_f64.powf((age - 7) as f64 / 20.0);
        assert!((f64::from(note.sample(&table, age, 7)) - expected).abs() < 0.000002);
    }
    assert_eq!(note.sample(&table, 27, 7), 0.0);
}

#[test]
fn query_partition_and_order_do_not_change_prepared_note_samples() {
    let table = SineTable::new();
    let oscillators = std::array::from_fn(|i| {
        Oscillator::new((i + 1) as f64, 0.2, i as f64 / 8.0).expect("oscillator")
    });
    let envelopes = std::array::from_fn(|i| {
        config([0.1, 0.8, 0.3], [i as u64, (i + 2) as u64, (i + 10) as u64])
    });
    let note = PreparedOperatorFm4::new(
        8000,
        57,
        64,
        oscillators,
        Routing::new([[0.0; 4]; 4], [0.25; 4]).expect("routing"),
        envelopes,
    )
    .expect("note");
    let expected: Vec<_> = (0..100)
        .map(|age| note.sample(&table, age, 7).to_bits())
        .collect();
    for size in [1, 3, 16, 99] {
        let actual: Vec<_> = (0..100)
            .collect::<Vec<_>>()
            .chunks(size)
            .flat_map(|ages| {
                ages.iter()
                    .map(|&age| note.sample(&table, age, 7).to_bits())
            })
            .collect();
        assert_eq!(actual, expected);
    }
    for age in (0..100).rev() {
        assert_eq!(
            note.sample(&table, age, 7).to_bits(),
            expected[age as usize]
        );
    }
    assert_eq!(note.sample(&table, u64::MAX, 0), 0.0);
    assert!(note.sample(&table, u64::MAX, u64::MAX).is_finite());
}

#[test]
fn input_validation_remains_control_side_and_nyquist_operators_are_admitted() {
    let quiet = Oscillator::new(1.0, 0.0, 0.0).expect("quiet oscillator");
    let routing = Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).expect("routing");
    let envelopes = [config([0.0, 1.0, 0.5], [1, 2, 3]); 4];
    for (rate, pitch, velocity) in [
        (0, 69, 127),
        (7999, 69, 127),
        (192001, 69, 127),
        (8000, 128, 127),
        (8000, 69, 0),
        (8000, 69, 128),
    ] {
        assert!(
            PreparedOperatorFm4::new(rate, pitch, velocity, [quiet; 4], routing, envelopes)
                .is_err()
        );
    }
    let mut oscillators = [quiet; 4];
    oscillators[3] = Oscillator::new(128.0, 0.0, 0.0).expect("bounded ratio");
    assert!(PreparedOperatorFm4::new(8000, 69, 127, oscillators, routing, envelopes).is_ok());
}
