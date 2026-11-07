//! Sub-bass/fractional-pitch evidence beyond the initial 55-Hz fixture.
use hex_warp::*;
#[path = "support/quality.rs"]
mod quality;
fn check_sub_bass(rate: u32, hz: f64, ratio: f64, cents: i16) {
    let source: Vec<Frame> = (0..rate / 2)
        .map(|i| {
            let x = (std::f64::consts::TAU * hz * f64::from(i) / f64::from(rate) + 0.4).sin()
                as f32
                * 0.5;
            [x, -x * 0.7]
        })
        .collect();
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
        return;
    }
    let pcm = rendered.unwrap();
    let material = &pcm.frames[(rate as usize / 20).min(output / 8)..output - rate as usize / 100];
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

macro_rules! bass_case {
    ($name:ident, $rate:expr, $hz:expr, $ratio:expr, $cents:expr) => {
        #[test]
        fn $name() {
            super::check_sub_bass($rate, $hz, $ratio, $cents);
        }
    };
}

// Each case preserves the original complete matrix and its independent oracle,
// while allowing the harness to enforce its per-test wall-time budget.
macro_rules! bass_group {
    ($module:ident, $rate:expr, $hz:expr) => {
        mod $module {
            bass_case!(half_zero, $rate, $hz, 0.5, 0);
            bass_case!(half_down, $rate, $hz, 0.5, -713);
            bass_case!(half_up, $rate, $hz, 0.5, 307);
            bass_case!(ordinary_zero, $rate, $hz, 1.375, 0);
            bass_case!(ordinary_down, $rate, $hz, 1.375, -713);
            bass_case!(ordinary_up, $rate, $hz, 1.375, 307);
            bass_case!(double_zero, $rate, $hz, 2.0, 0);
            bass_case!(double_down, $rate, $hz, 2.0, -713);
            bass_case!(double_up, $rate, $hz, 2.0, 307);
            bass_case!(extreme_zero, $rate, $hz, 8.0, 0);
            bass_case!(extreme_down, $rate, $hz, 8.0, -713);
            bass_case!(extreme_up, $rate, $hz, 8.0, 307);
        }
    };
}
bass_group!(hz_16000_27, 16_000, 27.5);
bass_group!(hz_16000_41, 16_000, 41.20344461410875);
bass_group!(hz_16000_73, 16_000, 73.0);
bass_group!(hz_48000_27, 48_000, 27.5);
bass_group!(hz_48000_41, 48_000, 41.20344461410875);
bass_group!(hz_48000_73, 48_000, 73.0);
