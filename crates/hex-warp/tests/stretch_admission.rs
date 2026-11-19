use hex_warp::{Error, Input, MapSegment, Mode, Request, TimeMap, render};
const Q32: i128 = 1_i128 << 32;
fn actual(map: &TimeMap, output: usize, mode: Mode, pitch: i16) -> Result<(), Error> {
    let pcm = [[0.125, -0.25]; 32];
    render(
        &Request {
            input: Input {
                frames: &pcm,
                sample_rate: 16_000,
                region_start: 0,
                region_length: 32,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map,
            output_frames: output,
            pitch_cents: pitch,
            mode,
            attacks: &[],
        },
        &mut || false,
    )
    .map(|_| ())
}
fn line(output: usize, source: i128) -> TimeMap {
    TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: output as u64,
            source_start_q32: 0,
            source_end_q32: source,
        }],
        output,
    )
    .unwrap()
}
#[test]
fn stretch_admission_matches_independent_pitch_law_and_real_render_all_modes() {
    for mode in [
        Mode::Resample,
        Mode::RePitch,
        Mode::Beats {
            division_frames: None,
            tail: hex_warp::Tail::Forward,
            decay: 100,
        },
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 0,
            seed: 7,
        },
        Mode::Complex,
        Mode::ComplexPro {
            formants: 100,
            envelope: 128,
        },
    ] {
        for pitch in [-4_800, -1_200, 0, 1_200, 4_800] {
            for output in [3, 4, 31, 32, 33, 255, 256, 257] {
                let map = line(output, 32 * Q32);
                let mut ratio = output as f64 / 32.0;
                if !matches!(mode, Mode::RePitch) {
                    ratio *= 2.0_f64.powf(f64::from(pitch) / 1_200.0);
                }
                let expected =
                    matches!(mode, Mode::Resample) || (0.125 - 1e-9..=8.0 + 1e-9).contains(&ratio);
                assert_eq!(
                    map.validate_stretch(mode, pitch).is_ok(),
                    expected,
                    "{mode:?} {pitch} {output}"
                );
                assert_eq!(
                    hex_warp::validate_stretch_ratio(output as f64 / 32.0, mode, pitch),
                    map.validate_stretch(mode, pitch),
                    "continuous and integer admission must share one capability law"
                );
                assert_eq!(
                    actual(&map, output, mode, pitch).is_ok(),
                    expected,
                    "actual {mode:?} {pitch} {output}"
                );
            }
        }
    }
}

#[test]
fn continuous_stretch_rejects_nonpositive_and_nonfinite_intervals() {
    for mode in [Mode::Resample, Mode::RePitch, Mode::Tones { grain_ms: 8 }] {
        for ratio in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(hex_warp::validate_stretch_ratio(ratio, mode, 0).is_err());
        }
    }
}
#[test]
fn stretch_admission_preserves_inclusive_epsilon_and_per_segment_not_average() {
    let mode = Mode::Tones { grain_ms: 8 };
    for (output, source, expected) in [
        (8, Q32, true),
        (8, Q32 - 1, false),
        (8, Q32 + 1, true),
        (1, 8 * Q32, true),
        (1, 8 * Q32 + 1, true),
        (1, 8 * Q32 + 512, false),
    ] {
        let map = line(output, source);
        assert_eq!(map.validate_stretch(mode, 0).is_ok(), expected);
        assert_eq!(actual(&map, output, mode, 0).is_ok(), expected);
    }
    let map = TimeMap::new(
        vec![
            MapSegment {
                output_start: 0,
                output_end: 16,
                source_start_q32: 0,
                source_end_q32: Q32,
            },
            MapSegment {
                output_start: 16,
                output_end: 32,
                source_start_q32: Q32,
                source_end_q32: 32 * Q32,
            },
        ],
        32,
    )
    .unwrap();
    assert_eq!(map.validate_stretch(mode, 0), actual(&map, 32, mode, 0));
    assert!(
        map.validate_stretch(mode, 0).is_err(),
        "average ratio is one, but first segment is sixteen"
    );
}
#[test]
fn stretch_admission_invalid_pitch_stays_refused_even_when_resample_is_exempt() {
    let map = line(32, 32 * Q32);
    for mode in [Mode::Resample, Mode::RePitch, Mode::Tones { grain_ms: 8 }] {
        for pitch in [i16::MIN, -4_801, 4_801, i16::MAX] {
            assert_eq!(
                map.validate_stretch(mode, pitch),
                actual(&map, 32, mode, pitch)
            );
            assert!(map.validate_stretch(mode, pitch).is_err());
        }
    }
}
