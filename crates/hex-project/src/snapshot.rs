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
    sync::Arc,
};

/// Transport representation of an editable project, without decoded PCM.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub aliases: crate::ClipAliasesSnapshot,
    pub native_sources: Vec<crate::NativeSourceSnapshot>,
    pub arrangement_sections: Vec<crate::ArrangementSectionSnapshot>,
    pub tuning: hex_tuning::ProjectTuning,
    pub return_feeds: Vec<crate::ReturnFeedSnapshot>,
    /// Strictly ascending IDs of tracks whose name is automatic (hex-generated
    /// default or device-derived). Every listed ID must name a track; omitted
    /// tracks carry user-set names.
    pub automatic_track_names: Vec<u64>,
    /// Strictly ascending receiver track IDs with a retained source path.
    pub retained_source_paths: Vec<u64>,
    pub audio_feeds: Vec<crate::AudioFeedSnapshot>,
    pub midi_routes: Vec<crate::MidiRouteSnapshot>,
    pub sidechains: Vec<crate::SidechainRouteSnapshot>,
    pub import_provenance: Option<crate::ImportProvenance>,
    pub sample_rate: u32,
    pub sample_rate_mode: crate::SampleRateMode,
    pub next_id: u64,
    pub assets: Vec<AssetSnapshot>,
    pub tracks: Vec<TrackSnapshot>,
    pub return_tracks: Vec<ReturnTrackSnapshot>,
    pub session: SessionSnapshot,
    pub loop_region: Option<LoopRegion>,
    pub master_gain: f32,
    pub main_devices: Vec<DeviceSnapshot>,
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
    pub active: bool,
    pub flavor: crate::ClipFlavor,
    pub id: u64,
    pub name: String,
    pub asset: u64,
    pub source_start: u64,
    pub source_end: u64,
    pub loop_range: Option<crate::SessionAudioLoop>,
    pub gain: crate::ClipGain,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMidiClipSnapshot {
    pub active: bool,
    pub playback_offset_raw: u64,
    pub volume: crate::MidiClipVolume,
    pub flavor: crate::ClipFlavor,
    pub id: u64,
    pub name: String,
    pub source: MidiSourceSnapshot,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSourceSnapshot {
    /// Selected nonrepeating audible end; notes beyond it remain authored metadata.
    pub one_shot_boundary: crate::MidiOneShotBoundary,
    pub source_length_raw: u64,
    pub source_start_raw: u64,
    pub loop_start_raw: u64,
    pub loop_end_raw: u64,
    pub looping: bool,
    pub notes: Vec<MidiNoteSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetSnapshot {
    pub original_source: Option<crate::OriginalSourceFacts>,
    pub id: u64,
    pub path: PathBuf,
    pub source_sample_rate: u32,
    pub source_frames: u64,
    pub playback_frames: u64,
    pub conversion_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackSnapshot {
    pub group_output_mutes: Vec<crate::GroupOutputMuteRange>,
    pub tags: Vec<String>,
    pub kind: crate::TrackKind,
    pub parent_group: Option<u64>,
    pub color: crate::ElementColor,
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
    /// Resolved read projection of the bound source instrument. Identity, state,
    /// bypass and comparison must match its registry device; `automation` and
    /// `modulation` must be empty. NativeSourceSnapshot is the sole source-envelope transport owner,
    /// avoiding one curve copy per shared binding. This is never a second state
    /// owner. Historical inline projections use the explicit legacy adapter.
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
    pub modulation: crate::DeviceModulation,
    pub parent: Option<u64>,
    pub automation: Vec<crate::DeviceAutomation>,
    pub comparison: Option<crate::DeviceComparison>,
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
    pub content_view: Option<crate::ContentWindow>,
    pub active: bool,
    pub playback_offset_raw: u64,
    /// Selected nonrepeating audible end; notes beyond it remain authored metadata.
    pub one_shot_boundary: crate::MidiOneShotBoundary,
    pub volume: crate::MidiClipVolume,
    pub name: String,
    pub flavor: crate::ClipFlavor,
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
    pub active: bool,
    pub start_raw: u64,
    pub duration_raw: u64,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipSnapshot {
    pub active: bool,
    pub name: String,
    pub flavor: crate::ClipFlavor,
    pub id: u64,
    pub asset: u64,
    pub start: u64,
    pub duration: u64,
    pub source_offset: u64,
    pub source_length: u64,
    pub looping: bool,
    pub source_phase: u64,
    pub edit: Option<crate::AudioClipEdit>,
    pub fade: ClipFade,
    pub gain: crate::ClipGain,
}

impl Project {
    /// Copy editable metadata for a control-side storage or interchange adapter.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            aliases: self.aliases.snapshot(),
            native_sources: self
                .native_sources
                .iter()
                .map(crate::NativeSourceSnapshot::from)
                .collect(),
            arrangement_sections: self
                .arrangement_sections
                .iter()
                .map(crate::ArrangementSectionSnapshot::from)
                .collect(),
            tuning: self.tuning,
            audio_feeds: self
                .audio_feeds
                .iter()
                .map(|f| crate::AudioFeedSnapshot {
                    id: f.id().value(),
                    source: f.source(),
                    destination: f.destination().value(),
                    gain: f.gain(),
                })
                .collect(),
            sidechains: self
                .sidechains
                .iter()
                .map(|route| crate::SidechainRouteSnapshot {
                    device: route.device.value(),
                    source: route.source.value(),
                })
                .collect(),
            midi_routes: self
                .midi_routes
                .iter()
                .map(|r| crate::MidiRouteSnapshot {
                    id: r.id().value(),
                    source: r.source().value(),
                    destination: r.destination().value(),
                })
                .collect(),
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
            automatic_track_names: self
                .tracks
                .iter()
                .filter(|track| track.automatic_name)
                .map(|track| track.id.0)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            retained_source_paths: self
                .tracks
                .iter()
                .filter(|track| track.retained_source_path)
                .map(|track| track.id.0)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            import_provenance: self.import_provenance.clone(),
            sample_rate: self.sample_rate,
            sample_rate_mode: self.sample_rate_mode,
            next_id: self.next_id,
            assets: self
                .assets
                .values()
                .map(|asset| AssetSnapshot {
                    original_source: asset.original_source,
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
                    group_output_mutes: track.group_output_mutes().to_vec(),
                    tags: track
                        .tags
                        .iter()
                        .map(|tag| tag.as_str().to_owned())
                        .collect(),
                    kind: track.kind,
                    parent_group: track.parent_group.map(TrackId::value),
                    color: track.color,
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
                            modulation: device.modulation.clone(),
                            parent: device.parent.map(|id| id.value()),
                            automation: device.automation().to_vec(),
                            comparison: device.comparison.clone(),
                            id: device.id.value(),
                            bypassed: device.bypassed,
                            state: device.state.clone(),
                        })
                        .collect(),
                    instrument: track.instrument.as_ref().map(|instrument| DeviceSnapshot {
                        modulation: Default::default(),
                        parent: instrument.parent.map(|id| id.value()),
                        automation: Vec::new(),
                        comparison: instrument.comparison.clone(),
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
                                active: clip.active,
                                name: clip.name().to_owned(),
                                flavor: clip.flavor,
                                id: clip.id.0,
                                asset: clip.asset.0,
                                start: clip.start,
                                duration: clip.duration,
                                source_offset: clip.source_offset,
                                source_length: clip.source_length,
                                looping: clip.looping,
                                source_phase: clip.source_phase,
                                edit: clip.audio_edit().cloned(),
                                fade: clip.fade,
                                gain: clip.gain,
                            })
                            .collect(),
                        midi_clips: track
                            .content
                            .midi_clips
                            .iter()
                            .map(|clip| MidiClipSnapshot {
                                content_view: clip.content_view,
                                active: clip.active,
                                playback_offset_raw: clip.playback_offset.raw(),
                                one_shot_boundary: clip.source.one_shot_boundary,
                                volume: clip.volume.clone(),
                                name: clip.name.clone(),
                                flavor: clip.flavor,
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
                                    .filter(|_| self.clip_alias(clip.id).is_none())
                                    .map(|note| MidiNoteSnapshot {
                                        id: note.id.0,
                                        active: note.active,
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
                            modulation: device.modulation.clone(),
                            parent: device.parent.map(|id| id.value()),
                            automation: device.automation().to_vec(),
                            comparison: device.comparison.clone(),
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
                                    active: clip.active,
                                    playback_offset_raw: clip.playback_offset.raw(),
                                    volume: clip.volume.clone(),
                                    flavor: clip.flavor,
                                    id: clip.id.value(),
                                    name: clip.name.clone(),
                                    source: MidiSourceSnapshot {
                                        one_shot_boundary: clip.source.one_shot_boundary,
                                        source_length_raw: clip.source.source_length.raw(),
                                        source_start_raw: clip.source.source_start.raw(),
                                        loop_start_raw: clip.source.loop_start.raw(),
                                        loop_end_raw: clip.source.loop_end.raw(),
                                        looping: clip.source.looping,
                                        notes: clip
                                            .source
                                            .notes
                                            .iter()
                                            .filter(|_| self.clip_alias(clip.id).is_none())
                                            .map(|note| MidiNoteSnapshot {
                                                id: note.id.value(),
                                                active: note.active,
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
                                    active: clip.active,
                                    flavor: clip.flavor,
                                    id: clip.id().value(),
                                    name: clip.name().to_owned(),
                                    asset: clip.asset().value(),
                                    source_start: clip.source_start(),
                                    source_end: clip.source_end(),
                                    loop_range: clip.loop_range(),
                                    gain: clip.gain(),
                                })
                            }
                        },
                    })
                    .collect(),
            },
            loop_region: self.loop_region,
            main_devices: self
                .main_devices
                .iter()
                .map(|device| DeviceSnapshot {
                    modulation: device.modulation.clone(),
                    parent: device.parent.map(|id| id.value()),
                    automation: device.automation().to_vec(),
                    comparison: device.comparison.clone(),
                    id: device.id.value(),
                    bypassed: device.bypassed,
                    state: device.state.clone(),
                })
                .collect(),
            master_gain: self.master_gain,
            tempo_points: self.tempo_map.points().to_vec(),
            meter_points: self.meter_map.points().to_vec(),
        }
    }

    /// Validate untrusted snapshot metadata before constructing a live model.
    /// Explicit in-memory adapter for historical inline-instrument snapshots.
    /// Frozen codec layers may edit their resolved instrument projection before
    /// the next layer validates it. Only this adapter treats that legacy field as
    /// authoritative. Current-format callers use strict `from_snapshot` instead.
    /// Original DeviceIds and source membership/order survive; fresh SourceIds
    /// are reserved after the old identity cursor, without writing any file.
    /// Generic device envelopes did not exist in these historical formats;
    /// snapshots containing them must use strict `from_snapshot` so an inline
    /// projection cannot replace their canonical source owner.
    pub fn from_legacy_snapshot(mut snapshot: Snapshot) -> Result<Self, Invalid> {
        if snapshot
            .tracks
            .iter()
            .flat_map(|track| track.devices.iter().chain(track.instrument.iter()))
            .chain(
                snapshot
                    .return_tracks
                    .iter()
                    .flat_map(|track| track.devices.iter()),
            )
            .chain(snapshot.main_devices.iter())
            .chain(
                snapshot
                    .native_sources
                    .iter()
                    .flat_map(|source| source.devices.iter()),
            )
            .any(|device| !device.automation.is_empty() || !device.modulation.is_empty())
        {
            return Err(Invalid(
                "legacy snapshots cannot carry device automation or modulation; use strict current snapshots",
            ));
        }
        let mut max_id = 0;
        let mut note_id = |id: u64| {
            max_id = max_id.max(id);
        };
        for section in &snapshot.arrangement_sections {
            note_id(section.id);
        }
        for asset in &snapshot.assets {
            note_id(asset.id);
        }
        for source in &snapshot.native_sources {
            note_id(source.id);
            for d in &source.devices {
                note_id(d.id);
            }
        }
        for d in &snapshot.main_devices {
            note_id(d.id);
        }
        for t in &snapshot.return_tracks {
            note_id(t.id);
            for d in &t.devices {
                note_id(d.id);
            }
        }
        for t in &snapshot.tracks {
            note_id(t.id);
            for d in t.devices.iter().chain(t.instrument.iter()) {
                note_id(d.id);
            }
            let (audio, midi): (&[_], &[_]) = match &t.content {
                TrackContentSnapshot::Audio(a) => (a, &[]),
                TrackContentSnapshot::Instrument(m) => (&[], m),
                TrackContentSnapshot::Hybrid {
                    audio_clips,
                    midi_clips,
                } => (audio_clips, midi_clips),
            };
            for c in audio {
                note_id(c.id);
            }
            for c in midi {
                note_id(c.id);
                for n in &c.notes {
                    note_id(n.id);
                }
            }
        }
        for scene in &snapshot.session.scenes {
            note_id(scene.id);
        }
        for cell in &snapshot.session.cells {
            match &cell.content {
                SessionCellContentSnapshot::Audio(c) => note_id(c.id),
                SessionCellContentSnapshot::Midi(c) => {
                    note_id(c.id);
                    for n in &c.source.notes {
                        note_id(n.id);
                    }
                }
                SessionCellContentSnapshot::Stop => {}
            }
        }
        for feed in &snapshot.return_feeds {
            note_id(feed.id);
        }
        for feed in &snapshot.audio_feeds {
            note_id(feed.id);
        }
        for route in &snapshot.midi_routes {
            note_id(route.id);
        }
        if snapshot.next_id <= max_id {
            return Err(Invalid("next project id must exceed all existing ids"));
        }
        // Historical inline formats did not carry a receiver. Reconstruct it
        // from their authoritative singleton binding, including removed/moved
        // inline instruments between frozen migration layers.
        for source in &mut snapshot.native_sources {
            source.receiver = None;
        }
        let mut legacy_bindings = BTreeSet::new();
        for track in &snapshot.tracks {
            let Some(instrument) = &track.instrument else {
                continue;
            };
            if let Some(source) = snapshot
                .native_sources
                .iter_mut()
                .find(|s| s.devices.iter().any(|d| d.id == instrument.id))
            {
                if !legacy_bindings.insert(source.id) {
                    return Err(Invalid("legacy snapshots cannot share source bindings"));
                }
                if source.devices.len() != 1 || source.instrument_index != 0 {
                    return Err(Invalid(
                        "legacy instrument does not identify a singleton source",
                    ));
                }
                source.devices[0] = instrument.clone();
                if source.receiver.is_none() {
                    source.receiver = Some(track.id);
                }
            } else {
                if snapshot.native_sources.len() >= crate::MAX_NATIVE_SOURCES {
                    return Err(Invalid("native source limit reached"));
                }
                let id = snapshot.next_id;
                snapshot.next_id = id
                    .checked_add(1)
                    .ok_or(Invalid("project id space exhausted"))?;
                snapshot.native_sources.push(crate::NativeSourceSnapshot {
                    id,
                    name: track.name.clone(),
                    devices: vec![instrument.clone()],
                    instrument_index: 0,
                    receiver: Some(track.id),
                });
                legacy_bindings.insert(id);
            }
        }
        Self::from_snapshot(snapshot)
    }

    pub fn from_snapshot(mut snapshot: Snapshot) -> Result<Self, Invalid> {
        if snapshot.audio_feeds.len() > crate::MAX_AUDIO_FEEDS
            || snapshot.midi_routes.len() > crate::MAX_MIDI_ROUTES
        {
            return Err(Invalid("authored routing edge limit reached"));
        }
        snapshot.validate_gain_lanes()?;
        let return_feeds = std::mem::take(&mut snapshot.return_feeds);
        let audio_feeds = std::mem::take(&mut snapshot.audio_feeds);
        let midi_routes = std::mem::take(&mut snapshot.midi_routes);
        let sidechains = std::mem::take(&mut snapshot.sidechains);
        let automatic_track_names = std::mem::take(&mut snapshot.automatic_track_names);
        let retained_source_paths = std::mem::take(&mut snapshot.retained_source_paths);
        if retained_source_paths.len() > MAX_TRACKS
            || retained_source_paths
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(Invalid(
                "retained source paths must be unique ascending track ids",
            ));
        }
        let mut retained_source_paths: BTreeSet<u64> = retained_source_paths.into_iter().collect();
        if automatic_track_names.len() > MAX_TRACKS
            || automatic_track_names
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(Invalid(
                "automatic track names must be unique ascending track ids",
            ));
        }
        let mut automatic_track_names: BTreeSet<u64> = automatic_track_names.into_iter().collect();
        let mut project = Project::new(snapshot.sample_rate)?;
        project.sample_rate_mode = snapshot.sample_rate_mode;
        project.tuning = snapshot.tuning;
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
        let (aliases, alias_note_count) =
            crate::clip_alias::ClipAliases::from_snapshot(snapshot.aliases, &mut ids, &mut max_id)?;
        project.aliases = aliases;
        if snapshot.arrangement_sections.len() > crate::MAX_ARRANGEMENT_SECTIONS {
            return Err(Invalid("arrangement section limit reached"));
        }
        for section in snapshot.arrangement_sections {
            insert_id(section.id, &mut ids, &mut max_id)?;
            project
                .arrangement_sections
                .push(crate::arrangement_sections::from_snapshot(section)?);
        }
        crate::arrangement_sections::validate(&project.arrangement_sections, project.tempo_map())?;
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
                    original_source: asset.original_source,
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
        if snapshot.main_devices.len() > MAX_DEVICES_PER_TRACK {
            return Err(Invalid("main device limit reached"));
        }
        let mut main_devices = Vec::with_capacity(snapshot.main_devices.len());
        for device in snapshot.main_devices {
            insert_id(device.id, &mut ids, &mut max_id)?;
            main_devices.push(Device {
                modulation: device.modulation,
                parent: device.parent.map(DeviceId),
                automation: device.automation,
                comparison: device.comparison,
                id: DeviceId(device.id),
                state: device.state,
                bypassed: device.bypassed,
            });
        }
        let mut total_devices = main_devices.len();
        if snapshot.native_sources.len() > crate::MAX_NATIVE_SOURCES {
            return Err(Invalid("native source limit reached"));
        }
        for source in snapshot.native_sources {
            insert_id(source.id, &mut ids, &mut max_id)?;
            crate::native_sources::validate_chain(
                &source.name,
                source.devices.len(),
                source.instrument_index,
            )?;
            let mut devices = Vec::with_capacity(source.devices.len());
            for device in source.devices {
                insert_id(device.id, &mut ids, &mut max_id)?;
                total_devices = total_devices
                    .checked_add(1)
                    .ok_or(Invalid("device count overflow"))?;
                if total_devices > MAX_DEVICES {
                    return Err(Invalid("project device limit reached"));
                }
                if let Some(comparison) = &device.comparison {
                    comparison.validate(&device.state)?;
                }
                devices.push(Device {
                    modulation: device.modulation,
                    parent: device.parent.map(DeviceId),
                    automation: device.automation,
                    id: DeviceId(device.id),
                    state: device.state,
                    bypassed: device.bypassed,
                    comparison: device.comparison,
                });
            }
            project.native_sources.push(crate::NativeSource::from_parts(
                crate::SourceId(source.id),
                source.name,
                devices,
                source.instrument_index,
                source.receiver.map(TrackId),
            )?);
        }
        let mut total_notes = alias_note_count;
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
                    modulation: device.modulation,
                    parent: device.parent.map(DeviceId),
                    automation: device.automation,
                    comparison: device.comparison,
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
                    modulation: device.modulation,
                    parent: device.parent.map(DeviceId),
                    automation: device.automation,
                    comparison: device.comparison,
                    id: DeviceId(device.id),
                    state: device.state,
                    bypassed: device.bypassed,
                });
            }
            let instrument = if let Some(instrument) = track.instrument {
                let source = project
                    .native_sources
                    .iter()
                    .find(|source| source.instrument().id().value() == instrument.id)
                    .ok_or(Invalid("track instrument has no retained source"))?;
                let expected = source.instrument();
                if !instrument.automation.is_empty()
                    || !instrument.modulation.is_empty()
                    || expected.state() != &instrument.state
                    || expected.bypassed() != instrument.bypassed
                    || expected.comparison() != instrument.comparison.as_ref()
                {
                    return Err(Invalid(
                        "track instrument projection differs from retained source",
                    ));
                }
                Some(source.binding()?)
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
                name_valid(&clip.name)?;
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
                    active: clip.active,
                    name: clip.name.into(),
                    flavor: clip.flavor,
                    id: ClipId(clip.id),
                    asset: AssetId(clip.asset),
                    start: clip.start,
                    duration: clip.duration,
                    source_offset: clip.source_offset,
                    source_length: clip.source_length,
                    looping: clip.looping,
                    source_phase: clip.source_phase,
                    edit: clip.edit.map(Arc::new),
                    fade: clip.fade,
                    gain: clip.gain,
                });
            }
            let mut compiled_midi = Vec::with_capacity(midi_clips.len());
            for clip in midi_clips {
                insert_id(clip.id, &mut ids, &mut max_id)?;
                name_valid(&clip.name)?;
                let start = crate::BeatTime::from_raw(clip.start_raw);
                let length = crate::BeatTime::from_raw(clip.length_raw);
                let source_length = crate::BeatTime::from_raw(clip.source_length_raw);
                let source_start = crate::BeatTime::from_raw(clip.source_start_raw);
                let loop_start = crate::BeatTime::from_raw(clip.loop_start_raw);
                let loop_end = crate::BeatTime::from_raw(clip.loop_end_raw);
                crate::midi::validate_clip(start, length)?;
                crate::midi::validate_source_boundary(
                    source_length,
                    source_start,
                    loop_start,
                    loop_end,
                    clip.looping,
                    clip.one_shot_boundary,
                )?;
                let alias_content = project.aliases.midi_content(ClipId(clip.id))?;
                if let Some(content) = &alias_content
                    && (!clip.notes.is_empty() || content.source_length != source_length)
                {
                    return Err(Invalid(
                        "alias midi member contains inline notes or wrong extent",
                    ));
                }
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
                        active: note.active,
                        start: note_start,
                        duration,
                        pitch: note.pitch,
                        channel: note.channel,
                        velocity: note.velocity,
                    });
                }
                crate::midi_note_overwrite::validate_occupancy(&notes)?;
                clip.volume.validate_source_length(source_length)?;
                clip.playback_offset_raw
                    .checked_add(length.raw())
                    .ok_or(Invalid("midi playback window end overflow"))?;
                if let Some(view) = clip.content_view
                    && view.length()? != length.raw()
                {
                    return Err(Invalid("midi content window differs from placement"));
                }
                compiled_midi.push(MidiClip {
                    content_view: clip.content_view,
                    active: clip.active,
                    playback_offset: crate::BeatTime::from_raw(clip.playback_offset_raw),
                    volume: clip.volume.clone(),
                    name: clip.name.clone(),
                    flavor: clip.flavor,
                    id: ClipId(clip.id),
                    start,
                    length,
                    source: std::sync::Arc::new(crate::MidiSource {
                        content: alias_content.unwrap_or_else(|| {
                            std::sync::Arc::new(crate::MidiContent {
                                source_length,
                                notes,
                            })
                        }),
                        one_shot_boundary: clip.one_shot_boundary,
                        source_start,
                        loop_start,
                        loop_end,
                        looping: clip.looping,
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
                group_output_mutes: crate::group_output_mute::snapshot_ranges(
                    track.kind,
                    track.group_output_mutes,
                )?,
                tags: crate::track_tags::snapshot_list(track.tags)?,
                kind: track.kind,
                parent_group: track.parent_group.map(TrackId),
                color: track.color,
                gain_lane: crate::GainLane::new(
                    track.gain_envelope,
                    track.gain_touch_spans,
                    track.gain_touch_epoch,
                )?,
                id: TrackId(track.id),
                name: track.name,
                automatic_name: automatic_track_names.remove(&track.id),
                gain: track.gain,
                balance: track.balance,
                muted: track.muted,
                soloed: track.soloed,
                devices,
                instrument,
                retained_source_path: retained_source_paths.remove(&track.id),
                instrument_position: track.instrument_position,
                content,
                sends,
            });
        }

        if !automatic_track_names.is_empty() {
            return Err(Invalid("automatic track name owner is missing"));
        }
        if !retained_source_paths.is_empty() {
            return Err(Invalid("retained source path owner is missing"));
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
                    if clip.playback_offset_raw == u64::MAX {
                        return Err(Invalid("session midi playback window exhausted"));
                    }
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
                    crate::midi::validate_source_boundary(
                        source_length,
                        source_start,
                        loop_start,
                        loop_end,
                        clip.source.looping,
                        clip.source.one_shot_boundary,
                    )?;
                    let alias_content = project.aliases.midi_content(ClipId(clip.id))?;
                    if let Some(content) = &alias_content
                        && (!clip.source.notes.is_empty() || content.source_length != source_length)
                    {
                        return Err(Invalid(
                            "alias midi member contains inline notes or wrong extent",
                        ));
                    }
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
                            active: note.active,
                            start,
                            duration,
                            pitch: note.pitch,
                            channel: note.channel,
                            velocity: note.velocity,
                        });
                    }
                    crate::midi_note_overwrite::validate_occupancy(&notes)?;
                    clip.volume.validate_source_length(source_length)?;
                    crate::SessionCellContent::Midi(crate::SessionMidiClip {
                        active: clip.active,
                        playback_offset: crate::BeatTime::from_raw(clip.playback_offset_raw),
                        volume: clip.volume,
                        flavor: clip.flavor,
                        id: ClipId(clip.id),
                        name: clip.name,
                        source: std::sync::Arc::new(crate::MidiSource {
                            content: alias_content.unwrap_or_else(|| {
                                std::sync::Arc::new(crate::MidiContent {
                                    source_length,
                                    notes,
                                })
                            }),
                            one_shot_boundary: clip.source.one_shot_boundary,
                            source_start,
                            loop_start,
                            loop_end,
                            looping: clip.source.looping,
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
                    if let Some(range) = clip.loop_range {
                        range.validate_source(clip.source_start, clip.source_end)?;
                    }
                    crate::SessionCellContent::Audio(crate::SessionAudioClip {
                        active: clip.active,
                        flavor: clip.flavor,
                        id: ClipId(clip.id),
                        name: clip.name,
                        asset: AssetId(clip.asset),
                        source_start: clip.source_start,
                        source_end: clip.source_end,
                        loop_range: clip.loop_range,
                        gain: clip.gain,
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
        project.main_devices = main_devices;
        project.session = crate::SessionGrid { scenes, cells };
        project.loop_region = snapshot.loop_region;
        project.master_gain = snapshot.master_gain;
        for track in project.tracks() {
            for device in track.devices().iter().chain(track.instrument()) {
                if let Some(comparison) = device.comparison() {
                    comparison.validate(device.state())?;
                }
            }
        }
        for track in project.return_tracks() {
            for device in track.devices() {
                if let Some(comparison) = device.comparison() {
                    comparison.validate(device.state())?;
                }
            }
        }
        for device in project.main_devices() {
            if let Some(comparison) = device.comparison() {
                comparison.validate(device.state())?;
            }
        }
        let aliases = std::mem::take(&mut project.aliases);
        aliases.validate_and_bind(&mut project)?;
        project.aliases = aliases;
        project.validate_retained_audio_roots()?;
        project.validate_device_asset_references()?;
        project.validate_device_automation()?;
        project.validate_device_modulation()?;
        for track in &project.tracks {
            for clip in track.clips() {
                if let Some(edit) = &clip.edit {
                    let frames = project
                        .asset(clip.asset)
                        .ok_or(Invalid("audio domain media missing"))?
                        .playback_frames();
                    edit.validate(clip, project.tempo_map(), frames)?;
                }
            }
        }
        project.validate_serial_units()?;
        project.validate_source_bindings()?;
        project.validate_track_hierarchy()?;
        if !return_feeds.is_empty() || !audio_feeds.is_empty() || !midi_routes.is_empty() {
            let mut candidate = project.snapshot();
            candidate.return_feeds = return_feeds;
            candidate.audio_feeds = audio_feeds;
            candidate.midi_routes = midi_routes;
            project.routing = Self::validate_return_feed_snapshot(&candidate)?;
            project.adopt_playable_snapshot(&candidate)?;
        }
        if !sidechains.is_empty() {
            project.adopt_sidechains(sidechains)?;
        }
        project.validate_arrangement_occupancy()?;
        project.validate_midi_volumes()?;
        Ok(project)
    }
}

pub(crate) fn insert_id(
    id: u64,
    seen: &mut BTreeSet<u64>,
    max_id: &mut u64,
) -> Result<(), Invalid> {
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
    /// Keep automatic-name origins aligned after an intentional render
    /// projection removed tracks. Surviving tracks keep their origin. Like the
    /// alias projection below, this is not a decoder repair; admit the result
    /// through Project::from_snapshot.
    pub fn retain_automatic_track_names_after_projection(&mut self) {
        let live: BTreeSet<u64> = self.tracks.iter().map(|track| track.id).collect();
        self.automatic_track_names.retain(|id| live.contains(id));
    }

    /// Keep alias references aligned after an intentional render projection of
    /// a snapshot captured from a validated Project. Removed placements release
    /// membership; a surviving source keeps its original identity and payload.
    /// This is not a decoder repair or validation substitute. Admit the resulting
    /// snapshot through Project::from_snapshot before preparing playback.
    pub fn retain_clip_aliases_after_projection(&mut self) {
        if self.aliases.members.is_empty() {
            return;
        }
        let mut live = BTreeSet::new();
        for track in &self.tracks {
            match &track.content {
                TrackContentSnapshot::Audio(clips) => live.extend(clips.iter().map(|c| c.id)),
                TrackContentSnapshot::Instrument(clips) => live.extend(clips.iter().map(|c| c.id)),
                TrackContentSnapshot::Hybrid {
                    audio_clips,
                    midi_clips,
                } => {
                    live.extend(audio_clips.iter().map(|c| c.id));
                    live.extend(midi_clips.iter().map(|c| c.id));
                }
            }
        }
        live.extend(
            self.session
                .cells
                .iter()
                .filter_map(|cell| match &cell.content {
                    SessionCellContentSnapshot::Audio(clip) => Some(clip.id),
                    SessionCellContentSnapshot::Midi(clip) => Some(clip.id),
                    SessionCellContentSnapshot::Stop => None,
                }),
        );
        self.aliases
            .members
            .retain(|member| live.contains(&member.clip));
        let sources: BTreeSet<_> = self
            .aliases
            .members
            .iter()
            .map(|member| member.source)
            .collect();
        self.aliases
            .sources
            .retain(|source| sources.contains(&source.id));
    }

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
