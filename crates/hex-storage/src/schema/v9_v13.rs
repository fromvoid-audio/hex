//! Saved-project schemas V9 through V13 (MIDI clips, Session grid, and tracks).
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV9 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV9>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV10 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV11 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV11,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV12 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV10>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV13 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV13>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV13 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionDtoV12 {
    pub(super) scenes: Vec<SceneDtoV11>,
    pub(super) cells: Vec<SessionCellDtoV12>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionCellDtoV12 {
    pub(super) track: u64,
    pub(super) scene: u64,
    pub(super) content: SessionContentDtoV12,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SessionContentDtoV12 {
    Midi { clip: SessionMidiClipDtoV11 },
    Audio { clip: SessionAudioClipDtoV12 },
    Stop,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionAudioClipDtoV12 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) asset: u64,
    pub(super) source_start: u64,
    pub(super) source_end: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionDtoV11 {
    pub(super) scenes: Vec<SceneDtoV11>,
    pub(super) cells: Vec<SessionCellDtoV11>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SceneDtoV11 {
    pub(super) id: u64,
    pub(super) name: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionCellDtoV11 {
    pub(super) track: u64,
    pub(super) scene: u64,
    pub(super) content: SessionContentDtoV11,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SessionContentDtoV11 {
    Midi { clip: SessionMidiClipDtoV11 },
    Stop,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionMidiClipDtoV11 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) source: MidiSourceDtoV11,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MidiSourceDtoV11 {
    pub(super) source_length_raw: u64,
    pub(super) source_start_raw: u64,
    pub(super) loop_start_raw: u64,
    pub(super) loop_end_raw: u64,
    pub(super) looping: bool,
    pub(super) notes: Vec<MidiNoteDto>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV9 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) content: ContentV9,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV10 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) content: ContentV10,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ContentV9 {
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
pub(super) enum ContentV10 {
    Audio { clips: Vec<ClipV7> },
    Instrument { clips: Vec<MidiClipDtoV10> },
}

impl DocumentV12 {
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
        let mut snapshot = v10.into_project()?.snapshot();
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
                                            expression: hex_project::NoteExpression::default(),
                                            expression_points: Vec::new(),
                                        })
                                        .collect(),
                                    expression_lanes: Vec::new(),
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
            mixer_automation: Vec::new(),
            frozen_tracks: Vec::new(),
            track_io: Vec::new(),
            scene_follow_actions: Vec::new(),
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
            sample_rate: doc.sample_rate,
            sample_rate_mode: Default::default(),
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
pub(super) struct HistoricalShape {
    #[serde(default, deserialize_with = "present_field")]
    pub(super) import_provenance: Option<serde::de::IgnoredAny>,
    #[serde(default, deserialize_with = "present_field")]
    pub(super) routing: Option<serde::de::IgnoredAny>,
}
pub(super) fn present_field<'de, D: serde::Deserializer<'de>>(
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

    pub(super) fn decode_legacy(bytes: &[u8]) -> Result<Project, StorageError> {
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
        let mut snapshot = v10.into_project()?.snapshot();
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
                                            expression: hex_project::NoteExpression::default(),
                                            expression_points: Vec::new(),
                                        })
                                        .collect(),
                                    expression_lanes: Vec::new(),
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
            10 => serde_json::from_slice::<Self>(bytes)?.into_project(),
            _ => DocumentV9::decode(bytes),
        }
    }

    pub(crate) fn into_project(self) -> Result<Project, StorageError> {
        let doc = self;
        if doc.format != "hex-project" || doc.version != 10 {
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
                                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
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
                                        expression: hex_project::NoteExpression::default(),
                                        expression_points: Vec::new(),
                                    })
                                    .collect(),
                                expression_lanes: Vec::new(),
                            })
                            .collect(),
                    ),
                };
                Ok(TrackSnapshot {
                    group_output_mutes: Vec::new(),
                    tags: Vec::new(),
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
            mixer_automation: Vec::new(),
            frozen_tracks: Vec::new(),
            track_io: Vec::new(),
            scene_follow_actions: Vec::new(),
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
            sample_rate: doc.sample_rate,
            sample_rate_mode: Default::default(),
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
                                            expression: hex_project::NoteExpression::default(),
                                            expression_points: Vec::new(),
                                        })
                                        .collect(),
                                    expression_lanes: Vec::new(),
                                })
                                .collect(),
                        )
                    }
                };
                Ok(TrackSnapshot {
                    group_output_mutes: Vec::new(),
                    tags: Vec::new(),
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
            mixer_automation: Vec::new(),
            frozen_tracks: Vec::new(),
            track_io: Vec::new(),
            scene_follow_actions: Vec::new(),
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
            sample_rate: doc.sample_rate,
            sample_rate_mode: Default::default(),
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

pub(super) fn midi_v10_from_snapshot(clip: MidiClipSnapshot) -> MidiClipDtoV10 {
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

pub(super) fn midi_v10_into_snapshot(clip: MidiClipDtoV10) -> MidiClipSnapshot {
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
                expression: hex_project::NoteExpression::default(),
                expression_points: Vec::new(),
            })
            .collect(),
        expression_lanes: Vec::new(),
    }
}

pub(super) fn session_snapshot_to_v12(snapshot: SessionSnapshot) -> SessionDtoV12 {
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

pub(super) fn session_v12_into_snapshot(session: SessionDtoV12) -> SessionSnapshot {
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
                                        expression: hex_project::NoteExpression::default(),
                                        expression_points: Vec::new(),
                                    })
                                    .collect(),
                                expression_lanes: Vec::new(),
                            },
                        })
                    }
                },
            })
            .collect(),
    }
}
