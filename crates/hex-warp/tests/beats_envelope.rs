//! Authored transient decay is once per selected logical segment.
use hex_warp::*;
fn render_decay(
    source: &[Frame],
    map: &TimeMap,
    output: usize,
    decay: u8,
    division: Option<u64>,
    points: &[u64],
) -> Rendered {
    render(
        &Request {
            input: Input {
                frames: source,
                sample_rate: 16_000,
                region_start: 0,
                region_length: source.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            attacks: points,
            map,
            output_frames: output,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: division,
                tail: Tail::Forward,
                decay,
            },
        },
        &mut || false,
    )
    .unwrap()
}
#[test]
fn forward_envelope_is_once_per_preserve_segment_not_per_grain_or_internal_attack() {
    let mut source = vec![[0.4, -0.2]; 8_000];
    source[1_000] = [0.9, -0.7];
    source[2_573] = [0.8, -0.6];
    for ratio in [1.0, 1.375, 2.0] {
        let output = (source.len() as f64 * ratio) as usize;
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: output as u64,
                source_start_q32: 0,
                source_end_q32: (source.len() as i128) << 32,
            }],
            output,
        )
        .unwrap();
        for (division, points, expected_source_points) in [
            (Some(4_000), &[][..], &[0, 4_000, 8_000][..]),
            (
                None,
                &[0, 2_003, 4_010, 7_777][..],
                &[0, 2_003, 4_010, 7_777, 8_000][..],
            ),
        ] {
            let unity = render_decay(&source, &map, output, 100, division, points);
            if ratio == 1.0 {
                assert_eq!(unity.frames, source);
            }
            let edges: Vec<_> = expected_source_points
                .iter()
                .map(|x| map.output_for_source(i128::from(*x) << 32).unwrap() as usize)
                .collect();
            for decay in [0, 20, 50, 80, 99, 100] {
                let actual = render_decay(&source, &map, output, decay, division, points);
                for pair in edges.windows(2) {
                    for at in pair[0]..pair[1] {
                        let gain = (-12.0 * f64::from(100 - decay) / 100.0 * (at - pair[0]) as f64
                            / (pair[1] - pair[0]) as f64)
                            .exp();
                        for channel in 0..2 {
                            let expected = f64::from(unity.frames[at][channel]) * gain;
                            assert!(
                                (f64::from(actual.frames[at][channel]) - expected).abs() < 1e-7,
                                "envelope restarted/multiplied at grain or physical attack ratio={ratio} decay={decay} frame={at}"
                            );
                        }
                    }
                }
                if decay == 100 {
                    assert_eq!(actual.frames, unity.frames);
                }
                assert!(actual.peak_scratch_bytes <= MAX_SCRATCH_BYTES);
            }
        }
    }
}
#[test]
fn forward_envelope_grid_density_still_refuses_before_unbounded_schedule() {
    let source = vec![[0.1; 2]; 2_000];
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 4_000,
            source_start_q32: 0,
            source_end_q32: 2_000_i128 << 32,
        }],
        4_000,
    )
    .unwrap();
    for decay in [0, 100] {
        assert!(matches!(
            render(
                &Request {
                    input: Input {
                        frames: &source,
                        sample_rate: 16_000,
                        region_start: 0,
                        region_length: 2_000,
                        phase: 0,
                        looping: false,
                        envelope: &[]
                    },
                    attacks: &[],
                    map: &map,
                    output_frames: 4_000,
                    pitch_cents: 0,
                    mode: Mode::Beats {
                        division_frames: Some(1),
                        tail: Tail::Forward,
                        decay
                    },
                },
                &mut || false
            ),
            Err(Error::Invalid("beat divisions exceed bounded source plan"))
        ));
    }
}
