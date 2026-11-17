//! Bounded overlap-normalization state for sequential granular output.
use crate::{Error, Frame, MAX_SCRATCH_BYTES, MAX_STREAMED_OUTPUT_FRAMES, buffer, finite};

/// A fixed ring of granular contributions with sequential caller-owned output.
///
/// The caller decides when all contributions to a frame have arrived. Draining
/// too early commits silence or a partial sum. This owns no source or output PCM;
/// allocation and use belong on control/worker threads, not the audio callback.
#[derive(Debug)]
pub struct GranularOverlap {
    total: u64,
    cursor: u64,
    samples: Vec<[f64; 2]>,
    weights: Vec<f64>,
}

impl GranularOverlap {
    /// Allocate a nonempty ring within the granular scratch ceiling.
    pub fn new(total_frames: u64, ring_frames: usize) -> Result<Self, Error> {
        if total_frames == 0 || total_frames > MAX_STREAMED_OUTPUT_FRAMES || ring_frames == 0 {
            return Err(Error::Invalid("invalid granular overlap extent"));
        }
        let requested = ring_frames.checked_mul(24).ok_or(Error::ScratchLimit)?;
        if requested > MAX_SCRATCH_BYTES {
            return Err(Error::ScratchLimit);
        }
        let result = Self {
            total: total_frames,
            cursor: 0,
            samples: buffer(ring_frames, [0.0; 2])?,
            weights: buffer(ring_frames, 0.0)?,
        };
        if result
            .backing_bytes()
            .is_none_or(|bytes| bytes > MAX_SCRATCH_BYTES)
        {
            return Err(Error::ScratchLimit);
        }
        Ok(result)
    }

    /// Actual retained ring allocations, excluding this inline value.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.samples
            .capacity()
            .checked_mul(std::mem::size_of::<[f64; 2]>())?
            .checked_add(
                self.weights
                    .capacity()
                    .checked_mul(std::mem::size_of::<f64>())?,
            )
    }

    /// The next output frame to be drained.
    pub fn position(&self) -> u64 {
        self.cursor
    }

    /// Accumulate a weighted sample. Frames outside the output extent are ignored;
    /// a frame already drained or beyond the ring window is rejected unchanged.
    #[inline]
    pub fn add(&mut self, frame: i128, sample: [f64; 2], weight: f64) -> Result<(), Error> {
        let Ok(frame) = u64::try_from(frame) else {
            return Ok(());
        };
        if frame >= self.total {
            return Ok(());
        }
        if frame < self.cursor || frame - self.cursor >= self.samples.len() as u64 {
            return Err(Error::Invalid(
                "audio overlap exceeds bounded normalization ring",
            ));
        }
        let index = (frame % self.samples.len() as u64) as usize;
        self.samples[index][0] += sample[0] * weight;
        self.samples[index][1] += sample[1] * weight;
        self.weights[index] += weight;
        Ok(())
    }

    /// Drain consecutive normalized frames into caller-owned storage.
    ///
    /// The requested length must not exceed the remaining output or the ring
    /// capacity. If a sample is nonfinite, earlier frames in this call remain
    /// committed; `position()` exposes the exact committed prefix and the
    /// failing frame remains in the ring.
    #[inline]
    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        let count = u64::try_from(output.len())
            .map_err(|_| Error::Invalid("granular drain length overflow"))?;
        if count > self.total - self.cursor || output.len() > self.samples.len() {
            return Err(Error::Invalid("granular drain exceeds available extent"));
        }
        for target in output {
            let index = (self.cursor % self.samples.len() as u64) as usize;
            let gain = if self.weights[index] > 1e-12 {
                1.0 / self.weights[index]
            } else {
                0.0
            };
            *target = finite(self.samples[index].map(|value| value * gain))?;
            self.samples[index] = [0.0; 2];
            self.weights[index] = 0.0;
            self.cursor += 1;
        }
        Ok(())
    }
}
