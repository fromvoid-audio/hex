//! Instance envelope in the unwrapped, forward destination clock. This query
//! grants no canonical admission, saved-format support or playback activation.
use super::{
    AudioClipDomain, AudioClipFadeClock, AudioDomainClock, AudioFadeProjection,
    AudioViewProjection, audio_projection_beat, audio_projection_frame,
};
use crate::{Clip, ClipFade, Invalid, TempoMap};
mod crop;
pub(crate) use crop::retain_outer_fade_crop_context;

/// Clip-context outer fade projection. Source repetition, slip and reflection
/// never restart or reverse this envelope. Gain samples use project frames;
/// knot/inverse queries also accept the exclusive clip-end boundary.
/// The borrowed placement and authoring maps retain live/frozen musical laws.
pub struct ProjectedOuterAudioFade<'a> {
    view: AudioViewProjection<'a>,
    fade: ClipFade,
    fade_start: u64,
    displacement: i128,
    author: Option<Author<'a>>,
}

#[derive(Clone, Copy)]
enum Author<'a> {
    Stored(&'a AudioFadeProjection),
    Placement,
}

impl Clip {
    /// Project an explicitly Outer instance fade without admitting that clock.
    /// Legacy roots and ordinary clips return None; their existing fade paths
    /// remain unchanged. Callers must separately admit/prepare supported state.
    pub fn projected_outer_fade<'a>(
        &'a self,
        tempo: &'a TempoMap,
    ) -> Result<Option<ProjectedOuterAudioFade<'a>>, Invalid> {
        let Some(edit) = self
            .audio_edit()
            .filter(|edit| edit.domain.fade_clock == AudioClipFadeClock::Outer)
        else {
            return Ok(None);
        };
        let domain = &edit.domain;
        if matches!(domain.clock, AudioDomainClock::Frames(_))
            && domain.fade.offset() != 0
            && domain.fade_start != 0
        {
            return Err(Invalid("outer frame fade context must be normalized"));
        }
        let author = domain.fade_projection.as_ref();
        if matches!(domain.clock, AudioDomainClock::Beats(_)) != author.is_some()
            || author.is_some_and(|author| author.tempo.sample_rate() != tempo.sample_rate())
        {
            return Err(Invalid(
                "outer fade authoring clock differs from its placement",
            ));
        }
        if let Some(author) = author {
            audio_projection_frame(&author.tempo, author.origin_beat_raw)?;
        }
        Ok(Some(ProjectedOuterAudioFade {
            view: AudioViewProjection::new(self, tempo)?,
            fade: domain.fade,
            fade_start: domain.fade_start,
            displacement: if author.is_some() {
                domain.outer_fade_displacement()?
            } else {
                0
            },
            author: author.map(Author::Stored),
        }))
    }

    /// Project a held/pending explicit fade edit as its prospective Outer law.
    /// The payload has the same local-frame span/offset requirements as
    /// SetClipFadeEnvelope. It adopts Outer even on a Legacy root, without
    /// changing the clip, cloning a tempo map or granting canonical admission.
    /// An already prospective clip should use projected_outer_fade instead.
    pub fn projected_outer_fade_preview<'a>(
        &'a self,
        tempo: &'a TempoMap,
        fade: ClipFade,
    ) -> Result<ProjectedOuterAudioFade<'a>, Invalid> {
        if fade.span() != self.duration() || fade.offset() != 0 {
            return Err(Invalid("clip fade envelope must match clip duration"));
        }
        fade.validate_segment(self.duration())?;
        let author = self
            .audio_edit()
            .filter(|edit| matches!(edit.domain.clock, AudioDomainClock::Beats(_)))
            .map(|_| Author::Placement);
        Ok(ProjectedOuterAudioFade {
            view: AudioViewProjection::new(self, tempo)?,
            fade,
            fade_start: 0,
            displacement: 0,
            author,
        })
    }
}

impl AudioClipDomain {
    /// Signed raw-beat displacement of an Outer musical envelope. The author
    /// origin and shape offset stay immutable: consumed ticks use local_start,
    /// prefix ticks use fade_start, and at most one may be nonzero.
    pub fn outer_fade_displacement(&self) -> Result<i128, Invalid> {
        if self.fade_clock != AudioClipFadeClock::Outer
            || !matches!(self.clock, AudioDomainClock::Beats(_))
        {
            return Err(Invalid(
                "outer fade displacement requires a musical outer clock",
            ));
        }
        let author = self
            .fade_projection
            .as_ref()
            .ok_or(Invalid("musical outer fade authoring clock is missing"))?;
        if author.local_start_raw != 0 && self.fade_start != 0 {
            return Err(Invalid("outer fade displacement must be normalized"));
        }
        Ok(i128::from(author.local_start_raw) - i128::from(self.fade_start))
    }

    /// Shift the unwrapped destination context without changing the captured
    /// author origin, tempo or shape. This metadata helper grants no admission.
    /// Overflow refuses before either half of the normalized pair changes.
    pub fn shift_outer_fade_displacement(&mut self, elapsed_raw: i128) -> Result<(), Invalid> {
        let next = self
            .outer_fade_displacement()?
            .checked_add(elapsed_raw)
            .ok_or(Invalid("outer fade displacement overflow"))?;
        let distance = u64::try_from(next.unsigned_abs())
            .map_err(|_| Invalid("outer fade displacement exceeds clock range"))?;
        let author = self
            .fade_projection
            .as_mut()
            .ok_or(Invalid("musical outer fade authoring clock is missing"))?;
        author.local_start_raw = if next >= 0 { distance } else { 0 };
        self.fade_start = if next < 0 { distance } else { 0 };
        Ok(())
    }
}

impl ProjectedOuterAudioFade<'_> {
    /// Whether this instance envelope contributes unity throughout the clip.
    pub fn is_neutral(&self) -> bool {
        self.fade.is_neutral()
    }

    /// Project a finite Frame envelope into the playback scalar fade slot.
    /// Positive prefix shifts both ramps. Padding after the authored end
    /// preserves its silent tail, and the retained offset stays unwrapped.
    /// Musical placement requires a separate prepared representation.
    pub fn frame_playback_envelope(&self) -> Result<ClipFade, Invalid> {
        if self.author.is_some() {
            return Err(Invalid(
                "musical outer fade requires musical playback preparation",
            ));
        }
        let (start, end) = self.view.outer_frame_bounds();
        let duration = end - start;
        let fade = self.fade;
        if fade.is_neutral() {
            return ClipFade::neutral(duration);
        }
        let prefix = self.fade_start;
        if prefix >= duration {
            return ClipFade::new(duration, 0, 0, 1)?.with_shape(0, duration - 1, 0, 0);
        }
        let shifted_span = fade
            .span()
            .checked_add(prefix)
            .ok_or(Invalid("outer frame fade prefix exceeds its playback span"))?;
        let required_span = fade
            .offset()
            .checked_add(duration)
            .ok_or(Invalid("outer frame fade playback span overflow"))?;
        let span = shifted_span.max(required_span);
        let out_end = shifted_span - fade.out_end_offset();
        ClipFade::new(span, fade.offset(), fade.fade_in(), fade.fade_out())?
            .with_shape(
                fade.in_start() + prefix,
                span - out_end,
                fade.in_curve(),
                fade.out_curve(),
            )?
            .with_curve_positions(fade.in_position(), fade.out_position())
            .map(|placed| placed.with_curve_laws(fade.curve_laws()))
    }

    fn author_clock(&self) -> Option<(&TempoMap, i128)> {
        self.author.map(|author| match author {
            Author::Stored(author) => (&author.tempo, author.origin_beat_raw),
            Author::Placement => (self.view.placement_tempo(), self.view.placement_reference()),
        })
    }
    fn elapsed_at_frame(&self, frame: u64) -> Result<u64, Invalid> {
        let (start, end) = self.view.outer_frame_bounds();
        if !(start..=end).contains(&frame) {
            return Err(Invalid("fade boundary is outside the outer clip"));
        }
        self.view.elapsed_at_frame(frame)
    }

    fn age_at_frame(&self, frame: u64) -> Result<Option<u64>, Invalid> {
        let elapsed = self.elapsed_at_frame(frame)?;
        if let Some((tempo, origin)) = self.author_clock() {
            let raw_age = self
                .displacement
                .checked_add(i128::from(elapsed))
                .ok_or(Invalid("outer fade elapsed coordinate overflow"))?;
            if raw_age < 0 {
                return Ok(None);
            }
            let start = audio_projection_frame(tempo, origin)?;
            let at = audio_projection_frame(
                tempo,
                origin
                    .checked_add(raw_age)
                    .ok_or(Invalid("outer fade authoring coordinate overflow"))?,
            )?;
            Ok(Some(
                u64::try_from(at - start).map_err(|_| Invalid("outer fade age exceeds frames"))?,
            ))
        } else {
            Ok(elapsed.checked_sub(self.fade_start))
        }
    }

    /// Gain at one project-frame sample inside the half-open clip bounds.
    pub fn gain_at(&self, frame: u64) -> Result<f32, Invalid> {
        let (start, end) = self.view.outer_frame_bounds();
        if frame < start || frame >= end {
            return Err(Invalid("fade sample is outside the outer clip"));
        }
        if self.fade.is_neutral() {
            return Ok(1.0);
        }
        Ok(self
            .age_at_frame(frame)?
            .map_or(0.0, |age| self.fade.gain_at(age)))
    }

    /// Authored fade-domain coordinate, including the full retained offset.
    /// Before-envelope context returns None. No repeat modulo is applied.
    pub fn domain_position_at_frame(&self, frame: u64) -> Result<Option<u64>, Invalid> {
        self.age_at_frame(frame)?
            .map(|age| {
                self.fade
                    .offset()
                    .checked_add(age)
                    .ok_or(Invalid("outer fade domain position overflow"))
            })
            .transpose()
    }

    /// Project a shape-domain boundary to the forward destination, if visible.
    /// Hidden knots return None; this does not clamp them onto an outer edge.
    pub fn frame_at_domain_position(&self, position: u64) -> Result<Option<u64>, Invalid> {
        if position > self.fade.span() {
            return Err(Invalid("fade boundary exceeds its authored span"));
        }
        let Some(age) = position.checked_sub(self.fade.offset()) else {
            return Ok(None);
        };
        let elapsed = if let Some((tempo, origin)) = self.author_clock() {
            // Zero author age is an identity even at a sub-frame signed origin.
            // Crop displacement is removed only after that original reference.
            let raw_age = if age == 0 {
                0
            } else {
                let first = audio_projection_frame(tempo, origin)?;
                let at = first
                    .checked_add(i128::from(age))
                    .ok_or(Invalid("outer fade authoring frame overflow"))?;
                audio_projection_beat(tempo, at)? - origin
            };
            let elapsed = raw_age
                .checked_sub(self.displacement)
                .ok_or(Invalid("outer fade inverse coordinate overflow"))?;
            let Ok(elapsed) = u64::try_from(elapsed) else {
                return Ok(None);
            };
            elapsed
        } else {
            self.fade_start
                .checked_add(age)
                .ok_or(Invalid("outer fade boundary overflow"))?
        };
        let (start, end) = self.view.outer_frame_bounds();
        if elapsed > self.view.elapsed_at_frame(end)? {
            return Ok(None);
        }
        let frame = self.view.frame_at_elapsed(elapsed)?;
        Ok((start..=end).contains(&frame).then_some(frame))
    }

    /// At most eight authored curve knots in project-frame coordinates.
    pub fn breakpoints(&self) -> Result<[Option<u64>; 8], Invalid> {
        let mut points = [None; 8];
        for (out, position) in points.iter_mut().zip(self.fade.breakpoints()) {
            if let Some(position) = position {
                *out = self.frame_at_domain_position(position)?;
            }
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests;
