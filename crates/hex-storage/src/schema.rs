pub(crate) mod alias_projection_v60;
pub(crate) mod appearance;
pub(crate) mod arrangement_locators;
mod arrangement_sections;
mod audio_alias_projection;
mod audio_event_roots;
pub(crate) mod audio_events;
pub(crate) mod automation_read;
pub(crate) mod bound_source_effects;
pub(crate) mod branch_audio;
pub(crate) mod clip_activation;
pub(crate) mod clip_aliases;
pub(crate) mod clip_names;
pub(crate) mod coincident_automation;
pub(crate) mod combined;
pub(crate) mod content_view_resampler;
pub(crate) mod content_views;
pub(crate) mod conversion_v2;
pub(crate) mod crossfades;
pub(crate) mod device_assets;
pub(crate) mod device_automation;
pub(crate) mod device_automation_touch;
pub(crate) mod device_modulation;
pub(crate) mod event_activation;
pub(crate) mod event_fades;
pub(crate) mod event_gain;
pub(crate) mod event_warp_sources;
pub(crate) mod fade_curve_positions;
pub(crate) mod fade_shapes;
pub(crate) mod gain_envelopes;
pub(crate) mod gain_touch;
pub(crate) mod group_output_mute;
pub(crate) mod grouped_audio;
pub(crate) mod host_automation;
mod host_gain;
pub(crate) mod large_vst3_state;
pub(crate) mod legacy;
pub(crate) mod legacy_early;
pub(crate) mod long_granular;
pub(crate) mod long_pitch;
pub(crate) mod long_pitch_fade;
pub(crate) mod long_reverse;
pub(crate) mod midi_clip_gain;
mod midi_clip_volume;
pub(crate) mod midi_one_shot;
mod migrations;
pub(crate) mod musical_audio;
pub(crate) mod musical_event_source;
pub(crate) mod native_sources;
pub(crate) mod nested_audio;
pub(crate) mod note_activation;
mod original_sources;
pub(crate) mod outer_clip_fades;
pub(crate) mod playable_routing;
pub(crate) mod provenance;
pub(crate) mod retained_receiver;
pub(crate) mod return_feeds;
pub(crate) mod routing;
pub(crate) mod s_curve_fades;
pub(crate) mod sample_rate_mode;
pub(crate) mod sampler_crossfade;
pub(crate) mod serial_units;
pub(crate) mod session_audio_loops;
pub(crate) mod sidechain;
pub(crate) mod silent_repeat;
pub(crate) mod track_groups;
pub(crate) mod track_name_origin;
pub(crate) mod track_tags;
pub(crate) mod tuning;
pub(crate) mod vst3_automation;
pub(crate) mod warp_resampler;
use crate::StorageError;
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipSnapshot, DeviceSnapshot, IDENTITY_CONVERSION, Meter,
    MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, MidiSourceSnapshot, Project,
    ReturnTrackSnapshot, SceneSnapshot, SendLevelSnapshot, SessionCellContentSnapshot,
    SessionCellSnapshot, SessionMidiClipSnapshot, SessionSnapshot, Snapshot, Tempo, TempoPoint,
    TrackContentSnapshot, TrackSnapshot,
};
use serde::{Deserialize, Serialize};

mod v14_v19;
mod v1_v8;
mod v9_v13;
pub(crate) use v1_v8::DocumentV8;
use v1_v8::*;
use v9_v13::*;
pub(crate) use v9_v13::{DocumentV11, DocumentV13};
use v14_v19::*;
pub(crate) use v14_v19::{DocumentV14, DocumentV15, DocumentV16, DocumentV19};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiClipDto {
    id: u64,
    start_raw: u64,
    length_raw: u64,
    notes: Vec<MidiNoteDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiClipDtoV10 {
    id: u64,
    start_raw: u64,
    length_raw: u64,
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    notes: Vec<MidiNoteDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiNoteDto {
    id: u64,
    start_raw: u64,
    duration_raw: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TempoPointDto {
    beat_raw: u64,
    micro_bpm: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeterPointDto {
    beat_raw: u64,
    numerator: u8,
    denominator: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopRegionDto {
    start: u64,
    end: u64,
    enabled: bool,
}

fn deserialize_required_loop_region<'de, D>(
    deserializer: D,
) -> Result<Option<LoopRegionDto>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<LoopRegionDto>::deserialize(deserializer)
}

fn deserialize_required_instrument_position<'de, D>(
    deserializer: D,
) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<usize>::deserialize(deserializer)
}

fn path_to_string(path: std::path::PathBuf) -> Result<String, StorageError> {
    path.into_os_string().into_string().map_err(|_| {
        StorageError::Invalid("project path is not utf-8; refusing lossy serialization")
    })
}

fn default_tempo_points() -> Vec<TempoPoint> {
    vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)]
}

fn default_meter_points() -> Vec<MeterPoint> {
    vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)]
}
