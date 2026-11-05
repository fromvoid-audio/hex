//! Per-signal rolling analysis windows: one full-rate ring and one decimated
//! bass ring. Construction allocates both rings on control; [`push`],
//! [`clear`] and [`reconfigure`] reuse that storage without allocating.
//!
//! [`push`]: SpectrumWindow::push
//! [`clear`]: SpectrumWindow::clear
//! [`reconfigure`]: SpectrumWindow::reconfigure

use super::{BASS_FFT_SIZE, FFT_SIZE, SpectrumError, bass_decimation_stages, validate_rate};

pub(super) const MAX_DECIMATION_STAGES: usize = 3;

/// One 2:1 decimation stage using the binomial kernel `[1, 4, 6, 4, 1] / 16`.
///
/// Its response, `cos^4(pi * f / input_rate)`, has a fourth-order zero at the
/// input Nyquist frequency, where the bands that alias into the analyzed bass
/// region fold. The kernel compensates the known passband droop exactly.
#[derive(Clone, Copy, Debug, Default)]
struct HalfBand {
    history: [[f64; 2]; 4],
    odd: bool,
}

impl HalfBand {
    fn push(&mut self, input: [f64; 2]) -> Option<[f64; 2]> {
        let [h0, h1, h2, h3] = self.history;
        let output = self.odd.then(|| {
            std::array::from_fn(|channel| {
                (h0[channel]
                    + 4.0 * h1[channel]
                    + 6.0 * h2[channel]
                    + 4.0 * h3[channel]
                    + input[channel])
                    / 16.0
            })
        });
        self.history = [h1, h2, h3, input];
        self.odd = !self.odd;
        output
    }
}

/// Rolling stereo history for one analyzed signal at one sample rate.
pub struct SpectrumWindow {
    sample_rate_hz: u32,
    stages: usize,
    halfbands: [HalfBand; MAX_DECIMATION_STAGES],
    pub(super) high: Box<[[f32; 2]]>,
    pub(super) high_write: usize,
    high_filled: usize,
    pub(super) bass: Box<[[f32; 2]]>,
    pub(super) bass_write: usize,
    bass_filled: usize,
}

impl SpectrumWindow {
    /// Heap bytes reserved by [`SpectrumWindow::new`] for any supported rate.
    pub(crate) fn heap_storage_quote() -> Option<usize> {
        FFT_SIZE
            .checked_add(BASS_FFT_SIZE)?
            .checked_mul(std::mem::size_of::<[f32; 2]>())
    }

    pub(crate) fn heap_data_bytes(&self) -> Option<usize> {
        std::mem::size_of_val(self.high.as_ref())
            .checked_add(std::mem::size_of_val(self.bass.as_ref()))
    }

    /// Allocate fixed rings for a supported sample rate.
    pub fn new(sample_rate_hz: u32) -> Result<Self, SpectrumError> {
        validate_rate(sample_rate_hz)?;
        Ok(Self {
            sample_rate_hz,
            stages: bass_decimation_stages(sample_rate_hz),
            halfbands: [HalfBand::default(); MAX_DECIMATION_STAGES],
            high: vec![[0.0; 2]; FFT_SIZE].into_boxed_slice(),
            high_write: 0,
            high_filled: 0,
            bass: vec![[0.0; 2]; BASS_FFT_SIZE].into_boxed_slice(),
            bass_write: 0,
            bass_filled: 0,
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Change rate in place, discarding history. No allocation.
    pub fn reconfigure(&mut self, sample_rate_hz: u32) -> Result<(), SpectrumError> {
        validate_rate(sample_rate_hz)?;
        self.sample_rate_hz = sample_rate_hz;
        self.stages = bass_decimation_stages(sample_rate_hz);
        self.clear();
        Ok(())
    }

    /// Discard history after a caller-observed gap or context change.
    /// Ring contents are left in place: analysis reads them only once the
    /// fill counters show a complete overwrite.
    pub fn clear(&mut self) {
        self.halfbands = [HalfBand::default(); MAX_DECIMATION_STAGES];
        self.high_write = 0;
        self.high_filled = 0;
        self.bass_write = 0;
        self.bass_filled = 0;
    }

    /// Append contiguous stereo frames. Non-finite input is refused before
    /// any state changes, so the caller decides whether to clear.
    pub fn push(&mut self, frames: &[[f32; 2]]) -> Result<(), SpectrumError> {
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
        for frame in frames {
            self.high[self.high_write] = *frame;
            self.high_write = (self.high_write + 1) % FFT_SIZE;
            self.high_filled = (self.high_filled + 1).min(FFT_SIZE);
            let mut sample = Some(frame.map(f64::from));
            for halfband in &mut self.halfbands[..self.stages] {
                sample = sample.and_then(|input| halfband.push(input));
            }
            if let Some(decimated) = sample {
                self.bass[self.bass_write] = decimated.map(|value| value as f32);
                self.bass_write = (self.bass_write + 1) % BASS_FFT_SIZE;
                self.bass_filled = (self.bass_filled + 1).min(BASS_FFT_SIZE);
            }
        }
        Ok(())
    }

    /// Full-rate frames currently held, at most [`FFT_SIZE`].
    pub fn filled(&self) -> usize {
        self.high_filled
    }

    /// Whether the full-rate ring holds one complete analysis window.
    pub fn is_ready(&self) -> bool {
        self.high_filled == FFT_SIZE
    }

    /// Whether the decimated bass ring is complete. Until then the kernel
    /// uses the full-rate stage at every frequency.
    pub fn bass_ready(&self) -> bool {
        self.bass_filled == BASS_FFT_SIZE
    }
}
