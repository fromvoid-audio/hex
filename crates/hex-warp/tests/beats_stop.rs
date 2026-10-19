//! Stop is one native traversal followed by intentional exact silence.
use hex_warp::*;

#[test]
fn stop_short_native_bodies_remain_finite_and_never_fill_the_intended_gap() {
    for length in [1, 2, 3, 7, 17] {
        for rate in [8_000, 44_100, 96_000] {
            let source = vec![[0.4, -0.2]; length];
            let output = length * 8;
            let map = TimeMap::new(
                vec![MapSegment {
                    output_start: 0,
                    output_end: output as u64,
                    source_start_q32: 0,
                    source_end_q32: (length as i128) << 32,
                }],
                output,
            )
            .unwrap();
            let request = Request {
                input: Input {
                    frames: &source,
                    sample_rate: rate,
                    region_start: 0,
                    region_length: length as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                attacks: &[],
                map: &map,
                output_frames: output,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail: Tail::Off,
                    decay: 100,
                },
            };
            let pcm = render(&request, &mut || false).unwrap().frames;
            assert_eq!(pcm.len(), output);
            assert_eq!(pcm[0], source[0], "first attack/body sample was faded");
            assert!(pcm.iter().flatten().all(|x| x.is_finite()));
            assert!(pcm[length..].iter().all(|x| *x == [0.0; 2]));
            for frame in &pcm[..length] {
                assert!((frame[1] + frame[0] * 0.5).abs() < 1e-7);
            }
            assert_eq!(render(&request, &mut || false).unwrap().frames, pcm);
        }
    }
}
