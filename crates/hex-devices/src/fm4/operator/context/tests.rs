use super::*;

#[test]
fn empty_modulation_fast_path_matches_full_routing_and_feedback() {
    let oscillators = [Oscillator::new(1.0, 0.75, 0.0).unwrap(); 4];
    let envelopes = [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.01, 0.01).unwrap(); 4];
    for (routed, zero) in [(false, 0.0), (false, -0.0), (true, 0.0)] {
        let mut matrix = [[zero; 4]; 4];
        if routed {
            matrix[0][1] = 0.375;
            matrix[1][2] = -0.25;
        }
        let config = PreparedOperatorConfiguration::new(
            48_000,
            oscillators,
            Routing::new(matrix, [0.25; 4]).unwrap(),
            envelopes,
        )
        .unwrap();
        assert_eq!(config.has_modulation(), routed);
        let note = config.prepare_note(60, 100).unwrap();
        let mut expected_history = [0.0; 4];
        let mut actual_history = [0.0; 4];
        for frame in 0..128u64 {
            let phases = std::array::from_fn(|i| frame.wrapping_mul(note.steps[i]));
            let sample = |operator: usize, phase: u64, _: u64| {
                ((phase >> 40) as f32 / 16_777_216.0) * (operator as f32 + 1.0) - 1.0
            };
            let expected = config.sample_with_waveform_sampler(
                &note,
                [0.5; 4],
                &phases,
                expected_history,
                [0.75; 4],
                [0.25; 4],
                [0.125, 0.0, 0.0, 0.0],
                true,
                None,
                sample,
            );
            let actual = config.sample_with_waveform_sampler(
                &note,
                [0.5; 4],
                &phases,
                actual_history,
                [0.75; 4],
                [0.25; 4],
                [0.125, 0.0, 0.0, 0.0],
                config.has_modulation(),
                None,
                sample,
            );
            assert_eq!(actual.0.to_bits(), expected.0.to_bits());
            assert_eq!(actual.1.map(f64::to_bits), expected.1.map(f64::to_bits));
            expected_history = expected.1;
            actual_history = actual.1;
        }
    }
}

#[test]
fn compact_notes_match_standalone_pcm_across_routes_gates_and_query_order() {
    let table = SineTable::new();
    let oscillators = [
        Oscillator::new(0.5, 0.7, 0.125).expect("oscillator"),
        Oscillator::new(1.5, 0.4, 0.25).expect("oscillator"),
        Oscillator::new(2.0, 0.8, 0.5).expect("oscillator"),
        Oscillator::new(3.0, 0.2, 0.75).expect("oscillator"),
    ];
    let envelopes = [
        OperatorEnvelope::new(0.2, 0.9, 0.4, 0.001, 0.003, 0.005).expect("envelope"),
        OperatorEnvelope::new(0.1, 0.7, 0.2, 0.0, 0.002, 0.007).expect("envelope"),
        OperatorEnvelope::new(0.5, 0.3, 0.6, 0.002, 0.0, 0.001).expect("envelope"),
        OperatorEnvelope::new(0.0, 1.0, 0.8, 0.001, 0.005, 0.0).expect("envelope"),
    ];
    for order in [[0, 1, 2, 3], [3, 1, 0, 2], [2, 0, 3, 1]] {
        let mut depths = [[0.0; 4]; 4];
        for index in 0..3 {
            depths[order[index]][order[index + 1]] = -0.17 * (index + 1) as f64;
        }
        let routing = Routing::new(depths, [0.3, -0.2, 0.1, 0.4]).expect("routing");
        for rate in [8000, 48000, 192000] {
            let context = PreparedOperatorConfiguration::new(rate, oscillators, routing, envelopes)
                .expect("context");
            for (pitch, velocity) in [(0, 1), (60, 100), (69, 127)] {
                let compact = context.prepare_note(pitch, velocity).expect("note");
                let standalone = PreparedOperatorFm4::new(
                    rate,
                    pitch,
                    velocity,
                    oscillators,
                    routing,
                    envelopes,
                )
                .expect("standalone");
                assert_eq!(context.release_frames(), standalone.release_frames());
                for gate in [0, 1, 16, 64, 1000, u64::MAX / 2] {
                    let mut ages: Vec<u64> = (0..256).collect();
                    ages.extend([
                        gate,
                        gate.saturating_add(context.release_frames()),
                        u64::MAX - 1,
                    ]);
                    for age in ages.into_iter().rev() {
                        assert_eq!(
                            context.sample(&table, &compact, age, gate).to_bits(),
                            standalone.sample(&table, age, gate).to_bits(),
                            "rate {rate}, pitch {pitch}, age {age}, gate {gate}, order {order:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rates_and_midi_values_are_validated_while_nyquist_operators_are_silenced() {
    let mut oscillators = [Oscillator::new(1.0, 1.0, 0.0).expect("oscillator"); 4];
    oscillators[3] = Oscillator::new(128.0, 0.0, 0.0).expect("inaudible oscillator");
    let routing = Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).expect("routing");
    let envelopes = [OperatorEnvelope::new(0.0, 1.0, 0.7, 0.0, 0.0, 0.0).expect("envelope"); 4];
    for rate in [0, 7999, 192001] {
        assert!(PreparedOperatorConfiguration::new(rate, oscillators, routing, envelopes).is_err());
    }
    let context =
        PreparedOperatorConfiguration::new(8000, oscillators, routing, envelopes).expect("context");
    assert!(context.prepare_note(0, 100).is_ok());
    assert!(context.prepare_note(60, 100).is_ok());
    assert!(context.prepare_note(128, 100).is_err());
    assert!(context.prepare_note(0, 0).is_err());
    assert!(context.prepare_note(0, 128).is_err());
}

#[test]
fn configuration_and_note_fit_the_shared_contract_without_heap_owners() {
    assert_eq!(size_of::<PreparedOperatorNote>(), 64);
    assert!(size_of::<PreparedOperatorConfiguration>() <= 4096);
    assert!(!std::mem::needs_drop::<PreparedOperatorNote>());
    assert!(!std::mem::needs_drop::<PreparedOperatorConfiguration>());
    println!(
        "operator context {} bytes, note {} bytes",
        size_of::<PreparedOperatorConfiguration>(),
        size_of::<PreparedOperatorNote>()
    );
}

#[test]
fn private_note_payload_preserves_original_midi_and_rejects_corrupt_origin() {
    let config = PreparedOperatorConfiguration::new(
        48000,
        [Oscillator::new(1.0, 1.0, 0.0).unwrap(); 4],
        Routing::new([[0.0; 4]; 4], [1.0, 0.0, 0.0, 0.0]).unwrap(),
        [OperatorEnvelope::new(1.0, 1.0, 1.0, 0.0, 0.001, 0.001).unwrap(); 4],
    )
    .unwrap();
    for (pitch, velocity) in [(0, 1), (60, 100), (127, 127)] {
        let note = config.prepare_note(pitch, velocity).unwrap();
        assert_eq!(note.midi_origin(), (pitch, velocity));
        let payload = note.into_shared_payload();
        assert_eq!(
            PreparedOperatorNote::from_shared_payload(&payload),
            Some(note)
        );
        for mask in [1u64 << 48, 1u64 << 47, 1u64 << 39, 1u64 << 40] {
            let mut bad = payload;
            bad[5] ^= mask;
            assert!(PreparedOperatorNote::from_shared_payload(&bad).is_none());
        }
    }
}
