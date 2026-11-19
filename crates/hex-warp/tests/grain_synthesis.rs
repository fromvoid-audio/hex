use hex_warp::*;

fn sample(source: &[Frame], at: i128) -> [f64; 2] {
    usize::try_from(at)
        .ok()
        .and_then(|index| source.get(index))
        .copied()
        .unwrap_or([0.0; 2])
        .map(f64::from)
}

#[test]
fn bounded_grains_match_resident_prefix_for_both_laws_and_modes() {
    let source: Vec<Frame> = (0..1024)
        .map(|i| {
            let x = (std::f64::consts::TAU * i as f64 / 73.0).sin() as f32;
            [x * 0.5, x * -0.2]
        })
        .collect();
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 1024,
            source_start_q32: 0,
            source_end_q32: 1024_i128 << 32,
        }],
        1024,
    )
    .unwrap();
    for law in [Resampler::V1, Resampler::V2] {
        for mode in [
            Mode::Tones { grain_ms: 8 },
            Mode::Texture {
                grain_ms: 8,
                fluctuation: 35,
                seed: 42,
            },
        ] {
            let request = Request {
                attacks: &[],
                input: Input {
                    frames: &source,
                    sample_rate: 16_000,
                    region_start: 0,
                    region_length: source.len() as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames: 1024,
                pitch_cents: 300,
                mode,
            };
            let expected = render_with_resampler(&request, law, &mut || false)
                .unwrap()
                .frames;
            let synth = GrainSynthesis::new(300, law, &mut || false).unwrap();
            assert!(
                synth.filter_backing_bytes().unwrap()
                    <= GrainSynthesis::filter_reservation_bytes(law)
            );
            let mut overlap = GranularOverlap::new(20_000_000, 292).unwrap();
            let mut actual = [[0.0; 2]; 512];
            let mut center = -128_i128;
            let mut prior = None;
            let mut index = 0;
            while overlap.position() < actual.len() as u64 {
                let decision = analyze_grain(
                    GrainAnalysis {
                        sample_rate: 16_000,
                        mode,
                        pitch_cents: 300,
                        center,
                        desired: center as f64,
                        previous_anchor: prior,
                        grain_index: index,
                        reference_channel: 0,
                    },
                    &mut |at| Ok::<_, Error>(sample(&source, at)),
                    &mut || false,
                )
                .unwrap();
                let range = synth
                    .required_source_range(decision.anchor, decision.length)
                    .unwrap();
                let frames: Vec<_> = range.clone().map(|at| sample(&source, at)).collect();
                synth
                    .add_grain(
                        center,
                        decision,
                        range.start,
                        &frames,
                        &mut overlap,
                        &mut || false,
                    )
                    .unwrap();
                prior = Some(decision.anchor);
                center += 32;
                index += 1;
                let end = (center - 128).max(0).min(actual.len() as i128) as usize;
                let start = overlap.position() as usize;
                overlap.drain(&mut actual[start..end]).unwrap();
            }
            assert_eq!(&actual[..], &expected[..512], "{law:?} {mode:?}");
        }
    }
}

#[test]
fn source_extent_validation_and_cancellation_do_not_mutate_overlap() {
    let synth = GrainSynthesis::new(0, Resampler::V1, &mut || false).unwrap();
    let decision = GrainDecision {
        anchor: 0.0,
        length: 128,
        work_units: 0,
    };
    let range = synth
        .required_source_range(decision.anchor, decision.length)
        .unwrap();
    let frames = vec![[1.0; 2]; (range.end - range.start) as usize];
    let mut overlap = GranularOverlap::new(256, 292).unwrap();
    assert!(matches!(
        synth.add_grain(
            0,
            decision,
            range.start,
            &frames[..frames.len() - 1],
            &mut overlap,
            &mut || false
        ),
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        synth.add_grain(0, decision, range.start, &frames, &mut overlap, &mut || {
            true
        }),
        Err(Error::Cancelled)
    );
    let mut silence = [[-1.0; 2]; 64];
    overlap.drain(&mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 64]);
    for (anchor, length) in [(f64::NAN, 128), (0.0, 1), (0.0, 76_801)] {
        assert!(matches!(
            synth.required_source_range(anchor, length),
            Err(Error::Invalid(_))
        ));
    }
    for cents in [-4800, 4800] {
        let extreme = GrainSynthesis::new(cents, Resampler::V2, &mut || false).unwrap();
        let range = extreme
            .required_source_range((1_u64 << 42) as f64, 76_800)
            .unwrap();
        assert!(
            (range.end - range.start) as usize * std::mem::size_of::<[f64; 2]>()
                <= MAX_SCRATCH_BYTES
        );
    }
}

#[test]
fn bounded_windows_cover_reconstruction_at_pitch_and_position_extremes() {
    for law in [Resampler::V1, Resampler::V2] {
        for pitch in [-4800, 0, 4800] {
            let synth = GrainSynthesis::new(pitch, law, &mut || false).unwrap();
            for anchor in [-10.25, 0.0, (1_u64 << 42) as f64 + 20_000.0] {
                let decision = GrainDecision {
                    anchor,
                    length: 128,
                    work_units: 0,
                };
                let range = synth.required_source_range(anchor, 128).unwrap();
                let source = vec![[0.25, -0.5]; (range.end - range.start) as usize];
                let mut overlap = GranularOverlap::new(128, 128).unwrap();
                synth
                    .add_grain(
                        64,
                        decision,
                        range.start,
                        &source,
                        &mut overlap,
                        &mut || false,
                    )
                    .unwrap();
                let mut output = [[0.0; 2]; 128];
                overlap.drain(&mut output).unwrap();
                for frame in &output[1..127] {
                    assert!(
                        (frame[0] - 0.25).abs() < 1e-6 && (frame[1] + 0.5).abs() < 1e-6,
                        "{law:?} {pitch} {anchor}"
                    );
                }
            }
        }
    }
}

#[test]
fn cancellation_during_large_grain_requires_discarding_overlap() {
    let synth = GrainSynthesis::new(0, Resampler::V1, &mut || false).unwrap();
    let decision = GrainDecision {
        anchor: 0.0,
        length: 2048,
        work_units: 0,
    };
    let range = synth
        .required_source_range(decision.anchor, decision.length)
        .unwrap();
    let source = vec![[1.0; 2]; (range.end - range.start) as usize];
    let mut overlap = GranularOverlap::new(2048, 2048).unwrap();
    let checks = std::cell::Cell::new(0);
    let result = synth.add_grain(
        1024,
        decision,
        range.start,
        &source,
        &mut overlap,
        &mut || {
            checks.set(checks.get() + 1);
            checks.get() == 2
        },
    );
    assert_eq!(result, Err(Error::Cancelled));
    assert_eq!(checks.get(), 2);
}
