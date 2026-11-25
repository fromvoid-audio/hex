//! Unified schema 126 DTOs for audio clips, edits, events, MIDI, session, and aliases.
use super::{bounded, invalid, required_option};
use crate::StorageError;
use crate::audio_edit::{warp_resampler_from_tag, warp_resampler_tag};
use hex_project::{
    AliasContentSnapshot, AliasMemberSnapshot, AliasSourceSnapshot, AudioClipContent,
    AudioClipDomain, AudioClipEdit, AudioClipFadeClock, AudioClipTiming, AudioContentView,
    AudioDomainClock, AudioEvent, AudioEventFade, AudioEventWarpSource, AudioEvents,
    AudioFadeProjection, AudioProcessingProjection, AudioViewMode, AudioViewPlacement, BeatTime,
    BeatsPreserve, BeatsTail, Clip, ClipAliasesSnapshot, ClipFade, ClipFlavor, ClipGain,
    ClipSnapshot, ContentWindow, FadeCurveLaw, MAX_AUDIO_EVENTS, MAX_AUDIO_POINTS, MAX_CLIPS,
    MAX_MIDI_VOLUME_POINTS_PER_CLIP, MAX_NOTES, MAX_RETAINED_AUDIO_DEPTH, MAX_SCENES,
    MAX_SESSION_CELLS, MAX_TEMPO_POINTS, MidiClip, MidiClipGain, MidiClipSnapshot, MidiClipVolume,
    MidiClipVolumePoint, MidiNote, MidiNoteSnapshot, MidiOneShotBoundary, MidiSource,
    MidiSourceSnapshot, MusicalEventSource, Project, RetainedAudioRoot, SceneSnapshot,
    SessionAudioClipSnapshot, SessionAudioLoop, SessionCellContent, SessionCellContentSnapshot,
    SessionCellSnapshot, SessionMidiClipSnapshot, SessionSnapshot, SourceGainPoint, Tempo,
    TempoMap, TempoPoint, WarpMarker, WarpMode, WarpSettings,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Shared clip enums & complete ClipFade
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ClipFlavorDto {
    Muted,
    Accent,
}

impl From<ClipFlavor> for ClipFlavorDto {
    fn from(f: ClipFlavor) -> Self {
        match f {
            ClipFlavor::Muted => Self::Muted,
            ClipFlavor::Accent => Self::Accent,
        }
    }
}

impl From<ClipFlavorDto> for ClipFlavor {
    fn from(f: ClipFlavorDto) -> Self {
        match f {
            ClipFlavorDto::Muted => Self::Muted,
            ClipFlavorDto::Accent => Self::Accent,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FadeDto {
    span: u64,
    offset: u64,
    fade_in: u64,
    fade_out: u64,
    in_start: u64,
    out_end: u64,
    bends: [i16; 2],
    positions: [u16; 2],
    smooth: [bool; 2],
}

impl From<ClipFade> for FadeDto {
    fn from(f: ClipFade) -> Self {
        Self {
            span: f.span(),
            offset: f.offset(),
            fade_in: f.fade_in(),
            fade_out: f.fade_out(),
            in_start: f.in_start(),
            out_end: f.out_end_offset(),
            bends: [f.in_curve(), f.out_curve()],
            positions: [f.in_position(), f.out_position()],
            smooth: f.curve_laws().map(|law| law == FadeCurveLaw::SmoothStep),
        }
    }
}

impl TryFrom<FadeDto> for ClipFade {
    type Error = StorageError;
    fn try_from(f: FadeDto) -> Result<Self, StorageError> {
        let base = ClipFade::new(f.span, f.offset, f.fade_in, f.fade_out).map_err(invalid)?;
        let shaped = base
            .with_shape(f.in_start, f.out_end, f.bends[0], f.bends[1])
            .and_then(|fade| fade.with_curve_positions(f.positions[0], f.positions[1]))
            .map_err(invalid)?;
        Ok(shaped.with_curve_laws(f.smooth.map(|smooth| {
            if smooth {
                FadeCurveLaw::SmoothStep
            } else {
                FadeCurveLaw::Linear
            }
        })))
    }
}

// ---------------------------------------------------------------------------
// Tempo / Projection helpers
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MiniTempoPoint {
    beat_raw: u64,
    micro_bpm: u32,
}

fn read_tempo_points<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MiniTempoPoint>, D::Error> {
    bounded(d, MAX_TEMPO_POINTS)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TempoMapDto {
    sample_rate: u32,
    #[serde(deserialize_with = "read_tempo_points")]
    points: Vec<MiniTempoPoint>,
}

impl TempoMapDto {
    fn from_model(tempo: &TempoMap) -> Self {
        Self {
            sample_rate: tempo.sample_rate(),
            points: tempo
                .points()
                .iter()
                .map(|p| MiniTempoPoint {
                    beat_raw: p.beat().raw(),
                    micro_bpm: p.tempo().micro_bpm(),
                })
                .collect(),
        }
    }

    fn into_model(self) -> Result<TempoMap, StorageError> {
        let points = self
            .points
            .into_iter()
            .map(|p| {
                Tempo::new(p.micro_bpm)
                    .map(|tempo| TempoPoint::new(BeatTime::from_raw(p.beat_raw), tempo))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        TempoMap::new(self.sample_rate, points).map_err(invalid)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FadeProjectionDto {
    tempo: TempoMapDto,
    origin_beat_raw: String,
    local_start_raw: u64,
}

impl From<&AudioFadeProjection> for FadeProjectionDto {
    fn from(p: &AudioFadeProjection) -> Self {
        Self {
            tempo: TempoMapDto::from_model(&p.tempo),
            origin_beat_raw: p.origin_beat_raw.to_string(),
            local_start_raw: p.local_start_raw,
        }
    }
}

impl TryFrom<FadeProjectionDto> for AudioFadeProjection {
    type Error = StorageError;
    fn try_from(p: FadeProjectionDto) -> Result<Self, StorageError> {
        let origin = p
            .origin_beat_raw
            .parse::<i128>()
            .map_err(|_| StorageError::Invalid("invalid audio fade projection origin"))?;
        if !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&origin) {
            return Err(StorageError::Invalid(
                "audio fade projection origin exceeds bounded range",
            ));
        }
        Ok(Self {
            tempo: p.tempo.into_model()?,
            origin_beat_raw: origin,
            local_start_raw: p.local_start_raw,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessingProjectionDto {
    tempo: TempoMapDto,
    origin_beat_raw: String,
}

impl From<&AudioProcessingProjection> for ProcessingProjectionDto {
    fn from(p: &AudioProcessingProjection) -> Self {
        Self {
            tempo: TempoMapDto::from_model(&p.tempo),
            origin_beat_raw: p.origin_beat_raw.to_string(),
        }
    }
}

impl TryFrom<ProcessingProjectionDto> for AudioProcessingProjection {
    type Error = StorageError;
    fn try_from(p: ProcessingProjectionDto) -> Result<Self, StorageError> {
        let origin = p
            .origin_beat_raw
            .parse::<i128>()
            .map_err(|_| StorageError::Invalid("invalid audio processing projection origin"))?;
        if !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&origin) {
            return Err(StorageError::Invalid(
                "audio processing projection origin exceeds bounded range",
            ));
        }
        Ok(Self {
            tempo: Arc::new(p.tempo.into_model()?),
            origin_beat_raw: origin,
        })
    }
}

// ---------------------------------------------------------------------------
// Audio Clip, Edit, Domain, Content, Events, Retained Roots
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ClockDto {
    Frames { length: u64 },
    Beats { length_raw: u64 },
}

impl From<AudioDomainClock> for ClockDto {
    fn from(c: AudioDomainClock) -> Self {
        match c {
            AudioDomainClock::Frames(length) => Self::Frames { length },
            AudioDomainClock::Beats(length) => Self::Beats {
                length_raw: length.raw(),
            },
        }
    }
}

impl From<ClockDto> for AudioDomainClock {
    fn from(c: ClockDto) -> Self {
        match c {
            ClockDto::Frames { length } => Self::Frames(length),
            ClockDto::Beats { length_raw } => Self::Beats(BeatTime::from_raw(length_raw)),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FadeClockDto {
    LegacyRoot,
    Outer,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TimingDto {
    Frames,
    Musical {
        start_raw: u64,
    },
    Frozen {
        reference_start_raw: String,
        tempo: TempoMapDto,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ViewModeDto {
    Once,
    Repeat,
    OnceWithLead { ticks: u64 },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementDto {
    mode: ViewModeDto,
    outer_length: ClockDto,
    cycle_phase: u64,
}

impl From<AudioViewPlacement> for PlacementDto {
    fn from(p: AudioViewPlacement) -> Self {
        Self {
            mode: match p.mode {
                AudioViewMode::Once => ViewModeDto::Once,
                AudioViewMode::Repeat => ViewModeDto::Repeat,
                AudioViewMode::OnceWithLead(ticks) => ViewModeDto::OnceWithLead { ticks },
            },
            outer_length: p.outer_length.into(),
            cycle_phase: p.cycle_phase,
        }
    }
}

impl From<PlacementDto> for AudioViewPlacement {
    fn from(p: PlacementDto) -> Self {
        Self {
            mode: match p.mode {
                ViewModeDto::Once => AudioViewMode::Once,
                ViewModeDto::Repeat => AudioViewMode::Repeat,
                ViewModeDto::OnceWithLead { ticks } => AudioViewMode::OnceWithLead(ticks),
            },
            outer_length: p.outer_length.into(),
            cycle_phase: p.cycle_phase,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContentWindowDto {
    start: u64,
    end: u64,
    reversed: bool,
}

impl From<ContentWindow> for ContentWindowDto {
    fn from(w: ContentWindow) -> Self {
        Self {
            start: w.start,
            end: w.end,
            reversed: w.reversed,
        }
    }
}

impl TryFrom<ContentWindowDto> for ContentWindow {
    type Error = StorageError;
    fn try_from(w: ContentWindowDto) -> Result<Self, StorageError> {
        ContentWindow::new(w.start, w.end, w.reversed).map_err(invalid)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioContentViewDto {
    restore_plain: bool,
    base_view_start: u64,
    base_view_end: u64,
    #[serde(deserialize_with = "required_option")]
    base_placement: Option<PlacementDto>,
    base_start_delta: String,
    window: ContentWindowDto,
    read_resampler: u8,
}

impl From<&AudioContentView> for AudioContentViewDto {
    fn from(v: &AudioContentView) -> Self {
        Self {
            restore_plain: v.restore_plain,
            base_view_start: v.base_view_start,
            base_view_end: v.base_view_end,
            base_placement: v.base_placement.map(PlacementDto::from),
            base_start_delta: v.base_start_delta.to_string(),
            window: v.window.into(),
            read_resampler: warp_resampler_tag(v.read_resampler),
        }
    }
}

impl TryFrom<AudioContentViewDto> for AudioContentView {
    type Error = StorageError;
    fn try_from(v: AudioContentViewDto) -> Result<Self, StorageError> {
        let base_start_delta = v
            .base_start_delta
            .parse::<i128>()
            .map_err(|_| StorageError::Invalid("invalid directed audio view start delta"))?;
        Ok(Self {
            restore_plain: v.restore_plain,
            base_view_start: v.base_view_start,
            base_view_end: v.base_view_end,
            base_placement: v.base_placement.map(AudioViewPlacement::from),
            base_start_delta,
            window: v.window.try_into()?,
            read_resampler: warp_resampler_from_tag(v.read_resampler)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceGainPointDto {
    frame: u64,
    gain_q16: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WarpMarkerDto {
    source_frame: u64,
    beat_raw: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BeatsTailDto {
    Off,
    Forward,
    Alternating,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WarpModeDto {
    RePitch,
    Beats {
        #[serde(deserialize_with = "required_option")]
        division_raw: Option<u64>,
        tail: BeatsTailDto,
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WarpSettingsDto {
    mode: WarpModeDto,
    #[serde(deserialize_with = "read_warp_markers")]
    markers: Vec<WarpMarkerDto>,
}

fn read_warp_markers<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<WarpMarkerDto>, D::Error> {
    bounded(d, MAX_AUDIO_POINTS)
}

fn read_source_gain<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SourceGainPointDto>, D::Error> {
    bounded(d, MAX_AUDIO_POINTS)
}

impl From<&WarpSettings> for WarpSettingsDto {
    fn from(w: &WarpSettings) -> Self {
        Self {
            mode: match w.mode {
                WarpMode::RePitch => WarpModeDto::RePitch,
                WarpMode::Beats {
                    preserve,
                    tail,
                    decay,
                } => WarpModeDto::Beats {
                    division_raw: match preserve {
                        BeatsPreserve::Transients => None,
                        BeatsPreserve::Division(beat) => Some(beat.raw()),
                    },
                    tail: match tail {
                        BeatsTail::Off => BeatsTailDto::Off,
                        BeatsTail::Forward => BeatsTailDto::Forward,
                        BeatsTail::Alternating => BeatsTailDto::Alternating,
                    },
                    decay,
                },
                WarpMode::Tones { grain_ms } => WarpModeDto::Tones { grain_ms },
                WarpMode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                } => WarpModeDto::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                },
                WarpMode::Complex => WarpModeDto::Complex,
                WarpMode::ComplexPro { formants, envelope } => {
                    WarpModeDto::ComplexPro { formants, envelope }
                }
            },
            markers: w
                .markers
                .iter()
                .map(|m| WarpMarkerDto {
                    source_frame: m.source_frame,
                    beat_raw: m.beat.raw(),
                })
                .collect(),
        }
    }
}

impl From<WarpSettingsDto> for WarpSettings {
    fn from(w: WarpSettingsDto) -> Self {
        Self {
            mode: match w.mode {
                WarpModeDto::RePitch => WarpMode::RePitch,
                WarpModeDto::Beats {
                    division_raw,
                    tail,
                    decay,
                } => WarpMode::Beats {
                    preserve: division_raw.map_or(BeatsPreserve::Transients, |raw| {
                        BeatsPreserve::Division(BeatTime::from_raw(raw))
                    }),
                    tail: match tail {
                        BeatsTailDto::Off => BeatsTail::Off,
                        BeatsTailDto::Forward => BeatsTail::Forward,
                        BeatsTailDto::Alternating => BeatsTail::Alternating,
                    },
                    decay,
                },
                WarpModeDto::Tones { grain_ms } => WarpMode::Tones { grain_ms },
                WarpModeDto::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                } => WarpMode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                },
                WarpModeDto::Complex => WarpMode::Complex,
                WarpModeDto::ComplexPro { formants, envelope } => {
                    WarpMode::ComplexPro { formants, envelope }
                }
            },
            markers: w
                .markers
                .into_iter()
                .map(|m| WarpMarker {
                    source_frame: m.source_frame,
                    beat: BeatTime::from_raw(m.beat_raw),
                })
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MusicalEventSourceDto {
    zero_in_destination: u64,
    length_raw: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioEventFadeDto {
    source_start: u64,
    shape: FadeDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioEventDto {
    active: bool,
    gain_millidb: i32,
    source: u64,
    id: u64,
    start: u64,
    end: u64,
    source_start: u64,
    #[serde(deserialize_with = "required_option")]
    fade: Option<AudioEventFadeDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventWarpSourceDto {
    id: u64,
    #[serde(deserialize_with = "read_warp_markers")]
    markers: Vec<WarpMarkerDto>,
}

#[derive(Serialize)]
struct RetainedRootSer<'a> {
    id: u64,
    clip: AudioClipDtoSer<'a>,
    tempo: TempoMapDto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedRootDe {
    id: u64,
    clip: AudioClipDtoDe,
    tempo: TempoMapDto,
}

#[derive(Serialize)]
struct AudioEventsSer<'a> {
    sources: Vec<RetainedRootSer<'a>>,
    warp_sources: Vec<EventWarpSourceDto>,
    next_id: u64,
    events: Vec<AudioEventDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioEventsDe {
    #[serde(deserialize_with = "read_retained_roots")]
    sources: Vec<RetainedRootDe>,
    #[serde(deserialize_with = "read_event_warp_sources")]
    warp_sources: Vec<EventWarpSourceDto>,
    next_id: u64,
    #[serde(deserialize_with = "read_audio_events")]
    events: Vec<AudioEventDto>,
}

fn read_retained_roots<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<RetainedRootDe>, D::Error> {
    bounded(d, MAX_AUDIO_EVENTS)
}

fn read_event_warp_sources<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<EventWarpSourceDto>, D::Error> {
    bounded(d, MAX_AUDIO_EVENTS)
}

fn read_audio_events<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AudioEventDto>, D::Error> {
    bounded(d, MAX_AUDIO_EVENTS)
}

#[derive(Serialize)]
pub(super) struct AudioContentSer<'a> {
    events: Option<AudioEventsSer<'a>>,
    musical_event_source: Option<MusicalEventSourceDto>,
    source_offset: u64,
    source_length: u64,
    clock: ClockDto,
    pitch_cents: i16,
    sample_micro_bpm: Option<u32>,
    warp: Option<WarpSettingsDto>,
    warp_enabled: bool,
    source_gain: Vec<SourceGainPointDto>,
    warp_resampler: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioContentDe {
    #[serde(deserialize_with = "required_option")]
    events: Option<AudioEventsDe>,
    #[serde(deserialize_with = "required_option")]
    musical_event_source: Option<MusicalEventSourceDto>,
    source_offset: u64,
    source_length: u64,
    clock: ClockDto,
    pitch_cents: i16,
    #[serde(deserialize_with = "required_option")]
    sample_micro_bpm: Option<u32>,
    #[serde(deserialize_with = "required_option")]
    warp: Option<WarpSettingsDto>,
    warp_enabled: bool,
    #[serde(deserialize_with = "read_source_gain")]
    source_gain: Vec<SourceGainPointDto>,
    warp_resampler: u8,
}

impl<'a> AudioContentSer<'a> {
    fn from_model(content: &'a AudioClipContent) -> Result<Self, StorageError> {
        let events = content
            .events
            .as_deref()
            .map(|seq| {
                let sources = seq
                    .sources
                    .iter()
                    .map(|root| {
                        Ok(RetainedRootSer {
                            id: root.id,
                            clip: AudioClipDtoSer::from_model(&root.clip)?,
                            tempo: TempoMapDto::from_model(&root.tempo),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let warp_sources = seq
                    .warp_sources
                    .iter()
                    .map(|ws| EventWarpSourceDto {
                        id: ws.id,
                        markers: ws
                            .markers
                            .iter()
                            .map(|m| WarpMarkerDto {
                                source_frame: m.source_frame,
                                beat_raw: m.beat.raw(),
                            })
                            .collect(),
                    })
                    .collect();
                let events = seq
                    .events
                    .iter()
                    .map(|ev| AudioEventDto {
                        active: ev.active,
                        gain_millidb: ev.gain.millidb(),
                        source: ev.source,
                        id: ev.id,
                        start: ev.start,
                        end: ev.end,
                        source_start: ev.source_start,
                        fade: ev.fade.map(|f| AudioEventFadeDto {
                            source_start: f.source_start(),
                            shape: f.shape().into(),
                        }),
                    })
                    .collect();
                Ok::<_, StorageError>(AudioEventsSer {
                    sources,
                    warp_sources,
                    next_id: seq.next_id,
                    events,
                })
            })
            .transpose()?;
        Ok(Self {
            events,
            musical_event_source: content.musical_event_source.map(|m| MusicalEventSourceDto {
                zero_in_destination: m.zero_in_destination,
                length_raw: m.length.raw(),
            }),
            source_offset: content.source_offset,
            source_length: content.source_length,
            clock: content.clock.into(),
            pitch_cents: content.pitch_cents,
            sample_micro_bpm: content.sample_bpm.map(Tempo::micro_bpm),
            warp: content.warp.as_ref().map(WarpSettingsDto::from),
            warp_enabled: content.warp_enabled,
            source_gain: content
                .source_gain
                .iter()
                .map(|p| SourceGainPointDto {
                    frame: p.frame,
                    gain_q16: p.gain_q16,
                })
                .collect(),
            warp_resampler: warp_resampler_tag(content.warp_resampler),
        })
    }
}

impl AudioContentDe {
    fn into_model(self, depth: usize) -> Result<AudioClipContent, StorageError> {
        let events = self
            .events
            .map(|seq| {
                let next_depth = depth + 1;
                if !seq.sources.is_empty() && next_depth > MAX_RETAINED_AUDIO_DEPTH {
                    return Err(StorageError::Invalid(
                        "retained audio nesting limit reached",
                    ));
                }
                let sources = seq
                    .sources
                    .into_iter()
                    .map(|root| {
                        let clip = root.clip.into_snapshot_at_depth(next_depth)?;
                        let tempo = Arc::new(root.tempo.into_model()?);
                        RetainedAudioRoot::from_snapshot(root.id, clip, tempo).map_err(invalid)
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let warp_sources = seq
                    .warp_sources
                    .into_iter()
                    .map(|ws| AudioEventWarpSource {
                        id: ws.id,
                        markers: ws
                            .markers
                            .into_iter()
                            .map(|m| WarpMarker {
                                source_frame: m.source_frame,
                                beat: BeatTime::from_raw(m.beat_raw),
                            })
                            .collect::<Vec<_>>()
                            .into(),
                    })
                    .collect();
                let events = seq
                    .events
                    .into_iter()
                    .map(|ev| {
                        let fade = ev
                            .fade
                            .map(|f| {
                                AudioEventFade::new(f.source_start, f.shape.try_into()?)
                                    .map_err(invalid)
                            })
                            .transpose()?;
                        Ok(AudioEvent {
                            active: ev.active,
                            gain: ClipGain::new(ev.gain_millidb).map_err(invalid)?,
                            source: ev.source,
                            id: ev.id,
                            start: ev.start,
                            end: ev.end,
                            source_start: ev.source_start,
                            fade,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Ok(Arc::new(AudioEvents {
                    sources,
                    warp_sources,
                    next_id: seq.next_id,
                    events,
                }))
            })
            .transpose()?;
        let sample_bpm = self
            .sample_micro_bpm
            .map(|m| Tempo::new(m).map_err(invalid))
            .transpose()?;
        Ok(AudioClipContent {
            events,
            musical_event_source: self.musical_event_source.map(|m| MusicalEventSource {
                zero_in_destination: m.zero_in_destination,
                length: BeatTime::from_raw(m.length_raw),
            }),
            source_offset: self.source_offset,
            source_length: self.source_length,
            clock: self.clock.into(),
            pitch_cents: self.pitch_cents,
            sample_bpm,
            warp: self.warp.map(WarpSettings::from),
            warp_enabled: self.warp_enabled,
            source_gain: self
                .source_gain
                .into_iter()
                .map(|p| SourceGainPoint {
                    frame: p.frame,
                    gain_q16: p.gain_q16,
                })
                .collect(),
            warp_resampler: warp_resampler_from_tag(self.warp_resampler)?,
        })
    }
}

#[derive(Serialize)]
struct AudioDomainSer<'a> {
    content: AudioContentSer<'a>,
    source_phase: u64,
    looping: bool,
    fade: FadeDto,
    fade_clock: FadeClockDto,
    fade_start: u64,
    fade_projection: Option<FadeProjectionDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioDomainDe {
    content: AudioContentDe,
    source_phase: u64,
    looping: bool,
    fade: FadeDto,
    fade_clock: FadeClockDto,
    fade_start: u64,
    #[serde(deserialize_with = "required_option")]
    fade_projection: Option<FadeProjectionDto>,
}

#[derive(Serialize)]
struct AudioEditSer<'a> {
    repeat_period: Option<u64>,
    content_view: Option<AudioContentViewDto>,
    processing_projection: Option<ProcessingProjectionDto>,
    placement_start_offset: i8,
    placement_end_offset: i8,
    domain: AudioDomainSer<'a>,
    view_start: u64,
    view_end: u64,
    timing: TimingDto,
    placement: Option<PlacementDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioEditDe {
    #[serde(deserialize_with = "required_option")]
    repeat_period: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    content_view: Option<AudioContentViewDto>,
    #[serde(deserialize_with = "required_option")]
    processing_projection: Option<ProcessingProjectionDto>,
    placement_start_offset: i8,
    placement_end_offset: i8,
    domain: AudioDomainDe,
    view_start: u64,
    view_end: u64,
    timing: TimingDto,
    #[serde(deserialize_with = "required_option")]
    placement: Option<PlacementDto>,
}

impl<'a> AudioEditSer<'a> {
    fn from_model(edit: &'a AudioClipEdit) -> Result<Self, StorageError> {
        let d = &edit.domain;
        Ok(Self {
            repeat_period: edit.repeat_period,
            content_view: edit.content_view.as_ref().map(AudioContentViewDto::from),
            processing_projection: edit
                .processing_projection
                .as_deref()
                .map(ProcessingProjectionDto::from),
            placement_start_offset: edit.placement_start_offset,
            placement_end_offset: edit.placement_end_offset,
            domain: AudioDomainSer {
                content: AudioContentSer::from_model(&d.content)?,
                source_phase: d.source_phase,
                looping: d.looping,
                fade: d.fade.into(),
                fade_clock: match d.fade_clock {
                    AudioClipFadeClock::LegacyRoot => FadeClockDto::LegacyRoot,
                    AudioClipFadeClock::Outer => FadeClockDto::Outer,
                },
                fade_start: d.fade_start,
                fade_projection: d.fade_projection.as_ref().map(FadeProjectionDto::from),
            },
            view_start: edit.view_start,
            view_end: edit.view_end,
            timing: match &edit.timing {
                AudioClipTiming::Frames => TimingDto::Frames,
                AudioClipTiming::Musical(start) => TimingDto::Musical {
                    start_raw: start.raw(),
                },
                AudioClipTiming::Frozen {
                    reference_start_raw,
                    tempo,
                } => TimingDto::Frozen {
                    reference_start_raw: reference_start_raw.to_string(),
                    tempo: TempoMapDto::from_model(tempo),
                },
            },
            placement: edit.placement.map(PlacementDto::from),
        })
    }
}

impl AudioEditDe {
    fn into_model(self, depth: usize) -> Result<AudioClipEdit, StorageError> {
        let timing = match self.timing {
            TimingDto::Frames => AudioClipTiming::Frames,
            TimingDto::Musical { start_raw } => {
                AudioClipTiming::Musical(BeatTime::from_raw(start_raw))
            }
            TimingDto::Frozen {
                reference_start_raw,
                tempo,
            } => {
                let reference = reference_start_raw
                    .parse::<i128>()
                    .map_err(|_| StorageError::Invalid("invalid frozen audio reference start"))?;
                AudioClipTiming::Frozen {
                    reference_start_raw: reference,
                    tempo: Arc::new(tempo.into_model()?),
                }
            }
        };
        let domain = AudioClipDomain {
            content: Arc::new(self.domain.content.into_model(depth)?),
            source_phase: self.domain.source_phase,
            looping: self.domain.looping,
            fade: self.domain.fade.try_into()?,
            fade_clock: match self.domain.fade_clock {
                FadeClockDto::LegacyRoot => AudioClipFadeClock::LegacyRoot,
                FadeClockDto::Outer => AudioClipFadeClock::Outer,
            },
            fade_start: self.domain.fade_start,
            fade_projection: self
                .domain
                .fade_projection
                .map(AudioFadeProjection::try_from)
                .transpose()?,
        };
        Ok(AudioClipEdit {
            repeat_period: self.repeat_period,
            content_view: self
                .content_view
                .map(AudioContentView::try_from)
                .transpose()?,
            processing_projection: self
                .processing_projection
                .map(AudioProcessingProjection::try_from)
                .transpose()?
                .map(Arc::new),
            placement_start_offset: self.placement_start_offset,
            placement_end_offset: self.placement_end_offset,
            domain: Arc::new(domain),
            view_start: self.view_start,
            view_end: self.view_end,
            timing,
            placement: self.placement.map(AudioViewPlacement::from),
        })
    }
}

#[derive(Serialize)]
pub(super) struct AudioClipDtoSer<'a> {
    active: bool,
    name: &'a str,
    flavor: ClipFlavorDto,
    id: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
    source_phase: u64,
    edit: Option<AudioEditSer<'a>>,
    fade: FadeDto,
    gain_millidb: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioClipDtoDe {
    active: bool,
    name: String,
    flavor: ClipFlavorDto,
    id: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
    source_phase: u64,
    #[serde(deserialize_with = "required_option")]
    edit: Option<AudioEditDe>,
    fade: FadeDto,
    gain_millidb: i32,
}

impl<'a> AudioClipDtoSer<'a> {
    pub(super) fn from_model(clip: &'a Clip) -> Result<Self, StorageError> {
        Ok(Self {
            active: clip.active(),
            name: clip.name(),
            flavor: clip.flavor().into(),
            id: clip.id().value(),
            asset: clip.asset().value(),
            start: clip.start(),
            duration: clip.duration(),
            source_offset: clip.source_offset(),
            source_length: clip.source_length(),
            looping: clip.looping(),
            source_phase: clip.source_phase(),
            edit: clip
                .audio_edit()
                .map(AudioEditSer::from_model)
                .transpose()?,
            fade: clip.fade().into(),
            gain_millidb: clip.gain().millidb(),
        })
    }
}

impl AudioClipDtoDe {
    pub(super) fn into_snapshot(self) -> Result<ClipSnapshot, StorageError> {
        self.into_snapshot_at_depth(0)
    }

    fn into_snapshot_at_depth(self, depth: usize) -> Result<ClipSnapshot, StorageError> {
        Ok(ClipSnapshot {
            active: self.active,
            name: self.name,
            flavor: self.flavor.into(),
            id: self.id,
            asset: self.asset,
            start: self.start,
            duration: self.duration,
            source_offset: self.source_offset,
            source_length: self.source_length,
            looping: self.looping,
            source_phase: self.source_phase,
            edit: self.edit.map(|e| e.into_model(depth)).transpose()?,
            fade: self.fade.try_into()?,
            gain: ClipGain::new(self.gain_millidb).map_err(invalid)?,
        })
    }
}

// ---------------------------------------------------------------------------
// MIDI clips, notes, volume curves
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OneShotBoundaryDto {
    SourceEnd,
    LoopEnd,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiVolumePointDto {
    beat_raw: u64,
    incoming: f64,
    value: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiVolumeDto {
    clip_gain_millidb: i32,
    #[serde(deserialize_with = "read_midi_volume_points")]
    points: Vec<MidiVolumePointDto>,
}

fn read_midi_volume_points<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MidiVolumePointDto>, D::Error> {
    bounded(d, MAX_MIDI_VOLUME_POINTS_PER_CLIP)
}

impl From<&MidiClipVolume> for MidiVolumeDto {
    fn from(v: &MidiClipVolume) -> Self {
        Self {
            clip_gain_millidb: v.clip_gain().millidb(),
            points: v
                .points()
                .iter()
                .map(|p| MidiVolumePointDto {
                    beat_raw: p.beat.raw(),
                    incoming: p.incoming.linear(),
                    value: p.value.linear(),
                })
                .collect(),
        }
    }
}

impl MidiVolumeDto {
    fn into_model(self, source_length: BeatTime) -> Result<MidiClipVolume, StorageError> {
        let clip_gain = ClipGain::new(self.clip_gain_millidb).map_err(invalid)?;
        let points = self
            .points
            .into_iter()
            .map(|p| {
                Ok(MidiClipVolumePoint {
                    beat: BeatTime::from_raw(p.beat_raw),
                    incoming: MidiClipGain::new(p.incoming).map_err(invalid)?,
                    value: MidiClipGain::new(p.value).map_err(invalid)?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let volume = MidiClipVolume::new(points, source_length).map_err(invalid)?;
        Ok(volume.with_clip_gain(clip_gain))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MidiNoteDto {
    id: u64,
    active: bool,
    start_raw: u64,
    duration_raw: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

fn read_notes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MidiNoteDto>, D::Error> {
    bounded(d, MAX_NOTES)
}

impl From<&MidiNote> for MidiNoteDto {
    fn from(n: &MidiNote) -> Self {
        Self {
            id: n.id().value(),
            active: n.active(),
            start_raw: n.start().raw(),
            duration_raw: n.duration().raw(),
            pitch: n.pitch(),
            channel: n.channel(),
            velocity: n.velocity(),
        }
    }
}

impl From<&MidiNoteSnapshot> for MidiNoteDto {
    fn from(n: &MidiNoteSnapshot) -> Self {
        Self {
            id: n.id,
            active: n.active,
            start_raw: n.start_raw,
            duration_raw: n.duration_raw,
            pitch: n.pitch,
            channel: n.channel,
            velocity: n.velocity,
        }
    }
}

impl From<MidiNoteDto> for MidiNoteSnapshot {
    fn from(n: MidiNoteDto) -> Self {
        Self {
            id: n.id,
            active: n.active,
            start_raw: n.start_raw,
            duration_raw: n.duration_raw,
            pitch: n.pitch,
            channel: n.channel,
            velocity: n.velocity,
        }
    }
}

#[derive(Serialize)]
pub(super) struct MidiClipDtoSer<'a> {
    content_view: Option<ContentWindowDto>,
    active: bool,
    playback_offset_raw: u64,
    one_shot_boundary: OneShotBoundaryDto,
    volume: MidiVolumeDto,
    name: &'a str,
    flavor: ClipFlavorDto,
    id: u64,
    start_raw: u64,
    length_raw: u64,
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    notes: super::SliceSeq<'a, MidiNote, MidiNoteDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MidiClipDtoDe {
    #[serde(deserialize_with = "required_option")]
    content_view: Option<ContentWindowDto>,
    active: bool,
    playback_offset_raw: u64,
    one_shot_boundary: OneShotBoundaryDto,
    volume: MidiVolumeDto,
    name: String,
    flavor: ClipFlavorDto,
    id: u64,
    start_raw: u64,
    length_raw: u64,
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    #[serde(deserialize_with = "read_notes")]
    notes: Vec<MidiNoteDto>,
}

impl<'a> MidiClipDtoSer<'a> {
    pub(super) fn from_model(clip: &'a MidiClip, aliased: bool) -> Self {
        let source = clip.source();
        Self {
            content_view: clip.content_view().map(ContentWindowDto::from),
            active: clip.active(),
            playback_offset_raw: clip.playback_offset().raw(),
            one_shot_boundary: match source.one_shot_boundary() {
                MidiOneShotBoundary::SourceEnd => OneShotBoundaryDto::SourceEnd,
                MidiOneShotBoundary::LoopEnd => OneShotBoundaryDto::LoopEnd,
            },
            volume: clip.volume().into(),
            name: clip.name(),
            flavor: clip.flavor().into(),
            id: clip.id().value(),
            start_raw: clip.start().raw(),
            length_raw: clip.length().raw(),
            source_length_raw: source.source_length().raw(),
            source_start_raw: source.source_start().raw(),
            loop_start_raw: source.loop_start().raw(),
            loop_end_raw: source.loop_end().raw(),
            looping: source.looping(),
            notes: super::SliceSeq(
                if aliased { &[] } else { source.notes() },
                MidiNoteDto::from,
            ),
        }
    }
}

impl MidiClipDtoDe {
    pub(super) fn into_snapshot(self) -> Result<MidiClipSnapshot, StorageError> {
        let source_length = BeatTime::from_raw(self.source_length_raw);
        Ok(MidiClipSnapshot {
            content_view: self.content_view.map(ContentWindow::try_from).transpose()?,
            active: self.active,
            playback_offset_raw: self.playback_offset_raw,
            one_shot_boundary: match self.one_shot_boundary {
                OneShotBoundaryDto::SourceEnd => MidiOneShotBoundary::SourceEnd,
                OneShotBoundaryDto::LoopEnd => MidiOneShotBoundary::LoopEnd,
            },
            volume: self.volume.into_model(source_length)?,
            name: self.name,
            flavor: self.flavor.into(),
            id: self.id,
            start_raw: self.start_raw,
            length_raw: self.length_raw,
            source_length_raw: self.source_length_raw,
            source_start_raw: self.source_start_raw,
            loop_start_raw: self.loop_start_raw,
            loop_end_raw: self.loop_end_raw,
            looping: self.looping,
            notes: self.notes.into_iter().map(MidiNoteSnapshot::from).collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// Session grid
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SceneSer<'a> {
    id: u64,
    name: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneDe {
    id: u64,
    name: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAudioLoopDto {
    start: u64,
    end: u64,
}

#[derive(Serialize)]
struct SessionAudioSer<'a> {
    active: bool,
    flavor: ClipFlavorDto,
    id: u64,
    name: &'a str,
    asset: u64,
    source_start: u64,
    source_end: u64,
    loop_range: Option<SessionAudioLoopDto>,
    gain_millidb: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAudioDe {
    active: bool,
    flavor: ClipFlavorDto,
    id: u64,
    name: String,
    asset: u64,
    source_start: u64,
    source_end: u64,
    #[serde(deserialize_with = "required_option")]
    loop_range: Option<SessionAudioLoopDto>,
    gain_millidb: i32,
}

#[derive(Serialize)]
struct MidiSourceSer<'a> {
    one_shot_boundary: OneShotBoundaryDto,
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    notes: super::SliceSeq<'a, MidiNote, MidiNoteDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiSourceDe {
    one_shot_boundary: OneShotBoundaryDto,
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    #[serde(deserialize_with = "read_notes")]
    notes: Vec<MidiNoteDto>,
}

impl<'a> MidiSourceSer<'a> {
    fn from_model(source: &'a MidiSource, aliased: bool) -> Self {
        Self {
            one_shot_boundary: match source.one_shot_boundary() {
                MidiOneShotBoundary::SourceEnd => OneShotBoundaryDto::SourceEnd,
                MidiOneShotBoundary::LoopEnd => OneShotBoundaryDto::LoopEnd,
            },
            source_length_raw: source.source_length().raw(),
            source_start_raw: source.source_start().raw(),
            loop_start_raw: source.loop_start().raw(),
            loop_end_raw: source.loop_end().raw(),
            looping: source.looping(),
            notes: super::SliceSeq(
                if aliased { &[] } else { source.notes() },
                MidiNoteDto::from,
            ),
        }
    }
}

#[derive(Serialize)]
struct SessionMidiSer<'a> {
    active: bool,
    playback_offset_raw: u64,
    volume: MidiVolumeDto,
    flavor: ClipFlavorDto,
    id: u64,
    name: &'a str,
    source: MidiSourceSer<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionMidiDe {
    active: bool,
    playback_offset_raw: u64,
    volume: MidiVolumeDto,
    flavor: ClipFlavorDto,
    id: u64,
    name: String,
    source: MidiSourceDe,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SessionCellContentSer<'a> {
    Midi(SessionMidiSer<'a>),
    Audio(SessionAudioSer<'a>),
    Stop,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SessionCellContentDe {
    Midi(SessionMidiDe),
    Audio(SessionAudioDe),
    Stop,
}

#[derive(Serialize)]
struct SessionCellSer<'a> {
    track: u64,
    scene: u64,
    content: SessionCellContentSer<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCellDe {
    track: u64,
    scene: u64,
    content: SessionCellContentDe,
}

#[derive(Serialize)]
pub(super) struct SessionSer<'a> {
    scenes: Vec<SceneSer<'a>>,
    cells: Vec<SessionCellSer<'a>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionDe {
    #[serde(deserialize_with = "read_scenes")]
    scenes: Vec<SceneDe>,
    #[serde(deserialize_with = "read_cells")]
    cells: Vec<SessionCellDe>,
}

fn read_scenes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SceneDe>, D::Error> {
    bounded(d, MAX_SCENES)
}

fn read_cells<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SessionCellDe>, D::Error> {
    bounded(d, MAX_SESSION_CELLS)
}

impl<'a> SessionSer<'a> {
    pub(super) fn from_model(project: &'a Project) -> Self {
        Self {
            scenes: project
                .scenes()
                .iter()
                .map(|s| SceneSer {
                    id: s.id().value(),
                    name: s.name(),
                })
                .collect(),
            cells: project
                .session_cells()
                .iter()
                .map(|cell| SessionCellSer {
                    track: cell.track().value(),
                    scene: cell.scene().value(),
                    content: match cell.content() {
                        SessionCellContent::Stop => SessionCellContentSer::Stop,
                        SessionCellContent::Midi(clip) => {
                            let aliased = project.clip_alias(clip.id()).is_some();
                            SessionCellContentSer::Midi(SessionMidiSer {
                                active: clip.active(),
                                playback_offset_raw: clip.playback_offset().raw(),
                                volume: clip.volume().into(),
                                flavor: clip.flavor().into(),
                                id: clip.id().value(),
                                name: clip.name(),
                                source: MidiSourceSer::from_model(clip.source(), aliased),
                            })
                        }
                        SessionCellContent::Audio(clip) => {
                            SessionCellContentSer::Audio(SessionAudioSer {
                                active: clip.active(),
                                flavor: clip.flavor().into(),
                                id: clip.id().value(),
                                name: clip.name(),
                                asset: clip.asset().value(),
                                source_start: clip.source_start(),
                                source_end: clip.source_end(),
                                loop_range: clip.loop_range().map(|l| SessionAudioLoopDto {
                                    start: l.start(),
                                    end: l.end(),
                                }),
                                gain_millidb: clip.gain().millidb(),
                            })
                        }
                    },
                })
                .collect(),
        }
    }
}

impl SessionDe {
    pub(super) fn into_snapshot(self) -> Result<SessionSnapshot, StorageError> {
        let scenes = self
            .scenes
            .into_iter()
            .map(|s| SceneSnapshot {
                id: s.id,
                name: s.name,
            })
            .collect();
        let cells = self
            .cells
            .into_iter()
            .map(|cell| {
                let content = match cell.content {
                    SessionCellContentDe::Stop => SessionCellContentSnapshot::Stop,
                    SessionCellContentDe::Midi(clip) => {
                        let source_length = BeatTime::from_raw(clip.source.source_length_raw);
                        SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                            active: clip.active,
                            playback_offset_raw: clip.playback_offset_raw,
                            volume: clip.volume.into_model(source_length)?,
                            flavor: clip.flavor.into(),
                            id: clip.id,
                            name: clip.name,
                            source: MidiSourceSnapshot {
                                one_shot_boundary: match clip.source.one_shot_boundary {
                                    OneShotBoundaryDto::SourceEnd => MidiOneShotBoundary::SourceEnd,
                                    OneShotBoundaryDto::LoopEnd => MidiOneShotBoundary::LoopEnd,
                                },
                                source_length_raw: clip.source.source_length_raw,
                                source_start_raw: clip.source.source_start_raw,
                                loop_start_raw: clip.source.loop_start_raw,
                                loop_end_raw: clip.source.loop_end_raw,
                                looping: clip.source.looping,
                                notes: clip
                                    .source
                                    .notes
                                    .into_iter()
                                    .map(MidiNoteSnapshot::from)
                                    .collect(),
                            },
                        })
                    }
                    SessionCellContentDe::Audio(clip) => {
                        SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                            active: clip.active,
                            flavor: clip.flavor.into(),
                            id: clip.id,
                            name: clip.name,
                            asset: clip.asset,
                            source_start: clip.source_start,
                            source_end: clip.source_end,
                            loop_range: clip
                                .loop_range
                                .map(|l| SessionAudioLoop::new(l.start, l.end).map_err(invalid))
                                .transpose()?,
                            gain: ClipGain::new(clip.gain_millidb).map_err(invalid)?,
                        })
                    }
                };
                Ok(SessionCellSnapshot {
                    track: cell.track,
                    scene: cell.scene,
                    content,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(SessionSnapshot { scenes, cells })
    }
}

// ---------------------------------------------------------------------------
// Clip Aliases
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AliasContentSer<'a> {
    Audio {
        asset: u64,
        content: AudioContentSer<'a>,
    },
    Midi {
        source_length_raw: u64,
        notes: Vec<MidiNoteDto>,
    },
    ConsoleAudio {
        asset: u64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AliasContentDe {
    Audio {
        asset: u64,
        content: AudioContentDe,
    },
    Midi {
        source_length_raw: u64,
        #[serde(deserialize_with = "read_notes")]
        notes: Vec<MidiNoteDto>,
    },
    ConsoleAudio {
        asset: u64,
    },
}

#[derive(Serialize)]
struct AliasSourceSer<'a> {
    id: u64,
    content: AliasContentSer<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AliasSourceDe {
    id: u64,
    content: AliasContentDe,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AliasMemberDto {
    clip: u64,
    source: u64,
}

#[derive(Serialize)]
pub(super) struct AliasesSer<'a> {
    sources: Vec<AliasSourceSer<'a>>,
    members: Vec<AliasMemberDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AliasesDe {
    #[serde(deserialize_with = "read_alias_sources")]
    sources: Vec<AliasSourceDe>,
    #[serde(deserialize_with = "read_alias_members")]
    members: Vec<AliasMemberDto>,
}

fn read_alias_sources<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AliasSourceDe>, D::Error> {
    bounded(d, MAX_CLIPS)
}

fn read_alias_members<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<AliasMemberDto>, D::Error> {
    bounded(d, MAX_CLIPS)
}

impl<'a> AliasesSer<'a> {
    pub(super) fn from_snapshot(aliases: &'a ClipAliasesSnapshot) -> Result<Self, StorageError> {
        let sources = aliases
            .sources
            .iter()
            .map(|s| {
                let content = match &s.content {
                    AliasContentSnapshot::Audio { asset, content } => AliasContentSer::Audio {
                        asset: *asset,
                        content: AudioContentSer::from_model(content)?,
                    },
                    AliasContentSnapshot::Midi {
                        source_length_raw,
                        notes,
                    } => AliasContentSer::Midi {
                        source_length_raw: *source_length_raw,
                        notes: notes.iter().map(MidiNoteDto::from).collect(),
                    },
                    AliasContentSnapshot::ConsoleAudio { asset } => {
                        AliasContentSer::ConsoleAudio { asset: *asset }
                    }
                };
                Ok(AliasSourceSer { id: s.id, content })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let members = aliases
            .members
            .iter()
            .map(|m| AliasMemberDto {
                clip: m.clip,
                source: m.source,
            })
            .collect();
        Ok(Self { sources, members })
    }
}

impl AliasesDe {
    pub(super) fn into_snapshot(self) -> Result<ClipAliasesSnapshot, StorageError> {
        let sources = self
            .sources
            .into_iter()
            .map(|s| {
                let content = match s.content {
                    AliasContentDe::Audio { asset, content } => AliasContentSnapshot::Audio {
                        asset,
                        content: Arc::new(content.into_model(0)?),
                    },
                    AliasContentDe::Midi {
                        source_length_raw,
                        notes,
                    } => AliasContentSnapshot::Midi {
                        source_length_raw,
                        notes: notes.into_iter().map(MidiNoteSnapshot::from).collect(),
                    },
                    AliasContentDe::ConsoleAudio { asset } => {
                        AliasContentSnapshot::ConsoleAudio { asset }
                    }
                };
                Ok(AliasSourceSnapshot { id: s.id, content })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let members = self
            .members
            .into_iter()
            .map(|m| AliasMemberSnapshot {
                clip: m.clip,
                source: m.source,
            })
            .collect();
        Ok(ClipAliasesSnapshot { sources, members })
    }
}
