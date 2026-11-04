//! Current musical/frozen audio metadata adapter; no DSP or PCM persistence.
use crate::StorageError;
use hex_project::{
    AudioClipDomain, AudioClipEdit, AudioClipTiming, AudioDomainClock, AudioFadeProjection,
    AudioProcessingProjection, AudioViewMode, AudioViewPlacement, BeatTime, BeatsPreserve,
    BeatsTail, ClipFade, SourceGainPoint, Tempo, TempoMap, TempoPoint, WarpMarker, WarpMode,
    WarpResampler, WarpSettings,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Saved tags of [`WarpResampler`]; a tag is never reused for another law.
const WARP_RESAMPLER_V1: u8 = 1;
const WARP_RESAMPLER_V2: u8 = 2;

/// Saved tag of `law`, shared by every record that names a [`WarpResampler`].
pub(crate) fn warp_resampler_tag(law: WarpResampler) -> u8 {
    match law {
        WarpResampler::V1 => WARP_RESAMPLER_V1,
        WarpResampler::V2 => WARP_RESAMPLER_V2,
    }
}

/// Law named by a saved tag; an unknown tag belongs to a newer writer.
pub(crate) fn warp_resampler_from_tag(tag: u8) -> Result<WarpResampler, StorageError> {
    match tag {
        WARP_RESAMPLER_V1 => Ok(WarpResampler::V1),
        WARP_RESAMPLER_V2 => Ok(WarpResampler::V2),
        _ => Err(StorageError::Invalid(
            "unsupported warp resampler version; open it with a newer hex version",
        )),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioEditDto {
    #[serde(deserialize_with = "required_processing_projection")]
    processing_projection: Option<ProcessingProjection>,
    placement_start_offset: i8,
    placement_end_offset: i8,
    domain: Domain,
    view_start: u64,
    view_end: u64,
    timing: Timing,
    #[serde(deserialize_with = "required_placement")]
    placement: Option<Placement>,
}
/// An omitted law is implied; a present one must be a tag, never null.
pub(crate) fn present_warp_resampler<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<u8>, D::Error> {
    u8::deserialize(d).map(Some)
}
fn required_processing_projection<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<ProcessingProjection>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessingProjection {
    sample_rate: u32,
    #[serde(deserialize_with = "bounded_tempo_points")]
    tempo_points: Vec<Point>,
    origin_beat_raw: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Timing {
    Frames,
    Musical {
        start_raw: u64,
    },
    Frozen {
        reference_start_raw: String,
        sample_rate: u32,
        #[serde(deserialize_with = "bounded_tempo_points")]
        tempo_points: Vec<Point>,
    },
}
fn bounded_tempo_points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    struct Points;
    impl<'de> serde::de::Visitor<'de> for Points {
        type Value = Vec<Point>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded frozen audio tempo points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut points = Vec::new();
            while let Some(point) = seq.next_element()? {
                if points.len() == hex_project::MAX_TEMPO_POINTS {
                    return Err(serde::de::Error::custom(
                        "frozen audio tempo point limit exceeded",
                    ));
                }
                points.push(point);
            }
            Ok(points)
        }
    }
    d.deserialize_seq(Points)
}
fn required_placement<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Placement>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    mode: ViewMode,
    outer_length: Clock,
    cycle_phase: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ViewMode {
    Once,
    Repeat,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Domain {
    source_offset: u64,
    source_length: u64,
    source_phase: u64,
    looping: bool,
    clock: Clock,
    pitch_cents: i16,
    sample_micro_bpm: Option<u32>,
    warp_enabled: bool,
    warp: Option<Warp>,
    source_gain: Vec<GainPoint>,
    fade: Fade,
    fade_start: u64,
    fade_projection: Option<Projection>,
    /// Schema65 content law, present only when it differs from
    /// [`WarpResampler::implied`]; older layouts never contain it.
    #[serde(
        default,
        deserialize_with = "present_warp_resampler",
        skip_serializing_if = "Option::is_none"
    )]
    warp_resampler: Option<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Clock {
    Frames { length: u64 },
    Beats { length_raw: u64 },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GainPoint {
    frame: u64,
    gain_q16: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fade {
    span: u64,
    offset: u64,
    fade_in: u64,
    fade_out: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Projection {
    sample_rate: u32,
    tempo_points: Vec<Point>,
    origin_beat_raw: String,
    local_start_raw: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    beat_raw: u64,
    micro_bpm: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Warp {
    mode: Mode,
    markers: Vec<Marker>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    source_frame: u64,
    beat_raw: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Mode {
    RePitch,
    Beats {
        division_raw: Option<u64>,
        tail: Tail,
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
#[serde(rename_all = "snake_case")]
enum Tail {
    Off,
    Forward,
    Alternating,
}

impl From<AudioClipEdit> for AudioEditDto {
    fn from(edit: AudioClipEdit) -> Self {
        let d = &edit.domain;
        Self {
            placement_start_offset: edit.placement_start_offset,
            placement_end_offset: edit.placement_end_offset,
            processing_projection: edit.processing_projection.as_ref().map(|p| {
                ProcessingProjection {
                    sample_rate: p.tempo.sample_rate(),
                    tempo_points: p
                        .tempo
                        .points()
                        .iter()
                        .map(|point| Point {
                            beat_raw: point.beat().raw(),
                            micro_bpm: point.tempo().micro_bpm(),
                        })
                        .collect(),
                    origin_beat_raw: p.origin_beat_raw.to_string(),
                }
            }),
            domain: Domain {
                source_offset: d.source_offset,
                source_length: d.source_length,
                source_phase: d.source_phase,
                looping: d.looping,
                clock: match d.clock {
                    AudioDomainClock::Frames(length) => Clock::Frames { length },
                    AudioDomainClock::Beats(length) => Clock::Beats {
                        length_raw: length.raw(),
                    },
                },
                pitch_cents: d.pitch_cents,
                sample_micro_bpm: d.sample_bpm.map(Tempo::micro_bpm),
                warp_resampler: d
                    .has_explicit_warp_resampler()
                    .then_some(warp_resampler_tag(d.warp_resampler)),
                warp_enabled: d.warp_enabled,
                warp: d.warp.as_ref().map(|warp| Warp {
                    mode: match warp.mode {
                        WarpMode::RePitch => Mode::RePitch,
                        WarpMode::Beats {
                            preserve,
                            tail,
                            decay,
                        } => Mode::Beats {
                            division_raw: match preserve {
                                BeatsPreserve::Transients => None,
                                BeatsPreserve::Division(beat) => Some(beat.raw()),
                            },
                            tail: match tail {
                                BeatsTail::Off => Tail::Off,
                                BeatsTail::Forward => Tail::Forward,
                                BeatsTail::Alternating => Tail::Alternating,
                            },
                            decay,
                        },
                        WarpMode::Tones { grain_ms } => Mode::Tones { grain_ms },
                        WarpMode::Texture {
                            grain_ms,
                            fluctuation,
                            seed,
                        } => Mode::Texture {
                            grain_ms,
                            fluctuation,
                            seed,
                        },
                        WarpMode::Complex => Mode::Complex,
                        WarpMode::ComplexPro { formants, envelope } => {
                            Mode::ComplexPro { formants, envelope }
                        }
                    },
                    markers: warp
                        .markers
                        .iter()
                        .map(|m| Marker {
                            source_frame: m.source_frame,
                            beat_raw: m.beat.raw(),
                        })
                        .collect(),
                }),
                source_gain: d
                    .source_gain
                    .iter()
                    .map(|p| GainPoint {
                        frame: p.frame,
                        gain_q16: p.gain_q16,
                    })
                    .collect(),
                fade: Fade {
                    span: d.fade.span(),
                    offset: d.fade.offset(),
                    fade_in: d.fade.fade_in(),
                    fade_out: d.fade.fade_out(),
                },
                fade_start: d.fade_start,
                fade_projection: d.fade_projection.as_ref().map(|p| Projection {
                    sample_rate: p.tempo.sample_rate(),
                    tempo_points: p
                        .tempo
                        .points()
                        .iter()
                        .map(|point| Point {
                            beat_raw: point.beat().raw(),
                            micro_bpm: point.tempo().micro_bpm(),
                        })
                        .collect(),
                    origin_beat_raw: p.origin_beat_raw.to_string(),
                    local_start_raw: p.local_start_raw,
                }),
            },
            view_start: edit.view_start,
            view_end: edit.view_end,
            timing: match &edit.timing {
                AudioClipTiming::Frames => Timing::Frames,
                AudioClipTiming::Musical(start) => Timing::Musical {
                    start_raw: start.raw(),
                },
                AudioClipTiming::Frozen {
                    reference_start_raw,
                    tempo,
                } => Timing::Frozen {
                    reference_start_raw: reference_start_raw.to_string(),
                    sample_rate: tempo.sample_rate(),
                    tempo_points: tempo
                        .points()
                        .iter()
                        .map(|p| Point {
                            beat_raw: p.beat().raw(),
                            micro_bpm: p.tempo().micro_bpm(),
                        })
                        .collect(),
                },
            },
            placement: edit.placement.map(|p| Placement {
                mode: match p.mode {
                    AudioViewMode::Once => ViewMode::Once,
                    AudioViewMode::Repeat => ViewMode::Repeat,
                },
                outer_length: match p.outer_length {
                    AudioDomainClock::Frames(length) => Clock::Frames { length },
                    AudioDomainClock::Beats(length) => Clock::Beats {
                        length_raw: length.raw(),
                    },
                },
                cycle_phase: p.cycle_phase,
            }),
        }
    }
}
impl TryFrom<AudioEditDto> for AudioClipEdit {
    type Error = StorageError;
    fn try_from(dto: AudioEditDto) -> Result<Self, Self::Error> {
        let d = dto.domain;
        if d.source_gain.len() > hex_project::MAX_AUDIO_POINTS
            || d.warp
                .as_ref()
                .is_some_and(|w| w.markers.len() > hex_project::MAX_AUDIO_POINTS)
        {
            return Err(StorageError::Invalid("audio domain point limit exceeded"));
        }
        let fade = ClipFade::new(d.fade.span, d.fade.offset, d.fade.fade_in, d.fade.fade_out)
            .map_err(|e| StorageError::Invalid(e.0))?;
        let implied = WarpResampler::implied(d.pitch_cents, d.warp_enabled);
        let warp_resampler = d
            .warp_resampler
            .map_or(Ok(implied), warp_resampler_from_tag)?;
        if d.warp_resampler.is_some() && warp_resampler == implied {
            return Err(StorageError::Invalid(
                "redundant warp resampler version in audio domain",
            ));
        }
        let fade_projection = d
            .fade_projection
            .map(|p| -> Result<_, StorageError> {
                let points = p
                    .tempo_points
                    .into_iter()
                    .map(|point| {
                        Tempo::new(point.micro_bpm)
                            .map(|tempo| TempoPoint::new(BeatTime::from_raw(point.beat_raw), tempo))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| StorageError::Invalid(e.0))?;
                let tempo =
                    TempoMap::new(p.sample_rate, points).map_err(|e| StorageError::Invalid(e.0))?;
                let origin = p
                    .origin_beat_raw
                    .parse::<i128>()
                    .map_err(|_| StorageError::Invalid("invalid audio fade projection origin"))?;
                // Match audio_projection_frame: consumed Once phase plus root offset.
                if !(-2 * i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&origin) {
                    return Err(StorageError::Invalid(
                        "audio fade projection origin exceeds bounded range",
                    ));
                }
                Ok(AudioFadeProjection {
                    tempo,
                    origin_beat_raw: origin,
                    local_start_raw: p.local_start_raw,
                })
            })
            .transpose()?;
        let warp = d.warp.map(|warp| WarpSettings {
            mode: match warp.mode {
                Mode::RePitch => WarpMode::RePitch,
                Mode::Beats {
                    division_raw,
                    tail,
                    decay,
                } => WarpMode::Beats {
                    preserve: division_raw.map_or(BeatsPreserve::Transients, |raw| {
                        BeatsPreserve::Division(BeatTime::from_raw(raw))
                    }),
                    tail: match tail {
                        Tail::Off => BeatsTail::Off,
                        Tail::Forward => BeatsTail::Forward,
                        Tail::Alternating => BeatsTail::Alternating,
                    },
                    decay,
                },
                Mode::Tones { grain_ms } => WarpMode::Tones { grain_ms },
                Mode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                } => WarpMode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                },
                Mode::Complex => WarpMode::Complex,
                Mode::ComplexPro { formants, envelope } => {
                    WarpMode::ComplexPro { formants, envelope }
                }
            },
            markers: warp
                .markers
                .into_iter()
                .map(|m| WarpMarker {
                    source_frame: m.source_frame,
                    beat: BeatTime::from_raw(m.beat_raw),
                })
                .collect(),
        });
        let processing_projection = dto
            .processing_projection
            .map(|p| -> Result<_, StorageError> {
                let points = p
                    .tempo_points
                    .into_iter()
                    .map(|point| {
                        Tempo::new(point.micro_bpm)
                            .map(|tempo| TempoPoint::new(BeatTime::from_raw(point.beat_raw), tempo))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| StorageError::Invalid(e.0))?;
                let tempo =
                    TempoMap::new(p.sample_rate, points).map_err(|e| StorageError::Invalid(e.0))?;
                let origin_beat_raw = p.origin_beat_raw.parse::<i128>().map_err(|_| {
                    StorageError::Invalid("invalid audio processing projection origin")
                })?;
                Ok(Arc::new(AudioProcessingProjection {
                    tempo: Arc::new(tempo),
                    origin_beat_raw,
                }))
            })
            .transpose()?;
        Ok(AudioClipEdit {
            content_view: None,
            processing_projection,
            placement_start_offset: dto.placement_start_offset,
            placement_end_offset: dto.placement_end_offset,
            domain: Arc::new(AudioClipDomain {
                content: Arc::new(hex_project::AudioClipContent {
                    events: None,
                    source_offset: d.source_offset,
                    source_length: d.source_length,
                    clock: match d.clock {
                        Clock::Frames { length } => AudioDomainClock::Frames(length),
                        Clock::Beats { length_raw } => {
                            AudioDomainClock::Beats(BeatTime::from_raw(length_raw))
                        }
                    },
                    pitch_cents: d.pitch_cents,
                    sample_bpm: d
                        .sample_micro_bpm
                        .map(Tempo::new)
                        .transpose()
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    warp_enabled: d.warp_enabled,
                    warp,
                    warp_resampler,
                    source_gain: d
                        .source_gain
                        .into_iter()
                        .map(|p| SourceGainPoint {
                            frame: p.frame,
                            gain_q16: p.gain_q16,
                        })
                        .collect(),
                }),
                source_phase: d.source_phase,
                looping: d.looping,
                fade,
                fade_start: d.fade_start,
                fade_projection,
            }),
            view_start: dto.view_start,
            view_end: dto.view_end,
            timing: match dto.timing {
                Timing::Frames => AudioClipTiming::Frames,
                Timing::Musical { start_raw } => {
                    AudioClipTiming::Musical(BeatTime::from_raw(start_raw))
                }
                Timing::Frozen {
                    reference_start_raw,
                    sample_rate,
                    tempo_points,
                } => {
                    let reference_start_raw = reference_start_raw
                        .parse::<i128>()
                        .map_err(|_| StorageError::Invalid("invalid frozen audio reference"))?;
                    let points = tempo_points
                        .into_iter()
                        .map(|p| {
                            Tempo::new(p.micro_bpm)
                                .map(|tempo| TempoPoint::new(BeatTime::from_raw(p.beat_raw), tempo))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| StorageError::Invalid(e.0))?;
                    let tempo = TempoMap::new(sample_rate, points)
                        .map_err(|e| StorageError::Invalid(e.0))?;
                    hex_project::audio_projection_frame(&tempo, reference_start_raw)
                        .map_err(|e| StorageError::Invalid(e.0))?;
                    AudioClipTiming::Frozen {
                        reference_start_raw,
                        tempo: Arc::new(tempo),
                    }
                }
            },
            placement: dto.placement.map(|p| AudioViewPlacement {
                mode: match p.mode {
                    ViewMode::Once => AudioViewMode::Once,
                    ViewMode::Repeat => AudioViewMode::Repeat,
                },
                outer_length: match p.outer_length {
                    Clock::Frames { length } => AudioDomainClock::Frames(length),
                    Clock::Beats { length_raw } => {
                        AudioDomainClock::Beats(BeatTime::from_raw(length_raw))
                    }
                },
                cycle_phase: p.cycle_phase,
            }),
        })
    }
}
