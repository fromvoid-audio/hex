pub(crate) mod alias_projection_v60;
pub(crate) mod appearance;
mod arrangement_sections;
mod audio_alias_projection;
pub(crate) mod audio_events;
pub(crate) mod automation_read;
pub(crate) mod branch_audio;
pub(crate) mod clip_activation;
pub(crate) mod clip_aliases;
pub(crate) mod clip_names;
pub(crate) mod combined;
pub(crate) mod content_view_resampler;
pub(crate) mod content_views;
pub(crate) mod conversion_v2;
pub(crate) mod crossfades;
pub(crate) mod device_assets;
pub(crate) mod device_automation;
pub(crate) mod event_activation;
pub(crate) mod event_fades;
pub(crate) mod event_gain;
pub(crate) mod fade_curve_positions;
pub(crate) mod fade_shapes;
pub(crate) mod gain_envelopes;
pub(crate) mod gain_touch;
pub(crate) mod grouped_audio;
mod host_gain;
pub(crate) mod legacy;
pub(crate) mod legacy_early;
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
pub(crate) mod playable_routing;
pub(crate) mod provenance;
pub(crate) mod retained_receiver;
pub(crate) mod return_feeds;
pub(crate) mod routing;
pub(crate) mod sampler_crossfade;
pub(crate) mod session_audio_loops;
pub(crate) mod sidechain;
pub(crate) mod track_groups;
pub(crate) mod track_name_origin;
pub(crate) mod tuning;
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV7 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV7>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV8 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV7>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV9 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV9>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV10 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV11 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV11,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV12 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV13 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV13>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV14 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV14>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV14 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV15 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV15>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV16 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV16>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV19 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV17>,
    return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV17 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
    sends: Vec<SendDtoV17>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReturnTrackDtoV17 {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    devices: Vec<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendDtoV17 {
    return_track: u64,
    gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV15 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    instrument: Option<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV16 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
    devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    instrument_position: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceDtoV14 {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    #[serde(default)]
    input_gain_millidb: i32,
    #[serde(default)]
    output_gain_millidb: i32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV13 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<ClipV7>,
    midi_clips: Vec<MidiClipDtoV10>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionDtoV12 {
    scenes: Vec<SceneDtoV11>,
    cells: Vec<SessionCellDtoV12>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCellDtoV12 {
    track: u64,
    scene: u64,
    content: SessionContentDtoV12,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SessionContentDtoV12 {
    Midi { clip: SessionMidiClipDtoV11 },
    Audio { clip: SessionAudioClipDtoV12 },
    Stop,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAudioClipDtoV12 {
    id: u64,
    name: String,
    asset: u64,
    source_start: u64,
    source_end: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionDtoV11 {
    scenes: Vec<SceneDtoV11>,
    cells: Vec<SessionCellDtoV11>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneDtoV11 {
    id: u64,
    name: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCellDtoV11 {
    track: u64,
    scene: u64,
    content: SessionContentDtoV11,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SessionContentDtoV11 {
    Midi { clip: SessionMidiClipDtoV11 },
    Stop,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionMidiClipDtoV11 {
    id: u64,
    name: String,
    source: MidiSourceDtoV11,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MidiSourceDtoV11 {
    source_length_raw: u64,
    source_start_raw: u64,
    loop_start_raw: u64,
    loop_end_raw: u64,
    looping: bool,
    notes: Vec<MidiNoteDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV5 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV4>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV6 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV6>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV4 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV3>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV3 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackV3>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV2 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<TrackLegacy>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetV2 {
    id: u64,
    path: String,
    source_sample_rate: u32,
    source_frames: u64,
    playback_frames: u64,
    conversion_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV1 {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV1>,
    tracks: Vec<TrackLegacy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetV1 {
    id: u64,
    path: String,
    frames: u64,
}

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackLegacy {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV3 {
    id: u64,
    name: String,
    gain: f32,
    muted: bool,
    soloed: bool,
    clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV4 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV6 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    clips: Vec<ClipV6>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV7 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    clips: Vec<ClipV7>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV9 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    content: ContentV9,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV10 {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    content: ContentV10,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ContentV9 {
    Audio {
        clips: Vec<ClipV7>,
    },
    Instrument {
        loop_policy: String,
        clips: Vec<MidiClipDto>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ContentV10 {
    Audio { clips: Vec<ClipV7> },
    Instrument { clips: Vec<MidiClipDtoV10> },
}

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

fn deserialize_required_instrument<'de, D>(
    deserializer: D,
) -> Result<Option<DeviceDtoV14>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<DeviceDtoV14>::deserialize(deserializer)
}

impl DocumentV14 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV13::from_project(project)?;
        let tracks = base
            .tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, authored)| -> Result<TrackV14, StorageError> {
                let devices = authored
                    .devices()
                    .iter()
                    .map(|device| {
                        let state = device.state();
                        Ok(DeviceDtoV14 {
                            id: device.id().value(),
                            bypassed: device.bypassed(),
                            type_id: state
                                .type_id()
                                .as_str()
                                .map_err(|_| {
                                    StorageError::Invalid("invalid native device type id")
                                })?
                                .to_owned(),
                            state_version: state.state_version(),
                            payload: state.payload().to_vec(),
                            input_gain_millidb: state.input_gain_millidb(),
                            output_gain_millidb: state.output_gain_millidb(),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Ok(TrackV14 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                    devices,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Self {
            format: base.format,
            version: 14,
            sample_rate: base.sample_rate,
            next_id: base.next_id,
            assets: base.assets,
            tracks,
            loop_region: base.loop_region,
            master_gain: base.master_gain,
            tempo_points: base.tempo_points,
            meter_points: base.meter_points,
            session: base.session,
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 14 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let mut device_rows = Vec::with_capacity(doc.tracks.len());
        let tracks = doc
            .tracks
            .into_iter()
            .map(|track| {
                let devices = track
                    .devices
                    .into_iter()
                    .map(device_dto_into_snapshot)
                    .collect::<Result<Vec<_>, _>>()?;
                device_rows.push((track.id, devices));
                Ok(TrackV13 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let legacy = DocumentV13 {
            format: doc.format,
            version: 13,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
        };
        let bytes = serde_json::to_vec(&legacy)?;
        let mut snapshot = DocumentV13::decode(&bytes)?.snapshot();
        for track in &mut snapshot.tracks {
            if let Some((_, devices)) = device_rows.iter().find(|(id, _)| *id == track.id) {
                track.devices = devices.clone();
            }
        }
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV15 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV14::from_project(project)?;
        let DocumentV14 {
            format,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
            ..
        } = base;
        let tracks = tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, authored)| {
                let instrument = authored
                    .instrument()
                    .map(|device| {
                        device_to_dto(device.id().value(), device.bypassed(), device.state())
                    })
                    .transpose()?;
                Ok(TrackV15 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                    devices: track.devices,
                    instrument,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Self {
            format,
            version: 15,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 15 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let mut instrument_rows = Vec::with_capacity(doc.tracks.len());
        let tracks = doc
            .tracks
            .into_iter()
            .map(|track| {
                let instrument = track.instrument.map(device_dto_into_snapshot).transpose()?;
                instrument_rows.push((track.id, instrument));
                Ok(TrackV14 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                    devices: track.devices,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let legacy = DocumentV14 {
            format: doc.format,
            version: 14,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
        };
        let legacy_bytes = serde_json::to_vec(&legacy)?;
        let project = DocumentV14::decode(&legacy_bytes)?;
        let mut snapshot = project.snapshot();
        for track in &mut snapshot.tracks {
            track.instrument = instrument_rows
                .iter()
                .find(|(id, _)| *id == track.id)
                .and_then(|(_, instrument)| instrument.clone());
            track.instrument_position = track.instrument.as_ref().map(|_| 0);
        }
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV16 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV15::from_project(project)?;
        let DocumentV15 {
            format,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
            ..
        } = base;
        let tracks = tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, authored)| TrackV16 {
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                audio_clips: track.audio_clips,
                midi_clips: track.midi_clips,
                devices: track.devices,
                instrument: track.instrument,
                instrument_position: authored.instrument_position(),
            })
            .collect();
        Ok(Self {
            format,
            version: 16,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 16 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let positions = doc
            .tracks
            .iter()
            .map(|track| track.instrument_position)
            .collect::<Vec<_>>();
        let legacy = DocumentV15 {
            format: doc.format,
            version: 15,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks: doc
                .tracks
                .into_iter()
                .map(|track| TrackV15 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                    devices: track.devices,
                    instrument: track.instrument,
                })
                .collect(),
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
        };
        let bytes = serde_json::to_vec(&legacy)?;
        let project = DocumentV15::decode(&bytes)?;
        let mut snapshot = project.snapshot();
        for (track, position) in snapshot.tracks.iter_mut().zip(positions) {
            track.instrument_position = position;
        }
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV19 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let base = DocumentV16::from_project(project)?;
        let DocumentV16 {
            format,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
            ..
        } = base;
        let tracks = tracks
            .into_iter()
            .zip(project.tracks())
            .map(|(track, authored)| TrackV17 {
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                audio_clips: track.audio_clips,
                midi_clips: track.midi_clips,
                devices: track.devices,
                instrument: track.instrument,
                instrument_position: track.instrument_position,
                sends: authored
                    .sends()
                    .iter()
                    .map(|send| SendDtoV17 {
                        return_track: send.return_track().value(),
                        gain: send.gain(),
                    })
                    .collect(),
            })
            .collect();
        let return_tracks = project
            .return_tracks()
            .iter()
            .map(|track| {
                Ok(ReturnTrackDtoV17 {
                    id: track.id().value(),
                    name: track.name().to_owned(),
                    gain: track.gain(),
                    muted: track.muted(),
                    devices: track
                        .devices()
                        .iter()
                        .map(|device| {
                            device_to_dto(device.id().value(), device.bypassed(), device.state())
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Self {
            format,
            version: 19,
            sample_rate,
            next_id,
            assets,
            tracks,
            return_tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        Self::into_project(doc)
    }
    fn into_project(doc: Self) -> Result<Project, StorageError> {
        if doc.format != "hex-project" || !matches!(doc.version, 17..=19) {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let mut sends = Vec::with_capacity(doc.tracks.len());
        let mut positions = Vec::with_capacity(doc.tracks.len());
        let tracks = doc
            .tracks
            .into_iter()
            .map(|track| {
                positions.push(track.instrument_position);
                sends.push((track.id, track.sends));
                TrackV16 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: track.audio_clips,
                    midi_clips: track.midi_clips,
                    devices: track.devices,
                    instrument: track.instrument,
                    instrument_position: track.instrument_position,
                }
            })
            .collect();
        let return_tracks = doc
            .return_tracks
            .into_iter()
            .map(|track| {
                Ok(ReturnTrackSnapshot {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    muted: track.muted,
                    devices: track
                        .devices
                        .into_iter()
                        .map(device_dto_into_snapshot)
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let legacy = DocumentV16 {
            format: doc.format,
            version: 16,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
            session: doc.session,
        };
        let legacy_bytes = serde_json::to_vec(&legacy)?;
        let project = DocumentV16::decode(&legacy_bytes)?;
        let mut snapshot = project.snapshot();
        snapshot.return_tracks = return_tracks;
        for (snapshot_track, track_sends) in snapshot.tracks.iter_mut().zip(sends) {
            if snapshot_track.id != track_sends.0 {
                return Err(StorageError::Invalid("track order changed during decode"));
            }
            snapshot_track.sends = track_sends
                .1
                .into_iter()
                .map(|send| SendLevelSnapshot {
                    return_track: send.return_track,
                    gain: send.gain,
                })
                .collect();
        }
        for (track, position) in snapshot.tracks.iter_mut().zip(positions) {
            track.instrument_position = position;
        }
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

fn device_to_dto(
    id: u64,
    bypassed: bool,
    state: &hex_lattice_contract::NativeDeviceState,
) -> Result<DeviceDtoV14, StorageError> {
    Ok(DeviceDtoV14 {
        id,
        bypassed,
        type_id: state
            .type_id()
            .as_str()
            .map_err(|_| StorageError::Invalid("invalid native device type id"))?
            .to_owned(),
        state_version: state.state_version(),
        payload: state.payload().to_vec(),
        input_gain_millidb: state.input_gain_millidb(),
        output_gain_millidb: state.output_gain_millidb(),
    })
}

fn device_dto_into_snapshot(device: DeviceDtoV14) -> Result<DeviceSnapshot, StorageError> {
    let type_id = hex_lattice_contract::DeviceTypeId::new(&device.type_id)
        .map_err(|_| StorageError::Invalid("invalid native device type id"))?;
    let state =
        hex_lattice_contract::NativeDeviceState::new(type_id, device.state_version, device.payload)
            .map_err(|_| StorageError::Invalid("invalid native device state envelope"))?;
    let state = state
        .with_input_gain_millidb(device.input_gain_millidb)
        .map_err(|_| StorageError::Invalid("invalid device input gain"))?
        .with_output_gain_millidb(device.output_gain_millidb)
        .map_err(|_| StorageError::Invalid("invalid device output gain"))?;
    Ok(DeviceSnapshot {
        automation: Vec::new(),
        id: device.id,
        bypassed: device.bypassed,
        comparison: None,
        state,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Clip {
    id: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipV6 {
    id: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
    fade: ClipFadeDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipV7 {
    id: u64,
    asset: u64,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
    looping: bool,
    source_phase: u64,
    fade: ClipFadeDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipFadeDto {
    span: u64,
    offset: u64,
    fade_in: u64,
    fade_out: u64,
}

impl DocumentV8 {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let header: Header = serde_json::from_slice(bytes)?;
        if header.format != "hex-project" {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let snapshot = match header.version {
            1 => {
                let document: DocumentV1 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 1 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let sample_rate = document.sample_rate;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: sample_rate,
                            source_frames: asset.frames,
                            playback_frames: asset.frames,
                            conversion_version: IDENTITY_CONVERSION,
                        })
                        .collect(),
                    tracks: tracks_legacy_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            2 => {
                let document: DocumentV2 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 2 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_legacy_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            3 => {
                let document: DocumentV3 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 3 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v3_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            4 => {
                let document: DocumentV4 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 4 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let loop_region = document
                    .loop_region
                    .map(|region| {
                        hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                            .map_err(|error| StorageError::Invalid(error.0))
                    })
                    .transpose()?;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v3_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            5 => {
                let document: DocumentV5 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 5 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let loop_region = document
                    .loop_region
                    .map(|region| {
                        hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                            .map_err(|error| StorageError::Invalid(error.0))
                    })
                    .transpose()?;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v4_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            6 => {
                let document: DocumentV6 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 6 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let loop_region = document
                    .loop_region
                    .map(|region| {
                        hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                            .map_err(|error| StorageError::Invalid(error.0))
                    })
                    .transpose()?;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v6_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            7 => {
                let document: DocumentV7 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 7 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let loop_region = document
                    .loop_region
                    .map(|region| {
                        hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                            .map_err(|error| StorageError::Invalid(error.0))
                    })
                    .transpose()?;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v7_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                    return_tracks: Vec::new(),
                }
            }
            8 => {
                let document: DocumentV8 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 8 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                let loop_region = document
                    .loop_region
                    .map(|region| {
                        hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                            .map_err(|error| StorageError::Invalid(error.0))
                    })
                    .transpose()?;
                let tempo_points = document
                    .tempo_points
                    .into_iter()
                    .map(|point| {
                        let tempo = Tempo::new(point.micro_bpm)
                            .map_err(|error| StorageError::Invalid(error.0))?;
                        Ok(TempoPoint::new(BeatTime::from_raw(point.beat_raw), tempo))
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let meter_points = document
                    .meter_points
                    .into_iter()
                    .map(|point| {
                        let meter = Meter::new(point.numerator, point.denominator)
                            .map_err(|error| StorageError::Invalid(error.0))?;
                        Ok(MeterPoint::new(BeatTime::from_raw(point.beat_raw), meter))
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: document
                        .assets
                        .into_iter()
                        .map(|asset| AssetSnapshot {
                            original_source: None,
                            id: asset.id,
                            path: asset.path.into(),
                            source_sample_rate: asset.source_sample_rate,
                            source_frames: asset.source_frames,
                            playback_frames: asset.playback_frames,
                            conversion_version: asset.conversion_version,
                        })
                        .collect(),
                    tracks: tracks_v7_into_snapshot(document.tracks)?,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: document.master_gain,
                    tempo_points,
                    meter_points,
                    return_tracks: Vec::new(),
                }
            }
            _ => {
                return Err(StorageError::Invalid(
                    "unsupported project format or version",
                ));
            }
        };
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV10 {
    #[allow(dead_code)] // retained for schema-10/12 fixture construction
    fn from_snapshot(snapshot: Snapshot) -> Result<Self, StorageError> {
        let assets = snapshot
            .assets
            .into_iter()
            .map(|asset| {
                Ok(AssetV2 {
                    id: asset.id,
                    path: path_to_string(asset.path)?,
                    source_sample_rate: asset.source_sample_rate,
                    source_frames: asset.source_frames,
                    playback_frames: asset.playback_frames,
                    conversion_version: asset.conversion_version,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let tracks = snapshot
            .tracks
            .into_iter()
            .map(|track| {
                let content = match track.content {
                    TrackContentSnapshot::Audio(clips) => ContentV10::Audio {
                        clips: clips.into_iter().map(clip_v7_from_snapshot).collect(),
                    },
                    TrackContentSnapshot::Instrument(clips) => ContentV10::Instrument {
                        clips: clips.into_iter().map(midi_v10_from_snapshot).collect(),
                    },
                    TrackContentSnapshot::Hybrid {
                        audio_clips,
                        midi_clips,
                    } => match (audio_clips.is_empty(), midi_clips.is_empty()) {
                        (false, true) | (true, true) => ContentV10::Audio {
                            clips: audio_clips.into_iter().map(clip_v7_from_snapshot).collect(),
                        },
                        (true, false) => ContentV10::Instrument {
                            clips: midi_clips.into_iter().map(midi_v10_from_snapshot).collect(),
                        },
                        (false, false) => {
                            return Err(StorageError::Invalid(
                                "schema 10 cannot encode mixed audio and midi track content",
                            ));
                        }
                    },
                };
                Ok(TrackV10 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    content,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(Self {
            format: "hex-project".into(),
            version: 10,
            sample_rate: snapshot.sample_rate,
            next_id: snapshot.next_id,
            assets,
            tracks,
            loop_region: snapshot.loop_region.map(|r| LoopRegionDto {
                start: r.start(),
                end: r.end(),
                enabled: r.enabled(),
            }),
            master_gain: snapshot.master_gain,
            tempo_points: snapshot
                .tempo_points
                .into_iter()
                .map(|p| TempoPointDto {
                    beat_raw: p.beat().raw(),
                    micro_bpm: p.tempo().micro_bpm(),
                })
                .collect(),
            meter_points: snapshot
                .meter_points
                .into_iter()
                .map(|p| MeterPointDto {
                    beat_raw: p.beat().raw(),
                    numerator: p.meter().numerator(),
                    denominator: p.meter().denominator(),
                })
                .collect(),
        })
    }
}

impl DocumentV12 {
    #[allow(dead_code)] // retained for legacy schema-12 fixture construction
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let mut snapshot = project.snapshot();
        let session_snapshot = std::mem::take(&mut snapshot.session);
        let base = DocumentV10::from_snapshot(snapshot)?;
        let cells = session_snapshot
            .cells
            .into_iter()
            .map(|cell| SessionCellDtoV12 {
                track: cell.track,
                scene: cell.scene,
                content: match cell.content {
                    SessionCellContentSnapshot::Stop => SessionContentDtoV12::Stop,
                    SessionCellContentSnapshot::Midi(clip) => SessionContentDtoV12::Midi {
                        clip: SessionMidiClipDtoV11 {
                            id: clip.id,
                            name: clip.name,
                            source: MidiSourceDtoV11 {
                                source_length_raw: clip.source.source_length_raw,
                                source_start_raw: clip.source.source_start_raw,
                                loop_start_raw: clip.source.loop_start_raw,
                                loop_end_raw: clip.source.loop_end_raw,
                                looping: clip.source.looping,
                                notes: clip
                                    .source
                                    .notes
                                    .into_iter()
                                    .map(|note| MidiNoteDto {
                                        id: note.id,
                                        start_raw: note.start_raw,
                                        duration_raw: note.duration_raw,
                                        pitch: note.pitch,
                                        channel: note.channel,
                                        velocity: note.velocity,
                                    })
                                    .collect(),
                            },
                        },
                    },
                    SessionCellContentSnapshot::Audio(clip) => SessionContentDtoV12::Audio {
                        clip: SessionAudioClipDtoV12 {
                            id: clip.id,
                            name: clip.name,
                            asset: clip.asset,
                            source_start: clip.source_start,
                            source_end: clip.source_end,
                        },
                    },
                },
            })
            .collect();
        Ok(Self {
            format: base.format,
            version: 12,
            sample_rate: base.sample_rate,
            next_id: base.next_id,
            assets: base.assets,
            tracks: base.tracks,
            loop_region: base.loop_region,
            master_gain: base.master_gain,
            tempo_points: base.tempo_points,
            meter_points: base.meter_points,
            session: SessionDtoV12 {
                scenes: session_snapshot
                    .scenes
                    .into_iter()
                    .map(|scene| SceneDtoV11 {
                        id: scene.id,
                        name: scene.name,
                    })
                    .collect(),
                cells,
            },
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 12 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let v10 = DocumentV10 {
            format: doc.format,
            version: 10,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc.assets,
            tracks: doc.tracks,
            loop_region: doc.loop_region,
            master_gain: doc.master_gain,
            tempo_points: doc.tempo_points,
            meter_points: doc.meter_points,
        };
        let legacy_bytes = serde_json::to_vec(&v10)?;
        let mut snapshot = DocumentV10::decode(&legacy_bytes)?.snapshot();
        snapshot.session = SessionSnapshot {
            scenes: doc
                .session
                .scenes
                .into_iter()
                .map(|scene| SceneSnapshot {
                    id: scene.id,
                    name: scene.name,
                })
                .collect(),
            cells: doc
                .session
                .cells
                .into_iter()
                .map(|cell| SessionCellSnapshot {
                    track: cell.track,
                    scene: cell.scene,
                    content: match cell.content {
                        SessionContentDtoV12::Stop => SessionCellContentSnapshot::Stop,
                        SessionContentDtoV12::Audio { clip } => SessionCellContentSnapshot::Audio(
                            hex_project::SessionAudioClipSnapshot {
                                loop_range: None,
                                active: true,
                                flavor: hex_project::ClipFlavor::default(),
                                gain: hex_project::ClipGain::UNITY,
                                id: clip.id,
                                name: clip.name,
                                asset: clip.asset,
                                source_start: clip.source_start,
                                source_end: clip.source_end,
                            },
                        ),
                        SessionContentDtoV12::Midi { clip } => {
                            SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                                active: true,
                                playback_offset_raw: 0,
                                volume: hex_project::MidiClipVolume::default(),
                                flavor: hex_project::ClipFlavor::default(),
                                id: clip.id,
                                name: clip.name,
                                source: MidiSourceSnapshot {
                                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                                    source_length_raw: clip.source.source_length_raw,
                                    source_start_raw: clip.source.source_start_raw,
                                    loop_start_raw: clip.source.loop_start_raw,
                                    loop_end_raw: clip.source.loop_end_raw,
                                    looping: clip.source.looping,
                                    notes: clip
                                        .source
                                        .notes
                                        .into_iter()
                                        .map(|note| MidiNoteSnapshot {
                                            active: true,
                                            id: note.id,
                                            start_raw: note.start_raw,
                                            duration_raw: note.duration_raw,
                                            pitch: note.pitch,
                                            channel: note.channel,
                                            velocity: note.velocity,
                                        })
                                        .collect(),
                                },
                            })
                        }
                    },
                })
                .collect(),
        };
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV13 {
    pub(crate) fn from_project(project: &Project) -> Result<Self, StorageError> {
        let snapshot = project.snapshot();
        let assets = snapshot
            .assets
            .into_iter()
            .map(|asset| {
                Ok(AssetV2 {
                    id: asset.id,
                    path: path_to_string(asset.path)?,
                    source_sample_rate: asset.source_sample_rate,
                    source_frames: asset.source_frames,
                    playback_frames: asset.playback_frames,
                    conversion_version: asset.conversion_version,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let tracks = snapshot
            .tracks
            .into_iter()
            .map(|track| {
                let (audio, midi) = match track.content {
                    TrackContentSnapshot::Audio(audio) => (audio, Vec::new()),
                    TrackContentSnapshot::Instrument(midi) => (Vec::new(), midi),
                    TrackContentSnapshot::Hybrid {
                        audio_clips,
                        midi_clips,
                    } => (audio_clips, midi_clips),
                };
                Ok(TrackV13 {
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    audio_clips: audio.into_iter().map(clip_v7_from_snapshot).collect(),
                    midi_clips: midi.into_iter().map(midi_v10_from_snapshot).collect(),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let session = session_snapshot_to_v12(snapshot.session);
        Ok(Self {
            format: "hex-project".into(),
            version: 13,
            sample_rate: snapshot.sample_rate,
            next_id: snapshot.next_id,
            assets,
            tracks,
            loop_region: snapshot.loop_region.map(|r| LoopRegionDto {
                start: r.start(),
                end: r.end(),
                enabled: r.enabled(),
            }),
            master_gain: snapshot.master_gain,
            tempo_points: snapshot
                .tempo_points
                .into_iter()
                .map(|p| TempoPointDto {
                    beat_raw: p.beat().raw(),
                    micro_bpm: p.tempo().micro_bpm(),
                })
                .collect(),
            meter_points: snapshot
                .meter_points
                .into_iter()
                .map(|p| MeterPointDto {
                    beat_raw: p.beat().raw(),
                    numerator: p.meter().numerator(),
                    denominator: p.meter().denominator(),
                })
                .collect(),
            session,
        })
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        if doc.format != "hex-project" || doc.version != 13 {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        let loop_region = doc
            .loop_region
            .map(|r| {
                hex_project::LoopRegion::new(r.start, r.end, r.enabled)
                    .map_err(|e| StorageError::Invalid(e.0))
            })
            .transpose()?;
        let tempo_points = doc
            .tempo_points
            .into_iter()
            .map(|p| {
                Ok(TempoPoint::new(
                    BeatTime::from_raw(p.beat_raw),
                    Tempo::new(p.micro_bpm).map_err(|e| StorageError::Invalid(e.0))?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let meter_points = doc
            .meter_points
            .into_iter()
            .map(|p| {
                Ok(MeterPoint::new(
                    BeatTime::from_raw(p.beat_raw),
                    Meter::new(p.numerator, p.denominator)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let tracks = doc
            .tracks
            .into_iter()
            .map(|track| {
                Ok(TrackSnapshot {
                    kind: hex_project::TrackKind::Ordinary,
                    parent_group: None,
                    color: hex_project::ElementColor::default(),
                    gain_envelope: Vec::new(),
                    gain_touch_spans: Vec::new(),
                    gain_touch_epoch: 0,
                    id: track.id,
                    name: track.name,
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    devices: Vec::new(),
                    instrument: None,
                    instrument_position: None,
                    content: TrackContentSnapshot::Hybrid {
                        audio_clips: clips_v7_into_snapshot(track.audio_clips)?,
                        midi_clips: track
                            .midi_clips
                            .into_iter()
                            .map(midi_v10_into_snapshot)
                            .collect(),
                    },
                    sends: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let snapshot = Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            return_feeds: Vec::new(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            import_provenance: None,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc
                .assets
                .into_iter()
                .map(|asset| AssetSnapshot {
                    original_source: None,
                    id: asset.id,
                    path: asset.path.into(),
                    source_sample_rate: asset.source_sample_rate,
                    source_frames: asset.source_frames,
                    playback_frames: asset.playback_frames,
                    conversion_version: asset.conversion_version,
                })
                .collect(),
            tracks,
            session: session_v12_into_snapshot(doc.session),
            loop_region,
            master_gain: doc.master_gain,
            tempo_points,
            meter_points,
            return_tracks: Vec::new(),
        };
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

#[derive(Deserialize)]
struct HistoricalShape {
    #[serde(default, deserialize_with = "present_field")]
    import_provenance: Option<serde::de::IgnoredAny>,
    #[serde(default, deserialize_with = "present_field")]
    routing: Option<serde::de::IgnoredAny>,
}
fn present_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde::de::IgnoredAny>, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(Some)
}

impl DocumentV11 {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let header: Header = serde_json::from_slice(bytes)?;
        if header.format != "hex-project" {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        if header.version == 30 {
            return Err(StorageError::Invalid("unsupported alpha project format 30"));
        }
        if header.version == 31 {
            return Err(StorageError::Invalid("unsupported alpha project format 31"));
        }
        if header.version == 32 {
            return Err(StorageError::Invalid("unsupported alpha project format 32"));
        }
        if header.version == 33 {
            return Err(StorageError::Invalid("unsupported alpha project format 33"));
        }
        if header.version == 37 {
            return tuning::decode(bytes);
        }
        if header.version == 29 {
            return fade_shapes::DocumentV29::decode(bytes);
        }
        if header.version == 28 {
            return audio_events::decode(bytes);
        }
        if header.version == 26 {
            return clip_names::DocumentV26::decode(bytes);
        }
        if header.version == 27 {
            let shape: serde_json::Value = serde_json::from_slice(bytes)?;
            return if shape.get("audio_event_state").is_some() {
                audio_events::decode_legacy(bytes)
            } else {
                appearance::DocumentV27::decode(bytes)
            };
        }
        if header.version == 25 {
            return musical_audio::DocumentV25::decode(bytes);
        }
        if matches!(header.version, 20 | 21) {
            let shape: HistoricalShape = serde_json::from_slice(bytes)?;
            match (shape.import_provenance.is_some(), shape.routing.is_some()) {
                (false, false) => return branch_audio::decode(bytes),
                (true, true) => {}
                _ => {
                    return Err(StorageError::Invalid(
                        "inconsistent historical project envelope",
                    ));
                }
            }
        }
        if header.version == 24 {
            return playable_routing::DocumentV24::decode(bytes);
        }
        if header.version == 23 {
            return host_gain::decode_historical(bytes, 23);
        }
        if header.version == 22 {
            return host_gain::decode_historical(bytes, 22);
        }
        if header.version == 21 {
            return gain_envelopes::DocumentV21::decode(bytes);
        }
        if header.version == 20 {
            return return_feeds::DocumentV20::decode(bytes);
        }
        if matches!(header.version, 18 | 19) {
            // These versions were written by independent development branches.
            // Select by explicit field presence, never by catching a parse error.
            let shape: HistoricalShape = serde_json::from_slice(bytes)?;
            match (shape.import_provenance.is_some(), shape.routing.is_some()) {
                (false, false) => return DocumentV19::decode(bytes),
                (true, false) if header.version == 18 => {
                    return provenance::DocumentV18::decode(bytes);
                }
                (true, true) if header.version == 19 => {
                    return routing::DocumentV19::decode(bytes);
                }
                _ => {
                    return Err(StorageError::Invalid(
                        "inconsistent historical project envelope",
                    ));
                }
            }
        }
        if header.version == 17 {
            return DocumentV19::decode(bytes);
        }
        if header.version == 16 {
            return DocumentV16::decode(bytes);
        }
        if header.version == 15 {
            return DocumentV15::decode(bytes);
        }
        let mut project = Self::decode_legacy(bytes)?;
        project
            .install_legacy_default_instruments()
            .map_err(|error| StorageError::Invalid(error.0))?;
        Ok(project)
    }

    fn decode_legacy(bytes: &[u8]) -> Result<Project, StorageError> {
        let header: Header = serde_json::from_slice(bytes)?;
        if header.format != "hex-project" {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        if header.version == 14 {
            return DocumentV14::decode(bytes);
        }
        if header.version == 13 {
            return DocumentV13::decode(bytes);
        }
        if header.version == 12 {
            return DocumentV12::decode(bytes);
        }
        if header.version != 11 {
            return DocumentV10::decode(bytes);
        }
        let doc: Self = serde_json::from_slice(bytes)?;
        let Self {
            format,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
            session,
            ..
        } = doc;
        let v10 = DocumentV10 {
            format,
            version: 10,
            sample_rate,
            next_id,
            assets,
            tracks,
            loop_region,
            master_gain,
            tempo_points,
            meter_points,
        };
        let legacy_bytes = serde_json::to_vec(&v10)?;
        let mut snapshot = DocumentV10::decode(&legacy_bytes)?.snapshot();
        snapshot.session = SessionSnapshot {
            scenes: session
                .scenes
                .into_iter()
                .map(|scene| SceneSnapshot {
                    id: scene.id,
                    name: scene.name,
                })
                .collect(),
            cells: session
                .cells
                .into_iter()
                .map(|cell| SessionCellSnapshot {
                    track: cell.track,
                    scene: cell.scene,
                    content: match cell.content {
                        SessionContentDtoV11::Stop => SessionCellContentSnapshot::Stop,
                        SessionContentDtoV11::Midi { clip } => {
                            SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                                active: true,
                                playback_offset_raw: 0,
                                volume: hex_project::MidiClipVolume::default(),
                                flavor: hex_project::ClipFlavor::default(),
                                id: clip.id,
                                name: clip.name,
                                source: MidiSourceSnapshot {
                                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                                    source_length_raw: clip.source.source_length_raw,
                                    source_start_raw: clip.source.source_start_raw,
                                    loop_start_raw: clip.source.loop_start_raw,
                                    loop_end_raw: clip.source.loop_end_raw,
                                    looping: clip.source.looping,
                                    notes: clip
                                        .source
                                        .notes
                                        .into_iter()
                                        .map(|note| MidiNoteSnapshot {
                                            active: true,
                                            id: note.id,
                                            start_raw: note.start_raw,
                                            duration_raw: note.duration_raw,
                                            pitch: note.pitch,
                                            channel: note.channel,
                                            velocity: note.velocity,
                                        })
                                        .collect(),
                                },
                            })
                        }
                    },
                })
                .collect(),
        };
        Project::from_legacy_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
    }
}

impl DocumentV10 {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let header: Header = serde_json::from_slice(bytes)?;
        if header.format != "hex-project" {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        match header.version {
            10 => {
                let doc: Self = serde_json::from_slice(bytes)?;
                let loop_region = doc
                    .loop_region
                    .map(|r| {
                        hex_project::LoopRegion::new(r.start, r.end, r.enabled)
                            .map_err(|e| StorageError::Invalid(e.0))
                    })
                    .transpose()?;
                let tempo_points = doc
                    .tempo_points
                    .into_iter()
                    .map(|p| {
                        Ok(TempoPoint::new(
                            BeatTime::from_raw(p.beat_raw),
                            Tempo::new(p.micro_bpm).map_err(|e| StorageError::Invalid(e.0))?,
                        ))
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let meter_points = doc
                    .meter_points
                    .into_iter()
                    .map(|p| {
                        Ok(MeterPoint::new(
                            BeatTime::from_raw(p.beat_raw),
                            Meter::new(p.numerator, p.denominator)
                                .map_err(|e| StorageError::Invalid(e.0))?,
                        ))
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let tracks = doc
                    .tracks
                    .into_iter()
                    .map(|t| {
                        let content = match t.content {
                            ContentV10::Audio { clips } => {
                                TrackContentSnapshot::Audio(clips_v7_into_snapshot(clips)?)
                            }
                            ContentV10::Instrument { clips } => TrackContentSnapshot::Instrument(
                                clips
                                    .into_iter()
                                    .map(|c| MidiClipSnapshot {
                                        content_view: None,
                                        active: true,
                                        playback_offset_raw: 0,
                                        one_shot_boundary:
                                            hex_project::MidiOneShotBoundary::SourceEnd,
                                        volume: hex_project::MidiClipVolume::default(),
                                        name: "clip".into(),
                                        flavor: hex_project::ClipFlavor::default(),
                                        id: c.id,
                                        start_raw: c.start_raw,
                                        length_raw: c.length_raw,
                                        source_length_raw: c.source_length_raw,
                                        source_start_raw: c.source_start_raw,
                                        loop_start_raw: c.loop_start_raw,
                                        loop_end_raw: c.loop_end_raw,
                                        looping: c.looping,
                                        notes: c
                                            .notes
                                            .into_iter()
                                            .map(|n| MidiNoteSnapshot {
                                                active: true,
                                                id: n.id,
                                                start_raw: n.start_raw,
                                                duration_raw: n.duration_raw,
                                                pitch: n.pitch,
                                                channel: n.channel,
                                                velocity: n.velocity,
                                            })
                                            .collect(),
                                    })
                                    .collect(),
                            ),
                        };
                        Ok(TrackSnapshot {
                            kind: hex_project::TrackKind::Ordinary,
                            parent_group: None,
                            color: hex_project::ElementColor::default(),
                            gain_envelope: Vec::new(),
                            gain_touch_spans: Vec::new(),
                            gain_touch_epoch: 0,
                            id: t.id,
                            name: t.name,
                            gain: t.gain,
                            balance: t.balance,
                            muted: t.muted,
                            soloed: t.soloed,
                            devices: Vec::new(),
                            instrument: None,
                            instrument_position: None,
                            content,
                            sends: Vec::new(),
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                let snapshot = Snapshot {
                    automatic_track_names: Vec::new(),
                    retained_source_paths: Vec::new(),
                    aliases: Default::default(),
                    native_sources: Vec::new(),
                    arrangement_sections: Vec::new(),
                    main_devices: Vec::new(),
                    tuning: Default::default(),
                    return_feeds: Vec::new(),
                    audio_feeds: Vec::new(),
                    midi_routes: Vec::new(),
                    sidechains: Vec::new(),
                    import_provenance: None,
                    sample_rate: doc.sample_rate,
                    next_id: doc.next_id,
                    assets: doc
                        .assets
                        .into_iter()
                        .map(|a| AssetSnapshot {
                            original_source: None,
                            id: a.id,
                            path: a.path.into(),
                            source_sample_rate: a.source_sample_rate,
                            source_frames: a.source_frames,
                            playback_frames: a.playback_frames,
                            conversion_version: a.conversion_version,
                        })
                        .collect(),
                    tracks,
                    session: SessionSnapshot::default(),
                    loop_region,
                    master_gain: doc.master_gain,
                    tempo_points,
                    meter_points,
                    return_tracks: Vec::new(),
                };
                Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
            }
            _ => DocumentV9::decode(bytes),
        }
    }
}

impl DocumentV9 {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
        let header: Header = serde_json::from_slice(bytes)?;
        if header.format != "hex-project" {
            return Err(StorageError::Invalid(
                "unsupported project format or version",
            ));
        }
        if header.version != 9 {
            return DocumentV8::decode(bytes);
        }
        let doc: Self = serde_json::from_slice(bytes)?;
        let loop_region = doc
            .loop_region
            .map(|r| {
                hex_project::LoopRegion::new(r.start, r.end, r.enabled)
                    .map_err(|e| StorageError::Invalid(e.0))
            })
            .transpose()?;
        let tempo_points = doc
            .tempo_points
            .into_iter()
            .map(|p| {
                Ok(TempoPoint::new(
                    BeatTime::from_raw(p.beat_raw),
                    Tempo::new(p.micro_bpm).map_err(|e| StorageError::Invalid(e.0))?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let meter_points = doc
            .meter_points
            .into_iter()
            .map(|p| {
                Ok(MeterPoint::new(
                    BeatTime::from_raw(p.beat_raw),
                    Meter::new(p.numerator, p.denominator)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                ))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let tracks = doc
            .tracks
            .into_iter()
            .map(|t| {
                let content = match t.content {
                    ContentV9::Audio { clips } => {
                        TrackContentSnapshot::Audio(clips_v7_into_snapshot(clips)?)
                    }
                    ContentV9::Instrument { loop_policy, clips } => {
                        if loop_policy != "one_shot" {
                            return Err(StorageError::Invalid("unsupported midi clip loop policy"));
                        }
                        TrackContentSnapshot::Instrument(
                            clips
                                .into_iter()
                                .map(|c| MidiClipSnapshot {
                                    content_view: None,
                                    active: true,
                                    playback_offset_raw: 0,
                                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                                    volume: hex_project::MidiClipVolume::default(),
                                    name: "clip".into(),
                                    flavor: hex_project::ClipFlavor::default(),
                                    id: c.id,
                                    start_raw: c.start_raw,
                                    length_raw: c.length_raw,
                                    source_length_raw: c.length_raw,
                                    source_start_raw: 0,
                                    loop_start_raw: 0,
                                    loop_end_raw: c.length_raw,
                                    looping: false,
                                    notes: c
                                        .notes
                                        .into_iter()
                                        .map(|n| MidiNoteSnapshot {
                                            active: true,
                                            id: n.id,
                                            start_raw: n.start_raw,
                                            duration_raw: n.duration_raw,
                                            pitch: n.pitch,
                                            channel: n.channel,
                                            velocity: n.velocity,
                                        })
                                        .collect(),
                                })
                                .collect(),
                        )
                    }
                };
                Ok(TrackSnapshot {
                    kind: hex_project::TrackKind::Ordinary,
                    parent_group: None,
                    color: hex_project::ElementColor::default(),
                    gain_envelope: Vec::new(),
                    gain_touch_spans: Vec::new(),
                    gain_touch_epoch: 0,
                    id: t.id,
                    name: t.name,
                    gain: t.gain,
                    balance: t.balance,
                    muted: t.muted,
                    soloed: t.soloed,
                    devices: Vec::new(),
                    instrument: None,
                    instrument_position: None,
                    content,
                    sends: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let snapshot = Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            return_feeds: Vec::new(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            import_provenance: None,
            sample_rate: doc.sample_rate,
            next_id: doc.next_id,
            assets: doc
                .assets
                .into_iter()
                .map(|a| AssetSnapshot {
                    original_source: None,
                    id: a.id,
                    path: a.path.into(),
                    source_sample_rate: a.source_sample_rate,
                    source_frames: a.source_frames,
                    playback_frames: a.playback_frames,
                    conversion_version: a.conversion_version,
                })
                .collect(),
            tracks,
            session: SessionSnapshot::default(),
            loop_region,
            master_gain: doc.master_gain,
            tempo_points,
            meter_points,
            return_tracks: Vec::new(),
        };
        Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
    }
}

fn midi_v10_from_snapshot(clip: MidiClipSnapshot) -> MidiClipDtoV10 {
    MidiClipDtoV10 {
        id: clip.id,
        start_raw: clip.start_raw,
        length_raw: clip.length_raw,
        source_length_raw: clip.source_length_raw,
        source_start_raw: clip.source_start_raw,
        loop_start_raw: clip.loop_start_raw,
        loop_end_raw: clip.loop_end_raw,
        looping: clip.looping,
        notes: clip
            .notes
            .into_iter()
            .map(|note| MidiNoteDto {
                id: note.id,
                start_raw: note.start_raw,
                duration_raw: note.duration_raw,
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            })
            .collect(),
    }
}

fn midi_v10_into_snapshot(clip: MidiClipDtoV10) -> MidiClipSnapshot {
    MidiClipSnapshot {
        content_view: None,
        active: true,
        playback_offset_raw: 0,
        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
        volume: hex_project::MidiClipVolume::default(),
        name: "clip".into(),
        flavor: hex_project::ClipFlavor::default(),
        id: clip.id,
        start_raw: clip.start_raw,
        length_raw: clip.length_raw,
        source_length_raw: clip.source_length_raw,
        source_start_raw: clip.source_start_raw,
        loop_start_raw: clip.loop_start_raw,
        loop_end_raw: clip.loop_end_raw,
        looping: clip.looping,
        notes: clip
            .notes
            .into_iter()
            .map(|note| MidiNoteSnapshot {
                active: true,
                id: note.id,
                start_raw: note.start_raw,
                duration_raw: note.duration_raw,
                pitch: note.pitch,
                channel: note.channel,
                velocity: note.velocity,
            })
            .collect(),
    }
}

fn session_snapshot_to_v12(snapshot: SessionSnapshot) -> SessionDtoV12 {
    SessionDtoV12 {
        scenes: snapshot
            .scenes
            .into_iter()
            .map(|scene| SceneDtoV11 {
                id: scene.id,
                name: scene.name,
            })
            .collect(),
        cells: snapshot
            .cells
            .into_iter()
            .map(|cell| SessionCellDtoV12 {
                track: cell.track,
                scene: cell.scene,
                content: match cell.content {
                    SessionCellContentSnapshot::Stop => SessionContentDtoV12::Stop,
                    SessionCellContentSnapshot::Audio(clip) => SessionContentDtoV12::Audio {
                        clip: SessionAudioClipDtoV12 {
                            id: clip.id,
                            name: clip.name,
                            asset: clip.asset,
                            source_start: clip.source_start,
                            source_end: clip.source_end,
                        },
                    },
                    SessionCellContentSnapshot::Midi(clip) => SessionContentDtoV12::Midi {
                        clip: SessionMidiClipDtoV11 {
                            id: clip.id,
                            name: clip.name,
                            source: MidiSourceDtoV11 {
                                source_length_raw: clip.source.source_length_raw,
                                source_start_raw: clip.source.source_start_raw,
                                loop_start_raw: clip.source.loop_start_raw,
                                loop_end_raw: clip.source.loop_end_raw,
                                looping: clip.source.looping,
                                notes: clip
                                    .source
                                    .notes
                                    .into_iter()
                                    .map(|note| MidiNoteDto {
                                        id: note.id,
                                        start_raw: note.start_raw,
                                        duration_raw: note.duration_raw,
                                        pitch: note.pitch,
                                        channel: note.channel,
                                        velocity: note.velocity,
                                    })
                                    .collect(),
                            },
                        },
                    },
                },
            })
            .collect(),
    }
}

fn session_v12_into_snapshot(session: SessionDtoV12) -> SessionSnapshot {
    SessionSnapshot {
        scenes: session
            .scenes
            .into_iter()
            .map(|scene| SceneSnapshot {
                id: scene.id,
                name: scene.name,
            })
            .collect(),
        cells: session
            .cells
            .into_iter()
            .map(|cell| SessionCellSnapshot {
                track: cell.track,
                scene: cell.scene,
                content: match cell.content {
                    SessionContentDtoV12::Stop => SessionCellContentSnapshot::Stop,
                    SessionContentDtoV12::Audio { clip } => {
                        SessionCellContentSnapshot::Audio(hex_project::SessionAudioClipSnapshot {
                            loop_range: None,
                            active: true,
                            flavor: hex_project::ClipFlavor::default(),
                            gain: hex_project::ClipGain::UNITY,
                            id: clip.id,
                            name: clip.name,
                            asset: clip.asset,
                            source_start: clip.source_start,
                            source_end: clip.source_end,
                        })
                    }
                    SessionContentDtoV12::Midi { clip } => {
                        SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                            active: true,
                            playback_offset_raw: 0,
                            volume: hex_project::MidiClipVolume::default(),
                            flavor: hex_project::ClipFlavor::default(),
                            id: clip.id,
                            name: clip.name,
                            source: MidiSourceSnapshot {
                                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                                source_length_raw: clip.source.source_length_raw,
                                source_start_raw: clip.source.source_start_raw,
                                loop_start_raw: clip.source.loop_start_raw,
                                loop_end_raw: clip.source.loop_end_raw,
                                looping: clip.source.looping,
                                notes: clip
                                    .source
                                    .notes
                                    .into_iter()
                                    .map(|note| MidiNoteSnapshot {
                                        active: true,
                                        id: note.id,
                                        start_raw: note.start_raw,
                                        duration_raw: note.duration_raw,
                                        pitch: note.pitch,
                                        channel: note.channel,
                                        velocity: note.velocity,
                                    })
                                    .collect(),
                            },
                        })
                    }
                },
            })
            .collect(),
    }
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

fn clip_v7_from_snapshot(clip: ClipSnapshot) -> ClipV7 {
    ClipV7 {
        id: clip.id,
        asset: clip.asset,
        start: clip.start,
        duration: clip.duration,
        source_offset: clip.source_offset,
        source_length: clip.source_length,
        looping: clip.looping,
        source_phase: clip.source_phase,
        fade: ClipFadeDto {
            span: clip.fade.span(),
            offset: clip.fade.offset(),
            fade_in: clip.fade.fade_in(),
            fade_out: clip.fade.fade_out(),
        },
    }
}

fn clips_v7_into_snapshot(clips: Vec<ClipV7>) -> Result<Vec<ClipSnapshot>, StorageError> {
    clips
        .into_iter()
        .map(|clip| {
            Ok(ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: clip.id,
                asset: clip.asset,
                start: clip.start,
                duration: clip.duration,
                source_offset: clip.source_offset,
                source_length: clip.source_length,
                looping: clip.looping,
                source_phase: clip.source_phase,
                fade: ClipFade::new(
                    clip.fade.span,
                    clip.fade.offset,
                    clip.fade.fade_in,
                    clip.fade.fade_out,
                )
                .map_err(|e| StorageError::Invalid(e.0))?,
            })
        })
        .collect()
}

fn tracks_legacy_into_snapshot(
    tracks: Vec<TrackLegacy>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track.id,
                name: track.name,
                gain: track.gain,
                muted: track.muted,
                soloed: false,
                balance: 0.0,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(clips_into_snapshot(track.clips)?),
                sends: Vec::new(),
            })
        })
        .collect()
}

fn tracks_v3_into_snapshot(tracks: Vec<TrackV3>) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track.id,
                name: track.name,
                gain: track.gain,
                muted: track.muted,
                soloed: track.soloed,
                balance: 0.0,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(clips_into_snapshot(track.clips)?),
                sends: Vec::new(),
            })
        })
        .collect()
}

fn tracks_v4_into_snapshot(tracks: Vec<TrackV4>) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(clips_into_snapshot(track.clips)?),
                sends: Vec::new(),
            })
        })
        .collect()
}

fn tracks_v6_into_snapshot(tracks: Vec<TrackV6>) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            let clips = track
                .clips
                .into_iter()
                .map(|clip| {
                    let fade = ClipFade::new(
                        clip.fade.span,
                        clip.fade.offset,
                        clip.fade.fade_in,
                        clip.fade.fade_out,
                    )
                    .map_err(|error| StorageError::Invalid(error.0))?;
                    Ok(ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: clip.id,
                        asset: clip.asset,
                        start: clip.start,
                        duration: clip.duration,
                        source_offset: clip.source_offset,
                        source_length: clip.source_length,
                        looping: clip.looping,
                        source_phase: 0,
                        fade,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            Ok(TrackSnapshot {
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(clips),
                sends: Vec::new(),
            })
        })
        .collect()
}

fn tracks_v7_into_snapshot(tracks: Vec<TrackV7>) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            let clips = track
                .clips
                .into_iter()
                .map(|clip| {
                    let fade = ClipFade::new(
                        clip.fade.span,
                        clip.fade.offset,
                        clip.fade.fade_in,
                        clip.fade.fade_out,
                    )
                    .map_err(|error| StorageError::Invalid(error.0))?;
                    Ok(ClipSnapshot {
                        active: true,
                        name: "clip".into(),
                        flavor: hex_project::ClipFlavor::default(),
                        edit: None,
                        gain: hex_project::ClipGain::UNITY,
                        id: clip.id,
                        asset: clip.asset,
                        start: clip.start,
                        duration: clip.duration,
                        source_offset: clip.source_offset,
                        source_length: clip.source_length,
                        looping: clip.looping,
                        source_phase: clip.source_phase,
                        fade,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            Ok(TrackSnapshot {
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track.id,
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(clips),
                sends: Vec::new(),
            })
        })
        .collect()
}

fn clips_into_snapshot(clips: Vec<Clip>) -> Result<Vec<ClipSnapshot>, StorageError> {
    clips
        .into_iter()
        .map(|clip| {
            let fade = ClipFade::neutral(clip.duration.max(1))
                .map_err(|error| StorageError::Invalid(error.0))?;
            Ok(ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: clip.id,
                asset: clip.asset,
                start: clip.start,
                duration: clip.duration,
                source_offset: clip.source_offset,
                source_length: clip.source_length,
                looping: clip.looping,
                source_phase: 0,
                fade,
            })
        })
        .collect()
}

pub(crate) mod host_automation;
