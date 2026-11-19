//! Destination-only fade finalization after the source program's last read.
//! Musical fades use bounded PCM on control. The callback sees an ordinary
//! resident clip and never evaluates a project tempo map.
use super::{Audio, AudioClipEdit, AudioTransformPreparation, Clip, Invalid};
use hex_project::{
    AudioClipFadeClock, AudioDomainClock, ClipFade, MAX_AUDIO_DOMAIN_FRAMES,
    ProjectedOuterAudioFade,
};

#[derive(Clone, Copy)]
pub(super) enum OuterPlaybackFade<'a> {
    Frame(ClipFade),
    Musical(&'a ProjectedOuterAudioFade<'a>),
}

impl<'a> OuterPlaybackFade<'a> {
    pub(super) fn projected(
        outer: Option<&'a ProjectedOuterAudioFade<'a>>,
        clock: AudioDomainClock,
    ) -> Result<Option<Self>, Invalid> {
        outer
            .map(|fade| match clock {
                AudioDomainClock::Frames(_) => Ok(Self::Frame(fade.frame_playback_envelope()?)),
                AudioDomainClock::Beats(_) => Ok(Self::Musical(fade)),
            })
            .transpose()
    }

    pub(super) fn admit(self, duration: u64, resident: bool) -> Result<(), Invalid> {
        if let Self::Musical(fade) = self
            && !fade.is_neutral()
        {
            if !resident {
                return Err(Invalid("musical outer fade requires resident audio"));
            }
            if duration == 0 || duration > MAX_AUDIO_DOMAIN_FRAMES {
                return Err(Invalid(
                    "musical outer fade exceeds bounded preparation frames",
                ));
            }
        }
        Ok(())
    }
}

/// A materialized output consumes one additional finite frame pass and buffer.
pub(crate) fn materializes_musical_outer_fade(edit: &AudioClipEdit) -> bool {
    edit.domain.fade_clock == AudioClipFadeClock::Outer
        && matches!(edit.domain.clock, AudioDomainClock::Beats(_))
        && !edit.domain.fade.is_neutral()
}

impl AudioTransformPreparation {
    pub(super) fn finish_outer_fade(
        &self,
        clip: Clip,
        fade: Option<OuterPlaybackFade<'_>>,
    ) -> Result<Clip, Invalid> {
        match fade {
            Some(OuterPlaybackFade::Frame(fade)) => clip.with_fade(fade),
            Some(OuterPlaybackFade::Musical(fade)) if !fade.is_neutral() => {
                self.materialize_musical_outer_fade(clip, fade)
            }
            _ => Ok(clip),
        }
    }

    fn materialize_musical_outer_fade(
        &self,
        mut clip: Clip,
        fade: &ProjectedOuterAudioFade<'_>,
    ) -> Result<Clip, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let source = clip
            .audio
            .as_ref()
            .ok_or(Invalid("musical outer fade requires resident audio"))?;
        let duration = clip
            .end
            .checked_sub(clip.start)
            .ok_or(Invalid("musical outer fade placement is reversed"))?;
        OuterPlaybackFade::Musical(fade).admit(duration, true)?;
        let phase = if let Some(metadata) = &clip.metadata {
            if metadata.fade.is_some()
                || metadata.sparse.is_some()
                || metadata.streamed.is_some()
                || metadata.reversed
            {
                return Err(Invalid(
                    "musical outer fade requires unfaded resident placement",
                ));
            }
            metadata.source_phase
        } else {
            0
        };
        let length = usize::try_from(duration)
            .map_err(|_| Invalid("musical outer fade span exceeds addressable frames"))?;
        let bytes = length
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("musical outer fade byte count overflow"))?;
        let reservation = self.reserve_with_eviction(bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(length)
            .map_err(|_| Invalid("musical outer fade allocation failed"))?;
        for (index, frame) in (clip.start..clip.end).enumerate() {
            if index % 1024 == 0 && self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let gain = fade.gain_at(frame)?;
            frames.push(clip.at(frame, phase).map(|sample| sample * gain));
        }
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        clip.audio = Some(Audio::from_derived(
            source.sample_rate(),
            frames,
            reservation,
        )?);
        clip.source_offset = 0;
        clip.source_length = length;
        clip.looping = false;
        if let Some(metadata) = clip.metadata.as_mut() {
            metadata.source_phase = 0;
        }
        Ok(clip)
    }
}
