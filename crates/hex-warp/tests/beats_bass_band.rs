//! Sub-bass/fractional-pitch evidence beyond the initial 55-Hz fixture.
use hex_warp::*;
#[path = "support/quality.rs"]
mod quality;
#[test]
fn forward_sub_bass_keeps_pitch_and_level_at_ordinary_and_extreme_ratios() {
    for rate in [16_000, 48_000] {
        for hz in [27.5, 41.20344461410875, 73.0] {
            let source: Vec<Frame> = (0..rate / 2)
                .map(|i| {
                    let x = (std::f64::consts::TAU * hz * f64::from(i) / f64::from(rate) + 0.4)
                        .sin() as f32
                        * 0.5;
                    [x, -x * 0.7]
                })
                .collect();
            for ratio in [0.5, 1.375, 2.0, 8.0] {
                for cents in [0, -713, 307] {
                    let output = (source.len() as f64 * ratio).round() as usize;
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
                    let rendered = render(
                        &Request {
                            input: Input {
                                frames: &source,
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
                            pitch_cents: cents,
                            mode: Mode::Beats {
                                division_frames: None,
                                tail: Tail::Forward,
                                decay: 100,
                            },
                        },
                        &mut || false,
                    );
                    let compensated = ratio * 2.0_f64.powf(f64::from(cents) / 1200.0);
                    if !(0.125..=8.0).contains(&compensated) {
                        assert!(matches!(rendered, Err(Error::Invalid(_))));
                        continue;
                    }
                    let pcm = rendered.unwrap();
                    let material = &pcm.frames
                        [(rate as usize / 20).min(output / 8)..output - rate as usize / 100];
                    let target = hz * 2.0_f64.powf(f64::from(cents) / 1200.0);
                    let (error, rms) = quality::residual(material, rate, target);
                    println!(
                        "bass band {rate}/{hz}/{ratio}/{cents}: residual={error} RMS={rms} work={}",
                        pcm.work_units
                    );
                    assert!(
                        error < rms * 0.1,
                        "sub-bass pitch/residual {rate}/{hz}/{ratio}/{cents}: {error}/{rms}"
                    );
                    assert!((0.30..0.40).contains(&rms));
                    assert!(pcm.frames.iter().all(|x| (x[1] + x[0] * 0.7).abs() < 1e-6));
                }
            }
        }
    }
}
