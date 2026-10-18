//! Reflected sustain is normalized stereo reconstruction, not raw slice indexing.
use hex_warp::*;
#[path = "support/quality.rs"]
mod quality;
fn prepare(source: &[Frame], output: usize, tail: Tail, division: Option<u64>) -> Rendered {
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
            attacks: &[],
            map: &map,
            output_frames: output,
            pitch_cents: 0,
            mode: Mode::Beats {
                division_frames: division,
                tail,
                decay: 100,
            },
        },
        &mut || false,
    )
    .unwrap()
}
#[test]
fn alternating_preserves_dc_stereo_and_continuous_tone_energy_across_turns_and_grids() {
    for division in [None, Some(1_000)] {
        let dc = vec![[0.4, -0.2]; 4_000];
        let pcm = prepare(&dc, 16_000, Tail::Alternating, division);
        assert!(pcm.frames.iter().all(|s| *s == dc[0]));
        for hz in [55.0, 220.0, 997.0] {
            let source: Vec<_> = (0..4_000)
                .map(|i| {
                    let x =
                        (std::f64::consts::TAU * hz * i as f64 / 16_000.0 + 0.3).sin() as f32 * 0.5;
                    [x, -x * 0.7]
                })
                .collect();
            let pcm = prepare(&source, 16_000, Tail::Alternating, division);
            let (residual, rms) = quality::residual(&pcm.frames[1_000..15_000], 16_000, hz);
            let peak_step = pcm
                .frames
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).abs())
                .fold(0.0_f32, f32::max);
            let natural_step = source
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).abs())
                .fold(0.0_f32, f32::max);
            eprintln!(
                "alternating hz={hz} division={division:?} residual={residual} rms={rms} peakstep={peak_step} source={natural_step}"
            );
            assert!((0.30..0.40).contains(&rms));
            assert!(
                residual < 0.025,
                "reversal damaged tone pitch/continuity {hz}/{division:?}: {residual}"
            );
            assert!(peak_step <= natural_step * 1.5 + 0.005);
            assert!(pcm.frames.iter().all(|s| (s[1] + s[0] * 0.7).abs() < 1e-6));
            assert_eq!(
                prepare(&source, 16_000, Tail::Alternating, division).frames,
                pcm.frames
            );
        }
    }
}
#[test]
fn alternating_never_reverses_or_duplicates_real_attacks_and_compression_is_forward() {
    let mut source = vec![[0.1, -0.05]; 8_000];
    for at in [0, 2_000, 4_000, 6_000] {
        source[at] = [0.9, -0.8];
    }
    for division in [None, Some(1_000)] {
        let pcm = prepare(&source, 16_000, Tail::Alternating, division);
        let attacks: Vec<_> = pcm
            .frames
            .iter()
            .enumerate()
            .filter_map(|(i, x)| (x[0] > 0.8).then_some(i))
            .collect();
        assert_eq!(attacks, [0, 4_000, 8_000, 12_000]);
        assert_eq!(
            prepare(&source, 4_000, Tail::Alternating, division).frames,
            prepare(&source, 4_000, Tail::Forward, division).frames
        );
    }
}
