//! Legacy saved-project schemas V1 through V8 and their migration into `Snapshot`.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV7 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV7>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV8 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV7>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV5 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV4>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV6 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV6>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV4 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV3>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV3 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV3>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV2 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackLegacy>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssetV2 {
    pub(super) id: u64,
    pub(super) path: String,
    pub(super) source_sample_rate: u32,
    pub(super) source_frames: u64,
    pub(super) playback_frames: u64,
    pub(super) conversion_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentV1 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV1>,
    pub(super) tracks: Vec<TrackLegacy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssetV1 {
    pub(super) id: u64,
    pub(super) path: String,
    pub(super) frames: u64,
}

#[derive(Deserialize)]
pub(super) struct Header {
    pub(super) format: String,
    pub(super) version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackLegacy {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) muted: bool,
    pub(super) clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV3 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV4 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV6 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) clips: Vec<ClipV6>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV7 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) clips: Vec<ClipV7>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Clip {
    pub(super) id: u64,
    pub(super) asset: u64,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) source_offset: u64,
    pub(super) source_length: u64,
    pub(super) looping: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClipV6 {
    pub(super) id: u64,
    pub(super) asset: u64,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) source_offset: u64,
    pub(super) source_length: u64,
    pub(super) looping: bool,
    pub(super) fade: ClipFadeDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClipV7 {
    pub(super) id: u64,
    pub(super) asset: u64,
    pub(super) start: u64,
    pub(super) duration: u64,
    pub(super) source_offset: u64,
    pub(super) source_length: u64,
    pub(super) looping: bool,
    pub(super) source_phase: u64,
    pub(super) fade: ClipFadeDto,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClipFadeDto {
    pub(super) span: u64,
    pub(super) offset: u64,
    pub(super) fade_in: u64,
    pub(super) fade_out: u64,
}

fn decode_loop_region(
    loop_region: Option<LoopRegionDto>,
) -> Result<Option<hex_project::LoopRegion>, StorageError> {
    loop_region
        .map(|region| {
            hex_project::LoopRegion::new(region.start, region.end, region.enabled)
                .map_err(|error| StorageError::Invalid(error.0))
        })
        .transpose()
}

fn assets_v2_into_snapshot(assets: Vec<AssetV2>) -> Vec<AssetSnapshot> {
    assets
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
        .collect()
}

struct LegacySnapshotFields {
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetSnapshot>,
    pub(super) tracks: Vec<TrackSnapshot>,
    pub(super) loop_region: Option<hex_project::LoopRegion>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPoint>,
    pub(super) meter_points: Vec<MeterPoint>,
}

fn legacy_snapshot(fields: LegacySnapshotFields) -> Snapshot {
    Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: fields.sample_rate,
        sample_rate_mode: Default::default(),
        next_id: fields.next_id,
        assets: fields.assets,
        tracks: fields.tracks,
        session: SessionSnapshot::default(),
        loop_region: fields.loop_region,
        master_gain: fields.master_gain,
        tempo_points: fields.tempo_points,
        meter_points: fields.meter_points,
        return_tracks: Vec::new(),
    }
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
                let assets = document
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
                    .collect();
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate,
                    next_id: document.next_id,
                    assets,
                    tracks: tracks_legacy_into_snapshot(document.tracks)?,
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            2 => {
                let document: DocumentV2 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 2 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_legacy_into_snapshot(document.tracks)?,
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            3 => {
                let document: DocumentV3 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 3 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v3_into_snapshot(document.tracks)?,
                    loop_region: None,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            4 => {
                let document: DocumentV4 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 4 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v3_into_snapshot(document.tracks)?,
                    loop_region: decode_loop_region(document.loop_region)?,
                    master_gain: 1.0,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            5 => {
                let document: DocumentV5 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 5 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v4_into_snapshot(document.tracks)?,
                    loop_region: decode_loop_region(document.loop_region)?,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            6 => {
                let document: DocumentV6 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 6 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v6_into_snapshot(document.tracks)?,
                    loop_region: decode_loop_region(document.loop_region)?,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            7 => {
                let document: DocumentV7 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 7 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v7_into_snapshot(document.tracks)?,
                    loop_region: decode_loop_region(document.loop_region)?,
                    master_gain: document.master_gain,
                    tempo_points: default_tempo_points(),
                    meter_points: default_meter_points(),
                })
            }
            8 => {
                let document: DocumentV8 = serde_json::from_slice(bytes)?;
                if document.format != "hex-project" || document.version != 8 {
                    return Err(StorageError::Invalid(
                        "unsupported project format or version",
                    ));
                }
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
                legacy_snapshot(LegacySnapshotFields {
                    sample_rate: document.sample_rate,
                    next_id: document.next_id,
                    assets: assets_v2_into_snapshot(document.assets),
                    tracks: tracks_v7_into_snapshot(document.tracks)?,
                    loop_region: decode_loop_region(document.loop_region)?,
                    master_gain: document.master_gain,
                    tempo_points,
                    meter_points,
                })
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

pub(super) fn clip_v7_from_snapshot(clip: ClipSnapshot) -> ClipV7 {
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

pub(super) fn clips_v7_into_snapshot(
    clips: Vec<ClipV7>,
) -> Result<Vec<ClipSnapshot>, StorageError> {
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

pub(super) fn tracks_legacy_into_snapshot(
    tracks: Vec<TrackLegacy>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
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

pub(super) fn tracks_v3_into_snapshot(
    tracks: Vec<TrackV3>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
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

pub(super) fn tracks_v4_into_snapshot(
    tracks: Vec<TrackV4>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
    tracks
        .into_iter()
        .map(|track| {
            Ok(TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
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

pub(super) fn tracks_v6_into_snapshot(
    tracks: Vec<TrackV6>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
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
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
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

pub(super) fn tracks_v7_into_snapshot(
    tracks: Vec<TrackV7>,
) -> Result<Vec<TrackSnapshot>, StorageError> {
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
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
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

pub(super) fn clips_into_snapshot(clips: Vec<Clip>) -> Result<Vec<ClipSnapshot>, StorageError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_v1_through_v8_rejects_wrong_format_or_unknown_version_and_decodes_empty_v1() {
        let wrong_format = br#"{"format":"other","version":1}"#;
        assert!(DocumentV8::decode(wrong_format).is_err());
        let unknown_version = br#"{"format":"hex-project","version":99}"#;
        assert!(DocumentV8::decode(unknown_version).is_err());

        let v1 = br#"{"format":"hex-project","version":1,"sample_rate":48000,"next_id":1,"assets":[],"tracks":[]}"#;
        let project = DocumentV8::decode(v1).expect("minimal v1 project decodes");
        assert_eq!(project.sample_rate(), 48_000);
        assert!(project.tracks().is_empty());
        assert_eq!(project.master_gain(), 1.0);
    }
}
