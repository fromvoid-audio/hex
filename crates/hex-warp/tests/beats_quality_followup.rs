//! Generated seams are measured against the original source edge, not silence.
//! Automated continuity evidence is separate from musical listening acceptance.
use hex_warp::*;

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
fn prepare(source: &[Frame], rate: u32, output: usize, tail: Tail) -> Vec<Frame> {
    let map = linear(source.len(), output);
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
            attacks: &[],
            map: &map,
            output_frames: output,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: Some((source.len() / 4) as u64),
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
fn beats_continuous_division_joins_do_not_manufacture_zero_to_dc_edges() {
    for rate in [8_000, 44_100, 48_000, 96_000] {
        let length = rate as usize / 2;
        let source = vec![[0.5, -0.25]; length];
        for ratio in [0.125, 0.5, 1.0, 1.375, 2.0, 8.0] {
            for tail in [Tail::Forward, Tail::Alternating] {
                let output = (length as f64 * ratio).round() as usize;
                let pcm = prepare(&source, rate, output, tail);
                for boundary in 1..4 {
                    let src = length / 4 * boundary;
                    let at = linear(length, output)
                        .output_for_source((src as i128) << 32)
                        .unwrap() as usize;
                    let jump = (pcm[at][0] - pcm[at - 1][0]).abs();
                    assert!(
                        jump < 0.01,
                        "generated DC edge {rate}/{ratio}/{tail:?} at {at}: {jump}"
                    );
                }
            }
        }
    }
}

#[test]
fn beats_neighbor_join_preserves_original_tone_edge_in_both_channels() {
    for rate in [16_000, 44_100, 48_000] {
        let length = rate as usize / 2;
        let source: Vec<Frame> = (0..length)
            .map(|i| {
                let phase = std::f64::consts::TAU * 73.0 * i as f64 / f64::from(rate);
                [
                    (0.15 + 0.5 * phase.sin()) as f32,
                    (-0.1 + 0.3 * (phase + 0.7).sin()) as f32,
                ]
            })
            .collect();
        for ratio in [0.5, 1.0, 1.375, 2.0, 8.0] {
            let output = (length as f64 * ratio).round() as usize;
            for tail in [Tail::Forward, Tail::Alternating] {
                let pcm = prepare(&source, rate, output, tail);
                for boundary in 1..4 {
                    let src = length / 4 * boundary;
                    let at = linear(length, output)
                        .output_for_source((src as i128) << 32)
                        .unwrap() as usize;
                    for channel in 0..2 {
                        let original = source[src][channel] - source[src - 1][channel];
                        let actual = pcm[at][channel] - pcm[at - 1][channel];
                        assert!(
                            (actual - original).abs() < 0.015,
                            "generated tone edge {rate}/{ratio}/{tail:?}/{channel}: {actual} vs original {original}"
                        );
                    }
                }
            }
        }
    }
}
