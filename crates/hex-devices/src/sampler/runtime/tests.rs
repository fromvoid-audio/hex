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

/// Linear release level `steps` frames after note-off. Expectations derive
/// from the prepared release so a host declick floor changes only its length.
fn release_level(steps: u64, release: u64) -> f64 {
    if steps >= release {
        0.0
    } else {
        1.0 - steps as f64 / release as f64
    }
}

/// Reconstructed mono source at a literal playback position in source frames,
/// before gain and envelope: the cursor law under test supplies the position,
/// the plan's interpolation law (linear at or below unity, band-limited above)
/// supplies the value.
fn source(prepared: &PreparedSamplerRuntime, pitch: u8, pcm: &[f32], frames: f64) -> f64 {
    let position = (frames * ONE as f64).round() as u128;
    prepared.notes[usize::from(pitch)]
        .unwrap()
        .source_value::<true>(DeviceResourcePcm::Mono(pcm), position)
        .map_or(0.0, |value| value[0])
}

fn assert_frames_near(actual: &[StereoFrame], expected: &[[f64; 2]]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            let error = (f64::from(actual[channel]) - expected[channel]).abs();
            assert!(
                error <= 1e-6 * expected[channel].abs().max(1.0),
                "frame {index}: {actual:?} != {expected:?}"
            );
        }
    }
}

#[test]
fn prepared_sampler_gain_tracks_samples_and_committed_tail_at_zero_authored_gain() {
    struct GainValues<'a>(&'a [Option<f32>]);
    impl NativeParameterAutomation for GainValues<'_> {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[GAIN]
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            (id == GAIN)
                .then(|| self.0.get(offset).copied().flatten())
                .flatten()
                .map(ParameterValue::Float)
        }
        fn has_sample_rate_modulation(&self, id: ParameterId) -> bool {
            id == GAIN
        }
    }

    let pcm = [0.25, 0.5, 0.75, 1.0, 0.125, 0.25, 0.5, 0.75];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region {
        start: 0,
        end: pcm.len(),
    });
    settings.gain = 0.0;
    settings.pan = 0.25;
    let prepared = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(1.0));
    let mut runtime = pair(&prepared);
    let mut next = pair(&prepared);
    let read_target = |runtime: &NativeInstrumentRuntimePair| {
        let mut target = [None];
        runtime
            .copy_committed_parameter_targets(&[GAIN], &mut target)
            .unwrap();
        target[0].unwrap()
    };
    let events = [on(0, 1, 60), off(2, 1)];
    let mut output = [[0.0; 2]; 4];
    let render = |runtime: &mut NativeInstrumentRuntimePair,
                  events: &[NativeInstrumentEvent],
                  output: &mut [StereoFrame],
                  values: &[Option<f32>]| {
        runtime.process_block_with_clock_and_automation(
            events,
            output,
            None,
            &[],
            &GainValues(values),
            &resources,
        )
    };
    runtime.begin_block();
    let mut refused = [[0.125; 2]; 4];
    assert_eq!(
        render(
            &mut runtime,
            &events,
            &mut refused,
            &[None, Some(f32::NAN), None, None]
        ),
        Err(Error::InvalidParameterAutomation)
    );
    assert_eq!(refused, [[0.125; 2]; 4]);
    let values = [Some(0.0), Some(0.5), Some(2.0), Some(0.5)];
    render(&mut runtime, &events, &mut output, &values).unwrap();
    assert_eq!(read_target(&runtime), ParameterValue::Float(0.0));
    runtime.rollback_block();
    runtime.begin_block();
    let mut replay = [[0.0; 2]; 4];
    render(&mut runtime, &events, &mut replay, &values).unwrap();
    assert_eq!(replay, output);
    runtime.commit_block();
    assert_eq!(read_target(&runtime), ParameterValue::Float(0.5));
    let release = prepared.notes[60].unwrap().release_frames();
    assert_frames_near(
        &output,
        &[
            [0.0; 2],
            [0.1875, 0.25],
            [1.125, 1.5],
            [0.375, 0.5].map(|value| value * release_level(1, release)),
        ],
    );

    next.inherit_from(&runtime).unwrap();
    let ParameterValue::Float(frozen) = read_target(&next) else {
        panic!("gain target must be scalar");
    };
    assert_eq!(frozen, 0.5);
    next.begin_block();
    let mut tail = [[0.0; 2]; 2];
    render(&mut next, &[], &mut tail, &[Some(frozen), None]).unwrap();
    next.commit_block();
    assert_frames_near(
        &tail,
        &[
            [0.046875, 0.0625].map(|value| value * release_level(2, release)),
            [0.0; 2],
        ],
    );
    assert_eq!(read_target(&next), ParameterValue::Float(0.0));
    next.reset();
    assert_eq!(read_target(&next), ParameterValue::Float(0.0));
    assert!(next.committed_is_quiescent());
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
    // No release is zero: an equal-offset gate-off makes the held voice
    // stealable, so the next onset takes its slot while the old voice
    // continues its floored release under the steal declick.
    committed.fill([0.0; 2]);
    process(
        &mut runtime,
        &[off(0, 1), on(0, 2, 60)],
        &mut committed,
        &resources,
    );
    let release = crate::sampler::envelope::release_floor_frames(8_000);
    let fade = prepared.steal_fade_frames() as f64;
    let stolen = 7.0 * release_level(1, release) * (1.0 - 1.0 / fade);
    assert_frames_near(&committed, &[[1.0 + 5.0; 2], [3.0 + stolen; 2]]);
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
    let release = prepared.max_release_tail_frames();
    assert!(release >= 2);
    let mut runtime = pair(&prepared);
    let note = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 61,
        velocity: 1,
    };
    let frames = 2 + release as usize + 2;
    let mut output = vec![[0.0; 2]; frames];
    process(&mut runtime, &[note, off(2, 1)], &mut output, &resources);
    // Pitch 61 steps 1.5 source frames per output frame around the 6-frame
    // loop. Positions stay unwrapped so wrapped passes read loop history.
    let expected: Vec<_> = (0..frames as u64)
        .map(|age| {
            let level = age
                .checked_sub(2)
                .map_or(1.0, |steps| release_level(steps, release));
            [source(&prepared, 61, &pcm, age as f64 * 1.5) * level; 2]
        })
        .collect();
    assert_frames_near(&output, &expected);
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
    // The declared tail always includes the release floor, even when the
    // authored release is zero; a zero-tail host refuses below.
    assert_eq!(
        prepared.max_release_tail_frames(),
        crate::sampler::envelope::release_floor_frames(8_000)
    );
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: prepared.declared_twin_bytes() * 2,
        max_events_per_block: 1024,
        max_active_occurrences: 32,
        max_tail_frames: prepared.max_release_tail_frames(),
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
        NativeInstrumentRuntimeLimits {
            max_tail_frames: limits.max_tail_frames - 1,
            ..limits
        },
        NativeInstrumentRuntimeLimits {
            max_tail_frames: 0,
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
    // Uniform octave retuning must move the authored root itself. Expected
    // values are the reconstructed source at literal positions.
    for hz in &mut frequencies {
        *hz *= 2.0;
    }
    let tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    for (mode, step) in [
        (Mode::Classic { loop_region: None }, 2.0),
        (
            Mode::OneShot {
                gate: GateMode::Trigger,
            },
            2.0,
        ),
        (
            Mode::Slice {
                starts: &[0, 3],
                base_note: 60,
                gate: GateMode::Trigger,
            },
            1.0,
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
        if step == 1.0 {
            // Slices select regions, not pitch: unity linear is literal PCM.
            assert_eq!(output, [[1.0; 2], [3.0; 2], [5.0; 2], [0.0; 2]]);
        } else {
            // Octave: source positions 0, 2, 4, then the region end.
            let expected =
                [0.0, 2.0, 4.0].map(|position| [source(&prepared, 60, &pcm, position); 2]);
            assert_frames_near(&output[..3], &expected);
            assert_eq!(output[3], [0.0; 2]);
        }
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
    let release = prepared.max_release_tail_frames();
    assert!(release >= 4);
    let frames = 2 + release as usize + 1;
    let mut runtime = pair(&prepared);
    let gain = NativeInstrumentOccurrenceGain::new(
        1,
        hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
        hex_lattice::INSTRUMENT_GAIN_UNITY / 2.0,
        frames as u64,
        0,
    )
    .unwrap();
    let mut output = vec![[0.0; 2]; frames];
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
    let expected: Vec<_> = (0..frames as u64)
        .map(|age| {
            let level = age
                .checked_sub(2)
                .map_or(1.0, |steps| release_level(steps, release));
            [1.0 + 0.5 * level; 2]
        })
        .collect();
    assert_frames_near(&output, &expected);
    let mut other_release = vec![[0.0; 2]; release as usize + 1];
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
    let expected: Vec<_> = (0..=release)
        .map(|steps| [release_level(steps, release); 2])
        .collect();
    assert_frames_near(&other_release, &expected);
    assert!(runtime.committed_is_quiescent());
}

#[test]
fn occurrence_gain_follows_retired_slot_reuse_and_unlisted_voices() {
    let pcm = [1.0; 2];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        1,
    );
    let mut runtime = pair(&prepared);
    let gains = [
        NativeInstrumentOccurrenceGain::new(1, 0.25, 0.25, 6, 0).unwrap(),
        NativeInstrumentOccurrenceGain::new(2, 0.75, 0.75, 6, 0).unwrap(),
    ];
    let events = [on(0, 1, 60), on(2, 2, 60), on(4, 3, 60)];
    let mut output = [[0.0; 2]; 6];
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
    assert_eq!(
        output,
        [
            [0.25; 2], [0.25; 2], [0.75; 2], [0.75; 2], [1.0; 2], [1.0; 2]
        ]
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
    // Every released voice is stolen into the declick pool with its gain
    // frozen at the steal, so the whole mix is the unity-gain mix halved.
    let mut unity = [[0.0; 2]; 7];
    process(&mut pair(&prepared), &events, &mut unity, &resources);
    assert!(
        unity[0][0] > 1.0,
        "released voices still sound while stolen"
    );
    for (actual, unity) in output.iter().zip(unity) {
        assert_eq!(*actual, [unity[0] * 0.5, unity[1] * 0.5]);
    }
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
    let release = target.max_release_tail_frames();
    assert!(release >= 4);
    // Release ramps from the frozen sustain (0.25) at gate age6 over the
    // looping source, with left/right channel gains 1 and 2.
    let released_frame = |age: u64| {
        let level = 0.25
            * release_level(age - 6, release)
            * f64::from(pcm[(age % 8) as usize])
            * (64.0 / 127.0);
        [level, 2.0 * level]
    };
    let mut released = [[0.0; 2]; 2];
    process(&mut candidate, &[off(0, 7)], &mut released, &resources);
    assert_frames_near(&released, &[released_frame(6), released_frame(7)]);
    // Carry a releasing voice again: its original gate age6 remains fixed.
    let mut carried_release = pair(&target);
    carried_release
        .retarget_from(&candidate, proof.as_ref())
        .unwrap();
    let mut tail = vec![[0.0; 2]; release as usize];
    process(&mut carried_release, &[], &mut tail, &resources);
    let expected: Vec<_> = (8..8 + release).map(released_frame).collect();
    assert_frames_near(&tail, &expected);
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
    // Far beyond any prepared declick floor, so the release frames differ.
    changed.envelope.release_seconds = settings.envelope.release_seconds + 0.25;
    push("release", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.velocity_amount = 0.5;
    push("velocity", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.root_note = 61;
    push("authored root", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.transpose_semitones = 1.0;
    push("authored transpose", changed, source_view, 4, 259, &tuning);
    let mut changed = settings;
    changed.detune_cents = 15.0;
    push("authored detune", changed, source_view, 4, 259, &tuning);
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

fn ratio_tuning(ratio: f64) -> hex_lattice::TuningTable {
    let frequencies = hex_lattice::TuningTable::default()
        .frequencies()
        .map(|frequency| frequency * ratio);
    hex_lattice::TuningTable::new(frequencies).unwrap()
}

#[test]
fn gated_tuning_retargets_actual_cursor_fast_slow_and_cancel_without_replaying_prefix() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(1.0));
    let mut old = pair(&original);
    let mut prefix = [[0.0; 2]; 3];
    process(&mut old, &[on(0, 7, 60)], &mut prefix, &resources);
    assert_eq!(prefix, [[1.0; 2], [2.0; 2], [3.0; 2]]);

    let fast_factory = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(2.0));
    let mut fast = pair(&fast_factory);
    assert!(!fast.can_inherit_from(&old));
    fast.retarget_from(&old, target_proof(&fast_factory).as_ref())
        .unwrap();
    let mut fast_pcm = [[0.0; 2]; 4];
    process(&mut fast, &[], &mut fast_pcm, &resources);
    // Source cursor 3 survives; only future steps double (9 wraps to 1).
    let expected =
        [3.0, 5.0, 7.0, 9.0].map(|position| [source(&fast_factory, 60, &pcm, position); 2]);
    assert_frames_near(&fast_pcm, &expected);

    let slow_factory = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(0.5));
    let mut slow = pair(&slow_factory);
    slow.retarget_from(&fast, target_proof(&slow_factory).as_ref())
        .unwrap();
    let mut slow_pcm = [[0.0; 2]; 4];
    for output in slow_pcm.chunks_mut(1) {
        process(&mut slow, &[], output, &resources);
    }
    assert_eq!(
        slow_pcm,
        [[4.0; 2], [4.5; 2], [5.0; 2], [5.5; 2]],
        "fractional cursor11 survives the second retune"
    );

    let mut cancelled = pair(&original);
    cancelled
        .retarget_from(&slow, target_proof(&original).as_ref())
        .unwrap();
    let mut cancel_pcm = [[0.0; 2]; 2];
    process(&mut cancelled, &[], &mut cancel_pcm, &resources);
    assert_eq!(
        cancel_pcm,
        [[6.0; 2], [7.0; 2]],
        "cancel restores rate1 at actual cursor13, not envelope age11"
    );
    let mut ids = [0; 32];
    assert_eq!(cancelled.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 7);
    let mut donor = [[0.0; 2]; 3];
    process(&mut old, &[], &mut donor, &resources);
    assert_eq!(
        donor,
        [[4.0; 2], [5.0; 2], [6.0; 2]],
        "retargeting never mutates the donor"
    );
}

#[test]
fn gated_retuning_retires_at_actual_source_end_instead_of_age_times_new_step() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let settings = Settings::new(Region { start: 0, end: 8 });
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(1.0));
    for (ratio, frames) in [(2.0, 3), (0.5, 10)] {
        let mut old = pair(&original);
        process(&mut old, &[on(0, 7, 60)], &mut [[0.0; 2]; 3], &resources);
        let target = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(ratio));
        // Cursor 3 carries over; the changed step applies only from there.
        let expected: Vec<f64> = (0..frames)
            .map(|offset| source(&target, 60, &pcm, 3.0 + f64::from(offset) * ratio))
            .collect();
        if ratio < 1.0 {
            // Below unity the original linear law is unchanged.
            assert_eq!(
                expected,
                vec![4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5, 8.0, 8.0]
            );
        }
        let mut next = pair(&target);
        next.retarget_from(&old, target_proof(&target).as_ref())
            .unwrap();
        let mut actual = vec![[0.0; 2]; expected.len()];
        process(&mut next, &[], &mut actual, &resources);
        let expected: Vec<_> = expected.into_iter().map(|sample| [sample; 2]).collect();
        assert_frames_near(&actual, &expected);
        assert!(
            next.committed_is_quiescent(),
            "cursor reaches region end exactly after the rendered prefix and changed-rate suffix"
        );
        assert_eq!(next.active_occurrence_ids(&mut []), Ok(0));
    }
}

#[test]
fn releasing_gated_tuning_keeps_gate_age_velocity_and_new_onsets_start_at_zero() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    settings.envelope.release_seconds = 4.0 / 8_000.0;
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(1.0));
    let mut old = pair(&original);
    let event = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 7,
        pitch: 60,
        velocity: 64,
    };
    process(&mut old, &[event], &mut [[0.0; 2]; 3], &resources);
    process(&mut old, &[off(0, 7)], &mut [[0.0; 2]; 2], &resources);
    let target = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(2.0));
    let mut next = pair(&target);
    next.retarget_from(&old, target_proof(&target).as_ref())
        .unwrap();
    let release = target.max_release_tail_frames();
    assert!(release >= 4);
    // Cursor5 and gate3 survive retuning at age5; only future steps double.
    let mut tail = vec![[0.0; 2]; release as usize];
    process(&mut next, &[], &mut tail, &resources);
    let expected: Vec<_> = (5..5 + release)
        .map(|age| {
            let cursor = 5 + 2 * (age - 5);
            let level = release_level(age - 3, release) * (64.0 / 127.0);
            [source(&target, 60, &pcm, cursor as f64) * level; 2]
        })
        .collect();
    assert_frames_near(&tail, &expected);
    assert!(next.committed_is_quiescent());
    let mut onset = [[0.0; 2]; 4];
    process(&mut next, &[on(0, 8, 60)], &mut onset, &resources);
    let expected = [0.0, 2.0, 4.0, 6.0].map(|position| [source(&target, 60, &pcm, position); 2]);
    assert_frames_near(&onset, &expected);
}

#[test]
fn trigger_tuning_and_output_rate_changes_refuse_without_mutating_donor_pcm() {
    let pcm = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let tuning = ratio_tuning(1.0);
    let original = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let mut old = pair(&original);
    process(&mut old, &[on(0, 7, 60)], &mut [[0.0; 2]; 3], &resources);
    let mut frequencies = *tuning.frequencies();
    frequencies[60] *= 2.0;
    let tuned = hex_lattice::TuningTable::new(frequencies).unwrap();
    let trigger = prepare_retarget_settings(views[0], settings, 4, 259, &tuned);
    assert_eq!(
        trigger.max_release_tail_frames(),
        original.max_release_tail_frames()
    );
    let mut next = pair(&trigger);
    assert_eq!(
        next.retarget_from(&old, target_proof(&trigger).as_ref()),
        Err(Error::IncompatibleHistory)
    );
    assert!(next.committed_is_quiescent());
    let mut donor = [[0.0; 2]; 3];
    process(&mut old, &[], &mut donor, &resources);
    assert_eq!(donor, [[4.0; 2], [5.0; 2], [6.0; 2]]);

    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    let gated = prepare_retarget_settings(views[0], settings, 4, 259, &tuning);
    let mut old = pair(&gated);
    process(&mut old, &[on(0, 7, 60)], &mut [[0.0; 2]; 3], &resources);
    let changed_rate = PreparedSamplerPlan::new(
        SampleShape::new(8, 8_000, SampleChannels::Mono).unwrap(),
        16_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(4, 259, &ratio_tuning(2.0))
    .unwrap();
    let mut next = pair(&changed_rate);
    assert_eq!(
        next.retarget_from(&old, target_proof(&changed_rate).as_ref()),
        Err(Error::IncompatibleHistory),
        "equal effective step still cannot change output clock"
    );
    let mut donor = [[0.0; 2]; 2];
    process(&mut old, &[], &mut donor, &resources);
    assert_eq!(donor, [[4.0; 2], [5.0; 2]]);
}

#[test]
fn looping_release_deadline_overflow_refuses_before_changing_cursor_or_gate() {
    let pcm = [1.0, 2.0, 3.0, 4.0];
    let views = [view(&pcm)];
    let mut settings = Settings::new(Region { start: 0, end: 4 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    settings.envelope.release_seconds = 4.0 / 8_000.0;
    let prepared = prepare_retarget_settings(views[0], settings, 4, 259, &ratio_tuning(1.0));
    // Seed the valid finite cursor/age state directly: rendering this many
    // output frames is impractical, while the public preflight must stay bounded.
    let age = u64::MAX - 2;
    let position = u128::from(age) * u128::from(prepared.notes[60].unwrap().step);
    let mut runtime = SamplerRuntime {
        last_gain: prepared.base_gain as f32,
        prepared,
        active: [None; MAX_SAMPLER_RUNTIME_VOICES],
        fading: [None; MAX_SAMPLER_RUNTIME_VOICES],
    };
    runtime.active[0] = Some(ActiveVoice {
        occurrence: 7,
        pitch: 60,
        gain: 1.0,
        age,
        position,
        gate: None,
    });
    assert_eq!(
        runtime.preflight_events(&[off(0, 7)], 1, &BorrowedDeviceResources(&views)),
        Err(Error::InvalidEventBatch)
    );
    let voice = runtime.active[0].unwrap();
    assert_eq!(
        (voice.age, voice.position, voice.gate),
        (age, position, None)
    );
}

mod release_floor;
mod steal;

// The frame-major schedule is an independent behavioral oracle for the
// optimized span schedule. It retains the same primitive voice/event laws.
fn frame_major_oracle(
    runtime: &mut SamplerRuntime,
    events: &[NativeInstrumentEvent],
    frames: &mut [StereoFrame],
    gains: &[NativeInstrumentOccurrenceGain],
    resources: &dyn DeviceResources,
) {
    let view = (0..resources.len())
        .filter_map(|index| resources.get(index))
        .find(|view| runtime.prepared.source.matches(*view))
        .unwrap();
    let mut active = runtime.active;
    let mut fading = runtime.fading;
    let fade = runtime.prepared.steal_fade_frames() as f64;
    let mut next = 0;
    for (offset, frame) in frames.iter_mut().enumerate() {
        let gain_at = |id| {
            gains
                .binary_search_by_key(&id, |gain| gain.occurrence_id())
                .ok()
                .map_or(1.0, |index| gains[index].value_at_offset(offset as u64))
        };
        while events
            .get(next)
            .is_some_and(|event| event.frame_offset() == offset)
        {
            runtime
                .event(&mut active, &mut fading, events[next], &gain_at)
                .unwrap();
            next += 1;
        }
        for voice in active.iter().flatten() {
            runtime.mix(frame, view.pcm, voice, gain_at(voice.occurrence));
        }
        for item in fading.iter().flatten() {
            let declick = 1.0 - item.voice.age.saturating_sub(item.start) as f64 / fade;
            runtime.mix(frame, view.pcm, &item.voice, item.gain * declick);
        }
        runtime.advance(&mut active, &mut fading, 1).unwrap();
    }
    runtime.active = active;
    runtime.fading = fading;
}

fn voice_signature(voice: ActiveVoice) -> (u64, u8, u64, u64, u128, Option<u64>) {
    (
        voice.occurrence,
        voice.pitch,
        voice.gain.to_bits(),
        voice.age,
        voice.position,
        voice.gate,
    )
}

#[test]
fn event_spans_match_frame_major_pcm_and_voice_state() {
    let pcm: Vec<_> = (0..24).map(|i| (i as f32 - 11.0) / 13.0).collect();
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    for (looping, pitch) in [(false, 60), (false, 72), (true, 48), (true, 60), (true, 72)] {
        let mode = if looping {
            Mode::Classic {
                loop_region: Some(Region { start: 2, end: 20 }),
            }
        } else {
            Mode::OneShot {
                gate: GateMode::Trigger,
            }
        };
        let prepared = prepare(views[0], mode, 2);
        let mut actual = SamplerRuntime {
            last_gain: prepared.base_gain as f32,
            prepared: prepared.clone(),
            active: [None; MAX_SAMPLER_RUNTIME_VOICES],
            fading: [None; MAX_SAMPLER_RUNTIME_VOICES],
        };
        let mut expected = SamplerRuntime {
            last_gain: prepared.base_gain as f32,
            prepared,
            active: [None; MAX_SAMPLER_RUNTIME_VOICES],
            fading: [None; MAX_SAMPLER_RUNTIME_VOICES],
        };
        let batches: Vec<Vec<NativeInstrumentEvent>> = if looping {
            vec![
                vec![on(0, 1, pitch), on(1, 2, pitch), off(4, 1), on(5, 3, pitch)],
                vec![off(0, 2), on(0, 4, pitch), off(7, 3)],
                vec![],
            ]
        } else {
            vec![
                vec![on(0, 1, pitch), on(2, 2, pitch)],
                vec![on(20, 3, pitch)],
                vec![],
            ]
        };
        for (batch_index, events) in batches.iter().enumerate() {
            let length = [16, 24, 17][batch_index];
            let mut ids: Vec<_> = actual
                .active
                .iter()
                .flatten()
                .map(|v| v.occurrence)
                .collect();
            ids.extend(events.iter().filter_map(|event| match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => Some(*occurrence_id),
                _ => None,
            }));
            ids.sort_unstable();
            ids.dedup();
            let gains: Vec<_> = ids
                .into_iter()
                .filter(|id| id % 2 != 0)
                .map(|id| {
                    NativeInstrumentOccurrenceGain::new(id, 0.17, 0.83, length as u64, 0).unwrap()
                })
                .collect();
            actual.preflight_events(events, length, &resources).unwrap();
            actual.preflight_occurrence_gains(events, &gains).unwrap();
            let mut out = vec![[-0.0, 0.125]; length];
            let mut oracle = out.clone();
            actual.process_with_gains::<false>(events, &mut out, &gains, &resources, &[]);
            frame_major_oracle(&mut expected, events, &mut oracle, &gains, &resources);
            assert_eq!(
                out.iter().map(|f| f.map(f32::to_bits)).collect::<Vec<_>>(),
                oracle
                    .iter()
                    .map(|f| f.map(f32::to_bits))
                    .collect::<Vec<_>>(),
                "looping={looping} pitch={pitch} batch={batch_index}"
            );
            assert_eq!(
                actual.active.map(|v| v.map(voice_signature)),
                expected.active.map(|v| v.map(voice_signature))
            );
            assert_eq!(
                actual.fading.map(|v| v.map(|v| (
                    voice_signature(v.voice),
                    v.start,
                    v.gain.to_bits()
                ))),
                expected.fading.map(|v| v.map(|v| (
                    voice_signature(v.voice),
                    v.start,
                    v.gain.to_bits()
                )))
            );
        }
    }
}
