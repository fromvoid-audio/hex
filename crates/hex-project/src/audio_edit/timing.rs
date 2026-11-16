//! Processing projection is independent of a frozen clip's destination frames.
use super::*;

/// Live musical placement follows the project map. Frozen placement keeps the
/// complete old processing clock (including event/fade/loop projections), while
/// `Clip::start` owns its destination. These maps are control-owned and shared
/// across clips/history; they never introduce a callback clock or PCM owner.
/// Only `Musical` may also carry a section `processing_projection` and its
/// bounded placement residuals. Retained outer Once/Repeat reads use the saved
/// processing map for every occurrence and translate its exact frames to that
/// live onset; outer endpoints still obey the musical residual contract.
/// `Frozen` is the sole processing clock, requires
/// warp off, and carries neither override nor residuals. Its destination
/// displacement is separate from its immutable reference/source coordinates.
/// `Frames` carries neither override nor residuals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioClipTiming {
    Frames,
    Musical(BeatTime),
    Frozen {
        /// Signed technical reference placement, not an authored negative clip.
        /// A moved crop may extend left into its retained pre-zero root.
        reference_start_raw: i128,
        tempo: Arc<TempoMap>,
    },
}

impl AudioClipEdit {
    pub fn live_musical_start(&self) -> Option<BeatTime> {
        match self.timing {
            AudioClipTiming::Musical(start) => Some(start),
            _ => None,
        }
    }

    pub fn reference_start_raw(&self) -> Option<i128> {
        match &self.timing {
            AudioClipTiming::Frames => None,
            AudioClipTiming::Musical(start) => Some(i128::from(start.raw())),
            AudioClipTiming::Frozen {
                reference_start_raw,
                ..
            } => Some(*reference_start_raw),
        }
    }

    pub fn effective_tempo<'a>(&'a self, project: &'a TempoMap) -> &'a TempoMap {
        match &self.timing {
            AudioClipTiming::Frozen { tempo, .. } => tempo,
            _ => project,
        }
    }

    pub fn destination_displacement(&self, clip: &Clip) -> Result<i128, Invalid> {
        match &self.timing {
            AudioClipTiming::Frozen {
                reference_start_raw,
                tempo,
            } => i128::from(clip.start)
                .checked_sub(audio_projection_frame(tempo, *reference_start_raw)?)
                .ok_or(Invalid("frozen audio destination displacement overflow")),
            _ => Ok(0),
        }
    }

    pub fn reference_beat_at_destination(
        &self,
        clip: &Clip,
        frame: u64,
        project: &TempoMap,
    ) -> Result<i128, Invalid> {
        let reference = i128::from(frame)
            .checked_sub(self.destination_displacement(clip)?)
            .ok_or(Invalid("frozen audio inverse placement overflow"))?;
        audio_projection_beat(self.effective_tempo(project), reference)
    }

    /// Update a crop's reference without changing its live/frozen timing kind.
    pub fn set_reference_start_raw(&mut self, start: Option<i128>) -> Result<(), Invalid> {
        match (&mut self.timing, start) {
            (
                AudioClipTiming::Frozen {
                    reference_start_raw,
                    ..
                },
                Some(start),
            ) => {
                *reference_start_raw = start;
            }
            (_, Some(start)) => {
                self.timing = AudioClipTiming::Musical(BeatTime::from_raw(
                    u64::try_from(start)
                        .map_err(|_| Invalid("musical audio placement exceeds beat range"))?,
                ));
            }
            (_, None) => self.timing = AudioClipTiming::Frames,
        }
        Ok(())
    }
}
