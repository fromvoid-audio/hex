//! Saved musical audio metadata. Processing roots are immutable; editing a crop
//! never restarts resampling, grains, analysis or fades at the crop boundary.
use crate::{BeatTime, Clip, ClipFade, Invalid, TempoMap, TrimEdge};
use std::sync::Arc;

mod timing;
pub use timing::AudioClipTiming;

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

/// Immutable original processing projection; never media or prepared PCM custody.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioProcessingProjection {
    pub tempo: Arc<TempoMap>,
    pub origin_beat_raw: i128,
}

/// A crop of a retained full processing root. View units follow `domain.clock`.
/// Live musical placement derives frame caches from the project map. A frozen
/// projection retains the old processing map and uses Clip::start as its frame
/// destination; its source/crop/loop/fade coordinates remain unchanged.
/// A section override is legal only with `AudioClipTiming::Musical`; the
/// `Frozen` variant owns its complete processing clock without an override or
/// residuals. Frame destination displacement never changes source coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipEdit {
    /// Complete source processing retained after structural song relocation.
    /// Only Musical timing may carry this override; Frozen owns its own clock.
    pub processing_projection: Option<Arc<AudioProcessingProjection>>,
    /// Explicit nearest-frame phase residuals, each bounded to one sample.
    pub placement_start_offset: i8,
    pub placement_end_offset: i8,
    pub domain: Arc<AudioClipDomain>,
    pub view_start: u64,
    pub view_end: u64,
    pub timing: AudioClipTiming,
    pub placement: Option<AudioViewPlacement>,
}

/// Read-only output/root-frame fade projection shared by preparation and views.
/// Construction and sampling allocate nothing; source gain is deliberately separate.
pub struct ProjectedAudioFade<'a> {
    edit: &'a AudioClipEdit,
    tempo: &'a TempoMap,
    first: i128,
    origin: i128,
    length: u64,
}
impl ProjectedAudioFade<'_> {
    /// Finite curve knots in the same root-output clock, with explicit rounding.
    pub fn breakpoints(&self) -> Result<[Option<u64>; 8], Invalid> {
        let mut result = [None; 8];
        for (index, position) in self.edit.domain.fade.breakpoints().into_iter().enumerate() {
            if let Some(position) = position {
                result[index] = self.frame_at_domain_position(position)?;
            }
        }
        Ok(result)
    }
    /// Project an authored fade-domain knot into the prepared root output clock.
    pub fn frame_at_domain_position(&self, position: u64) -> Result<Option<u64>, Invalid> {
        let domain = &self.edit.domain;
        let Some(age) = position.checked_sub(domain.fade.offset()) else {
            return Ok(None);
        };
        let root = if let Some(projection) = &domain.fade_projection {
            let first = audio_projection_frame(&projection.tempo, projection.origin_beat_raw)?;
            let beat = audio_projection_beat(&projection.tempo, first + i128::from(age))?;
            let local = i128::from(projection.local_start_raw) + beat - projection.origin_beat_raw;
            u64::try_from(audio_projection_frame(self.tempo, self.origin + local)? - self.first)
                .ok()
        } else {
            domain.fade_start.checked_add(age)
        };
        Ok(root.filter(|frame| *frame < self.length))
    }

    pub fn gain_at(&self, root_output_frame: u64) -> Result<f32, Invalid> {
        if root_output_frame >= self.length {
            return Err(Invalid("fade sample is outside the prepared root"));
        }
        let domain = &self.edit.domain;
        if domain.fade.is_neutral() {
            return Ok(1.0);
        }
        let age = if let Some(projection) = &domain.fade_projection {
            let beat =
                audio_projection_beat(self.tempo, self.first + i128::from(root_output_frame))?
                    - self.origin;
            let delta = beat - i128::from(projection.local_start_raw);
            if delta < 0 {
                None
            } else {
                let start = audio_projection_frame(&projection.tempo, projection.origin_beat_raw)?;
                let at =
                    audio_projection_frame(&projection.tempo, projection.origin_beat_raw + delta)?;
                u64::try_from(at - start).ok()
            }
        } else {
            root_output_frame.checked_sub(domain.fade_start)
        };
        Ok(age.map_or(0.0, |age| domain.fade.gain_at(age)))
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
    pub fn processing_tempo<'a>(&'a self, current: &'a TempoMap) -> &'a TempoMap {
        self.processing_projection.as_ref().map_or_else(
            || self.effective_tempo(current),
            |projection| &projection.tempo,
        )
    }
    pub fn processing_origin(&self) -> Result<i128, Invalid> {
        self.processing_projection.as_ref().map_or_else(
            || self.projection_origin(),
            |projection| Ok(projection.origin_beat_raw),
        )
    }
    pub(crate) fn retain_frame_bounds(
        &mut self,
        start: u64,
        end: u64,
        tempo: &TempoMap,
    ) -> Result<(), Invalid> {
        let Some(beat) = self.live_musical_start() else {
            return Ok(());
        };
        if self.processing_projection.is_none() {
            return Err(Invalid("audio residual needs frozen processing"));
        }
        let length = self
            .placement
            .map_or(self.view_end - self.view_start, |p| p.outer_length.length());
        let beat_end = beat
            .raw()
            .checked_add(length)
            .ok_or(Invalid("audio placement beat end overflow"))?;
        let residual = |actual: u64, nominal: u64| -> Result<i8, Invalid> {
            let difference = i128::from(actual) - i128::from(nominal);
            if !(-1..=1).contains(&difference) {
                return Err(Invalid(
                    "audio structural placement exceeds one-sample rounding residual",
                ));
            }
            i8::try_from(difference).map_err(|_| Invalid("audio placement residual overflow"))
        };
        self.placement_start_offset = residual(start, tempo.beat_to_frame(beat)?)?;
        self.placement_end_offset =
            residual(end, tempo.beat_to_frame(BeatTime::from_raw(beat_end))?)?;
        Ok(())
    }
    pub(crate) fn freeze_processing(&mut self, tempo: Arc<TempoMap>) -> Result<(), Invalid> {
        if self.live_musical_start().is_some() && self.processing_projection.is_none() {
            self.processing_projection = Some(Arc::new(AudioProcessingProjection {
                tempo,
                origin_beat_raw: self.projection_origin()?,
            }));
        }
        Ok(())
    }
    pub fn projected_fade<'a>(
        &'a self,
        tempo: &'a TempoMap,
    ) -> Result<ProjectedAudioFade<'a>, Invalid> {
        let tempo = self.processing_tempo(tempo);
        let (first, length, _, _) = self.projected_domain(tempo)?;
        let origin = if self.reference_start_raw().is_some() {
            self.processing_origin()?
        } else {
            0
        };
        Ok(ProjectedAudioFade {
            edit: self,
            tempo,
            first,
            origin,
            length,
        })
    }
    pub(crate) fn capture(clip: &Clip) -> Self {
        Self {
            processing_projection: None,
            placement_start_offset: 0,
            placement_end_offset: 0,
            domain: Arc::new(AudioClipDomain {
                events: None,
                source_offset: clip.source_offset,
                source_length: clip.source_length,
                source_phase: clip.source_phase,
                looping: clip.looping,
                clock: AudioDomainClock::Frames(clip.duration),
                pitch_cents: 0,
                sample_bpm: None,
                warp: None,
                warp_enabled: false,
                source_gain: Vec::new(),
                fade: clip.fade,
                fade_start: 0,
                fade_projection: None,
            }),
            view_start: 0,
            view_end: clip.duration,
            timing: AudioClipTiming::Frames,
            placement: None,
        }
    }
    pub fn root_frame_at(&self, position: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        let tempo = self.processing_tempo(tempo);
        if position > self.domain.clock.length() {
            return Err(Invalid("audio event position exceeds its root"));
        }
        match self.domain.clock {
            AudioDomainClock::Frames(_) => Ok(position),
            AudioDomainClock::Beats(_) => {
                let origin = self.processing_origin()?;
                u64::try_from(
                    audio_projection_frame(tempo, origin + i128::from(position))?
                        - audio_projection_frame(tempo, origin)?,
                )
                .map_err(|_| Invalid("audio event frame projection overflow"))
            }
        }
    }
    pub fn root_position_at(&self, frame: u64, tempo: &TempoMap) -> Result<u64, Invalid> {
        let tempo = self.processing_tempo(tempo);
        let position = match self.domain.clock {
            AudioDomainClock::Frames(_) => frame,
            AudioDomainClock::Beats(_) => {
                let origin = self.processing_origin()?;
                u64::try_from(
                    audio_projection_beat(
                        tempo,
                        audio_projection_frame(tempo, origin)? + i128::from(frame),
                    )? - origin,
                )
                .map_err(|_| Invalid("audio event clock projection overflow"))?
            }
        };
        Ok(position.min(self.domain.clock.length()))
    }
    pub fn projection_origin(&self) -> Result<i128, Invalid> {
        let start = self
            .reference_start_raw()
            .ok_or(Invalid("audio domain has no musical projection"))?;
        if !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&start) {
            return Err(Invalid(
                "audio reference placement exceeds bounded beat range",
            ));
        }
        start
            .checked_sub(i128::from(self.view_start))
            .and_then(|origin| {
                origin.checked_sub(i128::from(self.placement.map_or(0, |p| p.cycle_phase)))
            })
            .ok_or(Invalid("audio projection origin overflow"))
    }
    pub fn projected_domain(&self, tempo: &TempoMap) -> Result<(i128, u64, u64, u64), Invalid> {
        let tempo = self.processing_tempo(tempo);
        match self.domain.clock {
            AudioDomainClock::Frames(length) => Ok((0, length, self.view_start, self.view_end)),
            AudioDomainClock::Beats(length) => {
                let origin = self.processing_origin()?;
                let first = audio_projection_frame(tempo, origin)?;
                let end = audio_projection_frame(tempo, origin + i128::from(length.raw()))?;
                let view_start =
                    audio_projection_frame(tempo, origin + i128::from(self.view_start))?;
                let view_end = audio_projection_frame(tempo, origin + i128::from(self.view_end))?;
                let positive = |frame: i128| {
                    u64::try_from(frame - first)
                        .map_err(|_| Invalid("audio projection has invalid frame span"))
                };
                Ok((
                    first,
                    positive(end)?,
                    positive(view_start)?,
                    positive(view_end)?,
                ))
            }
        }
    }
    pub fn validate(
        &self,
        clip: &Clip,
        tempo: &TempoMap,
        asset_frames: u64,
    ) -> Result<(), Invalid> {
        let domain = &self.domain;
        if let Some(projection) = &self.processing_projection
            && (self.live_musical_start().is_none()
                || self.placement.is_some()
                || !(-1..=1).contains(&self.placement_start_offset)
                || !(-1..=1).contains(&self.placement_end_offset)
                || projection.tempo.sample_rate() != tempo.sample_rate()
                || !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX))
                    .contains(&projection.origin_beat_raw))
        {
            return Err(Invalid("retained audio processing projection is invalid"));
        }
        if self.processing_projection.is_none()
            && (self.placement_start_offset != 0 || self.placement_end_offset != 0)
        {
            return Err(Invalid(
                "audio placement residual requires retained processing",
            ));
        }
        if let AudioClipTiming::Frozen {
            reference_start_raw,
            tempo: frozen,
        } = &self.timing
        {
            if domain.warp_enabled || frozen.sample_rate() != tempo.sample_rate() {
                return Err(Invalid(
                    "frozen audio needs warp off and the project sample rate",
                ));
            }
            audio_projection_frame(frozen, *reference_start_raw)?;
        }
        if let Some(events) = &domain.events {
            events.validate(domain.clock.length())?;
        }
        let source_end = domain
            .source_offset
            .checked_add(domain.source_length)
            .ok_or(Invalid("audio domain source end overflow"))?;
        if domain.source_length == 0
            || source_end > asset_frames
            || (domain.looping && domain.source_phase >= domain.source_length)
            || (!domain.looping && domain.source_phase != 0)
        {
            return Err(Invalid("audio domain source interval or phase is invalid"));
        }
        if !(-4_800..=4_800).contains(&domain.pitch_cents) {
            return Err(Invalid("audio pitch must be within -4800..4800 cents"));
        }
        if self.view_start >= self.view_end || self.view_end > domain.clock.length() {
            return Err(Invalid("audio crop must be inside its retained domain"));
        }
        if domain.fade_start >= domain.clock.length() {
            return Err(Invalid("audio fade starts outside its domain"));
        }
        if matches!(domain.clock, AudioDomainClock::Beats(_))
            != self.reference_start_raw().is_some()
        {
            return Err(Invalid("audio clock and placement disagree"));
        }
        if domain.source_gain.len() > MAX_AUDIO_POINTS
            || domain
                .source_gain
                .iter()
                .any(|point| point.frame > asset_frames || point.gain_q16 > MAX_SOURCE_GAIN)
            || domain
                .source_gain
                .windows(2)
                .any(|pair| pair[0].frame >= pair[1].frame)
        {
            return Err(Invalid(
                "source gain points must be bounded, sorted and within media",
            ));
        }
        if let Some(warp) = &domain.warp {
            if domain.warp_enabled && self.reference_start_raw().is_none() {
                return Err(Invalid("warp requires musical placement"));
            }
            warp.mode.validate()?;
            if !(2..=MAX_AUDIO_POINTS).contains(&warp.markers.len())
                || warp.markers.first().is_none_or(|marker| {
                    marker.source_frame != domain.source_offset || marker.beat != BeatTime::ZERO
                })
                || warp.markers.last().is_none_or(|marker| {
                    marker.source_frame != source_end || marker.beat == BeatTime::ZERO
                })
                || warp.markers.windows(2).any(|pair| {
                    pair[0].source_frame >= pair[1].source_frame || pair[0].beat >= pair[1].beat
                })
            {
                return Err(Invalid(
                    "warp markers need increasing source/beat positions and explicit region endpoints",
                ));
            }
        }
        if domain.warp_enabled && domain.warp.is_none() {
            return Err(Invalid("active warp needs settings and markers"));
        }
        let (_, length, view_start, view_end) = self.projected_domain(tempo)?;
        if length == 0 || length > MAX_AUDIO_DOMAIN_FRAMES || view_start >= view_end {
            return Err(Invalid(
                "full audio domain exceeds its 16777216 frame limit or has an empty crop",
            ));
        }
        if self.processing_projection.is_some()
            && self.placement.is_none()
            && view_end - view_start != clip.duration
        {
            return Err(Invalid(
                "retained audio processing crop differs from its exact placed sample span",
            ));
        }
        if clip.source_offset != domain.source_offset
            || clip.source_length != domain.source_length
            || clip.source_phase != domain.source_phase
            || clip.looping != domain.looping
        {
            return Err(Invalid(
                "audio source cache differs from its canonical domain",
            ));
        }
        if self.placement.is_some() || matches!(self.timing, AudioClipTiming::Frozen { .. }) {
            AudioViewProjection::new(clip, tempo)?;
        } else if let Some(start) = self.live_musical_start() {
            let end = start
                .raw()
                .checked_add(self.view_end - self.view_start)
                .ok_or(Invalid("musical audio end overflow"))?;
            let start_frame =
                offset_frame(tempo.beat_to_frame(start)?, self.placement_start_offset)?;
            let end_frame = offset_frame(
                tempo.beat_to_frame(BeatTime::from_raw(end))?,
                self.placement_end_offset,
            )?;
            if clip.start != start_frame
                || clip.duration
                    != end_frame
                        .checked_sub(start_frame)
                        .ok_or(Invalid("musical audio frame span invalid"))?
            {
                return Err(Invalid(
                    "musical audio frame cache differs from beat endpoints",
                ));
            }
        } else if clip.duration != view_end - view_start {
            return Err(Invalid("audio crop duration differs from frame placement"));
        }
        if matches!(domain.clock, AudioDomainClock::Beats(_)) != domain.fade_projection.is_some() {
            return Err(Invalid("audio fade projection and domain clock disagree"));
        }
        if let Some(projection) = &domain.fade_projection {
            if projection.tempo.sample_rate() != tempo.sample_rate()
                || projection.local_start_raw >= domain.clock.length()
                || projection.local_start_raw != domain.fade_start
            {
                return Err(Invalid("audio fade projection is invalid"));
            }
            audio_projection_frame(&projection.tempo, projection.origin_beat_raw)?;
        }
        Ok(())
    }
    pub(crate) fn anchor(&mut self, clip_start: u64, tempo: &TempoMap) -> Result<(), Invalid> {
        if self.live_musical_start().is_some() {
            return Ok(());
        }
        if matches!(self.timing, AudioClipTiming::Frozen { .. }) {
            self.timing = AudioClipTiming::Musical(tempo.frame_to_beat(clip_start)?);
            return Ok(());
        }
        if self.domain.events.is_some() {
            return Err(Invalid(
                "timing mode cannot change after audio event edits; undo those edits first",
            ));
        }
        let phase = self.placement.map_or(0, |p| p.cycle_phase);
        let origin_frame = i128::from(clip_start) - i128::from(self.view_start) - i128::from(phase);
        let origin_beat = audio_projection_beat(tempo, origin_frame)?;
        let beat_at = |frame: u64| -> Result<u64, Invalid> {
            u64::try_from(
                audio_projection_beat(tempo, origin_frame + i128::from(frame))? - origin_beat,
            )
            .map_err(|_| Invalid("musical audio domain projection overflow"))
        };
        let length = beat_at(self.domain.clock.length())?;
        let start = beat_at(self.view_start)?;
        let end = beat_at(self.view_end)?;
        let fade_start = beat_at(self.domain.fade_start)?;
        let placement = self
            .placement
            .map(|p| -> Result<_, Invalid> {
                let outer_start = tempo.frame_to_beat(clip_start)?.raw();
                let outer_end = tempo
                    .frame_to_beat(
                        clip_start
                            .checked_add(p.outer_length.length())
                            .ok_or(Invalid("audio outer end overflow"))?,
                    )?
                    .raw();
                Ok(AudioViewPlacement {
                    mode: p.mode,
                    outer_length: AudioDomainClock::Beats(BeatTime::from_raw(
                        outer_end
                            .checked_sub(outer_start)
                            .ok_or(Invalid("audio outer beat span invalid"))?,
                    )),
                    cycle_phase: u64::try_from(
                        i128::from(outer_start) - origin_beat - i128::from(start),
                    )
                    .map_err(|_| Invalid("audio consumed offset is not representable in beats"))?,
                })
            })
            .transpose()?;
        let domain = Arc::make_mut(&mut self.domain);
        domain.clock = AudioDomainClock::Beats(BeatTime::from_raw(length));
        domain.fade_start = fade_start;
        domain.fade_projection = Some(AudioFadeProjection {
            tempo: tempo.clone(),
            origin_beat_raw: origin_beat + i128::from(fade_start),
            local_start_raw: fade_start,
        });
        self.view_start = start;
        self.view_end = end;
        self.timing = AudioClipTiming::Musical(tempo.frame_to_beat(clip_start)?);
        self.placement = placement;
        Ok(())
    }
    pub(crate) fn unanchor(&mut self, tempo: &TempoMap) -> Result<(), Invalid> {
        // Frozen beat processing is already frame-placed; do not flatten its
        // event/fade mapping into a different source program.
        if matches!(self.timing, AudioClipTiming::Frozen { .. }) {
            return Ok(());
        }
        if self.reference_start_raw().is_some() && self.domain.events.is_some() {
            return Err(Invalid(
                "timing mode cannot change after audio event edits; undo those edits first",
            ));
        }
        if self.reference_start_raw().is_some() && !self.domain.fade.is_neutral() {
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
                if let Some(musical_start) = self.live_musical_start() {
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
        )?
        .with_curve_positions(domain.fade.in_position(), domain.fade.out_position())?;
        domain.fade_start = 0;
        domain.fade_projection = None;
        self.view_start = start;
        self.view_end = end;
        self.timing = AudioClipTiming::Frames;
        self.placement = placement;
        Ok(())
    }
    pub(crate) fn crop_at_frame(
        &self,
        clip: &Clip,
        frame: u64,
        tempo: &TempoMap,
    ) -> Result<u64, Invalid> {
        if self.processing_projection.is_some() {
            let (_, _, view_start, _) = self.projected_domain(tempo)?;
            let root_frame =
                u64::try_from(i128::from(view_start) + i128::from(frame) - i128::from(clip.start))
                    .map_err(|_| Invalid("audio crop precedes retained processing root"))?;
            return self.root_position_at(root_frame, tempo);
        }
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
                self.reference_beat_at_destination(clip, frame, tempo)?
                    - self.projection_origin()?,
            )
            .map_err(|_| Invalid("audio crop precedes its root")),
        }
    }
}

mod commands;
pub use commands::default_audio_warp;
mod view;
pub(crate) use commands::{
    apply, move_clip, project_clip, reset_region, set_clip_fade_envelope, set_fade_envelope,
    set_fades, source_policy, trim_view,
};
pub(crate) use view::resize_view;
pub use view::{
    AudioViewMode, AudioViewOccurrence, AudioViewPlacement, AudioViewProjection, AudioViewResize,
    AudioViewResizeOperation, preview_audio_view_resize, preview_view_resize,
};

mod stretch;
pub use stretch::preview_audio_clip_stretch;

mod warp_projection;

/// Apply only an explicitly bounded saved nearest-frame phase residual.
pub(crate) fn offset_frame(frame: u64, offset: i8) -> Result<u64, Invalid> {
    u64::try_from(i128::from(frame) + i128::from(offset))
        .map_err(|_| Invalid("audio placement residual exceeds timeline"))
}
