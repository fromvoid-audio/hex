use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};

fn view(pcm: &[f32]) -> DeviceResourceView<'_> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(pcm),
    }
}
fn prepare(view: DeviceResourceView<'_>, mode: Mode<'_>, voices: usize) -> PreparedSamplerRuntime {
    let mut settings = Settings::new(Region {
        start: 0,
        end: view.pcm.frames(),
    });
    settings.mode = mode;
    PreparedSamplerPlan::new(
        SampleShape::new(view.pcm.frames(), 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(voices, 259, &hex_lattice::TuningTable::default())
    .unwrap()
}
fn pair(prepared: &PreparedSamplerRuntime) -> NativeInstrumentRuntimePair {
    NativeInstrumentRuntimePair::new(
        prepared,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: MAX_SAMPLER_RUNTIME_VOICES,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap()
}
fn on(frame_offset: usize, occurrence_id: u64, pitch: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset,
        occurrence_id,
        pitch,
        velocity: 127,
    }
}
fn off(frame_offset: usize, occurrence_id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset,
        occurrence_id,
    }
}
fn process(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    output: &mut [StereoFrame],
    resources: &dyn DeviceResources,
) {
    pair.begin_block();
    pair.process_block(events, output, resources).unwrap();
    pair.commit_block();
}

#[test]
fn actual_mutable_pair_keeps_trigger_audio_after_note_off_and_retires_at_source_end() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        4,
    );
    assert!(prepared.declared_twin_bytes() * 2 < 2 * 1024 * 1024);
    assert_eq!(prepared.max_events_per_block(), 259);
    let mut whole = pair(&prepared);
    let mut output = [[0.0; 2]; 12];
    process(
        &mut whole,
        &[on(0, 1, 60), off(1, 1)],
        &mut output,
        &resources,
    );
    for (actual, expected) in output[..8].iter().zip(pcm) {
        assert_eq!(*actual, [expected; 2]);
    }
    assert_eq!(output[8..], [[0.0; 2]; 4]);
    assert!(whole.committed_is_quiescent());
    // The actual twin is quiescent after source exhaustion, not MIDI note-off.
    let mut split = pair(&prepared);
    let mut partitioned = [[0.0; 2]; 12];
    process(
        &mut split,
        &[on(0, 1, 60), off(1, 1)],
        &mut partitioned[..2],
        &resources,
    );
    assert!(!split.committed_is_quiescent());
    process(&mut split, &[], &mut partitioned[2..7], &resources);
    assert!(!split.committed_is_quiescent());
    process(&mut split, &[], &mut partitioned[7..], &resources);
    assert!(split.committed_is_quiescent());
    assert_eq!(output, partitioned);
}

#[test]
fn capacity_refusal_and_speculative_rollback_leave_actual_pair_unchanged() {
    let pcm = [1.0, 3.0, 5.0, 7.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(views[0], Mode::Classic { loop_region: None }, 1);
    let mut runtime = pair(&prepared);
    let mut untouched = [[9.0; 2]; 2];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block(&[on(0, 1, 60), on(0, 2, 60)], &mut untouched, &resources),
        Err(Error::EventCapacityExceeded)
    );
    assert_eq!(untouched, [[9.0; 2]; 2]);
    runtime.rollback_block();
    let mut speculative = [[0.0; 2]; 2];
    runtime.begin_block();
    runtime
        .process_block(&[on(0, 1, 60)], &mut speculative, &resources)
        .unwrap();
    runtime.rollback_block();
    let mut committed = [[0.0; 2]; 2];
    process(&mut runtime, &[on(0, 1, 60)], &mut committed, &resources);
    assert_eq!(speculative, committed);
    assert_eq!(committed, [[1.0; 2], [3.0; 2]]);
    // Equal-offset gate-off frees a zero-release slot before the next onset.
    committed.fill([0.0; 2]);
    process(
        &mut runtime,
        &[off(0, 1), on(0, 2, 60)],
        &mut committed,
        &resources,
    );
    assert_eq!(committed, [[1.0; 2], [3.0; 2]]);
    assert!(!runtime.committed_is_quiescent());
    runtime.reset();
    assert!(runtime.committed_is_quiescent());
    committed.fill([0.0; 2]);
    process(&mut runtime, &[], &mut committed, &resources);
    assert_eq!(committed, [[0.0; 2]; 2]);
}

#[test]
fn slice_selection_and_source_refusal_use_original_pcm_through_native_runtime() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::Slice {
            starts: &[0, 3, 5],
            base_note: 60,
            gate: GateMode::Trigger,
        },
        4,
    );
    let mut runtime = pair(&prepared);
    let mut wrong = views;
    wrong[0].identity.content[31] ^= 1;
    let mut output = [[9.0; 2]; 4];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block(
            &[on(0, 1, 61)],
            &mut output,
            &BorrowedDeviceResources(&wrong)
        ),
        Err(Error::ResourceUnavailable)
    );
    runtime.rollback_block();
    assert_eq!(output, [[9.0; 2]; 4]);
    output.fill([0.0; 2]);
    process(
        &mut runtime,
        &[on(0, 1, 61), off(0, 1)],
        &mut output,
        &resources,
    );
    assert_eq!(output, [[7.0; 2], [9.0; 2], [0.0; 2], [0.0; 2]]);
}

#[test]
fn project_tuning_velocity_amount_and_gated_release_reach_actual_mutable_samples() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    frequencies[61] = frequencies[60] * 1.5;
    let tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    let mut settings = Settings::new(Region { start: 0, end: 6 });
    // A looping source cannot retire naturally: note-off plus release must do it.
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 0, end: 6 }),
    };
    settings.velocity_amount = 0.0;
    settings.envelope.release_seconds = 2.0 / 8_000.0;
    let bound = PreparedSamplerPlan::new(
        SampleShape::new(6, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap();
    let prepared = bound.prepare_runtime_on_control(4, 259, &tuning).unwrap();
    assert_eq!(prepared.max_release_tail_frames(), 2);
    let mut runtime = pair(&prepared);
    let note = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 61,
        velocity: 1,
    };
    let mut output = [[0.0; 2]; 6];
    process(&mut runtime, &[note, off(2, 1)], &mut output, &resources);
    assert_eq!(
        output,
        [[1.0; 2], [4.0; 2], [7.0; 2], [5.0; 2], [0.0; 2], [0.0; 2]]
    );
    assert!(runtime.committed_is_quiescent());
    let original = prepared.create_runtime();
    let same = prepared.create_runtime();
    assert!(same.can_copy_history_from(original.as_ref()));
    let neutral = bound
        .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
        .unwrap()
        .create_runtime();
    assert!(!neutral.can_copy_history_from(original.as_ref()));
}

#[test]
fn stereo_channels_and_velocity_depth_reach_the_actual_pair() {
    let pcm = [[1.0, -2.0], [3.0, -4.0], [5.0, -6.0], [7.0, -8.0]];
    let mut source = view(&[]);
    source.pcm = DeviceResourcePcm::Stereo(&pcm);
    let views = [source];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region {
        start: 0,
        end: pcm.len(),
    });
    settings.velocity_amount = 1.0;
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(pcm.len(), 8_000, SampleChannels::Stereo).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(source)
    .unwrap()
    .prepare_runtime_on_control(1, 259, &hex_lattice::TuningTable::default())
    .unwrap();
    let mut runtime = pair(&prepared);
    let mut note = on(0, 1, 60);
    if let NativeInstrumentEvent::NoteOn { velocity, .. } = &mut note {
        *velocity = 64;
    }
    let mut output = [[0.25; 2]; 4];
    process(&mut runtime, &[note], &mut output, &resources);
    for (actual, sample) in output.iter().zip(pcm) {
        for channel in 0..2 {
            let expected = (0.25 + f64::from(sample[channel]) * 64.0 / 127.0) as f32;
            assert!((actual[channel] - expected).abs() < 0.000_001);
        }
    }
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn preparation_and_pair_caller_limits_refuse_without_rendering() {
    let pcm = [1.0; 4];
    let source = view(&pcm);
    let bound = PreparedSamplerPlan::new(
        SampleShape::new(4, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        Settings::new(Region { start: 0, end: 4 }),
    )
    .unwrap()
    .bind_source_on_control(source)
    .unwrap();
    let tuning = hex_lattice::TuningTable::default();
    for (voices, events) in [(0, 1), (33, 1), (1, 0), (1, 1025)] {
        assert!(
            bound
                .prepare_runtime_on_control(voices, events, &tuning)
                .is_err()
        );
    }
    let prepared = bound.prepare_runtime_on_control(32, 1024, &tuning).unwrap();
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: prepared.declared_twin_bytes() * 2,
        max_events_per_block: 1024,
        max_active_occurrences: 32,
        max_tail_frames: 0,
    };
    assert!(NativeInstrumentRuntimePair::new(&prepared, limits).is_ok());
    for refused in [
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: limits.max_pair_bytes - 1,
            ..limits
        },
        NativeInstrumentRuntimeLimits {
            max_events_per_block: 1023,
            ..limits
        },
        NativeInstrumentRuntimeLimits {
            max_active_occurrences: 31,
            ..limits
        },
    ] {
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&prepared, refused),
            Err(Error::RuntimeBudgetExceeded)
        ));
    }
}

#[test]
fn absolute_project_retune_moves_authored_root_but_not_slice_selection() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    // Uniform octave retuning must move the authored root itself. Expected PCM
    // is literal source indices, independent of the coefficient implementation.
    for hz in &mut frequencies {
        *hz *= 2.0;
    }
    let tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    for (mode, expected) in [
        (Mode::Classic { loop_region: None }, [1.0, 5.0, 9.0, 0.0]),
        (
            Mode::OneShot {
                gate: GateMode::Trigger,
            },
            [1.0, 5.0, 9.0, 0.0],
        ),
        (
            Mode::Slice {
                starts: &[0, 3],
                base_note: 60,
                gate: GateMode::Trigger,
            },
            [1.0, 3.0, 5.0, 0.0],
        ),
    ] {
        let mut settings = Settings::new(Region {
            start: 0,
            end: pcm.len(),
        });
        settings.mode = mode;
        let prepared = PreparedSamplerPlan::new(
            SampleShape::new(pcm.len(), 8_000, SampleChannels::Mono).unwrap(),
            8_000,
            settings,
        )
        .unwrap()
        .bind_source_on_control(views[0])
        .unwrap()
        .prepare_runtime_on_control(1, 259, &tuning)
        .unwrap();
        let mut runtime = pair(&prepared);
        let mut output = [[0.0; 2]; 4];
        process(&mut runtime, &[on(0, 1, 60)], &mut output, &resources);
        assert_eq!(output, expected.map(|sample| [sample; 2]));
        assert!(runtime.committed_is_quiescent());
    }
}

#[test]
fn occurrence_ramp_preserves_midway_onset_release_other_voice_and_arbitrary_partitions() {
    let pcm = [1.0; 32];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        4,
    );
    let ramp =
        NativeInstrumentOccurrenceGain::new(1, 1.0 / 3.0, hex_lattice::INSTRUMENT_GAIN_UNITY, 7, 0)
            .unwrap();
    let mut whole = pair(&prepared);
    assert!(whole.supports_occurrence_gain());
    let mut output = [[0.125; 2]; 7];
    whole.begin_block();
    whole
        .process_block_with_clock_and_occurrence_gains(
            &[on(0, 2, 60), on(2, 1, 60), off(3, 1)],
            &mut output,
            None,
            &[ramp],
            &resources,
        )
        .unwrap();
    whole.commit_block();
    // Independent constant-source oracle. The unlisted voice and caller's
    // existing PCM remain untouched; a NoteOn at frame2 uses ramp phase2/7.
    let expected = [
        1.125,
        1.125,
        1.125 + 11.0 / 21.0,
        1.125 + 13.0 / 21.0,
        1.125 + 15.0 / 21.0,
        1.125 + 17.0 / 21.0,
        1.125 + 19.0 / 21.0,
    ];
    for (actual, expected) in output.iter().zip(expected) {
        for channel in actual {
            assert!((f64::from(*channel) - expected).abs() < 0.000001);
        }
    }
    // The prepared boundary gain 1/3 and split position 3/7 are not Q16.16
    // values. Retain original f64 endpoints and require identical split PCM.
    let mut split = pair(&prepared);
    let mut partitioned = [[0.125; 2]; 7];
    split.begin_block();
    split
        .process_block_with_clock_and_occurrence_gains(
            &[on(0, 2, 60), on(2, 1, 60)],
            &mut partitioned[..3],
            None,
            &[ramp],
            &resources,
        )
        .unwrap();
    split.commit_block();
    split.begin_block();
    split
        .process_block_with_clock_and_occurrence_gains(
            &[off(0, 1)],
            &mut partitioned[3..],
            None,
            &[ramp.with_offset_frames(3).unwrap()],
            &resources,
        )
        .unwrap();
    split.commit_block();
    assert_eq!(partitioned, output);
    // Gain is borrowed for this call only. Neither voice's velocity/history was
    // rewritten; ordinary continuation is unity even for the released trigger.
    let mut continuing = [[0.0; 2]; 2];
    process(&mut whole, &[], &mut continuing, &resources);
    assert_eq!(continuing, [[2.0; 2]; 2]);
    let mut inherited = pair(&prepared);
    inherited.inherit_from(&whole).unwrap();
    let mut next = [[0.0; 2]; 2];
    let mut copied = next;
    process(&mut whole, &[], &mut next, &resources);
    process(&mut inherited, &[], &mut copied, &resources);
    assert_eq!(next, copied);
}

#[test]
fn occurrence_gain_refusal_after_a_successful_segment_preserves_scratch_and_pcm() {
    let pcm = [1.0; 32];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        4,
    );
    let gain = |id| {
        NativeInstrumentOccurrenceGain::new(id, 0.0, hex_lattice::INSTRUMENT_GAIN_UNITY, 7, 0)
            .unwrap()
    };
    let mut runtime = pair(&prepared);
    runtime.begin_block();
    let mut first = [[0.0; 2]; 3];
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &[on(0, 1, 60)],
            &mut first,
            None,
            &[gain(1)],
            &resources,
        )
        .unwrap();
    let mut refused = [[9.0; 2]; 4];
    for gains in [
        vec![gain(9)],
        vec![gain(1), gain(1)],
        vec![gain(2), gain(1)],
        vec![gain(1).with_offset_frames(6).unwrap()],
    ] {
        assert_eq!(
            runtime.process_block_with_clock_and_occurrence_gains(
                &[],
                &mut refused,
                None,
                &gains,
                &resources
            ),
            Err(Error::InvalidOccurrenceGain)
        );
        assert_eq!(refused, [[9.0; 2]; 4]);
    }
    let too_many: Vec<_> = (1..=264).map(gain).collect();
    assert_eq!(
        runtime.process_block_with_clock_and_occurrence_gains(
            &[],
            &mut refused,
            None,
            &too_many,
            &resources
        ),
        Err(Error::EventCapacityExceeded)
    );
    assert_eq!(refused, [[9.0; 2]; 4]);
    let mut second = [[0.0; 2]; 4];
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &[],
            &mut second,
            None,
            &[gain(1).with_offset_frames(3).unwrap()],
            &resources,
        )
        .unwrap();
    for (index, sample) in second.iter().enumerate() {
        assert_eq!(*sample, [((index + 3) as f64 / 7.0) as f32; 2]);
    }
    runtime.rollback_block();
    assert!(runtime.committed_is_quiescent());
    let mut after = [[0.0; 2]; 2];
    process(&mut runtime, &[], &mut after, &resources);
    assert_eq!(after, [[0.0; 2]; 2]);
}

#[test]
fn occurrence_gain_scales_actual_gated_release_without_touching_the_other_voice() {
    let pcm = [1.0; 32];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 0, end: 32 }),
    };
    settings.envelope.release_seconds = 4.0 / 8_000.0;
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(32, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
    .unwrap();
    let mut runtime = pair(&prepared);
    let gain = NativeInstrumentOccurrenceGain::new(
        1,
        hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
        hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
        7,
        0,
    )
    .unwrap();
    let mut output = [[0.0; 2]; 7];
    runtime.begin_block();
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &[on(0, 1, 60), on(0, 2, 60), off(2, 1)],
            &mut output,
            None,
            &[gain],
            &resources,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(
        output,
        [
            [1.5; 2], [1.5; 2], [1.5; 2], [1.375; 2], [1.25; 2], [1.125; 2], [1.0; 2]
        ]
    );
    let mut other_release = [[0.0; 2]; 5];
    runtime.begin_block();
    runtime
        .process_block(&[off(0, 2)], &mut other_release[..1], &resources)
        .unwrap();
    let mut active = [0; 4];
    assert_eq!(runtime.active_occurrence_ids(&mut active), Ok(1));
    assert_eq!(
        active[0], 2,
        "released voice remains live until its tail retires"
    );
    runtime
        .process_block(&[], &mut other_release[1..], &resources)
        .unwrap();
    assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
    runtime.commit_block();
    assert_eq!(
        other_release,
        [[1.0; 2], [0.75; 2], [0.5; 2], [0.25; 2], [0.0; 2]]
    );
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn gain_batch_can_cover_more_incoming_identities_than_simultaneously_active_voices() {
    let pcm = [1.0; 32];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(views[0], Mode::Classic { loop_region: None }, 4);
    let mut runtime = pair(&prepared);
    let mut events = Vec::new();
    let mut gains = Vec::new();
    for id in 1..=64 {
        events.push(on(0, id, 60));
        if id < 64 {
            events.push(off(0, id));
        }
        gains.push(
            NativeInstrumentOccurrenceGain::new(
                id,
                hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
                hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
                7,
                0,
            )
            .unwrap(),
        );
    }
    let mut output = [[0.0; 2]; 7];
    runtime.begin_block();
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &events,
            &mut output,
            None,
            &gains,
            &resources,
        )
        .unwrap();
    runtime.commit_block();
    assert_eq!(output, [[0.5; 2]; 7]);
}

#[test]
fn exact_active_occurrences_drop_one_short_voice_while_another_survives_conservative_tail() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::Slice {
            starts: &[0, 2],
            base_note: 60,
            gate: GateMode::Trigger,
        },
        4,
    );
    assert!(prepared.supports_active_occurrence_ids());
    assert!(
        prepared.max_release_tail_frames() > 2,
        "a host factory tail is not an exact per-voice lifetime"
    );
    let mut runtime = pair(&prepared);
    let mut ids = [u64::MAX; 6];
    assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
    let notes = [on(0, 90, 60), on(0, 7, 61)];
    runtime.begin_block();
    let mut first = [[0.0; 2]; 1];
    runtime
        .process_block(&notes, &mut first, &resources)
        .unwrap();
    assert_eq!(first, [[6.0; 2]]);
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(2));
    assert_eq!(ids, [7, 90, u64::MAX, u64::MAX, u64::MAX, u64::MAX]);
    let before = ids;
    assert_eq!(
        runtime.active_occurrence_ids(&mut ids[..1]),
        Err(Error::ActiveOccurrenceCapacityExceeded)
    );
    assert_eq!(
        ids, before,
        "sampler refuses short output before any partial write"
    );
    let mut second = [[0.0; 2]; 1];
    runtime.process_block(&[], &mut second, &resources).unwrap();
    assert_eq!(second, [[10.0; 2]]);
    assert_eq!(runtime.active_occurrence_ids(&mut ids[..1]), Ok(1));
    assert_eq!(
        ids[0], 7,
        "short slice retired at2 while long slice remains; no whole-runtime quiescence shortcut"
    );
    assert!(!runtime.speculative_is_quiescent());
    runtime.rollback_block();
    assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
    let mut replay = [[0.0; 2]; 2];
    process(&mut runtime, &notes, &mut replay, &resources);
    assert_eq!(
        replay,
        [[6.0; 2], [10.0; 2]],
        "query and rollback preserve literal PCM"
    );
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 7);
    assert!(!runtime.committed_is_quiescent());
    let mut inherited = pair(&prepared);
    inherited.inherit_from(&runtime).unwrap();
    assert_eq!(inherited.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 7);
    let gain = |id| NativeInstrumentOccurrenceGain::new(id, 0.5, 0.5, 4, 0).unwrap();
    runtime.begin_block();
    let mut refused = [[9.0; 2]; 4];
    assert_eq!(
        runtime.process_block_with_clock_and_occurrence_gains(
            &[],
            &mut refused,
            None,
            &[gain(7), gain(90)],
            &resources
        ),
        Err(Error::InvalidOccurrenceGain)
    );
    assert_eq!(
        refused, [[9.0; 2]; 4],
        "conservative retained ID90 must still be refused"
    );
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(1));
    let mut actual = [[0.0; 2]; 4];
    runtime
        .process_block_with_clock_and_occurrence_gains(
            &[],
            &mut actual,
            None,
            &[gain(7)],
            &resources,
        )
        .unwrap();
    runtime.commit_block();
    inherited.begin_block();
    let mut copied = [[0.0; 2]; 4];
    inherited
        .process_block_with_clock_and_occurrence_gains(
            &[],
            &mut copied,
            None,
            &[gain(7)],
            &resources,
        )
        .unwrap();
    inherited.commit_block();
    assert_eq!(
        actual,
        [[4.5; 2], [5.5; 2], [6.5; 2], [7.5; 2]],
        "only exact surviving ID requires a gain; velocity/source law remains literal"
    );
    assert_eq!(copied, actual);
    assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
    assert!(runtime.committed_is_quiescent());
    runtime.reset();
    assert_eq!(runtime.active_occurrence_ids(&mut []), Ok(0));
}

fn prepare_retarget_settings(
    source: DeviceResourceView<'_>,
    settings: Settings<'_>,
    voices: usize,
    events: usize,
    tuning: &hex_lattice::TuningTable,
) -> PreparedSamplerRuntime {
    let channels = match source.pcm {
        DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
        DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
    };
    PreparedSamplerPlan::new(
        SampleShape::new(source.pcm.frames(), source.sample_rate, channels).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(source)
    .unwrap()
    .prepare_runtime_on_control(voices, events, tuning)
    .unwrap()
}

fn target_proof(prepared: &PreparedSamplerRuntime) -> Box<dyn PreparedInstrumentHistoryRetarget> {
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        prepared.prepare_history_retarget_target().unwrap()
    else {
        panic!("sampler target proof")
    };
    proof
}

#[test]
fn gain_and_ads_retarget_preserves_occurrence_velocity_source_age_and_gate() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    settings.envelope.release_seconds = 4.0 / 8_000.0;
    let tuning = hex_lattice::TuningTable::default();
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let mut source = pair(&original);
    process(
        &mut source,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 7,
            pitch: 60,
            velocity: 64,
        }],
        &mut [[0.0; 2]; 4],
        &resources,
    );
    settings.gain = 2.0;
    settings.pan = 0.5;
    settings.envelope.attack_seconds = 2.0 / 8_000.0;
    settings.envelope.decay_seconds = 4.0 / 8_000.0;
    settings.envelope.sustain = 0.25;
    let target = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let proof = target_proof(&target);
    // Both actual twins come from a clone of this exact prepared target.
    let mut candidate = pair(&target.clone());
    assert!(!candidate.can_inherit_from(&source));
    assert!(candidate.can_retarget_from(&source, proof.as_ref()));
    candidate.retarget_from(&source, proof.as_ref()).unwrap();
    let mut ids = [0; MAX_SAMPLER_RUNTIME_VOICES];
    assert_eq!(candidate.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 7);
    let mut held = [[0.0; 2]; 2];
    process(&mut candidate, &[], &mut held, &resources);
    let velocity = |value: f64| (value * (64.0 / 127.0)) as f32;
    assert_eq!(
        held,
        [
            [velocity(3.125), velocity(6.25)],
            [velocity(2.625), velocity(5.25)],
        ],
        "new ADS/gain law starts at carried age4, not a fresh attack or source cursor"
    );
    let mut released = [[0.0; 2]; 2];
    process(&mut candidate, &[off(0, 7)], &mut released, &resources);
    assert_eq!(
        released,
        [
            [velocity(1.75), velocity(3.5)],
            [velocity(1.5), velocity(3.0)]
        ]
    );
    // Carry a releasing voice again: its original gate age6 remains fixed.
    let mut carried_release = pair(&target);
    carried_release
        .retarget_from(&candidate, proof.as_ref())
        .unwrap();
    let mut tail = [[0.0; 2]; 4];
    process(&mut carried_release, &[], &mut tail, &resources);
    assert_eq!(
        tail,
        [
            [velocity(0.125), velocity(0.25)],
            [velocity(0.125), velocity(0.25)],
            [0.0; 2],
            [0.0; 2],
        ]
    );
    assert!(carried_release.committed_is_quiescent());
    let mut untouched = [[0.0; 2]; 2];
    process(&mut source, &[], &mut untouched, &resources);
    assert_eq!(untouched, [[velocity(5.0); 2], [velocity(6.0); 2]]);
}

#[test]
fn exact_copy_ignores_preparation_nonce_but_retarget_proof_is_exact_and_unforgeable() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let source_view = view(&pcm);
    let original = prepare(source_view, Mode::Classic { loop_region: None }, 4);
    let independent = prepare(source_view, Mode::Classic { loop_region: None }, 4);
    assert!(original == independent);
    assert!(original.configuration != independent.configuration);
    let source = original.create_runtime();
    let candidate = independent.create_runtime();
    assert!(candidate.can_copy_history_from(source.as_ref()));
    let wrong = target_proof(&original);
    assert!(!candidate.can_retarget_history_from(source.as_ref(), wrong.as_ref()));
    let own = target_proof(&independent);
    assert_eq!(
        PreparedSamplerRuntime::history_retarget_inline_bytes(own.as_ref()),
        Some(8)
    );
    assert!(candidate.can_retarget_history_from(source.as_ref(), own.as_ref()));
    let clone = independent.clone().create_runtime();
    assert!(clone.can_retarget_history_from(source.as_ref(), own.as_ref()));

    // An equally small outer type cannot borrow the genuine proof's as_any
    // projection to masquerade as the provider's audited scalar allocation.
    struct Projection(SamplerHistoryRetarget);
    impl PreparedInstrumentHistoryRetarget for Projection {
        fn declared_fixed_storage_bytes(&self) -> usize {
            8
        }
        fn as_any(&self) -> &dyn Any {
            &self.0
        }
    }
    let target = own
        .as_any()
        .downcast_ref::<SamplerHistoryRetarget>()
        .unwrap();
    let projected = Projection(SamplerHistoryRetarget(target.0));
    assert_eq!(std::mem::size_of_val(&projected), 8);
    assert_eq!(
        PreparedSamplerRuntime::history_retarget_inline_bytes(&projected),
        None
    );
    assert!(!candidate.can_retarget_history_from(source.as_ref(), &projected));
}

#[test]
fn retarget_refuses_effective_coordinate_source_velocity_release_and_budget_changes_atomically() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let source_view = view(&pcm);
    let views = [source_view];
    let resources = BorrowedDeviceResources(&views);
    let tuning = hex_lattice::TuningTable::default();
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    settings.envelope.release_seconds = 4.0 / 8_000.0;
    let original = prepare_retarget_settings(source_view, settings, 4, 259, &tuning);
    let mut source = pair(&original);
    process(&mut source, &[on(0, 7, 60)], &mut [[0.0; 2]; 4], &resources);
    let mut cases = Vec::new();
    let mut push =
        |name, changed, source_view, voices, events, tuning: &hex_lattice::TuningTable| {
            cases.push((
                name,
                prepare_retarget_settings(source_view, changed, voices, events, tuning),
                source_view,
            ));
        };
    let mut changed_source = source_view;
    changed_source.identity.content[0] ^= 1;
    push("source identity", settings, changed_source, 4, 259, &tuning);
    let mut changed = settings;
    changed.region = Region { start: 1, end: 8 };
    changed.mode = Mode::Classic {
        loop_region: Some(changed.region),
    };
    push("region", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.reverse = true;
    push("reverse", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.mode = Mode::Classic {
        loop_region: Some(Region { start: 1, end: 7 }),
    };
    push("loop", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    push("gate", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.mode = Mode::Slice {
        starts: &[0, 4],
        base_note: 60,
        gate: GateMode::Gate,
    };
    push("note mask", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.envelope.release_seconds = 8.0 / 8_000.0;
    push("release", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.velocity_amount = 0.5;
    push("velocity", changed, source_view, 4, 259, &tuning);
    let mut frequencies = *tuning.frequencies();
    frequencies[60] *= 1.5;
    let altered_tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    push("pitch step", settings, source_view, 4, 259, &altered_tuning);
    push("voice capacity", settings, source_view, 3, 259, &tuning);
    push("event capacity", settings, source_view, 4, 258, &tuning);
    for (name, target, target_view) in cases {
        let proof = target_proof(&target);
        let mut candidate = pair(&target);
        assert!(
            !candidate.can_retarget_from(&source, proof.as_ref()),
            "{name}"
        );
        assert_eq!(
            candidate.retarget_from(&source, proof.as_ref()),
            Err(Error::IncompatibleHistory),
            "{name}"
        );
        assert_eq!(candidate.active_occurrence_ids(&mut []), Ok(0), "{name}");
        let target_views = [target_view];
        let mut quiet = [[0.0; 2]; 4];
        process(
            &mut candidate,
            &[],
            &mut quiet,
            &BorrowedDeviceResources(&target_views),
        );
        assert_eq!(quiet, [[0.0; 2]; 4], "{name}");
    }
    let mut unchanged = [[0.0; 2]; 4];
    process(&mut source, &[], &mut unchanged, &resources);
    assert_eq!(unchanged, [[5.0; 2], [6.0; 2], [7.0; 2], [8.0; 2]]);
}

#[test]
fn retarget_finished_one_shot_never_retriggers_without_a_new_note_on() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let tuning = hex_lattice::TuningTable::default();
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let mut old = pair(&original);
    process(&mut old, &[on(0, 1, 60)], &mut [[0.0; 2]; 12], &resources);
    assert!(old.committed_is_quiescent());
    settings.gain = 2.0;
    let target = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let mut next = pair(&target);
    next.retarget_from(&old, target_proof(&target).as_ref())
        .unwrap();
    let mut quiet = [[0.0; 2]; 8];
    process(&mut next, &[], &mut quiet, &resources);
    assert_eq!(quiet, [[0.0; 2]; 8]);
    assert!(next.committed_is_quiescent());
    let mut audible = [[0.0; 2]; 4];
    process(&mut next, &[on(0, 2, 60)], &mut audible, &resources);
    assert_eq!(audible, [[2.0; 2], [4.0; 2], [6.0; 2], [8.0; 2]]);
}
