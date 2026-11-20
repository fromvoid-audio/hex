//! A directed musical clip keeps its envelope in forward destination time.
use super::*;
use hex_project::{AudioViewOccurrence, DirectedFadeClock};

/// Read the clip envelope separately from reflected event reads. Legacy
/// envelopes follow the same forward occurrences and fade clock as playback.
pub struct DirectedMusicalDestinationFade<'a> {
    outer: Option<ProjectedOuterAudioFade<'a>>,
    legacy: Option<LegacyFade<'a>>,
}

struct LegacyFade<'a> {
    projection: AudioViewProjection<'a>,
    clock: DirectedFadeClock,
    edit: &'a AudioClipEdit,
    tempo: &'a TempoMap,
    source: &'a PreparedMusicalEventSourceClock,
    current: Option<(Range<u64>, AudioViewOccurrence, AudioClipEdit)>,
}

impl<'a> DirectedMusicalDestinationFade<'a> {
    pub fn new(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        source: &'a PreparedMusicalEventSourceClock,
    ) -> Result<Self, Invalid> {
        let outer = clip.projected_outer_fade(tempo)?;
        let legacy = match clip.audio_edit() {
            Some(edit) if outer.is_none() && !edit.domain.fade.is_neutral() => {
                let projection = AudioViewProjection::new(clip, tempo)?;
                let clock = DirectedFadeClock::new(&projection)?;
                Some(LegacyFade {
                    projection,
                    clock,
                    edit,
                    tempo,
                    source,
                    current: None,
                })
            }
            _ => None,
        };
        Ok(Self { outer, legacy })
    }

    pub fn gain_at(&mut self, frame: u64) -> Result<f32, Invalid> {
        if let Some(outer) = &self.outer {
            return Ok(outer.gain_at(frame)?);
        }
        self.legacy
            .as_mut()
            .map_or(Ok(1.0), |legacy| legacy.gain_at(frame))
    }
}

impl LegacyFade<'_> {
    fn gain_at(&mut self, frame: u64) -> Result<f32, Invalid> {
        if !self
            .current
            .as_ref()
            .is_some_and(|(range, _, _)| range.contains(&frame))
        {
            let mut first = 0;
            let mut end = self.projection.occurrence_count();
            while first < end {
                let middle = first + (end - first) / 2;
                let occurrence = self.projection.occurrence(middle)?;
                if self.clock.destination(&self.projection, occurrence)?.end <= frame {
                    first = middle + 1;
                } else {
                    end = middle;
                }
            }
            if first >= self.projection.occurrence_count() {
                return Ok(0.0);
            }
            let occurrence = self.projection.occurrence(first)?;
            let range = self.clock.destination(&self.projection, occurrence)?;
            if !range.contains(&frame) {
                return Ok(0.0);
            }
            let root = bind_occurrence_clock(self.edit, occurrence.root_origin, || {
                Ok(self.source.shared_tempo())
            })?;
            self.current = Some((range, occurrence, root));
        }
        let (_, occurrence, root) = self
            .current
            .as_ref()
            .ok_or(Invalid("directed fade occurrence missing"))?;
        let sample = self
            .clock
            .root_sample(&self.projection, *occurrence, frame)?;
        Ok(root.projected_fade(self.tempo)?.gain_at(sample)?)
    }
}
