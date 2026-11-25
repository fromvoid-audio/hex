//! Unified sequential checkpointed warp engine covering Tones/Texture, Beats,
//! and Complex/ComplexPro under one bounded control-side interface.
use crate::{
    BeatsCheckpoint, Error, Frame, GranularCheckpoint, GranularSource, Mode, Resampler,
    SequentialBeats, SequentialBeatsConfig, SequentialGranular, SequentialGranularConfig,
    SequentialSpectral, SequentialSpectralConfig, SpectralCheckpoint, TimeMap,
};

/// Configuration for any checkpointed sequential warp mode.
pub enum SequentialWarpConfig {
    Granular(SequentialGranularConfig),
    Beats(SequentialBeatsConfig),
    Spectral(SequentialSpectralConfig),
}

impl SequentialWarpConfig {
    pub fn map(&self) -> &TimeMap {
        match self {
            Self::Granular(c) => &c.map,
            Self::Beats(c) => &c.map,
            Self::Spectral(c) => &c.map,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Granular(c) => c.sample_rate,
            Self::Beats(c) => c.sample_rate,
            Self::Spectral(c) => c.sample_rate,
        }
    }

    pub fn mode(&self) -> Mode {
        match self {
            Self::Granular(c) => c.mode,
            Self::Beats(c) => c.mode,
            Self::Spectral(c) => c.mode,
        }
    }

    pub fn pitch_cents(&self) -> i16 {
        match self {
            Self::Granular(c) => c.pitch_cents,
            Self::Beats(c) => c.pitch_cents,
            Self::Spectral(c) => c.pitch_cents,
        }
    }

    pub fn resampler(&self) -> Resampler {
        match self {
            Self::Granular(c) => c.resampler,
            Self::Beats(c) => c.resampler,
            Self::Spectral(c) => c.resampler,
        }
    }

    pub fn attacks(&self) -> &[u64] {
        match self {
            Self::Beats(c) => &c.attacks,
            Self::Granular(_) | Self::Spectral(_) => &[],
        }
    }
}

/// Instance-bound checkpoint for any sequential warp mode.
pub enum WarpCheckpoint {
    Granular(GranularCheckpoint),
    Beats(Box<BeatsCheckpoint>),
    Spectral(SpectralCheckpoint),
}

impl WarpCheckpoint {
    pub fn position(&self) -> u64 {
        match self {
            Self::Granular(c) => c.position(),
            Self::Beats(c) => c.position(),
            Self::Spectral(c) => c.position(),
        }
    }

    pub fn backing_bytes(&self) -> Option<usize> {
        match self {
            Self::Granular(c) => c.backing_bytes(),
            Self::Beats(c) => c.backing_bytes(),
            Self::Spectral(c) => c.backing_bytes(),
        }
    }

    pub fn as_granular(&self) -> Option<&GranularCheckpoint> {
        match self {
            Self::Granular(c) => Some(c),
            _ => None,
        }
    }

    pub fn into_granular(self) -> Result<GranularCheckpoint, Error> {
        match self {
            Self::Granular(c) => Ok(c),
            _ => Err(Error::Invalid("checkpoint is not a granular checkpoint")),
        }
    }
}

/// Bounded control-side sequential warp renderer for Tones, Texture, Beats,
/// Complex, and ComplexPro modes.
pub enum SequentialWarp {
    Granular(Box<SequentialGranular>),
    Beats(Box<SequentialBeats>),
    Spectral(Box<SequentialSpectral>),
}

impl SequentialWarp {
    pub fn new(
        config: SequentialWarpConfig,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, Error> {
        match config {
            SequentialWarpConfig::Granular(c) => Ok(Self::Granular(Box::new(
                SequentialGranular::new(c, cancel)?,
            ))),
            SequentialWarpConfig::Beats(c) => {
                Ok(Self::Beats(Box::new(SequentialBeats::new(c, cancel)?)))
            }
            SequentialWarpConfig::Spectral(c) => Ok(Self::Spectral(Box::new(
                SequentialSpectral::new(c, cancel)?,
            ))),
        }
    }

    pub fn from_granular(renderer: SequentialGranular) -> Self {
        Self::Granular(Box::new(renderer))
    }

    pub fn from_beats(renderer: SequentialBeats) -> Self {
        Self::Beats(Box::new(renderer))
    }

    pub fn from_spectral(renderer: SequentialSpectral) -> Self {
        Self::Spectral(Box::new(renderer))
    }

    pub fn is_pristine(&self) -> bool {
        match self {
            Self::Granular(r) => r.is_pristine(),
            Self::Beats(r) => r.is_pristine(),
            Self::Spectral(r) => r.is_pristine(),
        }
    }

    pub fn total_frames(&self) -> u64 {
        match self {
            Self::Granular(r) => r.total_frames(),
            Self::Beats(r) => r.total_frames(),
            Self::Spectral(r) => r.total_frames(),
        }
    }

    /// Sample rate and optional granular reference channel (`None` for Beats/Spectral).
    pub fn source_parameters(&self) -> (u32, Option<usize>) {
        match self {
            Self::Granular(r) => {
                let (rate, ch) = r.source_parameters();
                (rate, Some(ch))
            }
            Self::Beats(r) => (r.sample_rate(), None),
            Self::Spectral(r) => (r.sample_rate(), None),
        }
    }

    pub fn backing_bytes(&self) -> Option<usize> {
        match self {
            Self::Granular(r) => r.backing_bytes(),
            Self::Beats(r) => r.backing_bytes(),
            Self::Spectral(r) => r.backing_bytes(),
        }
    }

    pub fn max_source_frames(&self) -> usize {
        match self {
            Self::Granular(r) => r.max_source_frames(),
            Self::Beats(r) => r.max_source_frames(),
            Self::Spectral(r) => r.max_source_frames(),
        }
    }

    pub fn available(&self) -> usize {
        match self {
            Self::Granular(r) => r.available(),
            Self::Beats(r) => r.available(),
            Self::Spectral(r) => r.available(),
        }
    }

    pub fn position(&self) -> u64 {
        match self {
            Self::Granular(r) => r.position(),
            Self::Beats(r) => r.position(),
            Self::Spectral(r) => r.position(),
        }
    }

    pub fn is_poisoned(&self) -> bool {
        match self {
            Self::Granular(r) => r.is_poisoned(),
            Self::Beats(r) => r.is_poisoned(),
            Self::Spectral(r) => r.is_poisoned(),
        }
    }

    pub fn is_complete(&self) -> bool {
        match self {
            Self::Granular(r) => r.is_complete(),
            Self::Beats(r) => r.is_complete(),
            Self::Spectral(r) => r.is_complete(),
        }
    }

    pub fn checkpoint(&self, budget: usize) -> Result<WarpCheckpoint, Error> {
        match self {
            Self::Granular(r) => Ok(WarpCheckpoint::Granular(r.checkpoint(budget)?)),
            Self::Beats(r) => Ok(WarpCheckpoint::Beats(Box::new(r.checkpoint(budget)?))),
            Self::Spectral(r) => Ok(WarpCheckpoint::Spectral(r.checkpoint(budget)?)),
        }
    }

    pub fn owns_checkpoint(&self, checkpoint: &WarpCheckpoint) -> bool {
        match (self, checkpoint) {
            (Self::Granular(r), WarpCheckpoint::Granular(c)) => r.owns_checkpoint(c),
            (Self::Beats(r), WarpCheckpoint::Beats(c)) => r.owns_checkpoint(c),
            (Self::Spectral(r), WarpCheckpoint::Spectral(c)) => r.owns_checkpoint(c),
            _ => false,
        }
    }

    pub fn refresh_checkpoint(&self, checkpoint: &mut WarpCheckpoint) -> Result<(), Error> {
        match (self, checkpoint) {
            (Self::Granular(r), WarpCheckpoint::Granular(c)) => r.refresh_checkpoint(c),
            (Self::Beats(r), WarpCheckpoint::Beats(c)) => r.refresh_checkpoint(c),
            (Self::Spectral(r), WarpCheckpoint::Spectral(c)) => r.refresh_checkpoint(c),
            _ => Err(Error::Invalid("foreign warp checkpoint")),
        }
    }

    pub fn restore(&mut self, checkpoint: &WarpCheckpoint) -> Result<(), Error> {
        match (self, checkpoint) {
            (Self::Granular(r), WarpCheckpoint::Granular(c)) => r.restore(c),
            (Self::Beats(r), WarpCheckpoint::Beats(c)) => r.restore(c),
            (Self::Spectral(r), WarpCheckpoint::Spectral(c)) => r.restore(c),
            _ => Err(Error::Invalid("foreign warp checkpoint")),
        }
    }

    pub fn restore_granular(&mut self, checkpoint: &GranularCheckpoint) -> Result<(), Error> {
        match self {
            Self::Granular(r) => r.restore(checkpoint),
            _ => Err(Error::Invalid("foreign granular checkpoint")),
        }
    }

    pub fn advance<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        match self {
            Self::Granular(r) => r.advance(source, scratch, cancel),
            Self::Beats(r) => r.advance(source, scratch, cancel),
            Self::Spectral(r) => r.advance(source, scratch, cancel),
        }
    }

    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        match self {
            Self::Granular(r) => r.drain(output),
            Self::Beats(r) => r.drain(output),
            Self::Spectral(r) => r.drain(output),
        }
    }
}
