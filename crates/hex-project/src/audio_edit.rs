//! Saved musical audio metadata. Processing roots are immutable; editing a crop
//! never restarts resampling, grains, analysis or fades at the crop boundary.
use crate::{BeatTime, Clip, ClipFade, Invalid, TempoMap, TrimEdge};
use std::sync::Arc;

pub const MAX_AUDIO_POINTS: usize = 1_024;
pub const MAX_AUDIO_DOMAIN_FRAMES: u64 = 16_777_216;
pub const SOURCE_GAIN_UNITY: u32 = 65_536;
pub const MAX_SOURCE_GAIN: u32 = SOURCE_GAIN_UNITY * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceGainPoint {
    /// Absolute project-rate frame in the original decoded source.
    pub frame: u64,
    /// Unsigned Q16.16 linear gain, including exact zero and unity; maximum 4.
    pub gain_q16: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarpMarker {
    pub source_frame: u64,
    /// Local quarter-note beat in one declared source region, beginning at zero.
    pub beat: BeatTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeatsPreserve {
    Transients,
    Division(BeatTime),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeatsTail {
    Off,
    Forward,
    Alternating,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpMode {
    RePitch,
    Beats {
        preserve: BeatsPreserve,
        tail: BeatsTail,
        decay: u8,
    },
    Tones {
        grain_ms: u16,
    },
    Texture {
        grain_ms: u16,
        fluctuation: u8,
        seed: u64,
    },
    Complex,
    ComplexPro {
        formants: u8,
        envelope: u16,
    },
}
impl WarpMode {
    pub fn validate(self) -> Result<(), Invalid> {
        match self {
            Self::Beats {
                preserve, decay, ..
            } => {
                if decay > 100 {
                    return Err(Invalid("transient decay must be within 0..100"));
                }
                if let BeatsPreserve::Division(beat) = preserve
                    && (beat.raw() == 0 || beat.raw() > BeatTime::ONE.raw() * 16)
                {
                    return Err(Invalid("beat preserve division must be within 0..16 beats"));
                }
            }
            Self::Tones { grain_ms } | Self::Texture { grain_ms, .. } => {
                if !(8..=200).contains(&grain_ms) {
                    return Err(Invalid("grain size must be within 8..200 ms"));
                }
                if let Self::Texture { fluctuation, .. } = self
                    && fluctuation > 100
                {
                    return Err(Invalid("grain fluctuation must be within 0..100"));
                }
            }
            Self::ComplexPro { formants, envelope } => {
                if formants > 100 || !(32..=512).contains(&envelope) {
                    return Err(Invalid(
                        "formants must be within 0..100 and envelope within 32..512",
                    ));
                }
            }
            Self::RePitch | Self::Complex => {}
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarpSettings {
    pub mode: WarpMode,
    pub markers: Vec<WarpMarker>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDomainClock {
    Frames(u64),
    /// Authored musical extent. Preparation resolves an output-frame domain;
    /// unwarped sources still advance at their declared playback rate.
    Beats(BeatTime),
}
impl AudioDomainClock {
    pub fn length(self) -> u64 {
        match self {
            Self::Frames(length) => length,
            Self::Beats(length) => length.raw(),
        }
    }
}

/// Frozen projection for an output fade. The signed project origin is technical
/// context only; authored placement/source/crop coordinates stay nonnegative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFadeProjection {
    pub tempo: TempoMap,
    pub origin_beat_raw: i128,
    pub local_start_raw: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipDomain {
    pub events: Option<Arc<crate::AudioEvents>>,
    pub source_offset: u64,
    pub source_length: u64,
    pub source_phase: u64,
    pub looping: bool,
    pub clock: AudioDomainClock,
    pub pitch_cents: i16,
    /// Authored original tempo, independent of project tempo and manual edge stretch.
    pub sample_bpm: Option<crate::Tempo>,
    pub warp: Option<WarpSettings>,
    pub warp_enabled: bool,
    pub source_gain: Vec<SourceGainPoint>,
    pub fade: ClipFade,
    pub fade_start: u64,
    pub fade_projection: Option<AudioFadeProjection>,
}

/// A crop of a retained full processing root. View units follow `domain.clock`.
/// The sole authored musical placement is its start plus the exact view span;
/// `Clip::start/duration` are checked derived frame caches in this case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipEdit {
    pub domain: Arc<AudioClipDomain>,
    pub view_start: u64,
    pub view_end: u64,
    pub musical_start: Option<BeatTime>,
    pub placement: Option<AudioViewPlacement>,
}

/// Read-only output/root-frame fade projection shared by preparation and views.
/// Construction and sampling allocate nothing; source gain is deliberately separate.
pub struct ProjectedAudioFade<'a> {
    pub(crate) fade: ClipFade,
    pub(crate) fade_start: u64,
    pub(crate) projection: Option<crate::audio_preview::ProjectionRead<'a>>,
    pub(crate) tempo: &'a TempoMap,
    pub(crate) first: i128,
    pub(crate) origin: i128,
    pub(crate) length: u64,
}
impl ProjectedAudioFade<'_> {
    /// Finite curve knots in the same root-output clock, with explicit rounding.
    pub fn breakpoints(&self) -> Result<[Option<u64>; 8], Invalid> {
        let mut result = [None; 8];
        for (index, position) in self.fade.breakpoints().into_iter().enumerate() {
            if let Some(position) = position {
                result[index] = self.frame_at_domain_position(position)?;
            }
        }
        Ok(result)
    }
    /// Project an authored fade-domain knot into the prepared root output clock.
    pub fn frame_at_domain_position(&self, position: u64) -> Result<Option<u64>, Invalid> {
        let Some(age) = position.checked_sub(self.fade.offset()) else {
            return Ok(None);
        };
        let root = if let Some(projection) = &self.projection {
            let first = audio_projection_frame(projection.tempo, projection.origin_beat_raw)?;
            let beat = audio_projection_beat(projection.tempo, first + i128::from(age))?;
            let local = i128::from(projection.local_start_raw) + beat - projection.origin_beat_raw;
            u64::try_from(audio_projection_frame(self.tempo, self.origin + local)? - self.first)
                .ok()
        } else {
            self.fade_start.checked_add(age)
        };
        Ok(root.filter(|frame| *frame < self.length))
    }

    pub fn gain_at(&self, root_output_frame: u64) -> Result<f32, Invalid> {
        if root_output_frame >= self.length {
            return Err(Invalid("fade sample is outside the prepared root"));
        }
        if self.fade.is_neutral() {
            return Ok(1.0);
        }
        let age = if let Some(projection) = &self.projection {
            let beat =
                audio_projection_beat(self.tempo, self.first + i128::from(root_output_frame))?
                    - self.origin;
            let delta = beat - i128::from(projection.local_start_raw);
            if delta < 0 {
                None
            } else {
                let start = audio_projection_frame(projection.tempo, projection.origin_beat_raw)?;
                let at =
                    audio_projection_frame(projection.tempo, projection.origin_beat_raw + delta)?;
                u64::try_from(at - start).ok()
            }
        } else {
            root_output_frame.checked_sub(self.fade_start)
        };
        Ok(age.map_or(0.0, |age| self.fade.gain_at(age)))
    }
}

/// Signed nearest-frame conversion; exact half ties move later, including -0.5.
/// Negative projection uses tempo-at-zero only for invisible analysis context.
pub fn audio_projection_frame(tempo: &TempoMap, beat_raw: i128) -> Result<i128, Invalid> {
    if beat_raw >= 0 {
        return tempo
            .beat_to_frame(BeatTime::from_raw(
                u64::try_from(beat_raw)
                    .map_err(|_| Invalid("audio projection exceeds beat range"))?,
            ))
            .map(i128::from);
    }
    // A legal Once cursor and selected root offset are each u64. Their
    // technical signed origin can therefore precede zero by up to two ranges.
    if beat_raw < -2 * i128::from(u64::MAX) {
        return Err(Invalid("audio projection precedes bounded beat range"));
    }
    let numerator = beat_raw
        .checked_mul(i128::from(tempo.sample_rate()) * 60_000_000)
        .ok_or(Invalid("audio projection overflow"))?;
    signed_round(
        numerator,
        u128::from(tempo.tempo_at(BeatTime::ZERO).micro_bpm()) << 32,
    )
}
pub fn audio_projection_beat(tempo: &TempoMap, frame: i128) -> Result<i128, Invalid> {
    if frame >= 0 {
        return tempo
            .frame_to_beat(
                u64::try_from(frame)
                    .map_err(|_| Invalid("audio projection exceeds frame range"))?,
            )
            .map(|beat| i128::from(beat.raw()));
    }
    let numerator = frame
        .checked_mul(i128::from(tempo.tempo_at(BeatTime::ZERO).micro_bpm()) << 32)
        .ok_or(Invalid("inverse audio projection overflow"))?;
    signed_round(numerator, u128::from(tempo.sample_rate()) * 60_000_000)
}
fn signed_round(numerator: i128, denominator: u128) -> Result<i128, Invalid> {
    if denominator == 0 {
        return Err(Invalid("audio projection denominator must be positive"));
    }
    let magnitude = numerator.unsigned_abs();
    let quotient = magnitude / denominator;
    let remainder = magnitude % denominator;
    let advance = if numerator < 0 {
        remainder > denominator - remainder
    } else {
        remainder >= denominator - remainder
    };
    let rounded = quotient
        .checked_add(u128::from(advance))
        .ok_or(Invalid("audio projection rounding overflow"))?;
    let rounded =
        i128::try_from(rounded).map_err(|_| Invalid("audio projection result overflow"))?;
    Ok(if numerator < 0 { -rounded } else { rounded })
}

impl AudioClipEdit {
    pub fn projected_fade<'a>(
        &'a self,
        tempo: &'a TempoMap,
    ) -> Result<ProjectedAudioFade<'a>, Invalid> {
        let (first, length, _, _) = self.projected_domain(tempo)?;
        let origin = if self.musical_start.is_some() {
            self.projection_origin()?
        } else {
            0
        };
        Ok(ProjectedAudioFade {
            fade: self.domain.fade,
            fade_start: self.domain.fade_start,
            projection: self.domain.fade_projection.as_ref().map(|p| {
                crate::audio_preview::ProjectionRead {
                    tempo: &p.tempo,
                    origin_beat_raw: p.origin_beat_raw,
                    local_start_raw: p.local_start_raw,
                }
            }),
            tempo,
            first,
            origin,
            length,
        })
    }
    pub(crate) fn capture(clip: &Clip) -> Self {
        let facts = crate::AudioClipRootFacts::capture(clip);
        let domain = facts.domain;
        Self {
            domain: Arc::new(AudioClipDomain {
                events: None,
                source_offset: domain.source_offset,
                source_length: domain.source_length,
                source_phase: domain.source_phase,
                looping: domain.looping,
                clock: domain.clock,
                pitch_cents: domain.pitch_cents,
                sample_bpm: domain.sample_bpm,
                warp: None,
                warp_enabled: domain.warp_enabled,
                source_gain: Vec::new(),
                fade: domain.fade,
                fade_start: domain.fade_start,
                fade_projection: None,
            }),
            view_start: facts.view_start,
            view_end: facts.view_end,
            musical_start: facts.musical_start,
            placement: facts.placement,
        }
    }

    pub fn root_frame_at(&self, position: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        crate::audio_preview::root_frame_at(self, position, tempo)
    }
    pub fn root_position_at(&self, frame: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        crate::audio_preview::root_position_at(self, frame, tempo)
    }
    pub fn projection_origin(&self) -> Result<i128, Invalid> {
        crate::audio_preview::projection_origin(self)
    }
    pub fn projected_domain(&self, tempo: &TempoMap) -> Result<(i128, u64, u64, u64), Invalid> {
        crate::audio_preview::projected_domain(self, tempo)
    }
    pub fn validate(
        &self,
        clip: &Clip,
        tempo: &TempoMap,
        asset_frames: u64,
    ) -> Result<(), Invalid> {
        crate::audio_preview::validate_edit(self, clip, tempo, asset_frames)
    }
    pub(crate) fn anchor(&mut self, clip_start: u64, tempo: &TempoMap) -> Result<(), Invalid> {
        crate::audio_preview::anchor_edit(self, clip_start, tempo)
    }
    pub(crate) fn unanchor(&mut self, tempo: &TempoMap) -> Result<(), Invalid> {
        if self.musical_start.is_some() && self.domain.events.is_some() {
            return Err(Invalid(
                "timing mode cannot change after audio event edits; undo those edits first",
            ));
        }
        if self.musical_start.is_some() && !self.domain.fade.is_neutral() {
            return Err(Invalid(
                "musical audio with a retained fade cannot switch to frame anchoring; clear the fade first",
            ));
        }
        if self.domain.warp_enabled {
            return Err(Invalid("disable warp before frame anchoring"));
        }
        let (origin_frame, length, start, end) = self.projected_domain(tempo)?;
        let placement = self
            .placement
            .map(|p| -> Result<_, Invalid> {
                if let Some(musical_start) = self.musical_start {
                    let first = tempo.beat_to_frame(musical_start)?;
                    let last = tempo.beat_to_frame(BeatTime::from_raw(
                        musical_start
                            .raw()
                            .checked_add(p.outer_length.length())
                            .ok_or(Invalid("audio outer end overflow"))?,
                    ))?;
                    Ok(AudioViewPlacement {
                        mode: p.mode,
                        outer_length: AudioDomainClock::Frames(last - first),
                        cycle_phase: u64::try_from(
                            i128::from(first) - origin_frame - i128::from(start),
                        )
                        .map_err(|_| Invalid("audio phase frame span invalid"))?,
                    })
                } else {
                    Ok(p)
                }
            })
            .transpose()?;
        let domain = Arc::make_mut(&mut self.domain);
        domain.clock = AudioDomainClock::Frames(length);
        // Reanchoring is explicit. Preserve the visible fade endpoints in frames.
        domain.fade = ClipFade::new(
            length
                .checked_add(domain.fade.offset())
                .ok_or(Invalid("frame fade span overflow"))?,
            domain.fade.offset(),
            domain.fade.fade_in().min(length),
            domain.fade.fade_out().min(length),
        )?
        .with_shape(
            domain.fade.in_start(),
            domain.fade.out_end_offset(),
            domain.fade.in_curve(),
            domain.fade.out_curve(),
        )?;
        domain.fade_start = 0;
        domain.fade_projection = None;
        self.view_start = start;
        self.view_end = end;
        self.musical_start = None;
        self.placement = placement;
        Ok(())
    }
    pub(crate) fn crop_at_frame(
        &self,
        clip: &Clip,
        frame: u64,
        tempo: &TempoMap,
    ) -> Result<u64, Invalid> {
        match self.domain.clock {
            AudioDomainClock::Frames(_) => self
                .view_start
                .checked_add(
                    frame
                        .checked_sub(clip.start)
                        .ok_or(Invalid("audio crop precedes clip"))?,
                )
                .ok_or(Invalid("audio crop overflow")),
            AudioDomainClock::Beats(_) => u64::try_from(
                i128::from(tempo.frame_to_beat(frame)?.raw()) - self.projection_origin()?,
            )
            .map_err(|_| Invalid("audio crop precedes its root")),
        }
    }
}

mod commands;
pub use commands::default_audio_warp;
mod view;
pub(crate) use commands::{
    apply, move_clip, project_clip, reset_region, set_fade_envelope, set_fades, source_policy,
    trim_view,
};
pub(crate) use view::resize_view;
pub use view::{
    AudioViewMode, AudioViewOccurrence, AudioViewPlacement, AudioViewProjection, AudioViewResize,
    AudioViewResizeOperation, preview_audio_view_resize, preview_view_resize,
};

mod stretch;
pub use stretch::preview_audio_clip_stretch;

pub(crate) mod warp_projection;
