//! Saved-project schemas V14 through V19 (native devices, returns, sends, and routing).
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV14 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV14>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV14 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
    pub(super) devices: Vec<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV15 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV15>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV16 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV16>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentV19 {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) sample_rate: u32,
    pub(super) next_id: u64,
    pub(super) assets: Vec<AssetV2>,
    pub(super) tracks: Vec<TrackV17>,
    pub(super) return_tracks: Vec<ReturnTrackDtoV17>,
    #[serde(deserialize_with = "deserialize_required_loop_region")]
    pub(super) loop_region: Option<LoopRegionDto>,
    pub(super) master_gain: f32,
    pub(super) tempo_points: Vec<TempoPointDto>,
    pub(super) meter_points: Vec<MeterPointDto>,
    pub(super) session: SessionDtoV12,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV17 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
    pub(super) devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    pub(super) instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    pub(super) instrument_position: Option<usize>,
    pub(super) sends: Vec<SendDtoV17>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReturnTrackDtoV17 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) muted: bool,
    pub(super) devices: Vec<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendDtoV17 {
    pub(super) return_track: u64,
    pub(super) gain: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV15 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
    pub(super) devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    pub(super) instrument: Option<DeviceDtoV14>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TrackV16 {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) gain: f32,
    pub(super) balance: f32,
    pub(super) muted: bool,
    pub(super) soloed: bool,
    pub(super) audio_clips: Vec<ClipV7>,
    pub(super) midi_clips: Vec<MidiClipDtoV10>,
    pub(super) devices: Vec<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument")]
    pub(super) instrument: Option<DeviceDtoV14>,
    #[serde(deserialize_with = "deserialize_required_instrument_position")]
    pub(super) instrument_position: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceDtoV14 {
    pub(super) id: u64,
    pub(super) bypassed: bool,
    pub(super) type_id: String,
    pub(super) state_version: u32,
    pub(super) payload: Vec<u8>,
    #[serde(default)]
    pub(super) input_gain_millidb: i32,
    #[serde(default)]
    pub(super) output_gain_millidb: i32,
}

pub(super) fn deserialize_required_instrument<'de, D>(
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
    pub(super) fn into_project(doc: Self) -> Result<Project, StorageError> {
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

pub(super) fn device_to_dto(
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

pub(super) fn device_dto_into_snapshot(
    device: DeviceDtoV14,
) -> Result<DeviceSnapshot, StorageError> {
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
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        id: device.id,
        bypassed: device.bypassed,
        comparison: None,
        state,
    })
}
