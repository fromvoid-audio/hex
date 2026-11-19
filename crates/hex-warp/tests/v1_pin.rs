//! v1 reconstruction is frozen: `render` output for every mode family stays
//! bit-identical. Hashes were captured from the pre-versioning source
//! (6c4936019, before the v2 resampler existed).
use hex_warp::*;
const RATE: u32 = 48_000;

fn source() -> Vec<Frame> {
    (0..6_000)
        .map(|n| {
            let t = n as f64 / f64::from(RATE);
            let tone = 0.3 * (std::f64::consts::TAU * 997.0 * t).sin()
                + 0.2 * (std::f64::consts::TAU * 9_013.0 * t).sin();
            let noise = ((n * 7_919) % 1_013) as f64 / 1_013.0 - 0.5;
            [tone as f32, (0.1 * noise + tone * 0.5) as f32]
        })
        .collect()
}

fn map(source: usize, output: usize) -> TimeMap {
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

fn hash(frames: &[Frame]) -> u64 {
    frames
        .iter()
        .flatten()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

#[test]
fn v1_render_is_bit_identical_across_mode_families() {
    let frames = source();
    let n = frames.len();
    let cases: [(Mode, usize, usize, i16); 6] = [
        (Mode::Resample, n / 4, n / 4, 2_400),
        (Mode::Resample, n / 2, n / 2, 700),
        (Mode::RePitch, n, n * 3 / 5, 0),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Off,
                decay: 100,
            },
            n,
            n * 3 / 2,
            300,
        ),
        (Mode::Tones { grain_ms: 40 }, n, n * 4 / 5, -500),
        (Mode::Complex, n, n * 5 / 4, 0),
    ];
    let mut hashes = Vec::new();
    for (mode, map_source, output, pitch) in cases {
        let map = map(map_source, output);
        let rendered = render(
            &Request {
                attacks: &[],
                input: Input {
                    frames: &frames,
                    sample_rate: RATE,
                    region_start: 0,
                    region_length: n as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames: output,
                pitch_cents: pitch,
                mode,
            },
            &mut || false,
        )
        .unwrap();
        hashes.push(hash(&rendered.frames));
    }
    assert_eq!(
        hashes,
        [
            0xfdb1_a274_51c0_bd3a,
            0x0fcf_fdd8_3616_fdc3,
            0x51f4_278b_2555_0535,
            0x75fd_99ca_07f0_cae2,
            0x451a_011a_9670_5da1,
            0xfdbb_442d_3e09_c302,
        ]
    );
}
