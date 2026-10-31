//! Headless spectrum analyzer quality and cost probe; no device, UI or worker.
//!
//! Run optimized: `cargo run --release -p hex-audio --example spectrum_quality [rate]`.
//! Reports sine calibration, pink-noise flatness and frame jitter (3 dB/oct
//! slope, as displayed), then times one stereo analysis, one 256-frame window
//! push and a maximum 257-layer project analysis turn on the calling thread.
use hex_audio::spectrum::{
    BASS_FFT_SIZE, FFT_SIZE, SPECTRUM_BANDS, SpectrumBallistics, SpectrumKernel, SpectrumSettings,
    SpectrumWindow, band_center_hz,
};
use std::{error::Error, hint::black_box, time::Duration, time::Instant};

const MAX_PROJECT_LAYERS: usize = 257;
const RUNS: usize = 2_000;

fn sine(rate: u32, frequency: f64, amplitude: f64, len: usize) -> Vec<[f32; 2]> {
    (0..len)
        .map(|index| {
            let value = (std::f64::consts::TAU * frequency * index as f64 / f64::from(rate)).sin()
                * amplitude;
            [value as f32, value as f32]
        })
        .collect()
}

/// Paul Kellet's refined pink filter over a deterministic LCG; independent
/// channels. Accurate to about 0.05 dB above 10 Hz at 44.1 kHz.
fn pink(len: usize) -> Vec<[f32; 2]> {
    let mut seed = 0x1234_5678_u64;
    let mut filters = [[0.0f64; 7]; 2];
    (0..len)
        .map(|_| {
            std::array::from_fn(|channel| {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let white = ((seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0;
                let b = &mut filters[channel];
                b[0] = 0.99886 * b[0] + white * 0.055_517_9;
                b[1] = 0.99332 * b[1] + white * 0.075_075_9;
                b[2] = 0.96900 * b[2] + white * 0.153_852_0;
                b[3] = 0.86650 * b[3] + white * 0.310_485_6;
                b[4] = 0.55000 * b[4] + white * 0.532_952_2;
                b[5] = -0.7616 * b[5] - white * 0.016_898_0;
                let value = (b.iter().sum::<f64>() + white * 0.5362) * 0.05;
                b[6] = white * 0.115_926;
                value as f32
            })
        })
        .collect()
}

fn percentile(times: &mut [f64], fraction: f64) -> f64 {
    times.sort_by(f64::total_cmp);
    times[((times.len() - 1) as f64 * fraction) as usize]
}

fn time<F: FnMut()>(runs: usize, mut work: F) -> (f64, f64) {
    for _ in 0..runs / 10 {
        work();
    }
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    (percentile(&mut times, 0.5), percentile(&mut times, 0.99))
}

fn main() -> Result<(), Box<dyn Error>> {
    let rate = match std::env::args().nth(1) {
        Some(rate) => rate.parse::<u32>()?,
        None => 48_000,
    };
    let flat = SpectrumSettings {
        slope_db_per_octave: 0.0,
        ..SpectrumSettings::DEFAULT
    };
    let fill = BASS_FFT_SIZE * 8 + 64;
    println!(
        "rate {rate} hz: full-rate fft {FFT_SIZE} ({:.2} hz bins), bass fft {BASS_FFT_SIZE}, {SPECTRUM_BANDS} points",
        f64::from(rate) / FFT_SIZE as f64
    );
    let below = (0..SPECTRUM_BANDS)
        .filter(|point| band_center_hz(rate, *point).is_some_and(|hz| hz < 100.0))
        .count();
    println!("display points below 100 hz: {below}");

    let mut kernel = SpectrumKernel::with_settings(rate, flat)?;
    let mut window = SpectrumWindow::new(rate)?;
    let mut worst = 0.0f32;
    for frequency in [
        31.0, 47.0, 63.0, 100.0, 440.0, 1_000.0, 1_010.0, 3_000.0, 9_000.0, 15_000.0,
    ] {
        window.clear();
        window.push(&sine(rate, frequency, 0.5, fill))?;
        let frame = kernel.analyze(&window)?;
        let (point, level) = frame
            .bins_dbfs
            .iter()
            .copied()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .ok_or("empty frame")?;
        worst = worst.max((level + 6.0206).abs());
        println!(
            "sine {frequency:>7.1} hz: {level:>7.2} dbfs at point {:>8.1} hz (err {:+.3} db)",
            band_center_hz(rate, point).unwrap_or(0.0),
            level + 6.0206
        );
    }
    println!("worst sine level error: {worst:.3} db");

    let displayed = SpectrumSettings {
        slope_db_per_octave: 3.0,
        ..SpectrumSettings::DEFAULT
    };
    let mut kernel = SpectrumKernel::with_settings(rate, displayed)?;
    let noise = pink(rate as usize * 6);
    let hop = (rate / 30) as usize;
    window.clear();
    window.push(&noise[..fill])?;
    let mut raw = Vec::new();
    let mut smoothed = Vec::new();
    let mut ballistics = SpectrumBallistics::default();
    for block in noise[fill..].chunks_exact(hop) {
        window.push(block)?;
        let frame = kernel.analyze(&window)?;
        raw.push(frame.bins_dbfs);
        let mut shown = frame;
        ballistics.apply(
            &mut shown,
            Duration::from_secs_f64(hop as f64 / f64::from(rate)),
        );
        smoothed.push(shown.bins_dbfs);
    }
    let in_range: Vec<usize> = (0..SPECTRUM_BANDS)
        .filter(|point| {
            band_center_hz(rate, *point).is_some_and(|hz| (40.0..16_000.0).contains(&hz))
        })
        .collect();
    for (name, frames) in [("raw", &raw), ("with ballistics", &smoothed)] {
        let count = frames.len() as f32;
        let mean: Vec<f32> = (0..SPECTRUM_BANDS)
            .map(|point| frames.iter().map(|frame| frame[point]).sum::<f32>() / count)
            .collect();
        let jitter = (frames
            .iter()
            .flat_map(|frame| {
                in_range
                    .iter()
                    .map(|&point| (frame[point] - mean[point]).powi(2))
            })
            .sum::<f32>()
            / (count * in_range.len() as f32))
            .sqrt();
        let roughness = frames
            .iter()
            .map(|frame| {
                in_range
                    .windows(2)
                    .map(|pair| (frame[pair[1]] - frame[pair[0]]).abs())
                    .sum::<f32>()
                    / (in_range.len() - 1) as f32
            })
            .sum::<f32>()
            / count;
        let (low, high) = in_range
            .iter()
            .fold((f32::MAX, f32::MIN), |(low, high), &point| {
                (low.min(mean[point]), high.max(mean[point]))
            });
        println!(
            "pink {name}: mean spread {:.2} db (40 hz..16 khz, 3 db/oct slope), frame jitter {jitter:.2} db rms, adjacent-point roughness {roughness:.2} db",
            high - low
        );
    }

    let mut kernel = SpectrumKernel::new(rate)?;
    window.clear();
    window.push(&noise[..fill])?;
    let (median, p99) = time(RUNS, || {
        black_box(kernel.analyze(black_box(&window)).ok());
    });
    println!("analyze one stereo signal: median {median:.1} us, p99 {p99:.1} us ({RUNS} runs)");
    let block = &noise[fill..fill + 256];
    let (median, p99) = time(RUNS * 10, || {
        black_box(window.push(black_box(block)).ok());
    });
    println!("push 256 stereo frames: median {median:.2} us, p99 {p99:.2} us");
    let mut layers: Vec<SpectrumWindow> = (0..MAX_PROJECT_LAYERS)
        .map(|_| SpectrumWindow::new(rate))
        .collect::<Result<_, _>>()?;
    for layer in &mut layers {
        layer.push(&noise[..fill])?;
    }
    let (median, p99) = time(50, || {
        for layer in &layers {
            black_box(kernel.analyze(layer).ok());
        }
    });
    println!(
        "analyze {MAX_PROJECT_LAYERS} project layers: median {:.2} ms, p99 {:.2} ms",
        median / 1e3,
        p99 / 1e3
    );
    Ok(())
}
