//! Physical attacks and scheduling grids have different responsibilities.
use hex_warp::*;
fn prepared(
    source: &[Frame],
    map: &TimeMap,
    output: usize,
    division: Option<u64>,
    attacks: &[u64],
    pitch: i16,
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
            attacks,
            map,
            output_frames: output,
            pitch_cents: pitch,
            mode: Mode::Beats {
                division_frames: division,
                tail: Tail::Forward,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
}
#[test]
fn forward_unity_envelope_grid_points_do_not_create_attacks_or_dips() {
    let source: Vec<_> = (0..8_000)
        .map(|i| {
            let x = (std::f64::consts::TAU * 73.0 * i as f64 / 16_000.0 + 0.4).sin() as f32 * 0.5;
            [x + 0.1, -x * 0.7 - 0.2]
        })
        .collect();
    let output = 16_000;
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
    let actual = prepared(&source, &map, output, None, &[], 0).frames;
    assert_eq!(
        prepared(&source, &map, output, Some(200), &[], 0).frames,
        actual
    );
    assert_eq!(
        prepared(&source, &map, output, None, &[0, 401, 1003, 2307, 7101], 0).frames,
        actual
    );
    for at in [802, 2006, 4614, 14202] {
        let step = (actual[at][0] - actual[at - 1][0]).abs();
        assert!(
            step < 0.03,
            "scheduling boundary manufactured an edge at {at}: {step}"
        );
    }
}
#[test]
fn piecewise_map_keeps_close_attacks_at_exact_inverse_positions_and_fractional_pitch_finite() {
    let mut source = vec![[0.0; 2]; 8_000];
    for at in [0, 2_000, 2_032, 4_001, 6_777] {
        source[at] = [0.9, -0.7];
    }
    let map = TimeMap::new(
        vec![
            MapSegment {
                output_start: 0,
                output_end: 4_000,
                source_start_q32: 0,
                source_end_q32: 3_000_i128 << 32,
            },
            MapSegment {
                output_start: 4_000,
                output_end: 8_000,
                source_start_q32: 3_000_i128 << 32,
                source_end_q32: 5_000_i128 << 32,
            },
            MapSegment {
                output_start: 8_000,
                output_end: 12_000,
                source_start_q32: 5_000_i128 << 32,
                source_end_q32: 8_000_i128 << 32,
            },
        ],
        12_000,
    )
    .unwrap();
    let pcm = prepared(&source, &map, 12_000, Some(500), &[], 0);
    let expected: Vec<_> = [0, 2_000, 2_032, 4_001, 6_777]
        .iter()
        .map(|at| map.output_for_source(i128::from(*at) << 32).unwrap() as usize)
        .collect();
    let actual: Vec<_> = pcm
        .frames
        .iter()
        .enumerate()
        .filter_map(|(i, x)| (x[0] > 0.8).then_some(i))
        .collect();
    assert_eq!(
        actual, expected,
        "close attacks cannot disappear into a fixed refractory window"
    );
    for pitch in [-713, 307, 1200] {
        let pitched = prepared(&source, &map, 12_000, None, &[], pitch);
        assert_eq!(pitched.frames.len(), 12_000);
        assert!(
            pitched
                .frames
                .iter()
                .flatten()
                .all(|x| x.is_finite() && x.abs() <= 1.1)
        );
        assert_eq!(
            pitched.frames,
            prepared(&source, &map, 12_000, None, &[], pitch).frames
        );
        for (source_at, at) in [0, 2_000, 2_032, 4_001, 6_777].iter().zip(&expected) {
            // A close PREVIOUS attack can legitimately have a bandlimited
            // tail here. Change only the FUTURE impulse, preserving its energy
            // and detected onset, and require identical pre-onset samples.
            let mut changed = source.clone();
            changed[*source_at] = [-0.9, 0.7];
            let alternate = prepared(&changed, &map, 12_000, None, &[], pitch);
            assert_eq!(
                pitched.frames[at.saturating_sub(8)..*at],
                alternate.frames[at.saturating_sub(8)..*at],
                "future attack leaked backwards pitch={pitch} source={source_at} output={at}"
            );
        }
    }
}
