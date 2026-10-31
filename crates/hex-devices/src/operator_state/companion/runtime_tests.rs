use super::*;
use crate::operator_state::harmonic_runtime::Factory;
use hex_lattice::{
    InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime, NativeInstrumentClock,
    NativeInstrumentEvent, PreparedMutableInstrumentRuntime,
};
#[derive(Clone, Copy)]
struct Clock {
    start: u64,
    step: u64,
    jump: bool,
    running: Option<bool>,
    origin: Option<u64>,
    only_origin: bool,
}
impl Clock {
    fn at(start: u64) -> Self {
        Self {
            start,
            step: 1 << 25,
            jump: false,
            running: Some(true),
            origin: Some(1000),
            only_origin: false,
        }
    }
}
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        if self.only_origin && offset != 0 {
            return None;
        }
        let frame = self.start.checked_add(offset as u64)?;
        frame
            .checked_mul(self.step)?
            .checked_add(if self.jump && frame >= 40 {
                1000 << 32
            } else {
                0
            })
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.origin?
            .checked_add(self.start)?
            .checked_add(offset as u64)
    }
    fn transport_is_running(&self) -> Option<bool> {
        self.running
    }
}
fn settings() -> OperatorSettings {
    let mut settings = crate::operator_state::tests::legacy_settings();
    for i in 0..4 {
        settings.fields[i * 3] = 1.0;
        settings.fields[i * 3 + 1] = if i == 0 { 1.0 } else { 0.0 };
        settings.fields[i * 3 + 2] = 0.125;
        let base = CORE_FIELDS + i * ENVELOPE_FIELDS;
        settings.fields[base..base + 6].copy_from_slice(&[0.1, 1.0, 0.4, 0.001, 0.002, 0.003]);
    }
    settings.fields[12..32].fill(0.0);
    settings.fields[28] = 1.0;
    settings
}
fn factory(settings: OperatorSettings) -> Factory {
    Factory::new(
        settings,
        settings.prepare_configuration(8000).unwrap(),
        crate::shared_sine_table(),
        8000,
    )
    .unwrap()
}
fn render(
    runtime: &mut dyn MutableInstrumentRuntime,
    events: &[NativeInstrumentEvent],
    frames: usize,
    clock: Option<&dyn NativeInstrumentClock>,
) -> Vec<[f32; 2]> {
    runtime
        .preflight_events(events, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    runtime
        .preflight_clock(clock, frames, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut pcm = vec![[0.0; 2]; frames];
    runtime.process_block_with_clock(
        events,
        &mut pcm,
        clock,
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    pcm
}
fn on(id: u64, offset: usize) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: offset,
        occurrence_id: id,
        pitch: 60,
        velocity: 127,
    }
}
fn off(id: u64, offset: usize) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset: offset,
        occurrence_id: id,
    }
}

#[test]
fn unsynchronized_envelopes_do_not_query_beat_or_transport_during_render() {
    use std::cell::Cell;
    struct CountingClock {
        beats: Cell<usize>,
        transport: Cell<usize>,
    }
    impl NativeInstrumentClock for CountingClock {
        fn beat_at_offset(&self, _offset: usize) -> Option<u64> {
            self.beats.set(self.beats.get() + 1);
            Some(0)
        }
        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(offset as u64)
        }
        fn transport_is_running(&self) -> Option<bool> {
            self.transport.set(self.transport.get() + 1);
            Some(true)
        }
    }
    let mut settings = settings();
    settings.companion.words[0][5] = 1;
    let factory = factory(settings);
    let mut runtime = factory.create_runtime();
    let clock = CountingClock {
        beats: Cell::new(0),
        transport: Cell::new(0),
    };
    let pcm = render(runtime.as_mut(), &[on(1, 0), off(1, 70)], 128, Some(&clock));
    assert!(pcm.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(clock.beats.get(), 0);
    assert_eq!(clock.transport.get(), 0);
}
#[test]
fn all_loop_modes_are_partition_exact_through_tempo_jumps_and_noteoff() {
    for mode in 0..=4 {
        let mut settings = settings();
        settings.companion.words[0][5] = mode;
        settings.companion.words[0][7] = 0;
        settings.companion.words[0][6] = 0.001_f32.to_bits();
        let factory = factory(settings);
        let mut whole = factory.create_runtime();
        let mut split = factory.create_runtime();
        let mut clock = Clock::at(0);
        clock.jump = true;
        let expected = render(whole.as_mut(), &[on(1, 0), off(1, 70)], 128, Some(&clock));
        let mut actual = Vec::new();
        let mut start = 0;
        for frames in [1, 7, 32, 1, 29, 1, 57] {
            let events = if start == 0 {
                vec![on(1, 0)]
            } else if start == 70 {
                vec![off(1, 0)]
            } else {
                Vec::new()
            };
            clock.start = start;
            actual.extend(render(split.as_mut(), &events, frames, Some(&clock)));
            start += frames as u64;
        }
        assert_eq!(expected, actual, "mode{mode}");
        assert!(whole.is_quiescent());
        assert!(split.is_quiescent());
    }
}
#[test]
fn one_frame_sync_clock_and_unknown_transport_are_distinct_admission_cases() {
    let mut settings = settings();
    settings.companion.words[0][5] = 3;
    let factory = factory(settings);
    let mut runtime = factory.create_runtime();
    runtime
        .preflight_events(&[on(1, 0)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert!(
        runtime
            .preflight_clock(None, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    assert!(runtime.is_quiescent());
    let mut clock = Clock::at(0);
    clock.only_origin = true;
    clock.running = None;
    assert!(
        runtime
            .preflight_clock(Some(&clock), 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    assert!(runtime.is_quiescent());
    clock.running = Some(false);
    let _ = render(runtime.as_mut(), &[on(1, 0)], 1, Some(&clock));
    assert!(!runtime.is_quiescent());
    assert!(
        runtime
            .preflight_events(&[on(1, 0)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
}
#[test]
fn free_run_uses_exact_host_origin_and_carries_explicit_local_origin_in_copies() {
    let mut settings = settings();
    settings.fields[32..38].copy_from_slice(&[1.0, 1.0, 1.0, 0.0, 0.001, 0.003]);
    settings.companion.words[0][0] = 0;
    let factory = factory(settings);
    let note = factory.configuration.prepare_note(60, 127).unwrap();
    let step = PreparedOperatorConfiguration::lfo_steps(&note)[0];
    let mut host = factory.create_runtime();
    let clock = Clock::at(0);
    let actual = render(host.as_mut(), &[on(1, 0)], 64, Some(&clock));
    for (age, sample) in actual.iter().enumerate() {
        let expected = crate::shared_sine_table()
            .sample_phase(step.wrapping_mul(1000 + age as u64).wrapping_add(1 << 61));
        assert_eq!(sample[0], expected);
    }
    let mut local = factory.create_runtime();
    render(local.as_mut(), &[], 113, None);
    let mut copied = factory.create_runtime();
    assert!(copied.can_copy_history_from(local.as_ref()));
    copied.copy_history_from(local.as_ref());
    let actual = render(local.as_mut(), &[on(1, 0)], 32, None);
    assert_eq!(actual, render(copied.as_mut(), &[on(1, 0)], 32, None));
    assert_eq!(
        actual[0][0],
        crate::shared_sine_table().sample_phase(step.wrapping_mul(113).wrapping_add(1 << 61))
    );
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("prepared target")
    };
    let mut retargeted = factory.create_runtime();
    assert!(retargeted.can_retarget_history_from(local.as_ref(), proof.as_ref()));
    retargeted.retarget_history_from(local.as_ref(), proof.as_ref());
    assert_eq!(
        render(local.as_mut(), &[], 32, None),
        render(retargeted.as_mut(), &[], 32, None)
    );
    copied.reset();
    assert!(copied.is_quiescent());
    assert_eq!(
        render(copied.as_mut(), &[on(2, 0)], 1, None)[0][0],
        crate::shared_sine_table().sample_phase(1 << 61)
    );
}
#[test]
fn trigger_keeps_silent_occurrences_until_their_noteoff_without_stealing_ids() {
    let mut settings = settings();
    for row in &mut settings.companion.words {
        row[5] = 4;
    }
    let factory = factory(settings);
    assert_eq!(factory.max_active_occurrences(), 64);
    assert_eq!(factory.max_release_tail_frames(), 1_120_000);
    let mut runtime = factory.create_runtime();
    let events: Vec<_> = (1..=64).map(|id| on(id, 0)).collect();
    let pcm = render(runtime.as_mut(), &events, 64, None);
    assert_eq!(pcm[63], [0.0; 2]);
    assert!(!runtime.is_quiescent());
    assert!(
        runtime
            .preflight_events(&[on(65, 0)], 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    let events: Vec<_> = (1..=64).map(|id| off(id, 0)).collect();
    render(runtime.as_mut(), &events, 1, None);
    assert!(runtime.is_quiescent());
    let mut early = factory.create_runtime();
    render(early.as_mut(), &[on(1, 0), off(1, 1)], 16, None);
    assert!(!early.is_quiescent());
    render(early.as_mut(), &[], 48, None);
    assert!(early.is_quiescent());
}
#[test]
fn changed_companion_law_refuses_active_history_and_leaves_source_pcm_untouched() {
    let mut settings = settings();
    settings.companion.words[0][0] = 0;
    let source = factory(settings);
    let mut old = source.create_runtime();
    render(old.as_mut(), &[on(1, 0)], 8, None);
    let mut oracle = source.create_runtime();
    oracle.copy_history_from(old.as_ref());
    settings.companion.words[0][3] = 0.5_f32.to_bits();
    let target = factory(settings);
    let next = target.create_runtime();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.prepare_history_retarget_target().unwrap()
    else {
        panic!("prepared target")
    };
    assert!(!next.can_retarget_history_from(old.as_ref(), proof.as_ref()));
    assert!(!next.can_copy_history_from(old.as_ref()));
    assert_eq!(
        render(old.as_mut(), &[], 32, None),
        render(oracle.as_mut(), &[], 32, None)
    );
}

#[test]
fn mixed_trigger_and_loop_release_admission_reuses_capacity_at_the_exact_finish_frame() {
    for (ordinary_release, gate, finish) in [(0.001, 1, 48), (0.01, 1, 81), (0.001, 64, 72)] {
        let mut settings = settings();
        settings.companion.words[0][5] = 4;
        for i in 1..4 {
            settings.companion.words[i][5] = 1;
            settings.fields[CORE_FIELDS + i * ENVELOPE_FIELDS + 5] = ordinary_release;
        }
        let factory = factory(settings);
        let mut runtime = factory.create_runtime();
        let notes: Vec<_> = (1..=64).map(|id| on(id, 0)).collect();
        render(runtime.as_mut(), &notes, gate, None);
        let mut oracle = factory.create_runtime();
        oracle.copy_history_from(runtime.as_ref());
        let mut events: Vec<_> = (1..=64).map(|id| off(id, 0)).collect();
        events.push(on(65, finish - gate - 1));
        assert_eq!(
            runtime.preflight_events(
                &events,
                finish - gate + 1,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            ),
            Err(hex_lattice::NativeInstrumentRuntimeError::EventCapacityExceeded),
            "a silent Trigger and a releasing loop must both retain their ids until completion"
        );
        events[64] = on(65, finish - gate);
        let mut duplicate = events.clone();
        duplicate.push(events[64]);
        assert_eq!(
            runtime.preflight_events(
                &duplicate,
                finish - gate + 1,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            ),
            Err(hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch)
        );
        // Both failed proofs and repeated successful proofs must leave all live
        // oscillator/filter/envelope history untouched.
        runtime
            .preflight_events(
                &events,
                finish - gate + 1,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        runtime
            .preflight_events(
                &events,
                finish - gate + 1,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        let mut expected = vec![[0.0; 2]; finish - gate + 1];
        oracle.process_block(&events, &mut expected, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(
            render(runtime.as_mut(), &events, expected.len(), None),
            expected
        );
        assert!(!runtime.is_quiescent());
        render(runtime.as_mut(), &[off(65, 0)], 128, None);
        assert!(runtime.is_quiescent());
    }
}
