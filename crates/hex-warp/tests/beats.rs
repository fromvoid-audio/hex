//! Independent PCM checks for transient-based clip stretching, not beat fitting.
use hex_warp::*;

const RATE: u32 = 16_000;
fn beats() -> Mode {
    Mode::Beats {
        division_frames: None,
        tail: Tail::Forward,
        decay: 100,
    }
}
fn linear(source: usize, output: usize) -> TimeMap {
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
fn stretch(input: Input<'_>, map: &TimeMap, frames: usize, pitch: i16) -> Vec<Frame> {
    render(
        &Request {
            attacks: &[],
            input,
            map,
            output_frames: frames,
            pitch_cents: pitch,
            mode: beats(),
        },
        &mut || false,
    )
    .unwrap()
    .frames
}
fn input(frames: &[Frame]) -> Input<'_> {
    Input {
        frames,
        sample_rate: RATE,
        region_start: 0,
        region_length: frames.len() as u64,
        phase: 0,
        looping: false,
        envelope: &[],
    }
}
#[test]
fn automatic_transients_follow_piecewise_time_map_with_one_stereo_schedule() {
    let mut source = vec![[0.0; 2]; 1_600];
    // Unequal, independent channels: either side must trigger the shared schedule.
    source[200] = [1.0, 0.25];
    source[600] = [0.0, -0.75];
    source[1_200] = [0.5, 0.0];
    let map = TimeMap::new(
        vec![
            MapSegment {
                output_start: 0,
                output_end: 1_600,
                source_start_q32: 0,
                source_end_q32: 800_i128 << 32,
            },
            MapSegment {
                output_start: 1_600,
                output_end: 2_000,
                source_start_q32: 800_i128 << 32,
                source_end_q32: 1_600_i128 << 32,
            },
        ],
        2_000,
    )
    .unwrap();
    let result = stretch(input(&source), &map, 2_000, 0);
    let mut expected = vec![[0.0; 2]; 2_000];
    expected[400] = source[200];
    expected[1_200] = source[600];
    expected[1_800] = source[1_200];
    assert_eq!(
        result, expected,
        "attacks move in time without stretching their PCM"
    );
    let swapped: Vec<_> = source.iter().map(|frame| [frame[1], frame[0]]).collect();
    let swapped_result = stretch(input(&swapped), &map, 2_000, 0);
    assert!(
        result
            .iter()
            .zip(swapped_result)
            .all(|(a, b)| *a == [b[1], b[0]])
    );
}
#[test]
fn unity_is_exact_through_trim_phase_and_loop_boundaries() {
    let source: Vec<_> = (0..389)
        .map(|i| {
            [
                ((i * 43 % 101) as f32 - 50.0) / 64.0,
                ((i * 17 % 79) as f32 - 39.0) / 64.0,
            ]
        })
        .collect();
    for (start, length, phase, looping, output) in [
        (0, 389, 0, false, 389),
        (13, 173, 0, false, 173),
        (13, 173, 61, true, 601),
    ] {
        let input = Input {
            region_start: start,
            region_length: length,
            phase,
            looping,
            ..input(&source)
        };
        let actual = stretch(input, &linear(output, output), output, 0);
        let expected: Vec<_> = (0..output)
            .map(|i| source[start as usize + (i + phase as usize) % length as usize])
            .collect();
        assert_eq!(actual, expected);
    }
}
#[test]
fn stretch_duration_and_transpose_are_independent_on_percussive_tone() {
    let source: Vec<_> = (0..8_000)
        .map(|i| {
            let age = i % 4_000;
            let value = (std::f64::consts::TAU * 400.0 * age as f64 / f64::from(RATE)).sin()
                * (-age as f64 / 2_000.0).exp()
                * 0.5;
            [value as f32, -value as f32 * 0.25]
        })
        .collect();
    for (frames, pitch, expected_crossings) in
        [(12_000, 0, 40), (12_000, 1_200, 80), (4_000, 0, 40)]
    {
        let result = stretch(input(&source), &linear(source.len(), frames), frames, pitch);
        assert_eq!(result.len(), frames);
        // A 1,600-frame interior window has exactly40 cycles at400Hz,80 at800Hz.
        let crossings = result[200..1_801]
            .windows(2)
            .filter(|w| w[0][0] <= 0.0 && w[1][0] > 0.0)
            .count();
        assert!(
            crossings.abs_diff(expected_crossings) <= 1,
            "{frames}/{pitch}: {crossings}"
        );
        assert!(
            result
                .iter()
                .all(|f| f[0].is_finite() && f[1] == -f[0] * 0.25)
        );
    }
}
#[test]
fn silence_short_sources_and_extreme_ratios_are_finite_and_deterministic() {
    for length in [1, 2, 17, 800] {
        for ratio in [0.125, 1.0, 8.0] {
            let frames = ((length as f64 * ratio).ceil() as usize).max(1);
            if frames as f64 / (length as f64) > 8.0 {
                continue;
            }
            for value in [0.0, 0.125] {
                let source = vec![[value, -value]; length];
                let map = linear(length, frames);
                let result = stretch(input(&source), &map, frames, 0);
                assert_eq!(result, stretch(input(&source), &map, frames, 0));
                assert!(
                    result
                        .iter()
                        .all(|f| f[0].is_finite() && f[0] + f[1] == 0.0)
                );
                if value == 0.0 {
                    assert!(result.iter().all(|f| *f == [0.0; 2]));
                }
            }
        }
    }
}
