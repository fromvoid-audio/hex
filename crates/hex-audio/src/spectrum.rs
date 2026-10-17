//! Bounded, offline stereo spectrum analysis primitives.
//!
//! This module owns only a fixed-size windowed FFT and logarithmic band
//! reduction. A caller owns rolling-window assembly, update cadence, and gaps.
//! Construction allocates the fixed workspace on the control side;
//! [`SpectrumKernel::analyze`] performs no allocation.

use std::{error::Error, fmt};

pub const FFT_SIZE: usize = 4096;
pub const SPECTRUM_BANDS: usize = 96;
pub const MIN_FREQUENCY_HZ: f64 = 20.0;
pub const MAX_FREQUENCY_HZ: f64 = 20_000.0;
pub const FLOOR_DBFS: f32 = -120.0;

const MIN_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_SAMPLE_RATE_HZ: u32 = 192_000;
const POSITIVE_BIN_COUNT: usize = FFT_SIZE / 2 + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectrumError {
    InvalidSampleRate(u32),
    InvalidFrameCount(usize),
    NonFiniteSample { frame: usize, channel: usize },
}

impl fmt::Display for SpectrumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate(rate) => {
                write!(
                    formatter,
                    "spectrum sample rate {rate} is outside 8000..=192000 hz"
                )
            }
            Self::InvalidFrameCount(count) => {
                write!(
                    formatter,
                    "spectrum window has {count} frames; expected {FFT_SIZE}"
                )
            }
            Self::NonFiniteSample { frame, channel } => {
                write!(
                    formatter,
                    "spectrum input frame {frame} channel {channel} is not finite"
                )
            }
        }
    }
}

impl Error for SpectrumError {}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumFrame {
    pub sample_rate_hz: u32,
    /// Peak amplitude in each logarithmic band, in dBFS, with a -120 dBFS floor.
    pub bins_dbfs: [f32; SPECTRUM_BANDS],
}

#[derive(Clone, Copy)]
struct Band {
    first_bin: usize,
    end_bin: usize,
    nearest_bin: usize,
}

/// Fixed-window FFT and logarithmic stereo spectrum reducer.
pub struct SpectrumKernel {
    sample_rate_hz: u32,
    window: Box<[f64; FFT_SIZE]>,
    window_sum: f64,
    bands: [Band; SPECTRUM_BANDS],
    real: Box<[f64; FFT_SIZE]>,
    imaginary: Box<[f64; FFT_SIZE]>,
    left_power: Box<[f64; POSITIVE_BIN_COUNT]>,
}

impl SpectrumKernel {
    pub(crate) fn heap_data_bytes(&self) -> Option<usize> {
        let Self {
            sample_rate_hz: _,
            window,
            window_sum: _,
            bands: _,
            real,
            imaginary,
            left_power,
        } = self;
        std::mem::size_of_val(window.as_ref())
            .checked_add(std::mem::size_of_val(real.as_ref()))?
            .checked_add(std::mem::size_of_val(imaginary.as_ref()))?
            .checked_add(std::mem::size_of_val(left_power.as_ref()))
    }

    /// Construct reusable fixed storage for a supported audio sample rate.
    pub fn new(sample_rate_hz: u32) -> Result<Self, SpectrumError> {
        if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz) {
            return Err(SpectrumError::InvalidSampleRate(sample_rate_hz));
        }

        let window = Box::new(std::array::from_fn(|index| {
            let phase = std::f64::consts::TAU * index as f64 / FFT_SIZE as f64;
            0.5 - 0.5 * phase.cos()
        }));
        let window_sum = window.iter().sum();
        let bands = build_bands(sample_rate_hz);

        Ok(Self {
            sample_rate_hz,
            window,
            window_sum,
            bands,
            real: Box::new([0.0; FFT_SIZE]),
            imaginary: Box::new([0.0; FFT_SIZE]),
            left_power: Box::new([0.0; POSITIVE_BIN_COUNT]),
        })
    }

    /// Analyze exactly one finite 4096-frame stereo window.
    ///
    /// Invalid input is rejected before scratch or output state is modified.
    /// Stereo power is averaged between channels before conversion to dBFS, so
    /// opposite-polarity material does not cancel. Each output band reports its
    /// strongest FFT-bin amplitude, corrected for the Hann window's coherent
    /// gain. Inputs above full scale may produce positive dBFS values.
    pub fn analyze(&mut self, frames: &[[f32; 2]]) -> Result<SpectrumFrame, SpectrumError> {
        if frames.len() != FFT_SIZE {
            return Err(SpectrumError::InvalidFrameCount(frames.len()));
        }
        for (frame_index, frame) in frames.iter().enumerate() {
            for (channel, sample) in frame.iter().enumerate() {
                if !sample.is_finite() {
                    return Err(SpectrumError::NonFiniteSample {
                        frame: frame_index,
                        channel,
                    });
                }
            }
        }

        for (index, frame) in frames.iter().enumerate() {
            self.real[index] = f64::from(frame[0]) * self.window[index];
            self.imaginary[index] = 0.0;
        }
        fft(&mut self.real, &mut self.imaginary);
        for bin in 0..POSITIVE_BIN_COUNT {
            self.left_power[bin] =
                normalized_bin_power(self.real[bin], self.imaginary[bin], bin, self.window_sum);
        }

        for (index, frame) in frames.iter().enumerate() {
            self.real[index] = f64::from(frame[1]) * self.window[index];
            self.imaginary[index] = 0.0;
        }
        fft(&mut self.real, &mut self.imaginary);

        let bins_dbfs = std::array::from_fn(|band_index| {
            let band = self.bands[band_index];
            let mut maximum_power = 0.0f64;
            if band.first_bin < band.end_bin {
                for bin in band.first_bin..band.end_bin {
                    let right_power = normalized_bin_power(
                        self.real[bin],
                        self.imaginary[bin],
                        bin,
                        self.window_sum,
                    );
                    maximum_power = maximum_power.max((self.left_power[bin] + right_power) * 0.5);
                }
            } else {
                let bin = band.nearest_bin;
                let right_power =
                    normalized_bin_power(self.real[bin], self.imaginary[bin], bin, self.window_sum);
                maximum_power = (self.left_power[bin] + right_power) * 0.5;
            }
            if maximum_power <= 1.0e-12 {
                FLOOR_DBFS
            } else {
                (10.0 * maximum_power.log10()) as f32
            }
        });

        Ok(SpectrumFrame {
            sample_rate_hz: self.sample_rate_hz,
            bins_dbfs,
        })
    }

    /// Clear reusable transform workspaces after a caller-observed stream gap.
    /// The caller still supplies a complete fresh window to the next analysis.
    pub fn reset(&mut self) {
        self.real.fill(0.0);
        self.imaginary.fill(0.0);
        self.left_power.fill(0.0);
    }
}

/// Return the logarithmic frequency bounds for a spectrum band.
pub fn band_bounds_hz(sample_rate_hz: u32, band: usize) -> Option<(f32, f32)> {
    if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz)
        || band >= SPECTRUM_BANDS
    {
        return None;
    }
    let maximum = maximum_frequency(sample_rate_hz);
    let ratio = maximum / MIN_FREQUENCY_HZ;
    let lower = MIN_FREQUENCY_HZ * ratio.powf(band as f64 / SPECTRUM_BANDS as f64);
    let upper = MIN_FREQUENCY_HZ * ratio.powf((band + 1) as f64 / SPECTRUM_BANDS as f64);
    Some((lower as f32, upper as f32))
}

fn maximum_frequency(sample_rate_hz: u32) -> f64 {
    MAX_FREQUENCY_HZ.min(f64::from(sample_rate_hz) * 0.49)
}

fn build_bands(sample_rate_hz: u32) -> [Band; SPECTRUM_BANDS] {
    let maximum = maximum_frequency(sample_rate_hz);
    let ratio = maximum / MIN_FREQUENCY_HZ;
    let resolution = f64::from(sample_rate_hz) / FFT_SIZE as f64;
    std::array::from_fn(|band| {
        let lower = MIN_FREQUENCY_HZ * ratio.powf(band as f64 / SPECTRUM_BANDS as f64);
        let upper = MIN_FREQUENCY_HZ * ratio.powf((band + 1) as f64 / SPECTRUM_BANDS as f64);
        let first_bin = ((lower / resolution).ceil() as usize).clamp(1, POSITIVE_BIN_COUNT - 1);
        let end_bin = ((upper / resolution).ceil() as usize).clamp(1, POSITIVE_BIN_COUNT);
        let center = (lower * upper).sqrt();
        let nearest_bin = (center / resolution)
            .round()
            .clamp(1.0, (POSITIVE_BIN_COUNT - 1) as f64) as usize;
        Band {
            first_bin,
            end_bin: end_bin.max(first_bin),
            nearest_bin,
        }
    })
}

fn normalized_bin_power(real: f64, imaginary: f64, bin: usize, window_sum: f64) -> f64 {
    let one_sided_scale = if bin == 0 || bin == FFT_SIZE / 2 {
        1.0 / window_sum
    } else {
        2.0 / window_sum
    };
    (real.mul_add(real, imaginary * imaginary)) * one_sided_scale * one_sided_scale
}

fn fft(real: &mut [f64; FFT_SIZE], imaginary: &mut [f64; FFT_SIZE]) {
    let mut reversed = 0usize;
    for index in 1..FFT_SIZE {
        let mut bit = FFT_SIZE >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            real.swap(index, reversed);
            imaginary.swap(index, reversed);
        }
    }

    let mut stage = 2;
    while stage <= FFT_SIZE {
        let half = stage / 2;
        let angle = -std::f64::consts::TAU / stage as f64;
        let step_real = angle.cos();
        let step_imaginary = angle.sin();
        for start in (0..FFT_SIZE).step_by(stage) {
            let mut twiddle_real = 1.0;
            let mut twiddle_imaginary = 0.0;
            for offset in 0..half {
                let even = start + offset;
                let odd = even + half;
                let odd_real = real[odd] * twiddle_real - imaginary[odd] * twiddle_imaginary;
                let odd_imaginary = real[odd] * twiddle_imaginary + imaginary[odd] * twiddle_real;
                let even_real = real[even];
                let even_imaginary = imaginary[even];
                real[even] = even_real + odd_real;
                imaginary[even] = even_imaginary + odd_imaginary;
                real[odd] = even_real - odd_real;
                imaginary[odd] = even_imaginary - odd_imaginary;
                (twiddle_real, twiddle_imaginary) = (
                    twiddle_real * step_real - twiddle_imaginary * step_imaginary,
                    twiddle_real * step_imaginary + twiddle_imaginary * step_real,
                );
            }
        }
        stage *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_tone(
        sample_rate: u32,
        fft_bin: usize,
        amplitude: f32,
        right_polarity: f32,
    ) -> Vec<[f32; 2]> {
        let frequency = sample_rate as f64 * fft_bin as f64 / FFT_SIZE as f64;
        (0..FFT_SIZE)
            .map(|frame| {
                let phase =
                    std::f64::consts::TAU * frequency * frame as f64 / f64::from(sample_rate);
                let sample = (phase.sin() * f64::from(amplitude)) as f32;
                [sample, sample * right_polarity]
            })
            .collect()
    }

    fn peak_bin(frame: &SpectrumFrame) -> usize {
        frame
            .bins_dbfs
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map_or(0, |(index, _)| index)
    }

    #[test]
    fn spectrum_tone_frequency_and_hann_gain_are_calibrated() {
        let mut kernel = SpectrumKernel::new(48_000).expect("supported sample rate");
        let frame = kernel
            .analyze(&stereo_tone(48_000, 128, 0.5, 1.0))
            .expect("finite stereo tone");
        let peak = peak_bin(&frame);
        let (lower, upper) = band_bounds_hz(48_000, peak).expect("peak band bounds");
        assert!((lower..upper).contains(&1500.0));
        assert!((frame.bins_dbfs[peak] + 6.0206).abs() < 0.02);
    }

    #[test]
    fn stereo_power_avoids_antiphase_cancellation_and_silence_has_a_floor() {
        let mut kernel = SpectrumKernel::new(48_000).expect("supported sample rate");
        let aligned = kernel
            .analyze(&stereo_tone(48_000, 128, 0.5, 1.0))
            .expect("aligned tone");
        let opposite = kernel
            .analyze(&stereo_tone(48_000, 128, 0.5, -1.0))
            .expect("antiphase tone");
        assert_eq!(aligned.bins_dbfs, opposite.bins_dbfs);

        let silence = vec![[0.0; 2]; FFT_SIZE];
        let silent = kernel.analyze(&silence).expect("silence");
        assert!(silent.bins_dbfs.iter().all(|value| *value == FLOOR_DBFS));
    }

    #[test]
    fn supported_rates_have_finite_frequency_geometry_and_reset_discards_scratch() {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            let mut kernel = SpectrumKernel::new(rate).expect("supported sample rate");
            let frame = kernel
                .analyze(&stereo_tone(rate, 64, 0.25, 1.0))
                .expect("finite tone");
            assert_eq!(frame.sample_rate_hz, rate);
            assert!(frame.bins_dbfs.iter().all(|value| value.is_finite()));
            let (lower, upper) = band_bounds_hz(rate, peak_bin(&frame)).expect("band bounds");
            let frequency = rate as f32 * 64.0 / FFT_SIZE as f32;
            assert!((lower..upper).contains(&frequency));
            assert_eq!(band_bounds_hz(rate, SPECTRUM_BANDS), None);
            assert_eq!(band_bounds_hz(rate, 0).map(|bounds| bounds.0), Some(20.0));
            assert_eq!(
                band_bounds_hz(rate, SPECTRUM_BANDS - 1).map(|bounds| bounds.1),
                Some((20_000.0f64.min(f64::from(rate) * 0.49)) as f32)
            );
            kernel.reset();
            let silent = kernel
                .analyze(&vec![[0.0; 2]; FFT_SIZE])
                .expect("fresh silence after reset");
            assert!(silent.bins_dbfs.iter().all(|value| *value == FLOOR_DBFS));
        }
    }

    #[test]
    fn malformed_windows_and_rates_are_refused_without_corrupting_the_next_result() {
        assert_eq!(
            SpectrumKernel::new(7_999).err(),
            Some(SpectrumError::InvalidSampleRate(7_999))
        );
        assert_eq!(
            SpectrumKernel::new(192_001).err(),
            Some(SpectrumError::InvalidSampleRate(192_001))
        );
        let mut kernel = SpectrumKernel::new(48_000).expect("supported sample rate");
        let valid = stereo_tone(48_000, 128, 0.25, 1.0);
        let expected = kernel.analyze(&valid).expect("valid tone");
        assert_eq!(
            kernel.analyze(&valid[..FFT_SIZE - 1]),
            Err(SpectrumError::InvalidFrameCount(FFT_SIZE - 1))
        );
        let mut invalid = valid.clone();
        invalid[23][1] = f32::NAN;
        assert_eq!(
            kernel.analyze(&invalid),
            Err(SpectrumError::NonFiniteSample {
                frame: 23,
                channel: 1
            })
        );
        invalid[23][1] = f32::INFINITY;
        assert_eq!(
            kernel.analyze(&invalid),
            Err(SpectrumError::NonFiniteSample {
                frame: 23,
                channel: 1
            })
        );
        assert_eq!(
            kernel.analyze(&valid).expect("valid after refusal"),
            expected
        );
    }
}
