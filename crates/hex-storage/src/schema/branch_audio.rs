//! Strict issue27 development20/21 readers; never used for main envelopes.
use super::musical_audio::{AudioState, overlay};
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchDocument {
    format: String,
    version: u32,
    sample_rate: u32,
    next_id: u64,
    assets: Vec<AssetV2>,
    tracks: Vec<BranchTrack>,
    return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    loop_region: Option<LoopRegionDto>,
    master_gain: f32,
    tempo_points: Vec<TempoPointDto>,
    meter_points: Vec<MeterPointDto>,
    session: BranchSession,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchTrack {
    id: u64,
    name: String,
    gain: f32,
    balance: f32,
    muted: bool,
    soloed: bool,
    audio_clips: Vec<BranchClip>,
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
struct BranchClip {
    #[serde(default, deserialize_with = "deserialize_audio_edit")]
    audio_edit: Option<Option<crate::audio_edit::AudioEditDto>>,
    gain_millidb: i32,
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
struct BranchSession {
    scenes: Vec<SceneDtoV11>,
    cells: Vec<BranchCell>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchCell {
    track: u64,
    scene: u64,
    content: BranchContent,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchSessionAudio {
    gain_millidb: i32,
    id: u64,
    name: String,
    asset: u64,
    source_start: u64,
    source_end: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BranchContent {
    Midi { clip: SessionMidiClipDtoV11 },
    Audio { clip: BranchSessionAudio },
    Stop,
}
fn deserialize_audio_edit<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<crate::audio_edit::AudioEditDto>>, D::Error> {
    Option::deserialize(d).map(Some)
}
pub(super) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let doc: BranchDocument = serde_json::from_slice(bytes)?;
    if doc.format != "hex-project" || !matches!(doc.version, 20 | 21) {
        return Err(StorageError::Invalid(
            "unsupported development audio format",
        ));
    }
    let mut states = Vec::new();
    let mut tracks = Vec::new();
    for track in doc.tracks {
        let mut clips = Vec::new();
        for clip in track.audio_clips {
            if (doc.version == 21 && clip.audio_edit.is_none())
                || (doc.version == 20 && clip.audio_edit.is_some())
            {
                return Err(StorageError::Invalid(
                    "development audio state does not match its version",
                ));
            }
            states.push(AudioState {
                clip: clip.id,
                gain_millidb: clip.gain_millidb,
                audio_edit: clip.audio_edit.flatten(),
            });
            clips.push(ClipV7 {
                id: clip.id,
                asset: clip.asset,
                start: clip.start,
                duration: clip.duration,
                source_offset: clip.source_offset,
                source_length: clip.source_length,
                looping: clip.looping,
                source_phase: clip.source_phase,
                fade: clip.fade,
            });
        }
        tracks.push(TrackV17 {
            id: track.id,
            name: track.name,
            gain: track.gain,
            balance: track.balance,
            muted: track.muted,
            soloed: track.soloed,
            audio_clips: clips,
            midi_clips: track.midi_clips,
            devices: track.devices,
            instrument: track.instrument,
            instrument_position: track.instrument_position,
            sends: track.sends,
        });
    }
    let mut cells = Vec::new();
    for cell in doc.session.cells {
        let content = match cell.content {
            BranchContent::Stop => SessionContentDtoV12::Stop,
            BranchContent::Midi { clip } => SessionContentDtoV12::Midi { clip },
            BranchContent::Audio { clip } => {
                states.push(AudioState {
                    clip: clip.id,
                    gain_millidb: clip.gain_millidb,
                    audio_edit: None,
                });
                SessionContentDtoV12::Audio {
                    clip: SessionAudioClipDtoV12 {
                        id: clip.id,
                        name: clip.name,
                        asset: clip.asset,
                        source_start: clip.source_start,
                        source_end: clip.source_end,
                    },
                }
            }
        };
        cells.push(SessionCellDtoV12 {
            track: cell.track,
            scene: cell.scene,
            content,
        });
    }
    let base = DocumentV19 {
        format: doc.format,
        version: 17,
        sample_rate: doc.sample_rate,
        next_id: doc.next_id,
        assets: doc.assets,
        tracks,
        return_tracks: doc.return_tracks,
        loop_region: doc.loop_region,
        master_gain: doc.master_gain,
        tempo_points: doc.tempo_points,
        meter_points: doc.meter_points,
        session: SessionDtoV12 {
            scenes: doc.session.scenes,
            cells,
        },
    };
    let project = DocumentV19::into_project(base)?;
    overlay(project.snapshot(), states)
}
