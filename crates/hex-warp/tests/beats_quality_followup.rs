//! Generated seams are measured against the original source edge, not silence.
//! Automated continuity evidence is separate from musical listening acceptance.
use hex_warp::*;
#[path = "support/quality.rs"]
mod quality;

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
                let output = (length as f64 * ratio).ceil() as usize;
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
fn beats_neighbor_joins_add_no_edges_above_natural_tone_slope() {
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
                        // Neither Forward nor normalized reflected OLA promises
                        // the old source phase at an arbitrary grid point.
                        // Bound every local edge against the actual natural
                        // source slope, including the grid neighborhood.
                        let limit = source
                            .windows(2)
                            .map(|w| (w[1][channel] - w[0][channel]).abs())
                            .fold(0.0_f32, f32::max)
                            * 1.2
                            + 1e-6;
                        for pair in pcm[at.saturating_sub(64)..(at + 65).min(pcm.len())].windows(2)
                        {
                            assert!(
                                (pair[1][channel] - pair[0][channel]).abs() <= limit,
                                "generated OLA grid edge {rate}/{ratio}/{tail:?}/{channel}: {:?} limit={limit}",
                                pair
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn beats_off_gap_restart_preserves_attack_change_without_preecho_or_dc_click() {
    for rate in [8_000, 44_100, 48_000, 96_000] {
        let length = rate as usize / 2;
        let division = length / 4;
        for background in [[0.5, -0.25], [0.0, 0.0], [0.2, 0.0], [-0.3, 0.3]] {
            let mut source = vec![background; length];
            let background_only = prepare(&source, rate, length * 2, Tail::Off);
            source[division] = [background[0] + 0.35, background[1] - 0.2];
            let output = length * 2;
            let pcm = prepare(&source, rate, output, Tail::Off);
            let attack = linear(length, output)
                .output_for_source((division as i128) << 32)
                .unwrap() as usize;
            assert!(pcm[division..attack].iter().all(|frame| *frame == [0.0; 2]));
            assert!((pcm[attack][0] - 0.35).abs() < 1e-6);
            assert!((pcm[attack][1] + 0.2).abs() < 1e-6);
            assert_eq!(pcm[attack + rate as usize / 500 + 2], background);
            assert_eq!(
                &pcm[attack + 1..attack + rate as usize / 500],
                &background_only[attack + 1..attack + rate as usize / 500],
                "the impulse must not affect the background's ordinary restart"
            );
        }
    }
}

#[test]
fn beats_off_incoherent_zero_mean_background_does_not_become_an_offset_pulse() {
    for rate in [16_000, 48_000, 96_000] {
        let length = rate as usize / 2;
        let source: Vec<Frame> = (0..length)
            .map(|i| {
                let noise = if i % 2 == 0 { 0.3 } else { -0.3 };
                [noise, -noise * 0.7]
            })
            .collect();
        let pcm = prepare(&source, rate, length * 2, Tail::Off);
        let attack = length / 2;
        assert!(pcm[length / 4..attack].iter().all(|s| *s == [0.0; 2]));
        assert_eq!(
            &pcm[attack..attack + rate as usize / 500],
            &source[length / 4..length / 4 + rate as usize / 500]
        );
    }
}

#[test]
fn beats_forward_sustain_preserves_periodic_pitch_and_level_after_source_exhaustion() {
    for rate in [16_000, 48_000] {
        for hz in [55.0, 220.0, 997.0] {
            let length = rate as usize / 7;
            let source: Vec<Frame> = (0..length)
                .map(|i| {
                    let value = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate) + 0.4)
                        .sin() as f32
                        * 0.5;
                    [value, -value * 0.7]
                })
                .collect();
            let output = length * 4;
            let map = linear(length, output);
            let pcm = render(
                &Request {
                    input: Input {
                        frames: &source,
                        sample_rate: rate,
                        region_start: 0,
                        region_length: length as u64,
                        phase: 0,
                        looping: false,
                        envelope: &[],
                    },
                    attacks: &[0],
                    map: &map,
                    output_frames: output,
                    pitch_cents: 0,
                    mode: Mode::Beats {
                        division_frames: None,
                        tail: Tail::Forward,
                        decay: 100,
                    },
                },
                &mut || false,
            )
            .unwrap()
            .frames;
            let begin = length + rate as usize / 100;
            let end = output - rate as usize / 100;
            // Stretching does not promise absolute phase against an
            // indefinitely continued source oscillator. Fit the EXPECTED
            // frequency with free phase/DC; added sidebands, drift or gating
            // still contribute to this independent residual.
            let (error, rms) = quality::residual(&pcm[begin..end], rate, hz);
            assert!(
                error < 0.025,
                "sustain residual rate={rate}/hz={hz}: {error}"
            );
            assert!(
                (0.33..0.38).contains(&rms),
                "sustain gain changed {rate}/{hz}: {rms}"
            );
            // Channels share schedule and coefficients. Independent f32 input
            // and output rounding is permitted; a channel shift is not.
            assert!(pcm.iter().all(|s| (s[1] + s[0] * 0.7).abs() < 1e-6));
        }
    }
}
