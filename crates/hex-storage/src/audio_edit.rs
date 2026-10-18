//! Version-21 musical audio metadata adapter; no DSP or PCM persistence.
use crate::StorageError;
use hex_project::{
    AudioClipDomain, AudioClipEdit, AudioDomainClock, AudioFadeProjection, AudioViewMode,
    AudioViewPlacement, BeatTime, BeatsPreserve, BeatsTail, ClipFade, SourceGainPoint, Tempo,
    TempoMap, TempoPoint, WarpMarker, WarpMode, WarpSettings,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioEditDto {
    domain: Domain,
    view_start: u64,
    view_end: u64,
    musical_start_raw: Option<u64>,
    #[serde(deserialize_with = "required_placement")]
    placement: Option<Placement>,
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
            musical_start_raw: edit.musical_start.map(BeatTime::raw),
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
                if !(-i128::from(u64::MAX)..=i128::from(u64::MAX)).contains(&origin) {
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
        Ok(AudioClipEdit {
            domain: Arc::new(AudioClipDomain {
                events: None,
                source_offset: d.source_offset,
                source_length: d.source_length,
                source_phase: d.source_phase,
                looping: d.looping,
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
                source_gain: d
                    .source_gain
                    .into_iter()
                    .map(|p| SourceGainPoint {
                        frame: p.frame,
                        gain_q16: p.gain_q16,
                    })
                    .collect(),
                fade,
                fade_start: d.fade_start,
                fade_projection,
            }),
            view_start: dto.view_start,
            view_end: dto.view_end,
            musical_start: dto.musical_start_raw.map(BeatTime::from_raw),
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
