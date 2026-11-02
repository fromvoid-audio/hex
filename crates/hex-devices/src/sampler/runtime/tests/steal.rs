//! Voice stealing at capacity: the oldest releasing voice is declicked in the
//! fixed fade pool rather than refused or hard-cut.
use super::*;

const DC: [f32; 32] = [1.0; 32];

/// One-voice looping DC sampler with a 64-frame release at 8 kHz, so a
/// stolen voice is still clearly audible when the next onset arrives.
fn one_voice(view: DeviceResourceView<'_>) -> PreparedSamplerRuntime {
    let mut settings = Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 0, end: 32 }),
    };
    settings.envelope.release_seconds = 64.0 / 8_000.0;
    prepare_retarget_settings(view, settings, 1, 259, &hex_lattice::TuningTable::default())
}

/// Render `frames` frames in `block`-sized pieces, rebasing absolute event
/// offsets into each block.
fn render(
    prepared: &PreparedSamplerRuntime,
    events: &[NativeInstrumentEvent],
    frames: usize,
    block: usize,
    resources: &dyn DeviceResources,
) -> Vec<StereoFrame> {
    let mut runtime = pair(prepared);
    let mut output = vec![[0.0; 2]; frames];
    for (index, chunk) in output.chunks_mut(block).enumerate() {
        let start = index * block;
        let local: Vec<_> = events
            .iter()
            .filter(|event| (start..start + chunk.len()).contains(&event.frame_offset()))
            .map(|event| match *event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => NativeInstrumentEvent::NoteOn {
                    frame_offset: frame_offset - start,
                    occurrence_id,
                    pitch,
                    velocity,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => off(frame_offset - start, occurrence_id),
                other => other,
            })
            .collect();
        process(&mut runtime, &local, chunk, resources);
    }
    output
}

#[test]
fn note_on_at_capacity_steals_the_releasing_voice_with_a_bounded_declick() {
    let views = [view(&DC)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = one_voice(views[0]);
    assert!(prepared.steals_releasing_voices());
    let release = prepared.max_release_tail_frames();
    assert_eq!(release, 64);
    let fade = prepared.steal_fade_frames();
    assert_eq!(fade, 8, "1 ms at 8 kHz");
    let events = [on(0, 1, 60), off(4, 1), on(8, 2, 60)];
    let reference = render(&prepared, &events, 160, 160, &resources);
    for block in [1, 7, 64, 128] {
        assert_eq!(
            render(&prepared, &events, 160, block, &resources),
            reference,
            "block {block}"
        );
    }
    // The new held DC voice contributes exactly 1.0 from frame8; the rest is
    // the stolen voice's release times its linear declick.
    for (index, frame) in reference.iter().enumerate() {
        let age = index as u64;
        let expected = match age {
            0..4 => 1.0,
            4..8 => 1.0 - (age - 4) as f64 / release as f64,
            _ => {
                let elapsed = age - 8;
                let stolen = if elapsed < fade {
                    (1.0 - (age - 4) as f64 / release as f64) * (1.0 - elapsed as f64 / fade as f64)
                } else {
                    0.0
                };
                1.0 + stolen
            }
        };
        assert!(
            (f64::from(frame[0]) - expected).abs() < 1e-6,
            "frame {index}: {frame:?} != {expected}"
        );
    }
    // No hard cut: from the steal at frame8 on, the output never steps by
    // more than the declick slope (1 / fade) plus the release slope.
    let max_step = reference[8..]
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).abs())
        .fold(0.0_f32, f32::max);
    let bound = 1.0 / fade as f32 + 1.0 / release as f32;
    assert!(max_step <= bound + 1e-6, "step {max_step} > {bound}");
}

#[test]
fn stolen_voice_leaves_the_active_set_and_refusal_remains_when_every_voice_is_held() {
    let views = [view(&DC)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = one_voice(views[0]);
    let mut runtime = pair(&prepared);
    // Every slot held: the existing refusal applies and output is untouched.
    let mut untouched = [[9.0; 2]; 4];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block(&[on(0, 1, 60), on(2, 2, 60)], &mut untouched, &resources),
        Err(Error::EventCapacityExceeded)
    );
    assert_eq!(untouched, [[9.0; 2]; 4]);
    runtime.rollback_block();

    let mut output = [[0.0; 2]; 4];
    process(
        &mut runtime,
        &[on(0, 1, 60), off(1, 1), on(2, 2, 60)],
        &mut output,
        &resources,
    );
    let mut ids = [0; MAX_SAMPLER_RUNTIME_VOICES];
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], 2, "the fading voice is not an active occurrence");
    // The stolen occurrence takes no later gains or events.
    let gain = NativeInstrumentOccurrenceGain::new(1, 0.5, 0.5, 4, 0).unwrap();
    runtime.begin_block();
    assert_eq!(
        runtime.process_block_with_clock_and_occurrence_gains(
            &[],
            &mut [[0.0; 2]; 4],
            None,
            &[gain],
            &resources,
        ),
        Err(Error::InvalidOccurrenceGain)
    );
    runtime.rollback_block();
    let mut tail = [[0.0; 2]; 16];
    process(&mut runtime, &[off(0, 1)], &mut tail, &resources);
    assert!(!runtime.committed_is_quiescent(), "voice 2 is still held");
    process(&mut runtime, &[off(0, 2)], &mut [[0.0; 2]; 64], &resources);
    assert!(runtime.committed_is_quiescent());
}

/// `count` steals in one frame on a one-voice runtime: every note-on after
/// the first steals its predecessor, which is released on the same frame.
fn steal_burst(start_id: u64, count: u64) -> Vec<NativeInstrumentEvent> {
    let mut events = vec![on(0, start_id, 60)];
    for id in start_id..start_id + count {
        events.push(off(0, id));
        events.push(on(0, id + 1, 60));
    }
    events
}

#[test]
fn steal_burst_beyond_the_fade_pool_drops_the_onset_instead_of_cutting_a_fading_voice() {
    let views = [view(&DC)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = one_voice(views[0]);
    let fade = prepared.steal_fade_frames();
    let release = prepared.max_release_tail_frames() as f64;
    let pool = MAX_SAMPLER_RUNTIME_VOICES as u64;
    // Fill the pool, release the voice holding the slot, then one more onset:
    // no fade slot is free, so that onset is dropped and nothing is cut.
    let mut events = steal_burst(1, pool);
    events.extend([off(0, pool + 1), on(0, pool + 2, 60), off(3, pool + 2)]);
    let frames = 2 * fade as usize;
    let reference = render(&prepared, &events, frames, frames, &resources);
    for block in [1, 7, 64, 128] {
        assert_eq!(
            render(&prepared, &events, frames, block, &resources),
            reference,
            "block {block}"
        );
    }
    for (index, frame) in reference.iter().enumerate() {
        let age = index as f64;
        let releasing = 1.0 - age / release;
        let stolen = if index < fade as usize {
            pool as f64 * releasing * (1.0 - age / fade as f64)
        } else {
            0.0
        };
        assert!(
            (f64::from(frame[0]) - (releasing + stolen)).abs() < 1e-4,
            "frame {index}: {frame:?}"
        );
    }
    let mut runtime = pair(&prepared);
    process(
        &mut runtime,
        &events,
        &mut vec![[0.0; 2]; frames],
        &resources,
    );
    let mut ids = [0; MAX_SAMPLER_RUNTIME_VOICES];
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], pool + 1, "the dropped onset never became active");
    // Once the declicks finish, the next onset steals normally again.
    let mut next = [[0.0; 2]; 4];
    process(&mut runtime, &[on(0, pool + 3, 60)], &mut next, &resources);
    assert_eq!(runtime.active_occurrence_ids(&mut ids), Ok(1));
    assert_eq!(ids[0], pool + 3);
    assert!((next[0][0] - 1.0 - (1.0 - frames as f32 / release as f32)).abs() < 1e-4);
}
