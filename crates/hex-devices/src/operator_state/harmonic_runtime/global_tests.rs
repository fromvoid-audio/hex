use super::*;
use hex_lattice::NativeInstrumentExpression;

#[test]
fn expression_snapshot_reserves_three_events_without_reducing_note_capacity() {
    let mut r = runtime(settings());
    let mut events = vec![
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::ChannelPressure(0),
        },
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::PitchBend(0),
        },
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::ModWheel(0),
        },
    ];
    for id in 1..=128 {
        events.push(on(id, 60));
        events.push(off(id));
    }
    assert_eq!(r.factory.max_events_per_block(), 259);
    assert_eq!(events.len(), 259);
    process(&mut r, &events, 1);
    events.push(NativeInstrumentEvent::Expression {
        frame_offset: 0,
        expression: NativeInstrumentExpression::ModWheel(1),
    });
    assert_eq!(
        r.preflight_events(&events, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        Err(Error::EventCapacityExceeded)
    );
}

#[test]
fn repeated_controller_snapshot_keeps_held_filter_transition_and_pcm() {
    let mut s = settings();
    s.filter
        .patch(ParameterId::from_static(20001), ParameterValue::Bool(true))
        .unwrap();
    s.modulation
        .patch(ParameterId::from_static(628), ParameterValue::Choice(5))
        .unwrap();
    s.modulation
        .patch(ParameterId::from_static(629), ParameterValue::Float(25.0))
        .unwrap();
    let mut source = runtime(s);
    let pressure = NativeInstrumentEvent::Expression {
        frame_offset: 0,
        expression: NativeInstrumentExpression::ChannelPressure(64),
    };
    process(&mut source, &[on(1, 60)], 80);
    process(&mut source, &[pressure], 17);
    let mut repeated = runtime(s);
    repeated.copy_history_from(&source);
    assert_eq!(
        process(&mut source, &[], 128),
        process(&mut repeated, &[pressure], 128)
    );
}
fn settings() -> OperatorSettings {
    let mut s = super::super::tests::legacy_settings();
    s.global = global::Settings::default();
    for i in 0..4 {
        let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
        s.fields[base..base + 6].copy_from_slice(&[1.0, 1.0, 1.0, 0.0, 0.001, 0.001]);
    }
    s
}
fn runtime(settings: OperatorSettings) -> Runtime {
    let factory = Factory::new(
        settings,
        settings.prepare_configuration(48000).unwrap(),
        crate::shared_sine_table(),
        48000,
    )
    .unwrap();
    Runtime {
        source: factory.target,
        filter: factory.filter,
        factory,
        voices: [Voice::default(); VOICES],
        elapsed: 1,
        frames: 1,
        parameters: None,
        phase: (0.0, 0),
        local_frame: 0,
        ledger: Ledger::default(),
        expression: [0.0; 3],
    }
}
fn on(id: u64, pitch: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: id,
        pitch,
        velocity: 100,
    }
}
fn off(id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset: 0,
        occurrence_id: id,
    }
}
fn process(r: &mut Runtime, events: &[NativeInstrumentEvent], frames: usize) -> Vec<StereoFrame> {
    r.preflight_events(events, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut out = vec![[0.0; 2]; frames];
    r.process_block(events, &mut out, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    out
}
fn active(r: &Runtime) -> Vec<u64> {
    let mut ids: Vec<_> = r
        .voices
        .iter()
        .filter(|v| v.id != 0)
        .map(|v| v.id)
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn uniform_ordinary_envelopes_match_independent_render_across_handoff_and_release() {
    for global in [false, true] {
        let mut s = settings();
        s.global.active = global;
        for i in 0..4 {
            let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
            s.fields[base..base + 6].copy_from_slice(&[0.1, 1.0, 0.3, 0.001, 0.002, 0.003]);
        }
        let mut fast = runtime(s);
        let mut reference = runtime(s);
        assert!(fast.factory.uniform_envelopes);
        assert!(!fast.factory.companion_active);
        reference.factory.uniform_envelopes = false;
        assert_eq!(
            process(&mut fast, &[on(1, 60)], 128),
            process(&mut reference, &[on(1, 60)], 128)
        );
        let mut handoff = runtime(s);
        handoff.copy_history_from(&fast);
        assert_eq!(
            process(&mut handoff, &[off(1)], 512),
            process(&mut reference, &[off(1)], 512)
        );
        s.fields[CORE_FIELDS + 3 * ENVELOPE_FIELDS + 2] = 0.5;
        assert!(!runtime(s).factory.uniform_envelopes);
    }
}

#[test]
fn held_voice_preflight_rejects_frame_and_age_overflow() {
    for global in [false, true] {
        let mut s = settings();
        s.global.active = global;
        let mut r = runtime(s);
        process(&mut r, &[on(1, 60)], 1);
        let voice = r.voices.iter_mut().find(|voice| voice.id == 1).unwrap();
        voice.age = u64::MAX - 1;
        assert!(
            r.preflight_events(&[], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                .is_ok()
        );
        assert_eq!(
            r.preflight_events(&[], 2, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            Err(Error::InvalidEventBatch)
        );
        r.voices.iter_mut().find(|voice| voice.id == 1).unwrap().age = 1;
        r.local_frame = u64::MAX - 1;
        assert_eq!(
            r.preflight_events(&[], 2, &hex_lattice::EMPTY_DEVICE_RESOURCES),
            Err(Error::InvalidEventBatch)
        );
    }
}

#[test]
fn releasing_voice_can_retire_before_its_age_would_overflow() {
    let mut r = runtime(settings());
    process(&mut r, &[on(1, 60)], 1);
    let voice = r.voices.iter_mut().find(|voice| voice.id == 1).unwrap();
    voice.age = u64::MAX - 1;
    voice.gate = Some(0);
    voice.envelope_gate = Some(0);
    voice.envelope_age = u64::MAX as f64;
    voice.drain = quality::TAIL as u32 - 1;
    assert!(
        r.preflight_events(&[], 2, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_ok()
    );
}

#[test]
fn note_added_in_later_slot_mid_block_matches_partitioned_render() {
    for global in [false, true] {
        let mut s = settings();
        s.global.active = global;
        let mut whole = runtime(s);
        let mut partitioned = runtime(s);
        let mut one_voice = runtime(s);
        assert_eq!(
            process(&mut whole, &[on(1, 60)], 64),
            process(&mut partitioned, &[on(1, 60)], 64)
        );
        process(&mut one_voice, &[on(1, 60)], 64);
        let without_new_voice = process(&mut one_voice, &[], 128);
        let delayed = NativeInstrumentEvent::NoteOn {
            frame_offset: 33,
            occurrence_id: 2,
            pitch: 67,
            velocity: 100,
        };
        let actual = process(&mut whole, &[delayed], 128);
        let mut expected = process(&mut partitioned, &[], 33);
        expected.extend(process(&mut partitioned, &[on(2, 67)], 95));
        assert_eq!(actual, expected);
        assert_ne!(actual, without_new_voice);
    }
}

#[test]
fn held_voice_release_inside_block_matches_partitioned_render() {
    for global in [false, true] {
        let mut s = settings();
        s.global.active = global;
        let mut whole = runtime(s);
        let mut partitioned = runtime(s);
        assert_eq!(
            process(&mut whole, &[on(1, 60)], 128),
            process(&mut partitioned, &[on(1, 60)], 128)
        );
        let delayed_off = NativeInstrumentEvent::NoteOff {
            frame_offset: 33,
            occurrence_id: 1,
        };
        let actual = process(&mut whole, &[delayed_off], 256);
        let mut expected = process(&mut partitioned, &[], 33);
        expected.extend(process(&mut partitioned, &[off(1)], 223));
        assert_eq!(actual, expected);
        assert_eq!(
            process(&mut whole, &[], 5000),
            process(&mut partitioned, &[], 5000)
        );
        assert!(whole.is_quiescent());
        assert!(partitioned.is_quiescent());
    }
}

#[test]
fn uniform_companion_matches_independent_machines_through_handoff_and_release() {
    struct Clock(u64);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            let frame = self.0 + offset as u64;
            Some(frame * (1 << 25) + if frame >= 40 { 1000 << 32 } else { 0 })
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.0 + offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    fn render(
        r: &mut Runtime,
        events: &[NativeInstrumentEvent],
        frames: usize,
        at: u64,
    ) -> Vec<StereoFrame> {
        let clock = Clock(at);
        r.preflight_events(events, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        r.preflight_clock(Some(&clock), frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let mut output = vec![[0.0; 2]; frames];
        r.process_block_with_clock(
            events,
            &mut output,
            Some(&clock),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        output
    }
    for mode in 1..=4 {
        for global in [false, true] {
            let mut s = settings();
            s.global.active = global;
            for row in &mut s.companion.words {
                row[5] = mode;
                row[6] = 0.001_f32.to_bits();
                row[7] = 0;
            }
            let mut fast = runtime(s);
            let mut reference = runtime(s);
            assert!(fast.factory.uniform_companion);
            reference.factory.uniform_companion = false;
            assert_eq!(
                render(&mut fast, &[on(1, 60)], 128, 0),
                render(&mut reference, &[on(1, 60)], 128, 0)
            );
            let mut fast_handoff = runtime(s);
            let mut reference_handoff = runtime(s);
            reference_handoff.factory.uniform_companion = false;
            assert!(fast_handoff.can_copy_history_from(&fast));
            fast_handoff.copy_history_from(&fast);
            reference_handoff.copy_history_from(&reference);
            let mut retargeted = runtime(s);
            let mut reference_retargeted = runtime(s);
            reference_retargeted.factory.uniform_companion = false;
            let InstrumentHistoryRetargetPreparation::Prepared(proof) = retargeted
                .factory
                .prepare_history_retarget_target()
                .unwrap()
            else {
                panic!("prepared target")
            };
            let InstrumentHistoryRetargetPreparation::Prepared(reference_proof) =
                reference_retargeted
                    .factory
                    .prepare_history_retarget_target()
                    .unwrap()
            else {
                panic!("prepared target")
            };
            assert!(retargeted.can_retarget_history_from(&fast, proof.as_ref()));
            assert!(
                reference_retargeted
                    .can_retarget_history_from(&reference, reference_proof.as_ref())
            );
            retargeted.retarget_history_from(&fast, proof.as_ref());
            reference_retargeted.retarget_history_from(&reference, reference_proof.as_ref());
            assert_eq!(
                render(&mut fast_handoff, &[off(1)], 128, 128),
                render(&mut reference_handoff, &[off(1)], 128, 128)
            );
            assert_eq!(
                render(&mut retargeted, &[off(1)], 128, 128),
                render(&mut reference_retargeted, &[off(1)], 128, 128)
            );
            assert_eq!(
                render(&mut fast_handoff, &[], 5000, 256),
                render(&mut reference_handoff, &[], 5000, 256)
            );
            assert!(fast_handoff.is_quiescent());
            assert!(reference_handoff.is_quiescent());
        }
    }
}

#[test]
fn partial_companion_groups_match_independent_machines_across_lifecycle() {
    struct Clock(u64);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            let frame = self.0 + offset as u64;
            Some(frame * (1 << 25) + if frame >= 40 { 1000 << 32 } else { 0 })
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.0 + offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }
    fn render(
        r: &mut Runtime,
        events: &[NativeInstrumentEvent],
        frames: usize,
        at: u64,
    ) -> Vec<StereoFrame> {
        let clock = Clock(at);
        r.preflight_events(events, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        r.preflight_clock(Some(&clock), frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let mut output = vec![[0.0; 2]; frames];
        r.process_block_with_clock(
            events,
            &mut output,
            Some(&clock),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        output
    }
    let partitions = [
        [0, 0, 0, 0],
        [0, 0, 0, 1],
        [0, 0, 1, 0],
        [0, 0, 1, 1],
        [0, 0, 1, 2],
        [0, 1, 0, 0],
        [0, 1, 0, 1],
        [0, 1, 0, 2],
        [0, 1, 1, 0],
        [0, 1, 1, 1],
        [0, 1, 1, 2],
        [0, 1, 2, 0],
        [0, 1, 2, 1],
        [0, 1, 2, 2],
        [0, 1, 2, 3],
    ];
    for (index, pattern) in partitions.into_iter().enumerate() {
        for mode in 0..=if index == 1 || index == 6 { 4 } else { 1 } {
            for global in [false, true] {
                let mut s = settings();
                s.global.active = global;
                for (operator, group) in pattern.into_iter().enumerate() {
                    s.fields[28 + operator] = 0.25;
                    s.fields[CORE_FIELDS + operator * ENVELOPE_FIELDS + 2] =
                        [0.2, 0.4, 0.6, 0.8][group];
                    s.companion.words[operator][5] = mode;
                    s.companion.words[operator][4] = 0.1_f32.to_bits();
                    s.companion.words[operator][6] = 0.001_f32.to_bits();
                    s.companion.words[operator][7] = 0;
                }
                let mut fast = runtime(s);
                let mut reference = runtime(s);
                if pattern == [0, 0, 0, 0] {
                    assert!(fast.factory.uniform_companion);
                } else if pattern == [0, 1, 2, 3] {
                    assert!(!fast.factory.uniform_companion && !fast.factory.partial_companion);
                    assert_eq!(fast.factory.shared_loop_timing, mode == 1);
                } else {
                    assert!(fast.factory.partial_companion);
                }
                reference.factory.uniform_companion = false;
                reference.factory.partial_companion = false;
                reference.factory.shared_loop_timing = false;
                assert_eq!(
                    render(&mut fast, &[on(1, 60)], 128, 0),
                    render(&mut reference, &[on(1, 60)], 128, 0),
                    "pattern={pattern:?} mode={mode} global={global}"
                );
                let mut fast_handoff = runtime(s);
                let mut reference_handoff = runtime(s);
                reference_handoff.factory.uniform_companion = false;
                reference_handoff.factory.partial_companion = false;
                reference_handoff.factory.shared_loop_timing = false;
                assert!(fast_handoff.can_copy_history_from(&fast));
                fast_handoff.copy_history_from(&fast);
                reference_handoff.copy_history_from(&reference);
                if index == 1 || index == 6 || index == 14 {
                    let mut retargeted = runtime(s);
                    let mut reference_retargeted = runtime(s);
                    reference_retargeted.factory.uniform_companion = false;
                    reference_retargeted.factory.partial_companion = false;
                    reference_retargeted.factory.shared_loop_timing = false;
                    let InstrumentHistoryRetargetPreparation::Prepared(proof) = retargeted
                        .factory
                        .prepare_history_retarget_target()
                        .unwrap()
                    else {
                        panic!("prepared target")
                    };
                    let InstrumentHistoryRetargetPreparation::Prepared(reference_proof) =
                        reference_retargeted
                            .factory
                            .prepare_history_retarget_target()
                            .unwrap()
                    else {
                        panic!("prepared target")
                    };
                    assert!(retargeted.can_retarget_history_from(&fast, proof.as_ref()));
                    assert!(
                        reference_retargeted
                            .can_retarget_history_from(&reference, reference_proof.as_ref())
                    );
                    retargeted.retarget_history_from(&fast, proof.as_ref());
                    reference_retargeted
                        .retarget_history_from(&reference, reference_proof.as_ref());
                    assert_eq!(
                        render(&mut retargeted, &[off(1)], 128, 128),
                        render(&mut reference_retargeted, &[off(1)], 128, 128)
                    );
                }
                assert_eq!(
                    render(&mut fast_handoff, &[off(1)], 128, 128),
                    render(&mut reference_handoff, &[off(1)], 128, 128),
                    "pattern={pattern:?} mode={mode} global={global}"
                );
                let tail = if index == 1 || index == 6 || index == 14 {
                    5000
                } else {
                    256
                };
                assert_eq!(
                    render(&mut fast_handoff, &[], tail, 256),
                    render(&mut reference_handoff, &[], tail, 256),
                    "pattern={pattern:?} mode={mode} global={global}"
                );
                if tail == 5000 {
                    assert!(fast_handoff.is_quiescent());
                    assert!(reference_handoff.is_quiescent());
                }
            }
        }
    }
}

#[test]
fn shared_loop_timing_requires_matching_prepared_timing_inputs() {
    let mut s = settings();
    for i in 0..4 {
        s.companion.words[i][5] = 1;
        s.fields[CORE_FIELDS + i * ENVELOPE_FIELDS + 2] = 0.1 + i as f32 * 0.2;
    }
    assert!(runtime(s).factory.shared_loop_timing);
    for word in [3, 5, 6] {
        let mut different = s;
        different.companion.words[3][word] ^= 1;
        assert!(!runtime(different).factory.shared_loop_timing);
    }
    for field in [3, 4] {
        let mut different = s;
        different.fields[CORE_FIELDS + 3 * ENVELOPE_FIELDS + field] = 0.1;
        assert!(!runtime(different).factory.shared_loop_timing);
    }
}

#[test]
fn uniform_companion_requires_bit_identical_authored_laws() {
    let mut s = settings();
    for row in &mut s.companion.words {
        row[5] = 1;
    }
    assert!(runtime(s).factory.uniform_companion);
    s.companion.words[3][5] = 4;
    assert!(!runtime(s).factory.uniform_companion);
    s.fields[31] = 1.0;
    let mut independent = runtime(s);
    let mut incorrectly_shared = runtime(s);
    incorrectly_shared.factory.uniform_companion = true;
    assert_ne!(
        process(&mut independent, &[on(1, 60)], 512),
        process(&mut incorrectly_shared, &[on(1, 60)], 512)
    );
    s.companion.words[3][5] = 1;
    let mut different_law = s;
    different_law.fields[CORE_FIELDS + 3 * ENVELOPE_FIELDS + 2] = 0.8;
    let mut previous = runtime(different_law);
    process(&mut previous, &[on(1, 60)], 64);
    let target = runtime(s);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("prepared target")
    };
    assert!(!target.can_retarget_history_from(&previous, proof.as_ref()));
    for i in 0..4 {
        s.fields[CORE_FIELDS + i * ENVELOPE_FIELDS] = 0.0;
    }
    s.fields[CORE_FIELDS] = -0.0;
    assert!(!runtime(s).factory.uniform_companion);
}
#[test]
fn oldest_stealing_keeps_64_occurrence_admission_and_silent_noteoffs() {
    let mut s = settings();
    s.global.voices = 1;
    let mut r = runtime(s);
    process(&mut r, &[on(1, 60), on(2, 62), on(3, 64)], 1);
    assert_eq!(active(&r), vec![2, 3]);
    assert_eq!(r.factory.max_active_occurrences(), 64);
    assert_eq!(r.ledger.held.iter().filter(|h| h.id != 0).count(), 3);
    process(&mut r, &[off(1)], 1);
    assert_eq!(active(&r), vec![2, 3]);
    assert!(
        r.preflight_events(&[off(1)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    assert!(
        r.preflight_events(&[on(2, 60)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    let mut full = runtime(s);
    let events: Vec<_> = (1..=64).map(|id| on(id, 60 + (id % 12) as u8)).collect();
    process(&mut full, &events, 1);
    assert_eq!(
        full.preflight_events(&[on(65, 60)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES),
        Err(Error::EventCapacityExceeded)
    );
    let offs: Vec<_> = (1..=64).map(off).collect();
    process(&mut full, &offs, 5000);
    assert!(full.is_quiescent());
}
#[test]
fn same_pitch_retrigger_displaces_only_sound_and_mono_legato_returns_latest_held() {
    let mut s = settings();
    s.global.voices = 2;
    let mut r = runtime(s);
    process(&mut r, &[on(1, 60)], 80);
    process(&mut r, &[on(2, 60)], 1);
    assert_eq!(active(&r), vec![2]);
    assert_eq!(r.voices[0].age, 1);
    process(&mut r, &[off(1)], 1);
    assert_eq!(active(&r), vec![2]);
    s.global.voices = 0;
    s.global.retrigger = false;
    let mut r = runtime(s);
    process(&mut r, &[on(1, 60)], 80);
    let previous = r.voices[0];
    process(&mut r, &[on(2, 64)], 1);
    assert_eq!(r.voices[0].age, 81);
    assert_ne!(r.voices[0].phases, [0; 4]);
    assert_eq!(r.voices[0].envelope_age, previous.envelope_age + 1.0);
    process(&mut r, &[on(3, 67)], 1);
    process(&mut r, &[off(3)], 1);
    assert_eq!(active(&r), vec![2]);
    assert_eq!(r.voices[0].note.midi_origin(), (64, 100));
    process(&mut r, &[off(2)], 1);
    assert_eq!(active(&r), vec![1]);
    process(&mut r, &[off(1)], 5000);
    assert!(r.is_quiescent());
}
#[test]
fn shrink_pan_quality_expression_and_clock_survive_live_retarget_copy_reset() {
    let mut s = settings();
    s.global.retrigger = false;
    s.global.random = 80.0;
    let mut source = runtime(s);
    process(&mut source, &[on(1, 60), on(2, 64), on(3, 67)], 123);
    let mut target_settings = s;
    target_settings.global.voices = 0;
    target_settings.global.pan = 50.0;
    target_settings.global.antialias = false;
    target_settings.global.interpolation = false;
    let mut target = runtime(target_settings);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(target.can_retarget_history_from(&source, proof.as_ref()));
    target.retarget_history_from(&source, proof.as_ref());
    assert_eq!(active(&target), vec![3]);
    assert_eq!(target.ledger.held.iter().filter(|h| h.id != 0).count(), 3);
    let pcm = process(&mut target, &[], 100);
    assert!(pcm.iter().all(|frame| frame[0] == 0.0));
    assert!(pcm.iter().any(|frame| frame[1] != 0.0));
    process(&mut target, &[off(3)], 1);
    assert_eq!(active(&target), vec![2]);
    let mut copy = runtime(target_settings);
    assert!(copy.can_copy_history_from(&target));
    copy.copy_history_from(&target);
    assert_eq!(process(&mut target, &[], 100), process(&mut copy, &[], 100));
    copy.reset();
    assert!(copy.is_quiescent());
    assert!(
        copy.preflight_events(&[off(1)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
}
#[test]
fn expression_modulates_held_pitch_pan_and_time_without_editing_authored_state() {
    let mut s = settings();
    s.modulation
        .patch(ParameterId::from_static(628), ParameterValue::Choice(11))
        .unwrap();
    s.modulation
        .patch(ParameterId::from_static(629), ParameterValue::Float(100.0))
        .unwrap();
    s.modulation
        .patch(ParameterId::from_static(636), ParameterValue::Choice(13))
        .unwrap();
    s.modulation
        .patch(ParameterId::from_static(637), ParameterValue::Float(25.0))
        .unwrap();
    let mut r = runtime(s);
    process(&mut r, &[on(1, 60)], 100);
    let age = r.voices[0].envelope_age;
    let events = [
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::ChannelPressure(127),
        },
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::PitchBend(8191),
        },
        NativeInstrumentEvent::Expression {
            frame_offset: 0,
            expression: NativeInstrumentExpression::ModWheel(127),
        },
    ];
    let pcm = process(&mut r, &events, 10);
    assert!(pcm.iter().all(|frame| frame[0] == 0.0));
    assert_eq!(r.voices[0].effective.pitch_semitones, 5.0);
    assert_eq!(r.voices[0].envelope_age, age + 20.0);
    assert_eq!(r.factory.settings, s);
    assert!(
        r.preflight_events(
            &[NativeInstrumentEvent::Expression {
                frame_offset: 0,
                expression: NativeInstrumentExpression::PitchBend(8192)
            }],
            1,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .is_err()
    );
    let mut legacy = s;
    legacy.global = global::Settings::legacy();
    assert!(
        runtime(legacy)
            .preflight_events(&events, 10, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
}
#[test]
fn global_pcm_and_expression_are_partition_independent() {
    fn render(partition: usize) -> Vec<StereoFrame> {
        let mut s = settings();
        s.global.voices = 1;
        s.global.random = 50.0;
        s.global.key = 20.0;
        let mut r = runtime(s);
        let timeline = [
            (0, on(1, 60)),
            (17, on(2, 64)),
            (40, on(3, 67)),
            (55, off(1)),
            (
                70,
                NativeInstrumentEvent::Expression {
                    frame_offset: 0,
                    expression: NativeInstrumentExpression::PitchBend(-4000),
                },
            ),
            (120, off(2)),
            (200, off(3)),
        ];
        let mut out = Vec::new();
        for start in (0..4800).step_by(partition) {
            let end = (start + partition).min(4800);
            let events: Vec<_> = timeline
                .iter()
                .filter(|(at, _)| *at >= start && *at < end)
                .map(|(at, event)| match *event {
                    NativeInstrumentEvent::NoteOn {
                        occurrence_id,
                        pitch,
                        velocity,
                        ..
                    } => NativeInstrumentEvent::NoteOn {
                        frame_offset: at - start,
                        occurrence_id,
                        pitch,
                        velocity,
                    },
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        NativeInstrumentEvent::NoteOff {
                            frame_offset: at - start,
                            occurrence_id,
                        }
                    }
                    NativeInstrumentEvent::NotePressure { .. } => {
                        panic!("unexpected test note pressure")
                    }
                    NativeInstrumentEvent::Expression { expression, .. } => {
                        NativeInstrumentEvent::Expression {
                            frame_offset: at - start,
                            expression,
                        }
                    }
                })
                .collect();
            out.extend(process(&mut r, &events, end - start));
        }
        assert!(r.is_quiescent());
        out
    }
    let expected = render(1);
    for partition in [17, 64, 127, 256] {
        assert_eq!(expected, render(partition));
    }
}
#[test]
fn decimator_rejects_folded_high_band_and_retains_low_band_and_finite_tail() {
    fn rms(frequency: f64) -> f64 {
        let mut d = quality::Decimator::default();
        let mut energy = 0.0;
        for at in 0..8192 {
            let sample = d.sample((std::f64::consts::TAU * frequency * at as f64).sin() as f32);
            if at >= 4096 {
                energy += f64::from(sample).powi(2);
            }
        }
        (energy / 4096.0).sqrt()
    }
    assert!(rms(0.03) > 0.7);
    assert!(rms(0.4) < 0.001, "high band must be attenuated by over56dB");
    let mut d = quality::Decimator::default();
    d.sample(1.0);
    for _ in 0..8192 {
        d.sample(0.0);
    }
    assert!(d.quiet());
}
#[test]
fn interpolation_and_antialias_switch_actual_held_pcm() {
    let s = settings();
    let mut r = runtime(s);
    process(&mut r, &[on(1, 69)], 80);
    let mut altered = s;
    altered.global.interpolation = false;
    altered.global.antialias = false;
    let mut target = runtime(altered);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    target.retarget_history_from(&r, proof.as_ref());
    assert_ne!(process(&mut r, &[], 128), process(&mut target, &[], 128));
}
#[test]
fn mono_different_pitch_is_legato_with_retrigger_enabled() {
    let mut s = settings();
    s.global.voices = 0;
    assert!(s.global.retrigger);
    let mut r = runtime(s);
    process(&mut r, &[on(1, 60)], 80);
    process(&mut r, &[on(2, 64)], 1);
    assert_eq!(r.voices[0].age, 81);
    assert_eq!(r.voices[0].envelope_age, 81.0);
    process(&mut r, &[off(2)], 1);
    assert_eq!(r.voices[0].age, 82);
    assert_eq!(r.voices[0].note.midi_origin(), (60, 100));
}
#[test]
fn fm_sideband_fold_is_reduced_by_decimator() {
    fn render(antialias: bool) -> Vec<StereoFrame> {
        let mut s = settings();
        s.fields[3] = 16.0;
        s.fields[4] = 4.0;
        s.fields[16] = 0.25;
        s.global.antialias = antialias;
        let mut r = runtime(s);
        process(&mut r, &[on(1, 69)], 1024);
        process(&mut r, &[], 48000)
    }
    fn amplitude(frames: &[StereoFrame], hz: f64) -> f64 {
        let (real, imaginary) =
            frames
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(r, i), (at, frame)| {
                    let angle = std::f64::consts::TAU * hz * at as f64 / 48000.0;
                    (
                        r + f64::from(frame[0]) * angle.cos(),
                        i + f64::from(frame[0]) * angle.sin(),
                    )
                });
        2.0 * real.hypot(imaginary) / frames.len() as f64
    }
    let naive = render(false);
    let filtered = render(true);
    let before = amplitude(&naive, 19400.0);
    let after = amplitude(&filtered, 19400.0);
    assert!(before > 0.05, "sideband fixture {before}");
    assert!(after < before * 0.1, "FM fold {before} -> {after}");
}
#[test]
fn filters_and_quality_are_finite_at_supported_sample_rate_extremes() {
    for rate in [8000, 48000, 192000] {
        let mut s = settings();
        s.filter
            .patch(ParameterId::from_static(20001), ParameterValue::Bool(true))
            .unwrap();
        s.modulation
            .patch(ParameterId::from_static(620), ParameterValue::Choice(5))
            .unwrap();
        s.modulation
            .patch(ParameterId::from_static(621), ParameterValue::Float(100.0))
            .unwrap();
        let factory = Factory::new(
            s,
            s.prepare_configuration(rate).unwrap(),
            crate::shared_sine_table(),
            rate,
        )
        .unwrap();
        let mut runtime = factory.create_runtime();
        runtime
            .preflight_events(&[on(1, 48)], 1000, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        let mut pcm = [[0.0; 2]; 1000];
        runtime.process_block(&[on(1, 48)], &mut pcm, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(pcm.iter().flatten().all(|v| v.is_finite()));
        assert!(pcm.iter().flatten().any(|v| *v != 0.0));
    }
}
#[test]
fn new_mutable_default_uses_serial_levels_as_the_fm_controls() {
    let baseline = OperatorSettings::defaults();
    assert!(baseline.global.active);
    assert_eq!(baseline.global.capacity(), 8);
    let mut first = runtime(baseline);
    let dry = process(&mut first, &[on(1, 60)], 512);
    assert!(dry.iter().any(|f| f[0] != 0.0));
    for operator in 1..4 {
        assert_eq!(baseline.fields[operator * 3 + 1], 0.0);
    }
    let mut enabled = baseline;
    enabled.fields[4] = 1.0;
    let mut second = runtime(enabled);
    let modulated = process(&mut second, &[on(1, 60)], 512);
    assert_ne!(dry, modulated);
    enabled.fields[1] = 0.0;
    let mut muted = runtime(enabled);
    assert!(
        process(&mut muted, &[on(1, 60)], 512)
            .iter()
            .all(|f| *f == [0.0; 2])
    );
}

#[test]
fn legacy_held_notes_opt_into_globals_with_original_midi_and_retire_stolen_ids() {
    let mut old = settings();
    old.global = global::Settings::legacy();
    let mut source = runtime(old);
    process(&mut source, &[on(40, 60), on(10, 64), on(30, 67)], 90);
    let mut new = old;
    new.global = global::Settings::default();
    new.global.voices = 0;
    new.global.key = 100.0;
    let mut target = runtime(new);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(target.can_retarget_history_from(&source, proof.as_ref()));
    target.retarget_history_from(&source, proof.as_ref());
    assert_eq!(
        active(&target),
        vec![40],
        "equal-age adoption has a stable occurrence-id tie break"
    );
    assert_eq!(target.voices[0].note.midi_origin(), (60, 100));
    assert_eq!(target.voices[0].envelope_age, 90.0);
    process(&mut target, &[off(40)], 1);
    assert_eq!(active(&target), vec![30]);
    process(&mut target, &[off(10), off(30)], 5000);
    assert!(target.is_quiescent());
}

#[test]
fn time_modulated_release_admission_matches_pcm_across_block_boundaries() {
    fn render(partition: usize) -> Vec<StereoFrame> {
        let mut s = settings();
        s.modulation
            .patch(ParameterId::from_static(620), ParameterValue::Choice(13))
            .unwrap();
        s.modulation
            .patch(ParameterId::from_static(621), ParameterValue::Float(-100.0))
            .unwrap();
        let mut r = runtime(s);
        process(&mut r, &[on(1, 60)], 10);
        process(&mut r, &[off(1)], 1);
        let mut pcm = Vec::new();
        for start in (0..4400).step_by(partition) {
            pcm.extend(process(&mut r, &[], partition.min(4400 - start)));
        }
        assert!(r.is_quiescent());
        r.preflight_events(&[on(1, 60)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        pcm
    }
    assert_eq!(render(1), render(127));
}

#[test]
fn algorithm_retarget_changes_held_graph_without_resetting_note_history() {
    let mut before = settings();
    before.fields[12..32].copy_from_slice(&algorithms::CATALOG[0].routing_fields());
    let mut source = runtime(before);
    process(&mut source, &[on(1, 60)], 90);
    let voice = source.voices[0];
    let mut after = before;
    after.fields[12..32].copy_from_slice(&algorithms::CATALOG[10].routing_fields());
    let mut target = runtime(after);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(target.can_retarget_history_from(&source, proof.as_ref()));
    target.retarget_history_from(&source, proof.as_ref());
    assert_eq!(target.voices[0].phases, voice.phases);
    assert_eq!(target.voices[0].envelope_age, voice.envelope_age);
    assert_eq!(target.voices[0].age, voice.age);
    assert_eq!(target.voices[0].id, voice.id);
    assert_ne!(
        process(&mut source, &[], 512),
        process(&mut target, &[], 512)
    );
    let mut incompatible = after;
    incompatible.fields[0] = 2.0;
    assert!(!runtime(incompatible).can_retarget_history_from(&target, proof.as_ref()));
    let old = super::super::tests::legacy_state();
    let selected = OPERATOR_METADATA
        .patch_parameter(&old, parameter::ALGORITHM, ParameterValue::Choice(1))
        .unwrap();
    assert!(
        OperatorSettings::from_state(&selected)
            .unwrap()
            .global
            .active
    );
}

#[test]
fn cross_rate_retarget_and_active_to_legacy_history_are_refused() {
    let s = settings();
    let mut source = runtime(s);
    process(&mut source, &[on(1, 60)], 30);
    let other_factory = Factory::new(
        s,
        s.prepare_configuration(96000).unwrap(),
        crate::shared_sine_table(),
        96000,
    )
    .unwrap();
    let other = other_factory.create_runtime();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        other_factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(!other.can_retarget_history_from(&source, proof.as_ref()));
    let mut old = s;
    old.global = global::Settings::legacy();
    let target = runtime(old);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("proof")
    };
    assert!(!target.can_retarget_history_from(&source, proof.as_ref()));
    source.reset();
    assert!(target.can_retarget_history_from(&source, proof.as_ref()));
    let state = s.to_state().unwrap();
    let legacy = OPERATOR_METADATA
        .patch_parameter(&state, parameter::GLOBAL_MODE, ParameterValue::Choice(0))
        .unwrap();
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(&legacy, parameter::GLOBAL_MODE)
            .unwrap(),
        ParameterValue::Choice(0)
    );
    for id in [601, 602, 603, 604, 605, 606, 607, 620, 621, 639] {
        assert!(
            !OPERATOR_METADATA
                .parameter_enabled(&legacy, ParameterId::from_static(id))
                .unwrap()
        );
    }
    assert!(
        OPERATOR_METADATA
            .parameter_enabled(&legacy, parameter::GLOBAL_MODE)
            .unwrap()
    );
    assert!(
        OPERATOR_METADATA
            .parameter_enabled(&legacy, parameter::ALGORITHM)
            .unwrap()
    );
}

#[test]
fn sparse_history_copy_matches_full_copy_after_voice_reuse() {
    for global in [false, true] {
        let mut settings = settings();
        settings.global.active = global;
        let mut source = runtime(settings);
        process(&mut source, &[on(1, 60), on(2, 64)], 80);
        process(&mut source, &[off(2)], 8);
        // A released slot can leave a hole before a retained voice.
        source.voices.swap(1, 3);
        let mut actual = runtime(settings);
        let mut twin = runtime(settings);
        let mut full_copy = runtime(settings);
        for target in [&mut actual, &mut twin, &mut full_copy] {
            process(target, &[on(10, 48), on(11, 50), on(12, 52)], 24);
            target.copy_history_from(&source);
        }
        // Independent reference copies every slot, including empty ones.
        full_copy.voices.copy_from_slice(&source.voices);
        assert_eq!(active(&actual), active(&full_copy));
        assert_eq!(active(&twin), active(&full_copy));
        for events in [&[on(3, 67)][..], &[off(1)][..], &[][..]] {
            let expected = process(&mut full_copy, events, 128);
            assert_eq!(process(&mut actual, events, 128), expected);
            assert_eq!(process(&mut twin, events, 128), expected);
        }
    }
}
