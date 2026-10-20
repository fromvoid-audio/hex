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
