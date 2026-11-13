//! Saved musical audio metadata. Processing roots are immutable; editing a crop
//! never restarts resampling, grains, analysis or fades at the crop boundary.
use crate::{BeatTime, Clip, ClipFade, Invalid, TempoMap, TrimEdge};
use std::sync::Arc;

mod content;
mod prefix;
pub(crate) use prefix::extend_frame_left;
mod timing;
pub use content::AudioContentView;
pub use timing::AudioClipTiming;

pub const MAX_AUDIO_POINTS: usize = 1_024;
pub const MAX_AUDIO_DOMAIN_FRAMES: u64 = 16_777_216;
/// Virtual frame horizon for an Arrangement event root that can be read as
/// occupied spans. It bounds editor coordinates, not allocation or DSP work.
pub const MAX_SPARSE_AUDIO_TIMELINE_FRAMES: u64 = 1 << 36;

/// Refusal for an edit whose whole root must fit the bounded resident transform
/// workspace (processed reverse, pitch, warp and stretch; #4145). Durations are
/// `MAX_AUDIO_DOMAIN_FRAMES / rate` rounded down for common project rates, so
/// the static message stays exact; other rates name the frame cap instead.
pub(crate) fn transform_domain_limit(sample_rate: u32) -> Invalid {
    Invalid(match sample_rate {
        44_100 => {
            "processed reverse, pitch, warp and stretch support clips up to about 6:20 at 44.1 khz; trim or split the clip first"
        }
        48_000 => {
            "processed reverse, pitch, warp and stretch support clips up to about 5:49 at 48 khz; trim or split the clip first"
        }
        88_200 => {
            "processed reverse, pitch, warp and stretch support clips up to about 3:10 at 88.2 khz; trim or split the clip first"
        }
        96_000 => {
            "processed reverse, pitch, warp and stretch support clips up to about 2:54 at 96 khz; trim or split the clip first"
        }
        176_400 => {
            "processed reverse, pitch, warp and stretch support clips up to about 1:35 at 176.4 khz; trim or split the clip first"
        }
        192_000 => {
            "processed reverse, pitch, warp and stretch support clips up to about 1:27 at 192 khz; trim or split the clip first"
        }
        _ => {
            "processed reverse, pitch, warp and stretch support clips up to 16777216 frames; trim or split the clip first"
        }
    })
}

/// A retained domain whose crop selects no frames; distinct from the size limit.
pub(crate) const EMPTY_DOMAIN_CROP: Invalid = Invalid("audio domain crop selects no frames");
pub const SOURCE_GAIN_UNITY: u32 = 65_536;
pub const MAX_SOURCE_GAIN: u32 = SOURCE_GAIN_UNITY * 4;

/// Captured Arrangement import policy. Beats uses source-local default markers
/// at the current project tempo; it does not claim tempo or transient detection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioImportWarp {
    #[default]
    Off,
    Beats,
}

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
/// Versioned source reconstruction law for this content's pitch/warp render.
/// Each law's output is fixed once shipped; new content uses the newest law.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WarpResampler {
    /// Frozen 64-tap Blackman law that rendered every warp/pitch before v2.
    V1,
    /// Rate-scaled Kaiser law with a stopband at the output Nyquist.
    #[default]
    V2,
}
impl WarpResampler {
    /// Law implied when saved content does not name one. Content that reads
    /// fractional source positions (pitch or warp) was authored under v1 and
    /// keeps it. Other content reads integer positions at unity rate, which both
    /// laws reproduce exactly, so it takes the current default.
    pub fn implied(pitch_cents: i16, warp_enabled: bool) -> Self {
        if pitch_cents != 0 || warp_enabled {
            Self::V1
        } else {
            Self::V2
        }
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
/// Finite musical source read retained independently of event destinations.
/// Both fields use raw quarter-note beat ticks. The source origin translates
/// with the clip, and source zero stays fixed when the destination grows left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MusicalEventSource {
    pub zero_in_destination: u64,
    pub length: BeatTime,
}

/// Immutable authored processing content, independent of an instance's crop,
/// loop phase and fade. Retained event roots never refer back to live aliases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipContent {
    pub events: Option<Arc<crate::AudioEvents>>,
    pub musical_event_source: Option<MusicalEventSource>,
    pub source_offset: u64,
    pub source_length: u64,
    pub clock: AudioDomainClock,
    pub pitch_cents: i16,
    /// Authored original tempo, independent of project tempo and manual edge stretch.
    pub sample_bpm: Option<crate::Tempo>,
    pub warp: Option<WarpSettings>,
    pub warp_enabled: bool,
    pub source_gain: Vec<SourceGainPoint>,
    /// Law used by the root pitch/warp render. Directed content views carry
    /// their own law in [`AudioContentView::read_resampler`].
    pub warp_resampler: WarpResampler,
}
impl AudioClipContent {
    /// True when the stored law differs from [`WarpResampler::implied`], so a
    /// saved document must name it explicitly.
    pub fn has_explicit_warp_resampler(&self) -> bool {
        self.warp_resampler != WarpResampler::implied(self.pitch_cents, self.warp_enabled)
    }
}

/// Effective processing domain with instance-owned loop and fade settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipDomain {
    pub content: Arc<AudioClipContent>,
    pub source_phase: u64,
    pub looping: bool,
    pub fade: ClipFade,
    pub fade_start: u64,
    pub fade_projection: Option<AudioFadeProjection>,
}
impl std::ops::Deref for AudioClipDomain {
    type Target = AudioClipContent;
    fn deref(&self) -> &Self::Target {
        &self.content
    }
}
impl std::ops::DerefMut for AudioClipDomain {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.content)
    }
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
/// With an outer placement, the retained projection also owns its full repeat/
/// directed read clock. Live musical endpoints plus residuals remain validated
/// independently; translating a saved frame clock never re-quantizes its seams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipEdit {
    /// Directed finite outer read law; source processing and editable settings stay live.
    pub content_view: Option<AudioContentView>,
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

    /// Inverse authoring coordinate for a root-output frame boundary. The
    /// exclusive root end is accepted; negative retained context returns None.
    /// This retains the existing fade reference clock so editing one ramp does
    /// not re-quantize or reanchor the opposite authored ramp.
    pub fn domain_position_at_frame(&self, frame: u64) -> Result<Option<u64>, Invalid> {
        if frame > self.length {
            return Err(Invalid("fade boundary is outside the prepared root"));
        }
        let domain = &self.edit.domain;
        let age = if let Some(projection) = &domain.fade_projection {
            let beat =
                audio_projection_beat(self.tempo, self.first + i128::from(frame))? - self.origin;
            let delta = beat - i128::from(projection.local_start_raw);
            let start = audio_projection_frame(&projection.tempo, projection.origin_beat_raw)?;
            audio_projection_frame(&projection.tempo, projection.origin_beat_raw + delta)? - start
        } else {
            i128::from(frame) - i128::from(domain.fade_start)
        };
        Ok(u64::try_from(i128::from(domain.fade.offset()) + age).ok())
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

/// Fractional signed project frames (32 fraction bits) for control-prepared
/// read maps. Negative technical context uses tempo at zero, like the rounded
/// projection; no frame rounding is performed here.
pub fn audio_projection_frame_q32(tempo: &TempoMap, beat_raw: i128) -> Result<i128, Invalid> {
    if beat_raw >= 0 {
        return i128::try_from(
            tempo.beat_to_frame_q32(BeatTime::from_raw(
                u64::try_from(beat_raw)
                    .map_err(|_| Invalid("audio fractional projection exceeds beat range"))?,
            ))?,
        )
        .map_err(|_| Invalid("audio fractional frame projection overflow"));
    }
    if beat_raw < -2 * i128::from(u64::MAX) {
        return Err(Invalid(
            "audio fractional projection precedes bounded context",
        ));
    }
    beat_raw
        .checked_mul(i128::from(tempo.sample_rate()))
        .and_then(|n| n.checked_mul(60_000_000))
        .map(|n| n / i128::from(tempo.tempo_at(BeatTime::ZERO).micro_bpm()))
        .ok_or(Invalid("audio fractional frame projection overflow"))
}

/// Signed nearest-frame conversion; exact half ties move later, including -0.5.
/// Negative projection uses tempo-at-zero for bounded, non-transport context,
/// including the virtual clip-editor grid. It does not enable playback before zero.
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
    /// Reconstruct an exact plain-pitch edit from a plain clip. This does not
    /// establish asset extent or source custody; normal project validation does.
    pub fn reconstruct_plain_pitch(clip: &Clip, cents: i16) -> Result<Self, Invalid> {
        if clip.audio_edit().is_some() || cents == 0 || !(-4_800..=4_800).contains(&cents) {
            return Err(Invalid("invalid plain pitch reconstruction"));
        }
        let mut edit = Self::capture(clip);
        Arc::make_mut(&mut edit.domain).pitch_cents = cents;
        if !edit.is_plain_pitch_capture(clip) {
            return Err(Invalid(
                "plain pitch requires a complete neutral frame clip",
            ));
        }
        Ok(edit)
    }

    /// Exact, full-frame plain pitch captured from a neutral, unlooped clip.
    /// Callers must also establish that the clip covers the complete asset.
    pub fn is_plain_pitch_capture(&self, clip: &Clip) -> bool {
        if self.domain.pitch_cents == 0
            || clip.source_offset != 0
            || clip.source_length != clip.duration
            || clip.looping
            || clip.source_phase != 0
            || !clip.fade.is_neutral()
        {
            return false;
        }
        let mut expected = Self::capture(clip);
        Arc::make_mut(&mut expected.domain).pitch_cents = self.domain.pitch_cents;
        self == &expected
    }

    /// Whether this is exactly the reversal created from an unedited clip.
    /// Unlike the playback shape query, this proves that removing and then
    /// recreating the edit preserves every authored field.
    pub fn is_plain_reversed_capture(&self, clip: &Clip) -> bool {
        let Some((_, length)) = self.plain_reversed_source(clip) else {
            return false;
        };
        let Ok(window) = crate::ContentWindow::new(0, length, true) else {
            return false;
        };
        let mut expected = Self::capture(clip);
        expected.content_view = Some(AudioContentView {
            restore_plain: true,
            base_view_start: 0,
            base_view_end: length,
            base_placement: None,
            base_start_delta: 0,
            window,
            read_resampler: WarpResampler::default(),
        });
        self == &expected
    }

    /// Return the source offset and length of an unprocessed, full-frame
    /// reversal. This is a shape query only: callers still validate the clip
    /// and establish source availability and custody before reading it.
    pub fn plain_reversed_source(&self, clip: &Clip) -> Option<(u64, u64)> {
        let view = self.content_view?;
        let AudioDomainClock::Frames(length) = self.domain.clock else {
            return None;
        };
        (view.restore_plain
            && view.window.reversed
            && view.window.start == 0
            && view.window.end == length
            && view.base_view_start == 0
            && view.base_view_end == length
            && view.base_placement.is_none()
            && view.base_start_delta == 0
            && self.view_start == 0
            && self.view_end == length
            && self.placement.is_none()
            && self.processing_projection.is_none()
            && matches!(self.timing, AudioClipTiming::Frames)
            && self.placement_start_offset == 0
            && self.placement_end_offset == 0
            && clip.duration() == length
            && length <= self.domain.source_length
            && !self.domain.looping
            && self.domain.source_phase == 0
            && self.domain.pitch_cents == 0
            && !self.domain.warp_enabled
            && self.domain.warp.is_none()
            && self.domain.sample_bpm.is_none()
            && self.domain.events.is_none()
            && self.domain.musical_event_source.is_none()
            && self.domain.source_gain.is_empty()
            && self.domain.fade.is_neutral()
            && self.domain.fade_start == 0
            && self.domain.fade_projection.is_none())
        .then_some((self.domain.source_offset, length))
    }

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
    /// Recover the carrier reference from a processing origin without changing
    /// the retained crop, repeat phase or directed read base.
    pub(crate) fn reference_for_processing_origin(&self, origin: i128) -> Result<i128, Invalid> {
        let (delta, start, phase) = self.content_view.map_or_else(
            || {
                (
                    0,
                    self.view_start,
                    self.placement.map_or(0, |p| p.cycle_phase),
                )
            },
            |view| {
                (
                    view.base_start_delta,
                    view.base_view_start,
                    view.base_placement.map_or(0, |p| p.cycle_phase),
                )
            },
        );
        let lead = self.placement.map_or(0, |p| p.mode.leading_ticks());
        origin
            .checked_sub(delta)
            .and_then(|origin| origin.checked_sub(i128::from(lead)))
            .and_then(|origin| origin.checked_add(i128::from(start)))
            .and_then(|origin| origin.checked_add(i128::from(phase)))
            .ok_or(Invalid("audio frozen reference overflow"))
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
        self.placement_end_offset = if self.placement.is_some_and(|p| p.mode.leading_ticks() > 0) {
            0
        } else {
            residual(end, tempo.beat_to_frame(BeatTime::from_raw(beat_end))?)?
        };
        Ok(())
    }
    /// Retain a structurally moved musical span, keeping `candidate` when its
    /// start and end both stay within the one-sample residual of their beats.
    /// Otherwise the span moves (never resizes) to the allowed start nearest the
    /// candidate: frame-shifted fragments carry one rounding step per edit, and
    /// a second step must not refuse the edit. Returns the start.
    pub(crate) fn retain_moved_frame_span(
        &mut self,
        candidate: u64,
        duration: u64,
        tempo: &TempoMap,
    ) -> Result<u64, Invalid> {
        let Some(beat) = self.live_musical_start() else {
            return Ok(candidate);
        };
        let length = self
            .placement
            .map_or(self.view_end - self.view_start, |p| p.outer_length.length());
        let beat_end = beat
            .raw()
            .checked_add(length)
            .ok_or(Invalid("audio placement beat end overflow"))?;
        let start = i128::from(tempo.beat_to_frame(beat)?);
        let end = i128::from(tempo.beat_to_frame(BeatTime::from_raw(beat_end))?);
        let duration_i = i128::from(duration);
        let low = (start - 1).max(end - duration_i - 1).max(0);
        let high = (start + 1).min(end - duration_i + 1);
        if low > high {
            return Err(Invalid(
                "audio structural placement exceeds one-sample rounding residual",
            ));
        }
        let placed = u64::try_from(i128::from(candidate).clamp(low, high))
            .map_err(|_| Invalid("audio placement frame overflow"))?;
        let placed_end = placed
            .checked_add(duration)
            .ok_or(Invalid("audio placement frame overflow"))?;
        self.retain_frame_bounds(placed, placed_end, tempo)?;
        Ok(placed)
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
            content_view: None,
            processing_projection: None,
            placement_start_offset: 0,
            placement_end_offset: 0,
            domain: Arc::new(AudioClipDomain {
                content: Arc::new(AudioClipContent {
                    events: None,
                    musical_event_source: None,
                    source_offset: clip.source_offset,
                    source_length: clip.source_length,
                    clock: AudioDomainClock::Frames(clip.duration),
                    pitch_cents: 0,
                    sample_bpm: None,
                    warp: None,
                    warp_enabled: false,
                    source_gain: Vec::new(),
                    warp_resampler: WarpResampler::default(),
                }),
                source_phase: clip.source_phase,
                looping: clip.looping,
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
    /// Source zero uses this root's declared clock. A grouped source reads its
    /// independently retained finite output in frames, even beyond the containing
    /// root's destination length. This query transfers no source custody.
    pub fn event_source_clock(&self, source: u64) -> Result<AudioDomainClock, Invalid> {
        if source == 0 {
            if self
                .domain
                .events
                .as_ref()
                .is_some_and(|events| !events.sources.is_empty())
            {
                return Err(Invalid("grouped audio requires a retained event source"));
            }
            return Ok(self
                .domain
                .musical_event_source
                .map_or(self.domain.clock, |source| {
                    AudioDomainClock::Beats(source.length)
                }));
        }
        if !matches!(self.domain.clock, AudioDomainClock::Frames(_)) {
            return Err(Invalid("grouped event sources require a frame clock"));
        }
        self.domain
            .events
            .as_ref()
            .and_then(|events| events.source_length(source, self.domain.clock.length()))
            .map(AudioDomainClock::Frames)
            .ok_or(Invalid("audio event source is missing"))
    }
    /// Map an event's source-clock boundary to its processed source frame.
    pub fn event_source_frame_at(
        &self,
        source: u64,
        position: u64,
        tempo: &TempoMap,
    ) -> Result<u64, Invalid> {
        let clock = self.event_source_clock(source)?;
        if position > clock.length() {
            return Err(Invalid("audio event position exceeds its source"));
        }
        if source == 0 {
            if let Some(source) = self.domain.musical_event_source {
                let origin = self.processing_origin()? + i128::from(source.zero_in_destination);
                let tempo = self.processing_tempo(tempo);
                return u64::try_from(
                    audio_projection_frame(tempo, origin + i128::from(position))?
                        - audio_projection_frame(tempo, origin)?,
                )
                .map_err(|_| Invalid("audio event source projection overflow"));
            }
            self.root_frame_at(position, tempo)
        } else {
            Ok(position)
        }
    }
    /// Inverse source query; grouped PCM is already in finite output frames.
    pub fn event_source_position_at(
        &self,
        source: u64,
        frame: u64,
        tempo: &TempoMap,
    ) -> Result<u64, Invalid> {
        if source == 0 {
            self.event_source_clock(source)?;
            if let Some(source) = self.domain.musical_event_source {
                let origin = self.processing_origin()? + i128::from(source.zero_in_destination);
                let tempo = self.processing_tempo(tempo);
                let position = audio_projection_beat(
                    tempo,
                    audio_projection_frame(tempo, origin)? + i128::from(frame),
                )? - origin;
                return u64::try_from(position.clamp(0, i128::from(source.length.raw())))
                    .map_err(|_| Invalid("audio event source projection overflow"));
            }
            self.root_position_at(frame, tempo)
        } else {
            self.event_source_frame_at(source, frame, tempo)
        }
    }
    /// Project a signed authoring coordinate, including virtual space outside
    /// the retained destination. Only canonical edits can grow that extent.
    pub fn signed_root_frame_at(&self, position: i128, tempo: &TempoMap) -> Result<i128, Invalid> {
        match self.domain.clock {
            AudioDomainClock::Frames(_) => Ok(position),
            AudioDomainClock::Beats(_) => {
                let origin = self.processing_origin()?;
                let beat = origin
                    .checked_add(position)
                    .ok_or(Invalid("audio event beat overflow"))?;
                let tempo = self.processing_tempo(tempo);
                Ok(audio_projection_frame(tempo, beat)? - audio_projection_frame(tempo, origin)?)
            }
        }
    }

    /// Invert the signed authoring projection in the retained processing clock.
    pub fn signed_root_position_at(&self, frame: i128, tempo: &TempoMap) -> Result<i128, Invalid> {
        match self.domain.clock {
            AudioDomainClock::Frames(_) => Ok(frame),
            AudioDomainClock::Beats(_) => {
                let origin = self.processing_origin()?;
                let tempo = self.processing_tempo(tempo);
                let frame = audio_projection_frame(tempo, origin)?
                    .checked_add(frame)
                    .ok_or(Invalid("audio event frame overflow"))?;
                Ok(audio_projection_beat(tempo, frame)? - origin)
            }
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
        let (start, view_start, phase) = if let Some(view) = self.content_view {
            (
                start
                    .checked_add(view.base_start_delta)
                    .ok_or(Invalid("audio base reference overflow"))?,
                view.base_view_start,
                view.base_placement.map_or(0, |p| p.cycle_phase),
            )
        } else {
            (
                start,
                self.view_start,
                self.placement.map_or(0, |p| p.cycle_phase),
            )
        };
        let lead = self.placement.map_or(0, |p| p.mode.leading_ticks());
        start
            .checked_add(i128::from(lead))
            .and_then(|origin| origin.checked_sub(i128::from(view_start)))
            .and_then(|origin| origin.checked_sub(i128::from(phase)))
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
    /// The packed Arrangement reader requires a forward, one-shot frame root.
    pub(crate) fn plain_frame_event_root(&self) -> bool {
        self.frame_event_root_shape()
            && self
                .domain
                .events
                .as_ref()
                .is_some_and(|events| !events.sources.is_empty())
    }

    pub(crate) fn frame_event_root_shape(&self) -> bool {
        matches!(self.domain.clock, AudioDomainClock::Frames(_))
            && matches!(self.timing, AudioClipTiming::Frames)
            && self.placement.is_none()
            && self.content_view.is_none()
            && self.processing_projection.is_none()
            && !self.domain.warp_enabled
            && !self.domain.looping
            && self.domain.pitch_cents == 0
            && self.domain.source_gain.is_empty()
    }

    pub(crate) fn accepts_projected_extent(&self, length: u64, clip: &Clip) -> bool {
        if length <= MAX_AUDIO_DOMAIN_FRAMES {
            return true;
        }
        length <= MAX_SPARSE_AUDIO_TIMELINE_FRAMES
            && (self.is_plain_reversed_capture(clip)
                || self.is_plain_pitch_capture(clip)
                || (self.plain_frame_event_root()
                    && self.domain.events.as_ref().is_some_and(|events| {
                        events
                            .events
                            .iter()
                            .try_fold(0u64, |n, event| {
                                n.checked_add(event.end.checked_sub(event.start)?)
                                    .filter(|n| *n <= MAX_AUDIO_DOMAIN_FRAMES)
                            })
                            .is_some()
                    })))
    }

    pub fn validate(
        &self,
        clip: &Clip,
        tempo: &TempoMap,
        asset_frames: u64,
    ) -> Result<(), Invalid> {
        if let Some(view) = self.content_view {
            view.validate(self)?;
        }
        let domain = &self.domain;
        if let Some(projection) = &self.processing_projection
            && (self.live_musical_start().is_none()
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
        if domain
            .events
            .as_ref()
            .is_some_and(|events| !events.sources.is_empty())
            && (!matches!(domain.clock, AudioDomainClock::Frames(_))
                || !matches!(self.timing, AudioClipTiming::Frames)
                || domain.pitch_cents != 0
                || domain.warp_enabled
                || !domain.source_gain.is_empty()
                || domain.looping
                || domain.source_phase != 0)
        {
            return Err(Invalid(
                "grouped audio retains independent processing roots; musical anchoring, pitch and source processing are unsupported",
            ));
        }
        if let Some(events) = &domain.events {
            events.validate(domain.clock.length())?;
        }
        if let Some(source) = domain.musical_event_source {
            let length = source.length.raw();
            if !matches!(domain.clock, AudioDomainClock::Beats(_))
                || self.placement.is_some()
                || self.content_view.is_some()
                || length == 0
                || source
                    .zero_in_destination
                    .checked_add(length)
                    .is_none_or(|end| end > domain.clock.length())
            {
                return Err(Invalid("invalid retained musical event source"));
            }
            let events = domain
                .events
                .as_ref()
                .filter(|events| events.sources.is_empty())
                .ok_or(Invalid("musical event source needs its event sequence"))?;
            for event in &events.events {
                if event
                    .source_start
                    .checked_add(event.length())
                    .is_none_or(|end| end > length)
                {
                    return Err(Invalid("audio event exceeds its retained source clock"));
                }
                if let Some(fade) = event.fade {
                    fade.validate(length, event.source_start, event.length())?;
                }
            }
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
        if !self.accepts_projected_extent(length, clip) {
            return Err(transform_domain_limit(tempo.sample_rate()));
        }
        if length > MAX_AUDIO_DOMAIN_FRAMES
            && self.is_plain_pitch_capture(clip)
            && clip.source_length != asset_frames
        {
            return Err(Invalid("long pitch requires the complete audio source"));
        }
        if length == 0 || view_start >= view_end {
            return Err(EMPTY_DOMAIN_CROP);
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
        let leading = self
            .placement
            .is_some_and(|placement| placement.mode.leading_ticks() != 0);
        if leading && (self.content_view.is_some() || self.placement_end_offset != 0) {
            return Err(Invalid(
                "leading musical one-shot cannot use directed reads or a terminal residual",
            ));
        }
        if self.processing_projection.is_some() && self.placement.is_some() {
            // For a leading one-shot only the gate follows the live clock.
            // Its terminal frame still comes from the translated retained map
            // and is validated by AudioViewProjection below.
            let start = self
                .live_musical_start()
                .ok_or(Invalid("retained outer audio needs musical placement"))?;
            let length = self
                .placement
                .ok_or(Invalid("audio placement missing"))?
                .outer_length
                .length();
            let end = start
                .raw()
                .checked_add(length)
                .ok_or(Invalid("audio outer end overflow"))?;
            if offset_frame(tempo.beat_to_frame(start)?, self.placement_start_offset)? != clip.start
                || (!leading
                    && offset_frame(
                        tempo.beat_to_frame(BeatTime::from_raw(end))?,
                        self.placement_end_offset,
                    )? != clip
                        .start
                        .checked_add(clip.duration)
                        .ok_or(Invalid("audio outer end overflow"))?)
            {
                return Err(Invalid(
                    "retained outer frame cache differs from musical endpoints",
                ));
            }
        }
        if self.content_view.is_some()
            || self.placement.is_some()
            || matches!(self.timing, AudioClipTiming::Frozen { .. })
        {
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
        self.anchor_at_reference(clip_start, i128::from(clip_start), tempo)
    }
    pub(crate) fn anchor_at_reference(
        &mut self,
        clip_start: u64,
        reference_start: i128,
        tempo: &TempoMap,
    ) -> Result<(), Invalid> {
        if self.live_musical_start().is_some() {
            return Ok(());
        }
        if matches!(self.timing, AudioClipTiming::Frozen { .. }) {
            self.timing = AudioClipTiming::Musical(tempo.frame_to_beat(clip_start)?);
            return Ok(());
        }
        let phase = self.placement.map_or(0, |p| p.cycle_phase);
        let origin_frame = if let Some(view) = self.content_view {
            reference_start
                .checked_add(view.base_start_delta)
                .and_then(|n| n.checked_sub(i128::from(view.base_view_start)))
                .and_then(|n| {
                    n.checked_sub(i128::from(view.base_placement.map_or(0, |p| p.cycle_phase)))
                })
                .ok_or(Invalid("audio content anchor origin overflow"))?
        } else {
            reference_start - i128::from(self.view_start) - i128::from(phase)
        };
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
        let events = self
            .domain
            .events
            .as_ref()
            .map(|events| events.reproject_to_beats(length, beat_at).map(Arc::new))
            .transpose()?;
        let placement = self
            .placement
            .map(|p| -> Result<_, Invalid> {
                let outer_start = audio_projection_beat(tempo, reference_start)?;
                let outer_end = audio_projection_beat(
                    tempo,
                    reference_start
                        .checked_add(i128::from(p.outer_length.length()))
                        .ok_or(Invalid("audio outer end overflow"))?,
                )?;
                Ok(AudioViewPlacement {
                    mode: p.mode,
                    outer_length: AudioDomainClock::Beats(BeatTime::from_raw(
                        u64::try_from(
                            outer_end
                                .checked_sub(outer_start)
                                .ok_or(Invalid("audio outer beat span invalid"))?,
                        )
                        .map_err(|_| Invalid("audio outer beat span overflow"))?,
                    )),
                    cycle_phase: u64::try_from(outer_start - origin_beat - i128::from(start))
                        .map_err(|_| {
                            Invalid("audio consumed offset is not representable in beats")
                        })?,
                })
            })
            .transpose()?;
        if let Some(mut view) = self.content_view {
            let base = reference_start
                .checked_add(view.base_start_delta)
                .ok_or(Invalid("audio content anchor overflow"))?;
            let base_beat = audio_projection_beat(tempo, base)?;
            let at = |elapsed: u64| -> Result<u64, Invalid> {
                u64::try_from(audio_projection_beat(tempo, base + i128::from(elapsed))? - base_beat)
                    .map_err(|_| Invalid("audio content beat conversion overflow"))
            };
            let new_start = beat_at(view.base_view_start)?;
            let new_end = beat_at(view.base_view_end)?;
            view.base_placement = view
                .base_placement
                .map(|p| -> Result<_, Invalid> {
                    Ok(AudioViewPlacement {
                        mode: p.mode,
                        outer_length: AudioDomainClock::Beats(BeatTime::from_raw(at(p
                            .outer_length
                            .length())?)),
                        cycle_phase: u64::try_from(base_beat - origin_beat - i128::from(new_start))
                            .map_err(|_| Invalid("audio content phase conversion overflow"))?,
                    })
                })
                .transpose()?;
            view.base_view_start = new_start;
            view.base_view_end = new_end;
            view.base_start_delta = base_beat - audio_projection_beat(tempo, reference_start)?;
            view.window = crate::ContentWindow::new(
                at(view.window.start)?,
                at(view.window.end)?,
                view.window.reversed,
            )?;
            self.content_view = Some(view);
        }
        let domain = Arc::make_mut(&mut self.domain);
        domain.clock = AudioDomainClock::Beats(BeatTime::from_raw(length));
        domain.events = events;
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
        self.sync_content_length()?;
        Ok(())
    }
    pub(crate) fn unanchor(&mut self, tempo: &TempoMap) -> Result<(), Invalid> {
        // Frozen beat processing is already frame-placed; do not flatten its
        // event/fade mapping into a different source program.
        if matches!(self.timing, AudioClipTiming::Frozen { .. }) {
            return Ok(());
        }
        if self.reference_start_raw().is_some()
            && (self.domain.events.is_some()
                || self.placement.is_some_and(|p| p.mode.leading_ticks() > 0))
        {
            if self.domain.warp_enabled {
                return Err(Invalid("disable warp before frame anchoring"));
            }
            // Frame placement need not flatten a beat-domain event program
            // or a finite read with its own silent gate.
            // Freeze its complete processing clock just as global tempo edits
            // do for unwarped clips, preserving moved sources and retained fades.
            let reference_start_raw =
                self.reference_for_processing_origin(self.processing_origin()?)?;
            let frozen = self
                .processing_projection
                .as_ref()
                .map_or_else(|| Arc::new(tempo.clone()), |p| Arc::clone(&p.tempo));
            self.timing = AudioClipTiming::Frozen {
                reference_start_raw,
                tempo: frozen,
            };
            self.processing_projection = None;
            self.placement_start_offset = 0;
            self.placement_end_offset = 0;
            return Ok(());
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
        if let Some(mut view) = self.content_view {
            let current = self
                .reference_start_raw()
                .ok_or(Invalid("audio musical reference missing"))?;
            let base = current
                .checked_add(view.base_start_delta)
                .ok_or(Invalid("audio content frame conversion overflow"))?;
            let base_frame = audio_projection_frame(tempo, base)?;
            let at = |elapsed: u64| -> Result<u64, Invalid> {
                u64::try_from(
                    audio_projection_frame(tempo, base + i128::from(elapsed))? - base_frame,
                )
                .map_err(|_| Invalid("audio content frame conversion overflow"))
            };
            let new_start = self.root_frame_at(view.base_view_start, tempo)?;
            let new_end = self.root_frame_at(view.base_view_end, tempo)?;
            view.base_placement = view
                .base_placement
                .map(|p| -> Result<_, Invalid> {
                    Ok(AudioViewPlacement {
                        mode: p.mode,
                        outer_length: AudioDomainClock::Frames(at(p.outer_length.length())?),
                        cycle_phase: u64::try_from(
                            base_frame - origin_frame - i128::from(new_start),
                        )
                        .map_err(|_| Invalid("audio content phase frame conversion overflow"))?,
                    })
                })
                .transpose()?;
            view.base_view_start = new_start;
            view.base_view_end = new_end;
            view.base_start_delta = base_frame - audio_projection_frame(tempo, current)?;
            view.window = crate::ContentWindow::new(
                at(view.window.start)?,
                at(view.window.end)?,
                view.window.reversed,
            )?;
            self.content_view = Some(view);
        }
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
        .with_curve_positions(domain.fade.in_position(), domain.fade.out_position())?
        .with_curve_laws(domain.fade.curve_laws());
        domain.fade_start = 0;
        domain.fade_projection = None;
        self.view_start = start;
        self.view_end = end;
        self.timing = AudioClipTiming::Frames;
        self.placement = placement;
        self.sync_content_length()?;
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
    set_fades, source_policy, trim_view, validate_clip,
};
pub(crate) use view::resize_view;
pub use view::{
    AudioViewMode, AudioViewOccurrence, AudioViewPlacement, AudioViewProjection, AudioViewResize,
    AudioViewResizeOperation, ProjectedAudioCycles, preview_audio_view_resize, preview_view_resize,
};

mod stretch;
pub use stretch::preview_audio_clip_stretch;

mod warp_projection;

/// Apply only an explicitly bounded saved nearest-frame phase residual.
pub(crate) fn offset_frame(frame: u64, offset: i8) -> Result<u64, Invalid> {
    u64::try_from(i128::from(frame) + i128::from(offset))
        .map_err(|_| Invalid("audio placement residual exceeds timeline"))
}

#[cfg(test)]
mod content_tests;

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn transform_limit_names_the_exact_duration_for_common_rates() {
        for (rate, label) in [
            (44_100, "44.1 khz"),
            (48_000, "48 khz"),
            (88_200, "88.2 khz"),
            (96_000, "96 khz"),
            (176_400, "176.4 khz"),
            (192_000, "192 khz"),
        ] {
            let seconds = MAX_AUDIO_DOMAIN_FRAMES / u64::from(rate);
            let duration = format!("{}:{:02} at {label};", seconds / 60, seconds % 60);
            let message = transform_domain_limit(rate).0;
            assert!(message.contains(&duration), "{rate}: {message}");
            assert!(message.ends_with("trim or split the clip first"));
            assert_eq!(message, message.to_lowercase());
        }
        let other = transform_domain_limit(22_050).0;
        assert!(other.contains("16777216 frames") && !other.contains("khz"));
        assert_ne!(EMPTY_DOMAIN_CROP.0, transform_domain_limit(48_000).0);
    }
}

#[path = "audio_edit/aliases.rs"]
mod aliases;
pub(crate) use aliases::rebase_alias_content;

#[cfg(test)]
mod long_pitch_tests;
