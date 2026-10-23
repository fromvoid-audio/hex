use hex_warp::*;
const RATE: u32 = 16_000;
fn signal(frames: usize, frequency: f64) -> Vec<Frame> {
    (0..frames)
        .map(|i| {
            let x = (2.0 * std::f64::consts::PI * frequency * i as f64 / f64::from(RATE)).sin()
                as f32
                * 0.5;
            [x, -x]
        })
        .collect()
}
fn map(source: usize, output: usize) -> TimeMap {
    TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: (source as i128) << 32,
        }],
        output,
    )
    .unwrap()
}
fn prepare(source: &[Frame], output: usize, mode: Mode, pitch: i16) -> Rendered {
    let map = map(source.len(), output);
    render(
        &Request {
            attacks: &[],
            input: Input {
                frames: source,
                sample_rate: RATE,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: output,
            pitch_cents: pitch,
            mode,
        },
        &mut || false,
    )
    .unwrap()
}
fn peak_frequency(frames: &[Frame], low: u32, high: u32) -> f64 {
    let trim = (frames.len() / 8).min(2_048);
    let frames = &frames[trim..frames.len() - trim];
    let mut best = (0.0, 0.0);
    for frequency in low..=high {
        let mut re = 0.0;
        let mut im = 0.0;
        for (index, frame) in frames.iter().enumerate() {
            let phase =
                2.0 * std::f64::consts::PI * f64::from(frequency) * index as f64 / f64::from(RATE);
            re += f64::from(frame[0]) * phase.cos();
            im += f64::from(frame[0]) * phase.sin();
        }
        let energy = re * re + im * im;
        if energy > best.0 {
            best = (energy, f64::from(frequency));
        }
    }
    best.1
}
fn rms(frames: &[Frame]) -> f64 {
    (frames.iter().map(|s| f64::from(s[0]).powi(2)).sum::<f64>() / frames.len() as f64).sqrt()
}
#[test]
fn repitch_doubles_frequency_and_filters_downsampling_alias() {
    let source = signal(8_000, 440.0);
    let result = prepare(&source, 4_000, Mode::RePitch, 1_200);
    assert_eq!(result.frames.len(), source.len() / 2);
    assert!((peak_frequency(&result.frames, 860, 900) - 880.0).abs() <= 1.0);
    let high = prepare(&signal(8_000, 6_400.0), 4_000, Mode::RePitch, 0);
    assert!(rms(&high.frames[128..3_872]) < 0.005);
}
#[test]
fn distinct_stretch_modes_keep_stationary_pitch_and_antiphase_stereo() {
    let source = signal(8_000, 440.0);
    for mode in [
        Mode::Tones { grain_ms: 40 },
        Mode::Texture {
            grain_ms: 40,
            fluctuation: 0,
            seed: 7,
        },
        Mode::Complex,
        Mode::ComplexPro {
            formants: 100,
            envelope: 128,
        },
    ] {
        let result = prepare(&source, 12_000, mode, 0);
        let frequency = peak_frequency(&result.frames, 420, 460);
        assert!((frequency - 440.0).abs() <= 3.0, "{mode:?}: {frequency}");
        assert!(rms(&result.frames) > 0.1, "{mode:?}");
        assert!(
            result
                .frames
                .iter()
                .all(|sample| (sample[0] + sample[1]).abs() < 1e-5)
        );
        assert!(result.peak_scratch_bytes <= MAX_SCRATCH_BYTES);
    }
}
#[test]
fn source_envelope_runs_before_resampling_and_follows_loop_phase() {
    let source = vec![[1.0, -1.0]; 8];
    let map = map(16, 16);
    let envelope = [
        GainPoint {
            source_frame: 2,
            gain_q16: 0,
        },
        GainPoint {
            source_frame: 6,
            gain_q16: 65_536,
        },
    ];
    let result = render(
        &Request {
            attacks: &[],
            input: Input {
                frames: &source,
                sample_rate: RATE,
                region_start: 2,
                region_length: 4,
                phase: 1,
                looping: true,
                envelope: &envelope,
            },
            map: &map,
            output_frames: 16,
            pitch_cents: 0,
            mode: Mode::Resample,
        },
        &mut || false,
    )
    .unwrap();
    for (index, frame) in result.frames.iter().enumerate() {
        let gain = ((index + 1) % 4) as f32 / 4.0;
        assert_eq!(*frame, [gain, -gain]);
    }
}
#[test]
fn beats_attack_mapping_tail_modes_and_decay_are_real() {
    let mut source = vec![[0.25; 2]; 400];
    source[0] = [1.0; 2];
    source[200] = [1.0; 2];
    let off = prepare(
        &source,
        800,
        Mode::Beats {
            division_frames: Some(200),
            tail: Tail::Off,
            decay: 100,
        },
        0,
    );
    assert_eq!(off.frames[0], [1.0; 2]);
    // A real Off gap restarts with the original attack CHANGE above its
    // background; preserving raw DC here would manufacture a new step.
    assert_eq!(off.frames[400], source[200].map(|x| x - source[199][0]));
    assert_eq!(off.frames[300], [0.0; 2]);
    let forward = prepare(
        &source,
        800,
        Mode::Beats {
            division_frames: Some(200),
            tail: Tail::Forward,
            decay: 100,
        },
        0,
    );
    let alternating = prepare(
        &source,
        800,
        Mode::Beats {
            division_frames: Some(200),
            tail: Tail::Alternating,
            decay: 0,
        },
        0,
    );
    assert!(forward.frames[300][0] > 0.1);
    assert!(rms(&alternating.frames) < rms(&forward.frames) * 0.5);
    let transient = prepare(
        &source,
        800,
        Mode::Beats {
            division_frames: None,
            tail: Tail::Off,
            decay: 100,
        },
        0,
    );
    assert_eq!(
        transient.frames[400],
        source[200].map(|x| x - source[199][0])
    );
}
#[test]
fn texture_is_seeded_and_grain_size_changes_processing() {
    let source: Vec<_> = (0..4_000)
        .map(|i| {
            let value = ((i * 7_919 % 65_521) as f32 / 65_521.0 - 0.5) * 0.5;
            [value, -value]
        })
        .collect();
    let mode = Mode::Texture {
        grain_ms: 25,
        fluctuation: 80,
        seed: 7,
    };
    let first = prepare(&source, 6_000, mode, 0).frames;
    assert_eq!(first, prepare(&source, 6_000, mode, 0).frames);
    assert_ne!(
        first,
        prepare(
            &source,
            6_000,
            Mode::Texture {
                grain_ms: 25,
                fluctuation: 80,
                seed: 8
            },
            0
        )
        .frames
    );
    assert_ne!(
        first,
        prepare(
            &source,
            6_000,
            Mode::Texture {
                grain_ms: 60,
                fluctuation: 80,
                seed: 7
            },
            0
        )
        .frames
    );
}
#[test]
fn direct_requests_reject_bad_modes_extent_ratios_and_cancellation() {
    let source = signal(64, 440.0);
    let map = map(64, 64);
    let mut request = Request {
        attacks: &[],
        input: Input {
            frames: &source,
            sample_rate: RATE,
            region_start: 0,
            region_length: 64,
            phase: 0,
            looping: false,
            envelope: &[],
        },
        map: &map,
        output_frames: 64,
        pitch_cents: 0,
        mode: Mode::Complex,
    };
    assert!(matches!(
        render(&request, &mut || true),
        Err(Error::Cancelled)
    ));
    for mode in [
        Mode::Tones { grain_ms: 0 },
        Mode::Texture {
            grain_ms: 20,
            fluctuation: 101,
            seed: 0,
        },
        Mode::Beats {
            division_frames: Some(0),
            tail: Tail::Off,
            decay: 100,
        },
        Mode::ComplexPro {
            formants: 101,
            envelope: 128,
        },
    ] {
        request.mode = mode;
        assert!(render(&request, &mut || false).is_err());
    }
    request.mode = Mode::Complex;
    request.output_frames = 63;
    assert!(render(&request, &mut || false).is_err());
    assert!(map.position_q32(i128::MAX).is_err());
}

fn component_energy(frames: &[Frame], frequency: f64) -> f64 {
    let frames = &frames[2_048..frames.len() - 2_048];
    let mut re = 0.0;
    let mut im = 0.0;
    for (index, frame) in frames.iter().enumerate() {
        let phase = 2.0 * std::f64::consts::PI * frequency * index as f64 / f64::from(RATE);
        let window = 0.5
            - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / (frames.len() - 1) as f64).cos();
        re += f64::from(frame[0]) * window * phase.cos();
        im += f64::from(frame[0]) * window * phase.sin();
    }
    re * re + im * im
}
#[test]
fn complex_pro_reimposes_actual_formants_and_zero_transpose_is_an_exact_noop() {
    let source: Vec<_> = (0..12_000)
        .map(|i| {
            let time = i as f64 / f64::from(RATE);
            let value = (1..=60)
                .map(|harmonic| {
                    let frequency = harmonic as f64 * 100.0;
                    let envelope = (-0.5 * ((frequency - 1_200.0) / 400.0).powi(2)).exp()
                        + 0.4 * (-0.5 * ((frequency - 3_200.0) / 350.0).powi(2)).exp();
                    envelope * (2.0 * std::f64::consts::PI * frequency * time).sin() * 0.025
                })
                .sum::<f64>() as f32;
            [value, -value]
        })
        .collect();
    let shifted = prepare(&source, 16_000, Mode::Complex, 1_200).frames;
    let corrected = prepare(
        &source,
        16_000,
        Mode::ComplexPro {
            formants: 100,
            envelope: 128,
        },
        1_200,
    )
    .frames;
    let tween = prepare(
        &source,
        16_000,
        Mode::ComplexPro {
            formants: 50,
            envelope: 128,
        },
        1_200,
    )
    .frames;
    let ratio = |frames: &[Frame]| {
        component_energy(frames, 1_200.0) / component_energy(frames, 2_400.0).max(1e-12)
    };
    let shifted_ratio = ratio(&shifted);
    let corrected_ratio = ratio(&corrected);
    let tween_ratio = ratio(&tween);
    assert!(
        corrected_ratio > shifted_ratio * 4.0,
        "shifted={shifted_ratio} corrected={corrected_ratio}"
    );
    assert!(corrected_ratio > tween_ratio && tween_ratio > shifted_ratio);
    let dominant = |frames: &[Frame]| {
        (3..=15)
            .map(|harmonic| {
                let frequency = harmonic as f64 * 200.0;
                (component_energy(frames, frequency), frequency)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1
    };
    let shifted_peak = dominant(&shifted);
    let corrected_peak = dominant(&corrected);
    println!(
        "formant peaks shifted={shifted_peak} corrected={corrected_peak}; ratios={shifted_ratio}/{tween_ratio}/{corrected_ratio}"
    );
    assert!(
        (shifted_peak - 2_400.0).abs() <= 200.0,
        "shifted={shifted_peak}"
    );
    assert!(
        (corrected_peak - 1_200.0).abs() <= 24.0,
        "corrected={corrected_peak}"
    );
    let neutral = prepare(&source, 16_000, Mode::Complex, 0).frames;
    assert_eq!(
        neutral,
        prepare(
            &source,
            16_000,
            Mode::ComplexPro {
                formants: 100,
                envelope: 32
            },
            0
        )
        .frames
    );
    assert_ne!(
        corrected,
        prepare(
            &source,
            16_000,
            Mode::ComplexPro {
                formants: 100,
                envelope: 512
            },
            1_200
        )
        .frames
    );
}

#[test]
fn nonzero_transpose_is_independent_of_duration_in_tonal_texture_and_spectral_modes() {
    let source = signal(8_000, 440.0);
    let expected = 440.0 * 2.0_f64.powf(700.0 / 1_200.0);
    for mode in [
        Mode::Tones { grain_ms: 40 },
        Mode::Texture {
            grain_ms: 40,
            fluctuation: 0,
            seed: 1,
        },
        Mode::Complex,
    ] {
        let result = prepare(&source, 12_000, mode, 700).frames;
        let frequency = peak_frequency(&result, 640, 680);
        assert!(
            (frequency - expected).abs() <= 4.0,
            "{mode:?}: measured {frequency}, expected {expected}"
        );
    }
}

#[test]
fn transient_detector_has_no_warmup_attacks_on_sustained_material_at_different_rates() {
    for rate in [16_000, 48_000] {
        let length = rate as usize / 10;
        for tone in [false, true] {
            let source: Vec<_> = (0..length)
                .map(|i| {
                    let x = if tone {
                        (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(rate)).sin()
                            as f32
                            * 0.25
                    } else {
                        0.25
                    };
                    [x, -x]
                })
                .collect();
            let map = map(length, length * 2);
            let mut request = Request {
                attacks: &[],
                input: Input {
                    frames: &source,
                    sample_rate: rate,
                    region_start: 0,
                    region_length: length as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames: length * 2,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail: Tail::Off,
                    decay: 100,
                },
            };
            let transient = render(&request, &mut || false).unwrap().frames;
            request.mode = Mode::Beats {
                division_frames: Some(length as u64),
                tail: Tail::Off,
                decay: 100,
            };
            let single = render(&request, &mut || false).unwrap().frames;
            assert_eq!(transient, single, "rate {rate}, tone {tone}");
        }
    }
}
#[test]
fn complex_linked_stereo_keeps_independent_channel_partial_phase_and_level() {
    let source: Vec<_> = (0..8_000)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            [
                (0.3 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
                    + 0.08 * (2.0 * std::f64::consts::PI * 880.0 * t).sin()) as f32,
                (0.1 * (2.0 * std::f64::consts::PI * 440.0 * t + 0.7).sin()
                    + 0.32 * (2.0 * std::f64::consts::PI * 880.0 * t - 0.4).sin())
                    as f32,
            ]
        })
        .collect();
    let output = prepare(&source, 12_000, Mode::Complex, 0).frames;
    let amplitude_phase = |channel: usize, frequency: f64| {
        let mut re = 0.0;
        let mut im = 0.0;
        for (i, sample) in output[2_048..10_000].iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * frequency * i as f64 / f64::from(RATE);
            re += f64::from(sample[channel]) * phase.cos();
            im -= f64::from(sample[channel]) * phase.sin();
        }
        (re.hypot(im), im.atan2(re))
    };
    for (frequency, ratio, delta) in [(440.0, 1.0 / 3.0, 0.7), (880.0, 4.0, -0.4)] {
        let left = amplitude_phase(0, frequency);
        let right = amplitude_phase(1, frequency);
        assert!((right.0 / left.0 - ratio).abs() < 0.08);
        let phase = (right.1 - left.1 - delta + std::f64::consts::PI)
            .rem_euclid(2.0 * std::f64::consts::PI)
            - std::f64::consts::PI;
        assert!(phase.abs() < 0.06, "{frequency}: {phase}");
    }
}

#[test]
fn finite_short_silence_dc_nyquist_and_stretch_edges() {
    for source in [
        vec![[0.0; 2]; 64],
        vec![[0.25, -0.25]; 64],
        (0..64)
            .map(|i| {
                let x = if i % 2 == 0 { 0.25 } else { -0.25 };
                [x, -x]
            })
            .collect(),
    ] {
        for output in [8, 64, 512] {
            for mode in [
                Mode::RePitch,
                Mode::Beats {
                    division_frames: Some(32),
                    tail: Tail::Alternating,
                    decay: 100,
                },
                Mode::Tones { grain_ms: 8 },
                Mode::Texture {
                    grain_ms: 8,
                    fluctuation: 100,
                    seed: 1,
                },
                Mode::Complex,
                Mode::ComplexPro {
                    formants: 100,
                    envelope: 32,
                },
            ] {
                let result = prepare(&source, output, mode, 0);
                assert!(
                    result
                        .frames
                        .iter()
                        .flatten()
                        .all(|sample| sample.is_finite())
                );
            }
        }
    }
}
