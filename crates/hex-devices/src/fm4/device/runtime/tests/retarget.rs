use super::*;
fn proof(
    factory: &dyn PreparedMutableInstrumentRuntime,
) -> Box<dyn PreparedInstrumentHistoryRetarget> {
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("retarget proof");
    };
    proof
}
fn tune(multiplier: f64) -> hex_lattice::TuningTable {
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    frequencies[60] *= multiplier;
    hex_lattice::TuningTable::new(frequencies).unwrap()
}
#[test]
fn repeated_tuning_retargets_match_inline_phase_continuation_for_held_and_released_notes() {
    let resources = BorrowedDeviceResources(&[]);
    for gate in [None, Some(500)] {
        let settings = Fm4Settings::defaults();
        let tuning = tune(1.0);
        let (factory, inline) = prepared(48000, settings, &tuning);
        let mut oracle = inline.prepare_note(60, 93).unwrap();
        let mut current = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
        current.begin_block();
        let mut initial = [[0.0; 2]; 777];
        let mut events = vec![on(1, 0, 60, 93)];
        if let Some(gate) = gate {
            events.push(NativeInstrumentEvent::NoteOff {
                frame_offset: gate,
                occurrence_id: 1,
            });
        }
        current
            .process_block(&events, &mut initial, &resources)
            .unwrap();
        current.commit_block();
        let mut age = 777;
        for multiplier in [1.03, 0.97, 1.0] {
            let (target, inline) = prepared(48000, settings, &tune(multiplier));
            let next_oracle = inline
                .prepare_note(60, 93)
                .unwrap()
                .continue_from(&oracle, age)
                .unwrap();
            let mut next = NativeInstrumentRuntimePair::new(target.as_ref(), LIMITS).unwrap();
            let proof = proof(target.as_ref());
            assert!(!next.can_inherit_from(&current));
            assert!(next.can_retarget_from(&current, proof.as_ref()));
            next.retarget_from(&current, proof.as_ref()).unwrap();
            next.begin_block();
            let mut frames = [[0.0; 2]; 173];
            next.process_block(&[], &mut frames, &resources).unwrap();
            next.commit_block();
            for (offset, actual) in frames.iter().enumerate() {
                assert_eq!(
                    *actual,
                    next_oracle.sample(
                        age + offset as u64,
                        gate.map_or(u64::MAX, |gate| gate as u64),
                        &resources
                    )
                );
            }
            let mut ids = [0; VOICES];
            assert_eq!(next.active_occurrence_ids(&mut ids).unwrap(), 1);
            assert_eq!(ids[0], 1);
            current = next;
            oracle = next_oracle;
            age += frames.len() as u64;
        }
    }
}
#[test]
fn exact_target_rate_and_retained_pitch_are_required_and_refusal_preserves_source() {
    let settings = Fm4Settings::defaults();
    let (source, _) = prepared(8000, settings, &tune(1.0));
    let mut source_pair = NativeInstrumentRuntimePair::new(source.as_ref(), LIMITS).unwrap();
    source_pair.begin_block();
    source_pair
        .process_block(
            &[on(3, 0, 60, 127)],
            &mut [[0.0; 2]; 103],
            &BorrowedDeviceResources(&[]),
        )
        .unwrap();
    source_pair.commit_block();
    let (valid, _) = prepared(8000, settings, &tune(1.01));
    let valid_proof = proof(valid.as_ref());
    for (rate, tuning, settings) in [(8000, tune(4.0), settings), (44100, tune(1.0), settings)] {
        let (factory, _) = prepared(rate, settings, &tuning);
        let mut candidate = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
        let own_proof = proof(factory.as_ref());
        assert!(!candidate.can_retarget_from(&source_pair, own_proof.as_ref()));
        assert_eq!(
            candidate.retarget_from(&source_pair, own_proof.as_ref()),
            Err(Error::IncompatibleHistory)
        );
        assert!(!candidate.can_retarget_from(&source_pair, valid_proof.as_ref()));
        assert!(candidate.committed_is_quiescent());
    }
    let mut control = NativeInstrumentRuntimePair::new(source.as_ref(), LIMITS).unwrap();
    control.inherit_from(&source_pair).unwrap();
    source_pair.begin_block();
    control.begin_block();
    let mut actual = [[0.0; 2]; 37];
    let mut expected = actual;
    source_pair
        .process_block(&[], &mut actual, &BorrowedDeviceResources(&[]))
        .unwrap();
    control
        .process_block(&[], &mut expected, &BorrowedDeviceResources(&[]))
        .unwrap();
    assert_eq!(actual, expected);
}
#[test]
fn effective_carrier_ramp_survives_retarget_and_base_changes_retarget_from_current_weight() {
    let settings = Fm4Settings::defaults();
    let (factory, _) = prepared(48000, settings, &tune(1.0));
    let mut source = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    source.begin_block();
    source
        .process_block(
            &[on(1, 0, 60, 127)],
            &mut [[0.0; 2]; 100],
            &BorrowedDeviceResources(&[]),
        )
        .unwrap();
    source.commit_block();
    let authored = Values {
        weights: [None, Some(1.0), None, None],
        bad_at: None,
    };
    source.begin_block();
    automated(&mut source, &[], &mut [[0.0; 2]; 61], &authored).unwrap();
    source.commit_block();
    let mut candidate = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let same_proof = proof(factory.as_ref());
    candidate
        .retarget_from(&source, same_proof.as_ref())
        .unwrap();
    source.begin_block();
    candidate.begin_block();
    let mut a = [[0.0; 2]; 300];
    let mut b = a;
    automated(&mut source, &[], &mut a, &authored).unwrap();
    automated(&mut candidate, &[], &mut b, &authored).unwrap();
    assert_eq!(a, b);
    source.commit_block();
    candidate.rollback_block();
    let mut changed = settings;
    changed.carriers[1] = 0.5;
    let (factory, inline) = prepared(48000, changed, &tune(1.0));
    let mut next = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let proof = proof(factory.as_ref());
    next.retarget_from(&source, proof.as_ref()).unwrap();
    next.begin_block();
    let mut frames = [[0.0; 2]; 300];
    next.process_block(&[], &mut frames, &BorrowedDeviceResources(&[]))
        .unwrap();
    next.rollback_block();
    next.begin_block();
    let mut retry = [[0.0; 2]; 300];
    next.process_block(&[], &mut retry, &BorrowedDeviceResources(&[]))
        .unwrap();
    assert_eq!(frames, retry);
    let note = inline.prepare_note(60, 127).unwrap();
    for (offset, actual) in frames.iter().enumerate().skip(239) {
        assert_eq!(
            *actual,
            note.sample(461 + offset as u64, u64::MAX, &BorrowedDeviceResources(&[]))
        );
    }
}

#[test]
fn complete_base_law_edits_adopt_audible_target_coefficients_at_retained_note_age() {
    let settings = Fm4Settings::defaults();
    let tuning = tune(1.0);
    let mut variants = [settings; 5];
    variants[0].oscillators[0][0] = 1.5;
    variants[1].oscillators[0][1] = 0.4;
    variants[2].oscillators[0][2] = 0.25;
    variants[3].modulation[1][0] = 1.2;
    variants[4].envelope[0] = 0.020;
    for changed in variants {
        let (factory, old_inline) = prepared(48000, settings, &tuning);
        let mut source = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
        source.begin_block();
        source
            .process_block(
                &[on(1, 0, 60, 93)],
                &mut [[0.0; 2]; 777],
                &BorrowedDeviceResources(&[]),
            )
            .unwrap();
        source.commit_block();
        let (target, _) = prepared(48000, changed, &tuning);
        let proof = proof(target.as_ref());
        assert!(
            proof.declared_fixed_storage_bytes() < 4096,
            "native host proof envelope"
        );
        let mut next = NativeInstrumentRuntimePair::new(target.as_ref(), LIMITS).unwrap();
        assert!(!next.can_inherit_from(&source));
        next.retarget_from(&source, proof.as_ref()).unwrap();
        let (oscillators, routing, envelope) = changed.kernel_parts().unwrap();
        let mut oracle = PreparedFm4::new_with_frequency(
            48000,
            tuning.frequency(60).unwrap(),
            93,
            oscillators,
            routing,
            envelope,
        )
        .unwrap();
        let (oscillators, routing, envelope) = settings.kernel_parts().unwrap();
        let original = PreparedFm4::new_with_frequency(
            48000,
            tuning.frequency(60).unwrap(),
            93,
            oscillators,
            routing,
            envelope,
        )
        .unwrap();
        for index in 0..4 {
            oracle.core.configuration.phases[index] = oracle.core.configuration.phases[index]
                .wrapping_add(
                    original.core.steps[index]
                        .wrapping_sub(oracle.core.steps[index])
                        .wrapping_mul(777),
                );
        }
        next.begin_block();
        let mut actual = [[0.0; 2]; 64];
        next.process_block(&[], &mut actual, &BorrowedDeviceResources(&[]))
            .unwrap();
        let old = old_inline.prepare_note(60, 93).unwrap();
        let mut changed_audibly = false;
        for (offset, actual) in actual.iter().enumerate() {
            let sample = oracle.sample(crate::shared_sine_table(), 777 + offset as u64, u64::MAX);
            assert_eq!(*actual, [sample; 2]);
            changed_audibly |=
                *actual != old.sample(777 + offset as u64, u64::MAX, &BorrowedDeviceResources(&[]));
        }
        assert!(changed_audibly);
        next.rollback_block();
    }
}

#[test]
fn carried_release_ceilings_shorten_but_do_not_exceed_admitted_deadlines() {
    let rate = 8000;
    let mut settings = Fm4Settings::defaults();
    settings.envelope = [0.0, 0.0, 1.0, 0.010];
    let resources = BorrowedDeviceResources(&[]);
    for released in [false, true] {
        for release in [0.0_f32, 0.005, 0.020] {
            let (factory, _) = prepared(rate, settings, &tune(1.0));
            let mut source = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
            let mut events = vec![on(1, 0, 60, 100)];
            if released {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 100,
                    occurrence_id: 1,
                });
            }
            source.begin_block();
            source
                .process_block(&events, &mut [[0.0; 2]; 120], &resources)
                .unwrap();
            source.commit_block();
            let mut old_ceiling = settings.envelope[3];
            if release > old_ceiling {
                // A later increase cannot undo an earlier admitted shortening.
                let mut shorter = settings;
                shorter.envelope[3] = 0.005;
                let (factory, _) = prepared(rate, shorter, &tune(1.0));
                let mut intermediate =
                    NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
                intermediate
                    .retarget_from(&source, proof(factory.as_ref()).as_ref())
                    .unwrap();
                source = intermediate;
                old_ceiling = 0.005;
            }
            let mut target_settings = settings;
            target_settings.envelope[3] = release;
            let (factory, target) = prepared(rate, target_settings, &tune(1.0));
            let target_note = target.prepare_note(60, 100).unwrap();
            let mut candidate = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
            candidate
                .retarget_from(&source, proof(factory.as_ref()).as_ref())
                .unwrap();
            let mut capped = target_settings;
            capped.envelope[3] = release.min(old_ceiling);
            let (_, old) = prepared(rate, capped, &tune(1.0));
            let old_note = old.prepare_note(60, 100).unwrap();
            let mut events = vec![on(2, 0, 60, 100)];
            if !released {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: 0,
                    occurrence_id: 1,
                });
            }
            events.push(NativeInstrumentEvent::NoteOff {
                frame_offset: 10,
                occurrence_id: 2,
            });
            candidate.begin_block();
            let mut output = [[0.0; 2]; 200];
            candidate
                .process_block(&events, &mut output, &resources)
                .unwrap();
            candidate.commit_block();
            for (offset, actual) in output.iter().enumerate() {
                let old = old_note.sample(
                    120 + offset as u64,
                    if released { 100 } else { 120 },
                    &resources,
                )[0];
                let new = target_note.sample(offset as u64, 10, &resources)[0];
                assert_eq!(*actual, [(f64::from(old) + f64::from(new)) as f32; 2]);
            }
            assert!(candidate.committed_is_quiescent());
            assert_eq!(
                candidate.active_occurrence_ids(&mut [0; VOICES]).unwrap(),
                0
            );
        }
    }
}
