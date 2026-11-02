use super::*;

// Positive constant PCM makes first silence an observable end oracle.
#[test]
fn end_matches_rendered_exhaustion_across_rates_pitch_reverse_and_gates() {
    let pcm = [[1.0, 0.25]; 41];
    for source_rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
        let sample = Sample::new(&pcm, source_rate).unwrap();
        for output_rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            for note in [48, 59, 60, 61, 72] {
                for reverse in [false, true] {
                    for mode in [
                        Mode::Classic { loop_region: None },
                        Mode::OneShot {
                            gate: GateMode::Gate,
                        },
                        Mode::OneShot {
                            gate: GateMode::Trigger,
                        },
                    ] {
                        let mut settings = Settings::new(Region { start: 2, end: 39 });
                        settings.reverse = reverse;
                        settings.mode = mode;
                        settings.envelope.release_seconds = 8.0 / f64::from(output_rate);
                        let sampler = PreparedSampler::new(sample, output_rate, settings).unwrap();
                        let voice = sampler.prepare_voice(note, 127).unwrap().unwrap();
                        for gate in [0, 1, 19, 2_051] {
                            let mut rendered = [[0.0; 2]; 4_096];
                            voice.render(0, gate, &mut rendered);
                            let first_silent = rendered
                                .iter()
                                .position(|frame| *frame == [0.0; 2])
                                .unwrap();
                            assert_eq!(voice.end_age_frames(gate).unwrap(), first_silent as u64);
                            assert!(
                                rendered[first_silent..]
                                    .iter()
                                    .all(|frame| *frame == [0.0; 2])
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn loop_release_and_trigger_slice_ends_survive_partitioning_and_short_notes() {
    let pcm = [[0.75, -0.5]; 64];
    let sample = Sample::new(&pcm, 48_000).unwrap();
    for mode in [
        Mode::Classic {
            loop_region: Some(Region { start: 4, end: 9 }),
        },
        Mode::Slice {
            starts: &[0, 7, 31],
            base_note: 60,
            gate: GateMode::Gate,
        },
        Mode::Slice {
            starts: &[0, 7, 31],
            base_note: 60,
            gate: GateMode::Trigger,
        },
    ] {
        let mut settings = Settings::new(sample.whole_region());
        settings.mode = mode;
        settings.envelope.release_seconds = 4.0 / 48_000.0;
        let sampler = PreparedSampler::new(sample, 48_000, settings).unwrap();
        for note in [60, 61, 62] {
            let voice = sampler.prepare_voice(note, 127).unwrap().unwrap();
            let end = voice.end_age_frames(1).unwrap();
            let slice_frames = [7, 24, 33][usize::from(note - 60)];
            let released = 1 + voice.release_frames();
            let expected = match mode {
                Mode::Classic { .. } => released,
                Mode::Slice {
                    gate: GateMode::Gate,
                    ..
                } => released.min(slice_frames),
                _ => slice_frames,
            };
            assert_eq!(end, expected);
            let mut whole = [[0.0; 2]; 128];
            let mut parts = whole;
            voice.render(0, 1, &mut whole);
            let mut cursor = 0;
            for count in [1, 3, 17, 2, 64, 41] {
                voice.render(cursor as u64, 1, &mut parts[cursor..cursor + count]);
                cursor += count;
            }
            assert_eq!(whole, parts);
            assert_ne!(whole[end as usize - 1], [0.0; 2]);
            assert!(whole[end as usize..].iter().all(|frame| *frame == [0.0; 2]));
        }
    }
}

#[test]
fn exhausted_sources_bound_huge_gates_but_loop_release_overflow_refuses() {
    let pcm = [[1.0; 2]; 32];
    let sample = Sample::new(&pcm, 48_000).unwrap();
    let mut settings = Settings::new(sample.whole_region());
    settings.envelope.release_seconds = 4.0 / 48_000.0;
    for mode in [
        Mode::Classic { loop_region: None },
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
    ] {
        settings.mode = mode;
        let voice = PreparedSampler::new(sample, 48_000, settings)
            .unwrap()
            .prepare_voice(60, 127)
            .unwrap()
            .unwrap();
        assert_eq!(voice.end_age_frames(u64::MAX).unwrap(), 32);
        assert_eq!(voice.end_age_frames(u64::MAX - 3).unwrap(), 32);
    }
    settings.mode = Mode::Classic {
        loop_region: Some(sample.whole_region()),
    };
    let voice = PreparedSampler::new(sample, 48_000, settings)
        .unwrap()
        .prepare_voice(60, 127)
        .unwrap()
        .unwrap();
    let release = voice.release_frames();
    assert!(release >= 4);
    assert_eq!(voice.end_age_frames(u64::MAX - release).unwrap(), u64::MAX);
    assert!(voice.end_age_frames(u64::MAX - release + 1).is_err());
    assert!(voice.end_age_frames(u64::MAX).is_err());
    settings.envelope.release_seconds = 0.0;
    let voice = PreparedSampler::new(sample, 48_000, settings)
        .unwrap()
        .prepare_voice(60, 127)
        .unwrap()
        .unwrap();
    // The shortest prepared release still bounds a looping gate exactly.
    let release = voice.release_frames();
    assert_eq!(voice.end_age_frames(0).unwrap(), release);
    assert_eq!(voice.end_age_frames(u64::MAX - release).unwrap(), u64::MAX);
}
