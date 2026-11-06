use super::*;

fn prepare<'a>(pcm: &'a [StereoFrame], rate: u32, settings: Settings<'a>) -> PreparedSampler<'a> {
    PreparedSampler::new(Sample::new(pcm, rate).unwrap(), rate, settings).unwrap()
}

fn voice(sampler: PreparedSampler<'_>, note: u8) -> Voice<'_> {
    sampler.prepare_voice(note, 127).unwrap().unwrap()
}

#[test]
fn trim_reverse_pitch_and_rate_conversion_match_source_coordinates() {
    let pcm = [
        [100.0, -100.0],
        [1.0, -2.0],
        [3.0, -4.0],
        [5.0, -8.0],
        [200.0, -200.0],
    ];
    let mut settings = Settings::new(Region { start: 1, end: 4 });
    let sampler = prepare(&pcm, 8_000, settings);
    let root = voice(sampler, 60);
    assert_eq!(root.natural_frames(), Some(3));
    assert_eq!(root.sample(0, 10), pcm[1]);
    assert_eq!(root.sample(2, 10), pcm[3]);
    assert_eq!(root.sample(3, 10), [0.0; 2]);
    let octave = voice(sampler, 72);
    assert_eq!(octave.natural_frames(), Some(2));
    // Pitched up reconstructs band-limited around source frame 2 of the trim;
    // frames outside the trim never leak in (they read as silence).
    let mut outside_silent = pcm;
    outside_silent[0] = [0.0; 2];
    outside_silent[4] = [0.0; 2];
    let isolated = voice(prepare(&outside_silent, 8_000, settings), 72);
    assert_eq!(octave.sample(1, 10), isolated.sample(1, 10));
    assert!(octave.sample(1, 10)[0] > 0.0);
    assert_eq!(octave.sample(2, 10), [0.0; 2]);
    let lower = voice(sampler, 48);
    assert_eq!(lower.sample(1, 10), [2.0, -3.0]);
    assert_eq!(lower.sample(5, 10), pcm[3]); // terminal interpolation holds inside trim
    assert_eq!(lower.sample(6, 10), [0.0; 2]);
    settings.reverse = true;
    let reverse = voice(prepare(&pcm, 8_000, settings), 60);
    assert_eq!(reverse.sample(0, 10), pcm[3]);
    assert_eq!(reverse.sample(2, 10), pcm[1]);
    let converted =
        PreparedSampler::new(Sample::new(&pcm, 8_000).unwrap(), 16_000, settings).unwrap();
    assert_eq!(voice(converted, 60).sample(1, 10), [4.0, -6.0]);
}

#[test]
fn loop_keeps_attack_region_and_interpolates_across_wrap_in_both_directions() {
    let pcm = [[0.0; 2], [1.0; 2], [2.0; 2], [3.0; 2], [4.0; 2], [5.0; 2]];
    let mut settings = Settings::new(Region { start: 1, end: 6 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 2, end: 4 }),
    };
    let forward = voice(prepare(&pcm, 8_000, settings), 60);
    let expected = [1.0, 2.0, 3.0, 2.0, 3.0, 2.0];
    for (age, expected) in expected.into_iter().enumerate() {
        assert_eq!(forward.sample(age as u64, u64::MAX), [expected; 2]);
    }
    assert_eq!(forward.natural_frames(), None);
    // At half speed, the last loop frame blends into the loop start rather
    // than reading the sample after the exclusive loop endpoint.
    assert_eq!(
        voice(prepare(&pcm, 8_000, settings), 48).sample(5, 100),
        [2.5; 2]
    );
    // Integer phase remains exact beyond f64's integer precision.
    assert_eq!(forward.sample((1_u64 << 60) + 1, u64::MAX), [2.0; 2]);
    settings.reverse = true;
    let reverse = voice(prepare(&pcm, 8_000, settings), 60);
    for (age, expected) in [5.0, 4.0, 3.0, 2.0, 3.0, 2.0].into_iter().enumerate() {
        assert_eq!(reverse.sample(age as u64, 100), [expected; 2]);
    }
}

#[test]
fn envelope_releases_from_level_at_note_off_even_during_attack() {
    let pcm = [[1.0; 2]; 64];
    let mut settings = Settings::new(Region { start: 0, end: 64 });
    settings.envelope = Envelope {
        attack_seconds: 4.0 / 8_000.0,
        decay_seconds: 4.0 / 8_000.0,
        sustain: 0.5,
        release_seconds: 4.0 / 8_000.0,
    };
    let v = voice(prepare(&pcm, 8_000, settings), 60);
    // Expectations derive from the prepared release so a host declick floor
    // changes only its length, not the linear law under test.
    let release = v.release_frames();
    assert!(release >= 4);
    for (age, expected) in [0.0, 0.25, 0.5, 0.75, 1.0, 0.875, 0.75, 0.625, 0.5]
        .into_iter()
        .enumerate()
    {
        assert_eq!(v.sample(age as u64, 30), [expected; 2]);
    }
    // Note-off during attack (level 0.5 at gate 2) ramps linearly to zero.
    for step in 0..release {
        let expected = (0.5 * (1.0 - step as f64 / release as f64)) as f32;
        assert_eq!(v.sample(2 + step, 2), [expected; 2]);
    }
    assert_eq!(v.sample(2 + release, 2), [0.0; 2]);
    assert_eq!(v.sample(0, 0), [0.0; 2]);
}

#[test]
fn one_shot_trigger_ignores_note_off_gate_honors_it_and_velocity_zero_is_silent() {
    let pcm = [[1.0, -0.5]; 8];
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let sampler = prepare(&pcm, 8_000, settings);
    let trigger = voice(sampler, 60);
    assert_eq!(trigger.sample(7, 0), pcm[7]);
    assert_eq!(trigger.sample(8, 0), [0.0; 2]);
    assert_eq!(trigger.release_frames(), 0);
    assert!(sampler.prepare_voice(60, 0).unwrap().is_none());
    assert!(sampler.prepare_voice(255, 127).is_err());
    assert!(sampler.prepare_voice(60, 255).is_err());
    settings.mode = Mode::OneShot {
        gate: GateMode::Gate,
    };
    let gated = voice(prepare(&pcm, 8_000, settings), 60);
    assert_eq!(gated.sample(1, 2), pcm[1]);
    // A gated one-shot ends at note-off plus its prepared release, bounded
    // by the source's natural end.
    let end = gated.end_age_frames(2).unwrap();
    assert_eq!(end, (2 + gated.release_frames()).min(8));
    assert_eq!(gated.sample(end, 2), [0.0; 2]);
}

#[test]
fn slices_map_to_keys_stop_at_next_marker_and_reverse_within_selected_slice() {
    let pcm = [
        [99.0; 2], [1.0; 2], [2.0; 2], [3.0; 2], [4.0; 2], [5.0; 2], [99.0; 2],
    ];
    let mut settings = Settings::new(Region { start: 1, end: 6 });
    settings.mode = Mode::Slice {
        starts: &[1, 3, 5],
        base_note: 36,
        gate: GateMode::Trigger,
    };
    let sampler = prepare(&pcm, 8_000, settings);
    assert!(sampler.prepare_voice(35, 127).unwrap().is_none());
    assert!(sampler.prepare_voice(39, 127).unwrap().is_none());
    for (note, start, length) in [(36, 1, 2), (37, 3, 2), (38, 5, 1)] {
        let v = voice(sampler, note);
        assert_eq!(v.sample(0, 0), pcm[start]);
        assert_eq!(v.natural_frames(), Some(length));
        assert_eq!(v.sample(length, 0), [0.0; 2]);
    }
    settings.reverse = true;
    let reverse = voice(prepare(&pcm, 8_000, settings), 37);
    assert_eq!(reverse.sample(0, 0), pcm[4]);
    assert_eq!(reverse.sample(1, 0), pcm[3]);
    assert_eq!(reverse.sample(2, 0), [0.0; 2]);
}

#[test]
fn gain_pan_and_velocity_preserve_asymmetric_stereo_and_extreme_pcm_stays_finite() {
    let pcm = [[2.0, -4.0]; 2];
    let mut settings = Settings::new(Region { start: 0, end: 2 });
    settings.gain = 2.0;
    settings.pan = 0.5;
    let sampler = prepare(&pcm, 8_000, settings);
    assert_eq!(
        sampler
            .prepare_voice(60, 63)
            .unwrap()
            .unwrap()
            .sample(0, 100),
        [2.0 * 63.0 / 127.0, -8.0 * 63.0 / 127.0]
    );
    settings.velocity_amount = 0.0;
    assert_eq!(
        prepare(&pcm, 8_000, settings)
            .prepare_voice(60, 1)
            .unwrap()
            .unwrap()
            .sample(0, 100),
        [2.0, -8.0]
    );
    let extreme = [[f32::MAX, -f32::MAX], [-f32::MAX, f32::MAX]];
    settings.pan = 0.0;
    settings.gain = 4.0;
    let v = voice(prepare(&extreme, 8_000, settings), 48);
    assert_eq!(v.sample(0, 10), extreme[0]);
    assert_eq!(v.sample(1, 10), [0.0; 2]);
}

#[test]
fn rendering_is_partition_and_query_order_independent_and_age_never_wraps() {
    let pcm = [[1.0, -2.0], [3.0, -1.0], [0.5, 0.25]];
    let mut settings = Settings::new(Region { start: 0, end: 3 });
    settings.mode = Mode::Classic {
        loop_region: Some(settings.region),
    };
    settings.detune_cents = 13.0;
    settings.envelope.release_seconds = 0.003;
    let v = voice(prepare(&pcm, 44_100, settings), 59);
    let mut whole = [[0.0; 2]; 257];
    let mut split = whole;
    v.render(0, 200, &mut whole);
    let mut cursor = 0;
    for size in [1, 7, 64, 3, 128, 54] {
        v.render(cursor as u64, 200, &mut split[cursor..cursor + size]);
        cursor += size;
    }
    assert_eq!(whole, split);
    for age in (0..257).rev() {
        assert_eq!(whole[age], v.sample(age as u64, 200));
    }
    let mut terminal = [[9.0; 2]; 3];
    v.render(u64::MAX, u64::MAX, &mut terminal);
    assert_eq!(&terminal[1..], &[[0.0; 2]; 2]);
}

#[test]
fn malformed_pcm_regions_markers_and_numeric_settings_are_refused() {
    assert!(Sample::new(&[], 48_000).is_err());
    assert!(Sample::new(&[[0.0; 2]], 0).is_err());
    assert!(Sample::new(&[[f32::NAN, 0.0]], 48_000).is_err());
    assert!(Sample::new(&[[0.0, f32::INFINITY]], 48_000).is_err());
    let pcm = [[0.0; 2]; 4];
    let sample = Sample::new(&pcm, 48_000).unwrap();
    let base = Settings::new(sample.whole_region());
    for region in [
        Region { start: 0, end: 0 },
        Region { start: 3, end: 2 },
        Region { start: 0, end: 5 },
        Region {
            start: usize::MAX,
            end: usize::MAX,
        },
    ] {
        assert!(PreparedSampler::new(sample, 48_000, Settings { region, ..base }).is_err());
    }
    for starts in [&[][..], &[0, 0], &[0, 4], &[1, 2], &[0, 3, 2]] {
        let mode = Mode::Slice {
            starts,
            base_note: 36,
            gate: GateMode::Gate,
        };
        assert!(PreparedSampler::new(sample, 48_000, Settings { mode, ..base }).is_err());
    }
    let mode = Mode::Slice {
        starts: &[0, 2],
        base_note: 127,
        gate: GateMode::Gate,
    };
    assert!(PreparedSampler::new(sample, 48_000, Settings { mode, ..base }).is_err());
    for value in [f64::NAN, f64::INFINITY, -f64::INFINITY, -1.0, 5.0] {
        assert!(
            PreparedSampler::new(
                sample,
                48_000,
                Settings {
                    gain: value,
                    ..base
                }
            )
            .is_err()
        );
    }
    let envelope = Envelope {
        release_seconds: f64::NAN,
        ..Envelope::default()
    };
    assert!(PreparedSampler::new(sample, 48_000, Settings { envelope, ..base }).is_err());
    let mode = Mode::Classic {
        loop_region: Some(Region { start: 1, end: 5 }),
    };
    assert!(PreparedSampler::new(sample, 48_000, Settings { mode, ..base }).is_err());
    assert!(PreparedSampler::new(sample, 1, base).is_err());
}
