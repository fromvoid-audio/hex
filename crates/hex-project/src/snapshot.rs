//! Validated, std-only metadata snapshots for storage adapters.
use crate::edit::{interval_valid, name_valid};
use crate::{
    Asset, AssetId, Clip, ClipFade, ClipId, Device, DeviceId, Invalid, LoopRegion, MAX_ASSETS,
    MAX_CLIPS, MAX_DEVICES, MAX_DEVICES_PER_TRACK, MAX_TRACKS, MeterPoint, MidiClip, MidiNote,
    NoteId, Project, TempoPoint, Track, TrackContent, TrackId, validate_conversion_metadata,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// Transport representation of an editable project, without decoded PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub return_feeds: Vec<crate::ReturnFeedSnapshot>,
    pub import_provenance: Option<crate::ImportProvenance>,
    pub sample_rate: u32,
    pub next_id: u64,
    pub assets: Vec<AssetSnapshot>,
    pub tracks: Vec<TrackSnapshot>,
    pub return_tracks: Vec<ReturnTrackSnapshot>,
    pub session: SessionSnapshot,
    pub loop_region: Option<LoopRegion>,
    pub master_gain: f32,
    pub tempo_points: Vec<TempoPoint>,
    pub meter_points: Vec<MeterPoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub scenes: Vec<SceneSnapshot>,
    pub cells: Vec<SessionCellSnapshot>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSnapshot {
    pub id: u64,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCellSnapshot {
    pub track: u64,
    pub scene: u64,
    pub content: SessionCellContentSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCellContentSnapshot {
    Midi(SessionMidiClipSnapshot),
    Audio(SessionAudioClipSnapshot),
    Stop,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAudioClipSnapshot {
    pub id: u64,
    pub name: String,
    pub asset: u64,
    pub source_start: u64,
    pub source_end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMidiClipSnapshot {
    pub id: u64,
    pub name: String,
    pub source: MidiSourceSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSourceSnapshot {
    pub source_length_raw: u64,
    pub source_start_raw: u64,
    pub loop_start_raw: u64,
    pub loop_end_raw: u64,
    pub looping: bool,
    pub notes: Vec<MidiNoteSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetSnapshot {
    pub id: u64,
    pub path: PathBuf,
    pub source_sample_rate: u32,
    pub source_frames: u64,
    pub playback_frames: u64,
    pub conversion_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackSnapshot {
    pub gain_envelope: Vec<crate::GainPoint>,
    pub gain_touch_spans: Vec<crate::GainTouchSpan>,
    pub gain_touch_epoch: u64,
    pub id: u64,
    pub name: String,
    pub gain: f32,
    pub balance: f32,
    pub muted: bool,
    pub soloed: bool,
    pub devices: Vec<DeviceSnapshot>,
    pub instrument: Option<DeviceSnapshot>,
    /// Number of effects before `instrument`; required exactly when present.
    pub instrument_position: Option<usize>,
    pub content: TrackContentSnapshot,
    pub sends: Vec<SendLevelSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnTrackSnapshot {
    pub id: u64,
    pub name: String,
    pub gain: f32,
    pub muted: bool,
    pub devices: Vec<DeviceSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendLevelSnapshot {
    pub return_track: u64,
    pub gain: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub id: u64,
    pub bypassed: bool,
    pub state: hex_lattice_contract::NativeDeviceState,
}
impl TrackSnapshot {
    pub fn audio_clips_mut(&mut self) -> Result<&mut Vec<ClipSnapshot>, Invalid> {
        match &mut self.content {
            TrackContentSnapshot::Audio(clips) => Ok(clips),
            TrackContentSnapshot::Instrument(_) => {
                Err(Invalid("track snapshot does not contain audio clips"))
            }
            TrackContentSnapshot::Hybrid { audio_clips, .. } => Ok(audio_clips),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrackContentSnapshot {
    Audio(Vec<ClipSnapshot>),
    Instrument(Vec<MidiClipSnapshot>),
    Hybrid {
        audio_clips: Vec<ClipSnapshot>,
        midi_clips: Vec<MidiClipSnapshot>,
    },
}
impl TrackContentSnapshot {
    fn into_parts(self) -> (Vec<ClipSnapshot>, Vec<MidiClipSnapshot>) {
        match self {
            Self::Audio(clips) => (clips, Vec::new()),
            Self::Instrument(clips) => (Vec::new(), clips),
            Self::Hybrid {
                audio_clips,
                midi_clips,
            } => (audio_clips, midi_clips),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClipSnapshot {
    pub id: u64,
    pub start_raw: u64,
    pub length_raw: u64,
    pub source_length_raw: u64,
    pub source_start_raw: u64,
    pub loop_start_raw: u64,
    pub loop_end_raw: u64,
    pub looping: bool,
    pub notes: Vec<MidiNoteSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiNoteSnapshot {
    pub id: u64,
    pub start_raw: u64,
    pub duration_raw: u64,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipSnapshot {
    pub id: u64,
    pub asset: u64,
    pub start: u64,
    pub duration: u64,
    pub source_offset: u64,
    pub source_length: u64,
    pub looping: bool,
    pub source_phase: u64,
    pub fade: ClipFade,
}

impl Project {
    /// Copy editable metadata for a control-side storage or interchange adapter.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            return_feeds: self
                .routing
                .iter()
                .map(|feed| crate::ReturnFeedSnapshot {
                    id: feed.id.value(),
                    source: feed.source.value(),
                    destination: feed.destination.value(),
                    gain: feed.gain,
                })
                .collect(),
            import_provenance: self.import_provenance.clone(),
            sample_rate: self.sample_rate,
            next_id: self.next_id,
            assets: self
                .assets
                .values()
                .map(|asset| AssetSnapshot {
                    id: asset.id.0,
                    path: asset.path.clone(),
                    source_sample_rate: asset.source_sample_rate,
                    source_frames: asset.source_frames,
                    playback_frames: asset.playback_frames,
                    conversion_version: asset.conversion_version,
                })
                .collect(),
            tracks: self
                .tracks
                .iter()
                .map(|track| TrackSnapshot {
                    gain_envelope: track.gain_lane.base.clone(),
                    gain_touch_spans: track.gain_lane.spans.clone(),
                    gain_touch_epoch: track.gain_lane.recording_epoch,
                    id: track.id.0,
                    name: track.name.clone(),
                    gain: track.gain,
                    balance: track.balance,
                    muted: track.muted,
                    soloed: track.soloed,
                    devices: track
                        .devices
                        .iter()
                        .map(|device| DeviceSnapshot {
                            id: device.id.value(),
                            bypassed: device.bypassed,
                            state: device.state.clone(),
                        })
                        .collect(),
                    instrument: track.instrument.as_ref().map(|instrument| DeviceSnapshot {
                        id: instrument.id.value(),
                        bypassed: instrument.bypassed,
                        state: instrument.state.clone(),
                    }),
                    instrument_position: track.instrument_position,
                    content: TrackContentSnapshot::Hybrid {
                        audio_clips: track
                            .content
                            .audio_clips
                            .iter()
                            .map(|clip| ClipSnapshot {
                                id: clip.id.0,
                                asset: clip.asset.0,
                                start: clip.start,
                                duration: clip.duration,
                                source_offset: clip.source_offset,
                                source_length: clip.source_length,
                                looping: clip.looping,
                                source_phase: clip.source_phase,
                                fade: clip.fade,
                            })
                            .collect(),
                        midi_clips: track
                            .content
                            .midi_clips
                            .iter()
                            .map(|clip| MidiClipSnapshot {
                                id: clip.id.0,
                                start_raw: clip.start.raw(),
                                length_raw: clip.length.raw(),
                                source_length_raw: clip.source.source_length.raw(),
                                source_start_raw: clip.source.source_start.raw(),
                                loop_start_raw: clip.source.loop_start.raw(),
                                loop_end_raw: clip.source.loop_end.raw(),
                                looping: clip.source.looping,
                                notes: clip
                                    .source
                                    .notes
                                    .iter()
                                    .map(|note| MidiNoteSnapshot {
                                        id: note.id.0,
                                        start_raw: note.start.raw(),
                                        duration_raw: note.duration.raw(),
                                        pitch: note.pitch,
                                        channel: note.channel,
                                        velocity: note.velocity,
                                    })
                                    .collect(),
                            })
                            .collect(),
                    },
                    sends: track
                        .sends
                        .iter()
                        .map(|send| SendLevelSnapshot {
                            return_track: send.return_track.value(),
                            gain: send.gain,
                        })
                        .collect(),
                })
                .collect(),
            return_tracks: self
                .returns
                .iter()
                .map(|track| ReturnTrackSnapshot {
                    id: track.id.value(),
                    name: track.name.clone(),
                    gain: track.gain,
                    muted: track.muted,
                    devices: track
                        .devices
                        .iter()
                        .map(|device| DeviceSnapshot {
                            id: device.id.value(),
                            bypassed: device.bypassed,
                            state: device.state.clone(),
                        })
                        .collect(),
                })
                .collect(),
            session: SessionSnapshot {
                scenes: self
                    .session
                    .scenes
                    .iter()
                    .map(|scene| SceneSnapshot {
                        id: scene.id.value(),
                        name: scene.name.clone(),
                    })
                    .collect(),
                cells: self
                    .session
                    .cells
                    .iter()
                    .map(|cell| SessionCellSnapshot {
                        track: cell.track.value(),
                        scene: cell.scene.value(),
                        content: match &cell.content {
                            crate::SessionCellContent::Stop => SessionCellContentSnapshot::Stop,
                            crate::SessionCellContent::Midi(clip) => {
                                SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                                    id: clip.id.value(),
                                    name: clip.name.clone(),
                                    source: MidiSourceSnapshot {
                                        source_length_raw: clip.source.source_length.raw(),
                                        source_start_raw: clip.source.source_start.raw(),
                                        loop_start_raw: clip.source.loop_start.raw(),
                                        loop_end_raw: clip.source.loop_end.raw(),
                                        looping: clip.source.looping,
                                        notes: clip
                                            .source
                                            .notes
                                            .iter()
                                            .map(|note| MidiNoteSnapshot {
                                                id: note.id.value(),
                                                start_raw: note.start.raw(),
                                                duration_raw: note.duration.raw(),
                                                pitch: note.pitch,
                                                channel: note.channel,
                                                velocity: note.velocity,
                                            })
                                            .collect(),
                                    },
                                })
                            }
                            crate::SessionCellContent::Audio(clip) => {
                                SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                                    id: clip.id().value(),
                                    name: clip.name().to_owned(),
                                    asset: clip.asset().value(),
                                    source_start: clip.source_start(),
                                    source_end: clip.source_end(),
                                })
                            }
                        },
                    })
                    .collect(),
            },
            loop_region: self.loop_region,
            master_gain: self.master_gain,
            tempo_points: self.tempo_map.points().to_vec(),
            meter_points: self.meter_map.points().to_vec(),
        }
    }

    /// Validate untrusted snapshot metadata before constructing a live model.
    pub fn from_snapshot(mut snapshot: Snapshot) -> Result<Self, Invalid> {
        snapshot.validate_gain_lanes()?;
        let gain_touch_unsupported = snapshot
            .tracks
            .iter()
            .any(|track| !track.gain_touch_spans.is_empty());
        let return_feeds = std::mem::take(&mut snapshot.return_feeds);
        let mut project = Project::new(snapshot.sample_rate)?;
        project.import_provenance = snapshot.import_provenance;
        project.tempo_map = crate::TempoMap::new(snapshot.sample_rate, snapshot.tempo_points)?;
        project.meter_map = crate::MeterMap::new(snapshot.meter_points)?;
        if snapshot.next_id == 0 {
            return Err(Invalid("next project id must be nonzero"));
        }
        if snapshot.assets.len() > MAX_ASSETS {
            return Err(Invalid("asset limit reached"));
        }
        if snapshot.tracks.len() > MAX_TRACKS {
            return Err(Invalid("track limit reached"));
        }
        if snapshot.return_tracks.len() > crate::MAX_RETURN_TRACKS {
            return Err(Invalid("return track limit reached"));
        }
        if snapshot.session.scenes.len() > crate::MAX_SCENES {
            return Err(Invalid("scene limit reached"));
        }
        if snapshot.session.cells.len() > crate::MAX_SESSION_CELLS {
            return Err(Invalid("session cell limit reached"));
        }
        if !snapshot.master_gain.is_finite() || !(0.0..=2.0).contains(&snapshot.master_gain) {
            return Err(Invalid("master gain must be finite and within 0..2"));
        }

        let mut ids = BTreeSet::new();
        let mut max_id = 0_u64;
        let mut assets = BTreeMap::new();
        for asset in snapshot.assets {
            insert_id(asset.id, &mut ids, &mut max_id)?;
            if asset.path.as_os_str().is_empty() {
                return Err(Invalid("media needs a path and frames"));
            }
            validate_conversion_metadata(
                asset.source_frames,
                asset.source_sample_rate,
                asset.playback_frames,
                snapshot.sample_rate,
                asset.conversion_version,
            )?;
            let id = AssetId(asset.id);
            assets.insert(
                id,
                Asset {
                    id,
                    path: asset.path,
                    source_sample_rate: asset.source_sample_rate,
                    source_frames: asset.source_frames,
                    playback_frames: asset.playback_frames,
                    conversion_version: asset.conversion_version,
                },
            );
        }

        let mut total_clips = 0_usize;
        let mut total_devices = 0_usize;
        let mut total_notes = 0_usize;
        let mut return_ids = BTreeSet::new();
        let mut returns = Vec::with_capacity(snapshot.return_tracks.len());
        for return_track in snapshot.return_tracks {
            insert_id(return_track.id, &mut ids, &mut max_id)?;
            return_ids.insert(return_track.id);
            name_valid(&return_track.name)?;
            if !return_track.gain.is_finite() || !(0.0..=2.0).contains(&return_track.gain) {
                return Err(Invalid("gain must be finite and within 0..2"));
            }
            if return_track.devices.len() > MAX_DEVICES_PER_TRACK {
                return Err(Invalid("return device limit reached"));
            }
            let mut devices = Vec::with_capacity(return_track.devices.len());
            for device in return_track.devices {
                insert_id(device.id, &mut ids, &mut max_id)?;
                total_devices = total_devices
                    .checked_add(1)
                    .ok_or(Invalid("device count overflow"))?;
                if total_devices > MAX_DEVICES {
                    return Err(Invalid("project device limit reached"));
                }
                devices.push(Device {
                    id: DeviceId(device.id),
                    state: device.state,
                    bypassed: device.bypassed,
                });
            }
            returns.push(crate::ReturnTrack {
                id: crate::ReturnId(return_track.id),
                name: return_track.name,
                gain: return_track.gain,
                muted: return_track.muted,
                devices,
            });
        }
        let mut tracks = Vec::with_capacity(snapshot.tracks.len());
        for track in snapshot.tracks {
            insert_id(track.id, &mut ids, &mut max_id)?;
            name_valid(&track.name)?;
            if !track.gain.is_finite() || !(0.0..=2.0).contains(&track.gain) {
                return Err(Invalid("gain must be finite and within 0..2"));
            }
            if !track.balance.is_finite() || !(-1.0..=1.0).contains(&track.balance) {
                return Err(Invalid("balance must be finite and within -1..1"));
            }
            if track.devices.len() > MAX_DEVICES_PER_TRACK {
                return Err(Invalid("track device limit reached"));
            }
            match (track.instrument.as_ref(), track.instrument_position) {
                (Some(_), Some(position)) if position <= track.devices.len() => {}
                (None, None) => {}
                (Some(_), Some(_)) => {
                    return Err(Invalid("instrument position exceeds effect count"));
                }
                (Some(_), None) => {
                    return Err(Invalid("instrument position is missing"));
                }
                (None, Some(_)) => {
                    return Err(Invalid("instrument position exists without instrument"));
                }
            }
            total_devices = total_devices
                .checked_add(track.devices.len())
                .ok_or(Invalid("device count overflow"))?;
            if total_devices > MAX_DEVICES {
                return Err(Invalid("project device limit reached"));
            }
            let mut devices = Vec::with_capacity(track.devices.len());
            for device in track.devices {
                insert_id(device.id, &mut ids, &mut max_id)?;
                devices.push(Device {
                    id: DeviceId(device.id),
                    state: device.state,
                    bypassed: device.bypassed,
                });
            }
            let instrument = if let Some(instrument) = track.instrument {
                total_devices = total_devices
                    .checked_add(1)
                    .ok_or(Invalid("device count overflow"))?;
                if total_devices > MAX_DEVICES {
                    return Err(Invalid("project device limit reached"));
                }
                insert_id(instrument.id, &mut ids, &mut max_id)?;
                Some(Device {
                    id: DeviceId(instrument.id),
                    state: instrument.state,
                    bypassed: instrument.bypassed,
                })
            } else {
                None
            };
            let (audio_clips, midi_clips) = track.content.into_parts();
            let track_clip_count = audio_clips
                .len()
                .checked_add(midi_clips.len())
                .ok_or(Invalid("clip count overflow"))?;
            total_clips = total_clips
                .checked_add(track_clip_count)
                .ok_or(Invalid("clip count overflow"))?;
            if total_clips > MAX_CLIPS {
                return Err(Invalid("clip limit reached"));
            }
            let mut compiled_audio = Vec::with_capacity(audio_clips.len());
            for clip in audio_clips {
                insert_id(clip.id, &mut ids, &mut max_id)?;
                let media = assets
                    .get(&AssetId(clip.asset))
                    .ok_or(Invalid("clip media reference is missing"))?;
                interval_valid(
                    clip.start,
                    clip.duration,
                    clip.source_offset,
                    clip.source_length,
                    media,
                )?;
                clip.fade.validate_segment(clip.duration)?;
                if (clip.looping && clip.source_phase >= clip.source_length)
                    || (!clip.looping && clip.source_phase != 0)
                {
                    return Err(Invalid("clip source phase is outside its source interval"));
                }
                compiled_audio.push(Clip {
                    id: ClipId(clip.id),
                    asset: AssetId(clip.asset),
                    start: clip.start,
                    duration: clip.duration,
                    source_offset: clip.source_offset,
                    source_length: clip.source_length,
                    looping: clip.looping,
                    source_phase: clip.source_phase,
                    fade: clip.fade,
                });
            }
            let mut compiled_midi = Vec::with_capacity(midi_clips.len());
            for clip in midi_clips {
                insert_id(clip.id, &mut ids, &mut max_id)?;
                let start = crate::BeatTime::from_raw(clip.start_raw);
                let length = crate::BeatTime::from_raw(clip.length_raw);
                let source_length = crate::BeatTime::from_raw(clip.source_length_raw);
                let source_start = crate::BeatTime::from_raw(clip.source_start_raw);
                let loop_start = crate::BeatTime::from_raw(clip.loop_start_raw);
                let loop_end = crate::BeatTime::from_raw(clip.loop_end_raw);
                crate::midi::validate_clip(start, length)?;
                crate::midi::validate_source(
                    source_length,
                    source_start,
                    loop_start,
                    loop_end,
                    clip.looping,
                )?;
                total_notes = total_notes
                    .checked_add(clip.notes.len())
                    .ok_or(Invalid("note count overflow"))?;
                if total_notes > crate::MAX_NOTES {
                    return Err(Invalid("note limit reached"));
                }
                let mut notes = Vec::with_capacity(clip.notes.len());
                for note in clip.notes {
                    insert_id(note.id, &mut ids, &mut max_id)?;
                    let note_start = crate::BeatTime::from_raw(note.start_raw);
                    let duration = crate::BeatTime::from_raw(note.duration_raw);
                    crate::midi::validate_note(
                        note_start,
                        duration,
                        source_length,
                        note.pitch,
                        note.channel,
                        note.velocity,
                    )?;
                    notes.push(MidiNote {
                        id: NoteId(note.id),
                        start: note_start,
                        duration,
                        pitch: note.pitch,
                        channel: note.channel,
                        velocity: note.velocity,
                    });
                }
                compiled_midi.push(MidiClip {
                    id: ClipId(clip.id),
                    start,
                    length,
                    source: std::sync::Arc::new(crate::MidiSource {
                        source_length,
                        source_start,
                        loop_start,
                        loop_end,
                        looping: clip.looping,
                        notes,
                    }),
                });
            }
            let content = TrackContent {
                audio_clips: compiled_audio,
                midi_clips: compiled_midi,
            };
            if track.sends.len() > crate::MAX_RETURN_SENDS_PER_TRACK {
                return Err(Invalid("track return send limit reached"));
            }
            let mut send_targets = BTreeSet::new();
            let mut sends = Vec::with_capacity(track.sends.len());
            for send in track.sends {
                if !return_ids.contains(&send.return_track) {
                    return Err(Invalid("send return target is missing"));
                }
                if !send_targets.insert(send.return_track) {
                    return Err(Invalid("track return send targets must be unique"));
                }
                if !send.gain.is_finite() || !(0.0..=2.0).contains(&send.gain) || send.gain == 0.0 {
                    return Err(Invalid(
                        "send gain must be finite, nonzero, and within 0..2",
                    ));
                }
                sends.push(crate::SendLevel {
                    return_track: crate::ReturnId(send.return_track),
                    gain: send.gain,
                });
            }
            tracks.push(Track {
                gain_lane: crate::GainLane::new(
                    track.gain_envelope,
                    track.gain_touch_spans,
                    track.gain_touch_epoch,
                )?,
                id: TrackId(track.id),
                name: track.name,
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                devices,
                instrument,
                instrument_position: track.instrument_position,
                content,
                sends,
            });
        }

        let mut scenes = Vec::with_capacity(snapshot.session.scenes.len());
        let mut scene_ids = BTreeSet::new();
        for scene in snapshot.session.scenes {
            insert_id(scene.id, &mut ids, &mut max_id)?;
            name_valid(&scene.name)?;
            scene_ids.insert(scene.id);
            scenes.push(crate::Scene {
                id: crate::SceneId(scene.id),
                name: scene.name,
            });
        }
        let mut cell_keys = BTreeSet::new();
        let mut cells = Vec::with_capacity(snapshot.session.cells.len());
        for cell in snapshot.session.cells {
            if !cell_keys.insert((cell.track, cell.scene)) {
                return Err(Invalid("session cell keys must be unique"));
            }
            if !tracks.iter().any(|track| track.id.value() == cell.track) {
                return Err(Invalid("session cell track is missing"));
            }
            if !scene_ids.contains(&cell.scene) {
                return Err(Invalid("session cell scene is missing"));
            }
            let content = match cell.content {
                SessionCellContentSnapshot::Stop => crate::SessionCellContent::Stop,
                SessionCellContentSnapshot::Midi(clip) => {
                    if !tracks.iter().any(|track| track.id.value() == cell.track) {
                        return Err(Invalid("session cell track is missing"));
                    }
                    insert_id(clip.id, &mut ids, &mut max_id)?;
                    name_valid(&clip.name)?;
                    total_clips = total_clips
                        .checked_add(1)
                        .ok_or(Invalid("clip count overflow"))?;
                    if total_clips > MAX_CLIPS {
                        return Err(Invalid("clip limit reached"));
                    }
                    let source_length = crate::BeatTime::from_raw(clip.source.source_length_raw);
                    let source_start = crate::BeatTime::from_raw(clip.source.source_start_raw);
                    let loop_start = crate::BeatTime::from_raw(clip.source.loop_start_raw);
                    let loop_end = crate::BeatTime::from_raw(clip.source.loop_end_raw);
                    crate::midi::validate_source(
                        source_length,
                        source_start,
                        loop_start,
                        loop_end,
                        clip.source.looping,
                    )?;
                    total_notes = total_notes
                        .checked_add(clip.source.notes.len())
                        .ok_or(Invalid("note count overflow"))?;
                    if total_notes > crate::MAX_NOTES {
                        return Err(Invalid("note limit reached"));
                    }
                    let mut notes = Vec::with_capacity(clip.source.notes.len());
                    for note in clip.source.notes {
                        insert_id(note.id, &mut ids, &mut max_id)?;
                        let start = crate::BeatTime::from_raw(note.start_raw);
                        let duration = crate::BeatTime::from_raw(note.duration_raw);
                        crate::midi::validate_note(
                            start,
                            duration,
                            source_length,
                            note.pitch,
                            note.channel,
                            note.velocity,
                        )?;
                        notes.push(MidiNote {
                            id: NoteId(note.id),
                            start,
                            duration,
                            pitch: note.pitch,
                            channel: note.channel,
                            velocity: note.velocity,
                        });
                    }
                    crate::SessionCellContent::Midi(crate::SessionMidiClip {
                        id: ClipId(clip.id),
                        name: clip.name,
                        source: std::sync::Arc::new(crate::MidiSource {
                            source_length,
                            source_start,
                            loop_start,
                            loop_end,
                            looping: clip.source.looping,
                            notes,
                        }),
                    })
                }
                SessionCellContentSnapshot::Audio(clip) => {
                    if !tracks.iter().any(|track| track.id.value() == cell.track) {
                        return Err(Invalid("session cell track is missing"));
                    }
                    let media = assets
                        .get(&AssetId(clip.asset))
                        .ok_or(Invalid("session audio media reference is missing"))?;
                    if clip.source_start >= clip.source_end
                        || clip.source_end > media.playback_frames()
                    {
                        return Err(Invalid("session audio source interval is outside media"));
                    }
                    insert_id(clip.id, &mut ids, &mut max_id)?;
                    name_valid(&clip.name)?;
                    total_clips = total_clips
                        .checked_add(1)
                        .ok_or(Invalid("clip count overflow"))?;
                    if total_clips > MAX_CLIPS {
                        return Err(Invalid("clip limit reached"));
                    }
                    crate::SessionCellContent::Audio(crate::SessionAudioClip {
                        id: ClipId(clip.id),
                        name: clip.name,
                        asset: AssetId(clip.asset),
                        source_start: clip.source_start,
                        source_end: clip.source_end,
                    })
                }
            };
            cells.push(crate::SessionCell {
                track: TrackId(cell.track),
                scene: crate::SceneId(cell.scene),
                content,
            });
        }

        if snapshot.next_id <= max_id {
            return Err(Invalid("next project id must exceed all existing ids"));
        }
        project.next_id = snapshot.next_id;
        project.assets = assets;
        project.tracks = tracks;
        project.returns = returns;
        project.session = crate::SessionGrid { scenes, cells };
        project.loop_region = snapshot.loop_region;
        project.master_gain = snapshot.master_gain;
        if !return_feeds.is_empty() {
            let mut candidate = project.snapshot();
            candidate.return_feeds = return_feeds;
            project.routing = Self::validate_return_feed_snapshot(&candidate)?;
        }
        if gain_touch_unsupported {
            return Err(Invalid(crate::GAIN_TOUCH_UNSUPPORTED));
        }
        Ok(project)
    }
}

fn insert_id(id: u64, seen: &mut BTreeSet<u64>, max_id: &mut u64) -> Result<(), Invalid> {
    if id == 0 {
        return Err(Invalid("project ids must be nonzero"));
    }
    if !seen.insert(id) {
        return Err(Invalid("project ids must be globally unique"));
    }
    *max_id = (*max_id).max(id);
    Ok(())
}

impl Snapshot {
    /// Structural proposal validation is separate from live Project admission.
    pub fn validate_gain_lanes(&self) -> Result<(), Invalid> {
        let mut units = 0_usize;
        for track in &self.tracks {
            let count = crate::validate_gain_lane_parts(
                &track.gain_envelope,
                &track.gain_touch_spans,
                track.gain_touch_epoch,
            )?;
            for span in &track.gain_touch_spans {
                if span.context.sample_rate != self.sample_rate {
                    return Err(Invalid(
                        "gain touch context sample rate differs from project",
                    ));
                }
            }
            units = units
                .checked_add(count)
                .ok_or(Invalid("gain lane count overflow"))?;
            if units > crate::MAX_GAIN_POINTS {
                return Err(Invalid("project gain lane limit reached"));
            }
        }
        Ok(())
    }
}
