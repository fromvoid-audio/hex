//! Prepared declick floor: a zero authored release still ramps to silence.
use super::*;

const DC: [f32; 32] = [1.0; 32];

/// Looping DC sampler with zero authored release at `rate`.
fn zero_release(view: DeviceResourceView<'_>, rate: u32) -> PreparedSamplerRuntime {
    let mut settings = Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 0, end: 32 }),
    };
    assert_eq!(settings.envelope.release_seconds, 0.0);
    PreparedSamplerPlan::new(
        SampleShape::new(32, 8_000, SampleChannels::Mono).unwrap(),
        rate,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(1, 259, &hex_lattice::TuningTable::default())
    .unwrap()
}

#[test]
fn zero_authored_release_note_off_on_sustained_dc_steps_at_most_one_floor_increment() {
    let views = [view(&DC)];
    let resources = BorrowedDeviceResources(&views);
    // About 2 ms: 16 frames at 8 kHz, 96 at 48 kHz.
    for (rate, floor, threshold) in [(8_000, 16, 1.0 / 16.0), (48_000, 96, 1.0 / 96.0)] {
        let prepared = zero_release(views[0], rate);
        assert_eq!(crate::sampler::envelope::release_floor_frames(rate), floor);
        assert_eq!(
            prepared.max_release_tail_frames(),
            floor,
            "tail includes the floor"
        );
        let mut runtime = NativeInstrumentRuntimePair::new(
            &prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: MAX_SAMPLER_RUNTIME_VOICES,
                max_tail_frames: floor,
            },
        )
        .unwrap();
        let frames = 8 + floor as usize + 8;
        let mut output = vec![[0.0; 2]; frames];
        process(
            &mut runtime,
            &[on(0, 1, 60), off(8, 1)],
            &mut output,
            &resources,
        );
        assert_eq!(output[7], [1.0; 2], "sustained DC before note-off");
        let largest = output
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            f64::from(largest) <= threshold + 1e-6,
            "{rate} Hz step {largest} exceeds {threshold}"
        );
        assert_eq!(output[8 + floor as usize..], vec![[0.0; 2]; 8][..]);
        assert!(runtime.committed_is_quiescent());
    }
}

#[test]
fn note_off_inside_the_pitched_loop_crossfade_preserves_both_transitions() {
    const RATE: u32 = 8_000;
    const NOTE_OFF: usize = 45;
    const FRAMES: usize = 80;
    let pcm: [f32; 320] = std::array::from_fn(|frame| frame as f32 / 320.0);
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let render = |crossfade_ms, block| {
        let mut settings = Settings::new(Region {
            start: 10,
            end: 300,
        });
        settings.mode = Mode::Classic {
            loop_region: Some(Region {
                start: 100,
                end: 200,
            }),
        };
        settings.loop_crossfade_ms = crossfade_ms;
        let prepared = PreparedSamplerPlan::new(
            SampleShape::new(320, RATE, SampleChannels::Mono).unwrap(),
            RATE,
            settings,
        )
        .unwrap()
        .bind_source_on_control(views[0])
        .unwrap()
        .prepare_runtime_on_control(1, 259, &hex_lattice::TuningTable::default())
        .unwrap();
        assert_eq!(prepared.max_release_tail_frames(), 16);
        let mut runtime = pair(&prepared);
        let mut output = [[0.0; 2]; FRAMES];
        let mut events = Vec::with_capacity(2);
        for (index, chunk) in output.chunks_mut(block).enumerate() {
            let start = index * block;
            events.clear();
            if start == 0 {
                events.push(on(0, 1, 84));
            }
            if (start..start + chunk.len()).contains(&NOTE_OFF) {
                events.push(off(NOTE_OFF - start, 1));
            }
            process(&mut runtime, &events, chunk, &resources);
        }
        assert!(runtime.committed_is_quiescent());
        assert_eq!(output[NOTE_OFF + 16..], [[0.0; 2]; FRAMES - NOTE_OFF - 16]);
        output
    };
    let largest_step = |pcm: &[StereoFrame]| {
        pcm[24..]
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).abs())
            .fold(0.0_f32, f32::max)
    };
    // At +24 semitones the source advances four frames per output frame.
    // The first wrap is at age 48, after the default release has started.
    let faded = render(5.0, 64);
    let hard = render(0.0, 64);
    assert!(faded[NOTE_OFF - 1][0] > 0.25);
    assert!(
        faded[48][0] > 0.1,
        "the note is still releasing at the seam"
    );
    assert!(largest_step(&faded) < 0.08, "{:?}", largest_step(&faded));
    assert!(largest_step(&hard) > largest_step(&faded) * 2.0);
    for block in [1, 7, 16] {
        assert_eq!(render(5.0, block), faded, "block {block}");
    }
}
