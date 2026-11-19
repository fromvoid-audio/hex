//! Validated project-rate source regions used by control-side readers.
use crate::{Error, GainPoint, MAX_POINTS, MAX_STREAMED_OUTPUT_FRAMES, source_gain};

/// Immutable logical source region and source-frame gain envelope.
/// The region owns no audio; source identity and generation remain caller-owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRegion {
    source_frames: u64,
    start: u64,
    length: u64,
    phase: u64,
    looping: bool,
    envelope: Vec<GainPoint>,
}
impl SourceRegion {
    /// Validate a project-rate region and its bounded, ordered source gain points.
    pub fn new(
        source_frames: u64,
        start: u64,
        length: u64,
        phase: u64,
        looping: bool,
        envelope: Vec<GainPoint>,
    ) -> Result<Self, Error> {
        if source_frames == 0
            || source_frames > MAX_STREAMED_OUTPUT_FRAMES
            || length == 0
            || start
                .checked_add(length)
                .is_none_or(|end| end > source_frames)
            || (looping && phase >= length)
            || (!looping && phase != 0)
        {
            return Err(Error::Invalid(
                "invalid audio preparation source interval, phase or rate",
            ));
        }
        if envelope.len() > MAX_POINTS
            || envelope.capacity() > MAX_POINTS
            || envelope
                .iter()
                .any(|p| p.source_frame > source_frames || p.gain_q16 > 4 * 65_536)
            || envelope
                .windows(2)
                .any(|p| p[0].source_frame >= p[1].source_frame)
        {
            return Err(Error::Invalid("invalid source gain envelope"));
        }
        Ok(Self {
            source_frames,
            start,
            length,
            phase,
            looping,
            envelope,
        })
    }
    pub fn source_frames(&self) -> u64 {
        self.source_frames
    }
    /// Immutable inputs that define how relative frames resolve into source audio.
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn phase(&self) -> u64 {
        self.phase
    }
    pub fn looping(&self) -> bool {
        self.looping
    }
    pub fn envelope(&self) -> &[GainPoint] {
        &self.envelope
    }
    pub fn length(&self) -> u64 {
        self.length
    }
    /// Actual retained envelope allocation, excluding this inline object.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.envelope
            .capacity()
            .checked_mul(std::mem::size_of::<GainPoint>())
    }
    /// Resolve an unwrapped relative frame into a source index and linear gain.
    /// Non-looping positions outside the region are silence; overflow is an error.
    pub fn resolve(&self, relative: i128) -> Result<Option<(u64, f64)>, Error> {
        let position = relative
            .checked_add(i128::from(self.phase))
            .ok_or(Error::Invalid("source region position overflow"))?;
        let local = if self.looping {
            // Normal transport and grain positions fit i64. Keep the i128
            // fallback for the full public input domain and preserve Euclidean
            // wrapping for negative positions.
            match (i64::try_from(position), i64::try_from(self.length)) {
                (Ok(position), Ok(length)) => i128::from(position.rem_euclid(length)),
                _ => position.rem_euclid(i128::from(self.length)),
            }
        } else if position < 0 || position >= i128::from(self.length) {
            return Ok(None);
        } else {
            position
        };
        let index = self
            .start
            .checked_add(local as u64)
            .ok_or(Error::Invalid("source region index overflow"))?;
        Ok(Some((index, source_gain(&self.envelope, index))))
    }
}
