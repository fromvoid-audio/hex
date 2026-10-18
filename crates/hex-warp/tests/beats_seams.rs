//! Original deterministic seam fixtures; abrupt source attacks are excluded
//! from continuity measurements. These tests are not listening acceptance.
use hex_warp::*;

fn render_tail(source: &[Frame], tail: Tail, rate: u32) -> Vec<Frame> {
    let frames = source.len() * 2;
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: frames as u64,
            source_start_q32: 0,
            source_end_q32: (source.len() as i128) << 32,
        }],
        frames,
    )
    .unwrap();
    render(
        &Request {
            input: Input {
                frames: source,
                sample_rate: rate,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: &[0],
            map: &map,
            output_frames: frames,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: None,
                tail,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
    .frames
}

#[test]
fn beats_forward_tail_removes_reset_click_without_repeating_attack_or_collapsing_stereo() {
    for rate in [16_000, 44_100, 48_000, 96_000] {
        let length = (rate / 13) as usize;
        let mut source: Vec<Frame> = (0..length)
            .map(|i| {
                let phase = std::f64::consts::TAU * 73.0 * i as f64 / rate as f64;
                [
                    (0.22 + 0.38 * phase.sin()) as f32,
                    (-0.17 + 0.29 * (phase + 0.7).sin()) as f32,
                ]
            })
            .collect();
        source[0] = [1.0, -0.9];
        let pcm = render_tail(&source, Tail::Forward, rate);
        assert_eq!(
            &pcm[..length / 4],
            &source[..length / 4],
            "original attack is unchanged"
        );
        let worst = pcm[length - (rate / 200) as usize..pcm.len() - (rate / 200) as usize]
            .windows(2)
            .flat_map(|pair| (0..2).map(move |c| (pair[1][c] - pair[0][c]).abs()))
            .fold(0.0_f32, f32::max);
        println!("forward rate={rate} worst nonattack step={worst}");
        assert!(worst < 0.045, "{rate}: avoidable tail reset jump {worst}");
        assert!(
            pcm[length..].iter().all(|s| s[0] < 0.7 && s[1] > -0.7),
            "tail must not repeat attack"
        );
        assert!(
            pcm.iter()
                .all(|s| s.iter().all(|x| x.is_finite() && x.abs() <= 1.01))
        );
    }
}

#[test]
fn beats_off_nonzero_segment_end_reaches_silence_without_a_full_scale_step() {
    let source = vec![[0.5, -0.25]; 800];
    let pcm = render_tail(&source, Tail::Off, 48_000);
    assert_eq!(&pcm[..600], &source[..600]);
    assert_eq!(&pcm[800..], &[[0.0; 2]; 800]);
    let worst = pcm[600..900]
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(worst < 0.025, "avoidable Off-to-silence step {worst}");
}

#[test]
fn beats_back_and_forth_reflects_sample_centers_without_endpoint_duplication() {
    let source: Vec<_> = (0..64)
        .map(|i| [i as f32 / 100.0, -i as f32 / 200.0])
        .collect();
    let pcm = render_tail(&source, Tail::Alternating, 8_000);
    assert_eq!(&pcm[..64], &source);
    // First reversal walks 62,61,... after the original sample63. It must
    // neither replay63 nor jump back to the attack or beyond this segment.
    for i in 0..16 {
        assert_eq!(pcm[64 + i], source[62 - i]);
    }
}

#[test]
fn beats_fractional_pitch_filter_cannot_read_the_next_transient_into_a_tail() {
    let mut source = vec![[0.1, -0.2]; 400];
    source[200] = [1.0, -1.0];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 800,
            source_start_q32: 0,
            source_end_q32: 400_i128 << 32,
        }],
        800,
    )
    .unwrap();
    for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
        let pcm = render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: 48_000,
                    region_start: 0,
                    region_length: 400,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[0, 200],
                map: &map,
                output_frames: 800,
                pitch_cents: 700,
                mode: Mode::Beats {
                    division_frames: None,
                    tail,
                    decay: 100,
                },
            },
            &mut || false,
        )
        .unwrap()
        .frames;
        assert!(
            pcm[..400]
                .iter()
                .all(|s| s[0] <= 0.10001 && s[1] >= -0.20001),
            "next attack leaked into {tail:?}"
        );
    }
}

#[test]
fn beats_tail_modes_keep_fractional_attack_times_and_stereo_schedule() {
    for frames in [360, 720, 1_137] {
        let mut source: Vec<Frame> = (0..480)
            .map(|i| {
                let x = (std::f64::consts::TAU * 73.0 * i as f64 / 16_000.0).sin() as f32 * 0.1;
                [x + 0.05, -x * 0.7 - 0.08]
            })
            .collect();
        source[0] = [1.0, -0.7];
        source[240] = [0.8, -0.9];
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: frames,
                source_start_q32: 0,
                source_end_q32: 480_i128 << 32,
            }],
            frames as usize,
        )
        .unwrap();
        let attack = map.output_for_source(240_i128 << 32).unwrap() as usize;
        for tail in [Tail::Off, Tail::Forward, Tail::Alternating] {
            let prepare = |source: &[Frame]| {
                render(
                    &Request {
                        input: Input {
                            frames: source,
                            sample_rate: 16_000,
                            region_start: 0,
                            region_length: 480,
                            phase: 0,
                            looping: false,
                            envelope: &[],
                        },
                        attacks: &[0, 240],
                        map: &map,
                        output_frames: frames as usize,
                        pitch_cents: 0,
                        mode: Mode::Beats {
                            division_frames: None,
                            tail,
                            decay: 100,
                        },
                    },
                    &mut || false,
                )
                .unwrap()
                .frames
            };
            let pcm = prepare(&source);
            assert_eq!(pcm[0], source[0]);
            if tail == Tail::Off && attack > 240 {
                // The approved Off restart law preserves the attack change,
                // while its preceding silence remains exact (no pre-roll).
                for c in 0..2 {
                    let original_edge = source[240][c] - source[239][c];
                    assert!((pcm[attack][c] - original_edge).abs() < 0.005);
                }
                assert!(pcm[240..attack].iter().all(|frame| *frame == [0.0; 2]));
            } else {
                assert_eq!(pcm[attack], source[240]);
            }
            assert!(
                pcm.iter()
                    .enumerate()
                    .all(|(i, s)| i == 0 || i == attack || s[0].abs() < 0.3),
                "attack replay {tail:?}/{frames}"
            );
            let swapped: Vec<_> = source.iter().map(|s| [s[1], s[0]]).collect();
            assert!(
                pcm.iter()
                    .zip(prepare(&swapped))
                    .all(|(a, b)| *a == [b[1], b[0]])
            );
        }
    }
}

#[test]
fn beats_work_budget_charges_overlap_reads_without_doubling_stop_playback() {
    let source = vec![[0.1; 2]; 4_000];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 8_000,
            source_start_q32: 0,
            source_end_q32: 4_000_i128 << 32,
        }],
        8_000,
    )
    .unwrap();
    let prepare = |tail| {
        render(
            &Request {
                input: Input {
                    frames: &source,
                    sample_rate: 48_000,
                    region_start: 0,
                    region_length: 4_000,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[0],
                map: &map,
                output_frames: 8_000,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail,
                    decay: 100,
                },
            },
            &mut || false,
        )
        .unwrap()
    };
    let stop = prepare(Tail::Off);
    let forward = prepare(Tail::Forward);
    assert!(
        forward.work_units > stop.work_units,
        "the additional tail reads must be charged"
    );
    assert!(
        forward.work_units < stop.work_units * 2,
        "single-read playback must not pay for a second read on every frame"
    );
}
