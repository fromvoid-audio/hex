//! Private, nonowning provenance for one canonical MIDI command in global history.
use crate::{
    BeatTime, ClipId, Command, Created, GainLane, Invalid, MidiClip, MidiSource, NoteId, Project,
    SceneId, SessionCellContent, Track, TrackContent, TrackId,
};

#[derive(Clone, Copy, Debug)]
enum Location {
    Arrange {
        track: TrackId,
        clip: ClipId,
    },
    Console {
        track: TrackId,
        scene: SceneId,
        clip: ClipId,
    },
}
impl Location {
    fn clip(self) -> ClipId {
        match self {
            Self::Arrange { clip, .. } | Self::Console { clip, .. } => clip,
        }
    }
    fn find(project: &Project, clip: ClipId) -> Option<Self> {
        project
            .tracks
            .iter()
            .find(|t| t.midi_clips().iter().any(|c| c.id == clip))
            .map(|t| Self::Arrange { track: t.id, clip })
            .or_else(|| {
                project.session.cells.iter().find_map(|c| match &c.content {
                    SessionCellContent::Midi(m) if m.id == clip => Some(Self::Console {
                        track: c.track,
                        scene: c.scene,
                        clip,
                    }),
                    _ => None,
                })
            })
    }
}
#[derive(Clone, Copy, Debug)]
enum Notes {
    Update(Option<NoteId>),
    Edit {
        first: u64,
        inserted: usize,
        removed: usize,
        only_removed: Option<NoteId>,
    },
}
#[derive(Clone, Copy, Debug)]
enum Kind {
    AppendScene {
        scene: SceneId,
    },
    ArrangeBirth {
        track: TrackId,
        clip: ClipId,
        start: BeatTime,
        length: BeatTime,
        copied: Option<ClipId>,
    },
    ArrangeDeath {
        track: TrackId,
        clip: ClipId,
    },
    ConsoleBirth {
        track: TrackId,
        scene: SceneId,
        clip: ClipId,
        copied: Option<ClipId>,
        length: BeatTime,
    },
    ConsoleDeath {
        track: TrackId,
        scene: SceneId,
        clip: ClipId,
    },
    Notes {
        location: Location,
        edit: Notes,
    },
    Source {
        location: Location,
    },
}
/// Inseparable fixed metadata of a canonical forward edge. No caller can mint it
/// or install it in history, and it retains no source, command vector or audio owner.
#[derive(Clone, Copy, Debug)]
pub struct MidiHistoryEffect {
    kind: Kind,
}
#[derive(Clone, Copy)]
pub(crate) struct Plan {
    kind: Kind,
}

impl Plan {
    pub(crate) fn capture(command: &Command, source: &Project) -> Result<Option<Self>, Invalid> {
        const MISSING: Invalid = Invalid("midi edit target is unavailable");
        let location = |clip| Location::find(source, clip).ok_or(MISSING);
        let kind = match command {
            Command::AddScene { .. } => Kind::AppendScene { scene: SceneId(0) },
            Command::PlaceMidiClip {
                track,
                start,
                length,
            }
            | Command::PlaceMidiClipWithOverwrite {
                track,
                start,
                length,
            } => {
                let t = source.track(*track).ok_or(MISSING)?;
                if matches!(command, Command::PlaceMidiClipWithOverwrite { .. }) {
                    let end = start
                        .raw()
                        .checked_add(length.raw())
                        .ok_or(Invalid("midi placement overflow"))?;
                    if t.midi_clips().iter().any(|c| {
                        c.start.raw() < end
                            && c.start.raw().saturating_add(c.length.raw()) > start.raw()
                    }) || t.clips().iter().any(|c| {
                        let a = source.tempo_map().frame_to_beat(c.start()).ok();
                        let b = c
                            .start()
                            .checked_add(c.duration())
                            .and_then(|f| source.tempo_map().frame_to_beat(f).ok());
                        a.zip(b).is_none_or(|(a, b)| a.raw() < end && b > *start)
                    }) {
                        return Err(Invalid("sampler midi paint requires an empty interval"));
                    }
                }
                Kind::ArrangeBirth {
                    track: *track,
                    clip: ClipId(0),
                    start: *start,
                    length: *length,
                    copied: None,
                }
            }
            Command::DuplicateMidiClip(clip) => {
                let Location::Arrange { track, .. } = location(*clip)? else {
                    return Err(MISSING);
                };
                let c = source.midi_clip(*clip).ok_or(MISSING)?;
                Kind::ArrangeBirth {
                    track,
                    clip: ClipId(0),
                    start: BeatTime::from_raw(
                        c.start
                            .raw()
                            .checked_add(c.length.raw())
                            .ok_or(Invalid("midi placement overflow"))?,
                    ),
                    length: c.length,
                    copied: Some(*clip),
                }
            }
            Command::RemoveMidiClip(clip) => {
                let Location::Arrange { track, .. } = location(*clip)? else {
                    return Err(MISSING);
                };
                Kind::ArrangeDeath { track, clip: *clip }
            }
            Command::CreateSessionMidiClip {
                track,
                scene,
                source_length,
                ..
            } => Kind::ConsoleBirth {
                track: *track,
                scene: *scene,
                clip: ClipId(0),
                copied: None,
                length: *source_length,
            },
            Command::CopyArrangementMidiToSession {
                source: clip,
                scene,
            } => {
                let Location::Arrange { track, .. } = location(*clip)? else {
                    return Err(MISSING);
                };
                Kind::ConsoleBirth {
                    track,
                    scene: *scene,
                    clip: ClipId(0),
                    copied: Some(*clip),
                    length: source.midi_clip(*clip).ok_or(MISSING)?.source_length(),
                }
            }
            Command::ClearSessionCell { track, scene } => {
                let Some(SessionCellContent::Midi(c)) = source.session_cell(*track, *scene) else {
                    return Err(Invalid("sampler midi clear requires a midi cell"));
                };
                Kind::ConsoleDeath {
                    track: *track,
                    scene: *scene,
                    clip: c.id,
                }
            }
            Command::AddMidiNote { clip, .. } => Kind::Notes {
                location: location(*clip)?,
                edit: Notes::Edit {
                    first: source.next_id,
                    inserted: 1,
                    removed: 0,
                    only_removed: None,
                },
            },
            Command::AddMidiNotes { clip, notes } => Kind::Notes {
                location: location(*clip)?,
                edit: Notes::Edit {
                    first: source.next_id,
                    inserted: notes.len(),
                    removed: 0,
                    only_removed: None,
                },
            },
            Command::EditMidiNotes {
                clip,
                insert,
                remove,
            } => Kind::Notes {
                location: location(*clip)?,
                edit: Notes::Edit {
                    first: source.next_id,
                    inserted: insert.len(),
                    removed: remove.len(),
                    only_removed: None,
                },
            },
            Command::RemoveMidiNotes { clip, notes } => Kind::Notes {
                location: location(*clip)?,
                edit: Notes::Edit {
                    first: source.next_id,
                    inserted: 0,
                    removed: notes.len(),
                    only_removed: None,
                },
            },
            Command::UpdateMidiNotes { clip, .. } => Kind::Notes {
                location: location(*clip)?,
                edit: Notes::Update(None),
            },
            Command::UpdateMidiNote { note, .. } | Command::RemoveMidiNote(note) => {
                let (clip, _) = crate::midi::find_note_source(source, *note).ok_or(MISSING)?;
                Kind::Notes {
                    location: location(clip)?,
                    edit: if matches!(command, Command::UpdateMidiNote { .. }) {
                        Notes::Update(Some(*note))
                    } else {
                        Notes::Edit {
                            first: source.next_id,
                            inserted: 0,
                            removed: 1,
                            only_removed: Some(*note),
                        }
                    },
                }
            }
            Command::SetMidiClipSource { clip, .. }
            | Command::SetSessionMidiClipSource { clip, .. } => Kind::Source {
                location: location(*clip)?,
            },
            _ => return Ok(None),
        };
        Ok(Some(Self { kind }))
    }
    pub(crate) fn finish(
        mut self,
        created: Created,
        source: &Project,
        target: &Project,
    ) -> Option<MidiHistoryEffect> {
        match (&mut self.kind, created) {
            (Kind::AppendScene { scene }, Created::Scene(id)) => *scene = id,
            (Kind::ArrangeBirth { clip, .. }, Created::Clip(id))
            | (Kind::ConsoleBirth { clip, .. }, Created::SessionClip(id)) => *clip = id,
            (
                Kind::AppendScene { .. } | Kind::ArrangeBirth { .. } | Kind::ConsoleBirth { .. },
                _,
            ) => return None,
            _ => {}
        }
        if let Kind::Notes {
            location,
            edit: Notes::Edit {
                first, inserted, ..
            },
        } = self.kind
        {
            let last = u64::try_from(inserted)
                .ok()
                .and_then(|n| first.checked_add(n.saturating_sub(1)));
            let valid = match created {
                Created::None => inserted == 0,
                Created::Note(id) => inserted == 1 && id.value() == first,
                Created::Notes {
                    clip,
                    first: a,
                    last: b,
                } => {
                    inserted > 0
                        && clip == location.clip()
                        && a.value() == first
                        && Some(b.value()) == last
                }
                _ => false,
            };
            if !valid {
                return None;
            }
        }
        let effect = MidiHistoryEffect { kind: self.kind };
        effect.validates_forward(source, target).then_some(effect)
    }
}

impl MidiHistoryEffect {
    /// Validate the original forward direction. Undo supplies (target,current),
    /// Redo supplies (current,target); canonical ID/epoch normalization is ignored.
    pub fn validates_forward(self, source: &Project, target: &Project) -> bool {
        if !same_shell(source, target) {
            return false;
        }
        let (arrange, console) = match self.kind {
            Kind::ArrangeBirth { track, clip, .. } | Kind::ArrangeDeath { track, clip } => {
                (Some((track, clip)), None)
            }
            Kind::ConsoleBirth { track, scene, .. } | Kind::ConsoleDeath { track, scene, .. } => {
                (None, Some((track, scene)))
            }
            Kind::Notes { location, .. } | Kind::Source { location } => match location {
                Location::Arrange { track, clip } => (Some((track, clip)), None),
                Location::Console { track, scene, .. } => (None, Some((track, scene))),
            },
            Kind::AppendScene { .. } => (None, None),
        };
        for (a, b) in source.tracks.iter().zip(&target.tracks) {
            if a.content
                .midi_clips
                .iter()
                .filter(|c| arrange != Some((a.id, c.id)))
                .ne(b
                    .content
                    .midi_clips
                    .iter()
                    .filter(|c| arrange != Some((b.id, c.id))))
            {
                return false;
            }
        }
        if source
            .session
            .cells
            .iter()
            .filter(|c| console != Some((c.track, c.scene)))
            .ne(target
                .session
                .cells
                .iter()
                .filter(|c| console != Some((c.track, c.scene))))
        {
            return false;
        }
        match self.kind {
            Kind::AppendScene { scene } => {
                source.session.scenes.len().checked_add(1) == Some(target.session.scenes.len())
                    && target.session.scenes.last().is_some_and(|s| s.id == scene)
                    && source.session.scenes == target.session.scenes[..source.session.scenes.len()]
                    && !source.session.scenes.iter().any(|s| s.id == scene)
                    && !target.session.cells.iter().any(|c| c.scene == scene)
            }
            kind => {
                if source.session.scenes != target.session.scenes {
                    return false;
                }
                match kind {
                    Kind::ArrangeBirth {
                        track,
                        clip,
                        start,
                        length,
                        copied,
                    } => {
                        if source.midi_source(clip).is_some() {
                            return false;
                        }
                        target
                            .track(track)
                            .and_then(|t| t.midi_clips().last())
                            .is_some_and(|c| {
                                c.id == clip
                                    && (copied.is_some() || c.name() == "clip")
                                    && c.start == start
                                    && c.length == length
                                    && birth_source(
                                        source,
                                        c.name(),
                                        c.flavor,
                                        &c.source,
                                        clip,
                                        copied,
                                        length,
                                    )
                            })
                    }
                    Kind::ArrangeDeath { track, clip } => {
                        source
                            .track(track)
                            .is_some_and(|t| t.midi_clips().iter().any(|c| c.id == clip))
                            && target.midi_source(clip).is_none()
                    }
                    Kind::ConsoleBirth {
                        track,
                        scene,
                        clip,
                        copied,
                        length,
                    } => {
                        source.session_cell(track, scene).is_none()
                            && matches!(target.session_cell(track,scene), Some(SessionCellContent::Midi(c)) if c.id == clip && birth_source(source,&c.name,c.flavor,&c.source,clip,copied,length))
                    }
                    Kind::ConsoleDeath { track, scene, clip } => {
                        matches!(source.session_cell(track,scene),Some(SessionCellContent::Midi(c)) if c.id == clip)
                            && target.session_cell(track, scene).is_none()
                    }
                    Kind::Notes { location, edit } => {
                        matching_clip_shell(source, target, location, false)
                            && source
                                .midi_source(location.clip())
                                .zip(target.midi_source(location.clip()))
                                .is_some_and(|(a, b)| source_shell(a, b) && notes_match(a, b, edit))
                    }
                    Kind::Source { location } => {
                        matching_clip_shell(source, target, location, true)
                            && source
                                .midi_source(location.clip())
                                .zip(target.midi_source(location.clip()))
                                .is_some_and(|(a, b)| a.notes == b.notes)
                    }
                    Kind::AppendScene { .. } => false,
                }
            }
        }
    }
}
fn source_shell(a: &MidiSource, b: &MidiSource) -> bool {
    let MidiSource {
        source_length,
        source_start,
        loop_start,
        loop_end,
        looping,
        notes: _,
    } = a;
    (
        *source_length,
        *source_start,
        *loop_start,
        *loop_end,
        *looping,
    ) == (
        b.source_length,
        b.source_start,
        b.loop_start,
        b.loop_end,
        b.looping,
    )
}
fn notes_match(a: &MidiSource, b: &MidiSource, edit: Notes) -> bool {
    match edit {
        Notes::Update(only) => {
            a.notes.len() == b.notes.len()
                && a.notes
                    .iter()
                    .zip(&b.notes)
                    .all(|(x, y)| x.id == y.id && (only.is_none_or(|id| x.id == id) || x == y))
        }
        Notes::Edit {
            first,
            inserted,
            removed,
            only_removed,
        } => {
            let Some(end) = u64::try_from(inserted)
                .ok()
                .and_then(|n| first.checked_add(n))
            else {
                return false;
            };
            let split = b.notes.partition_point(|n| n.id.value() < first);
            if b.notes[split..].len() != inserted
                || b.notes[split..]
                    .iter()
                    .enumerate()
                    .any(|(i, n)| n.id.value() != first + i as u64 || n.id.value() >= end)
            {
                return false;
            }
            if a.notes.len().checked_sub(split) != Some(removed)
                || only_removed.is_some_and(|id| {
                    removed != 1
                        || !a.notes.iter().any(|n| n.id == id)
                        || b.notes[..split].iter().any(|n| n.id == id)
                })
            {
                return false;
            }
            let mut old = a.notes.iter();
            for n in &b.notes[..split] {
                if removed > 0 {
                    if !old.by_ref().any(|x| x == n) {
                        return false;
                    }
                } else if old.next() != Some(n) {
                    return false;
                }
            }
            removed > 0 || old.next().is_none()
        }
    }
}
fn birth_source(
    project: &Project,
    name: &str,
    flavor: crate::ClipFlavor,
    source: &MidiSource,
    id: ClipId,
    copied: Option<ClipId>,
    length: BeatTime,
) -> bool {
    if let Some(original) = copied.and_then(|id| project.midi_clip(id)) {
        name == original.name
            && flavor == original.flavor
            && source_shell(source, &original.source)
            && source.notes.len() == original.notes().len()
            && source
                .notes
                .iter()
                .zip(original.notes())
                .enumerate()
                .all(|(i, (a, b))| {
                    id.value()
                        .checked_add(1)
                        .and_then(|n| n.checked_add(i as u64))
                        == Some(a.id.value())
                        && (a.start, a.duration, a.pitch, a.channel, a.velocity)
                            == (b.start, b.duration, b.pitch, b.channel, b.velocity)
                })
    } else {
        copied.is_none()
            && flavor == crate::ClipFlavor::default()
            && source.notes.is_empty()
            && source.source_length == length
            && source.source_start == BeatTime::ZERO
            && source.loop_start == BeatTime::ZERO
            && source.loop_end == length
            && !source.looping
    }
}
fn matching_clip_shell(a: &Project, b: &Project, location: Location, source_edit: bool) -> bool {
    match location {
        Location::Arrange { track, clip } => {
            a.track(track).zip(b.track(track)).is_some_and(|(a, b)| {
                a.midi_clips()
                    .iter()
                    .find(|c| c.id == clip)
                    .zip(b.midi_clips().iter().find(|c| c.id == clip))
                    .is_some_and(|(a, b)| {
                        let MidiClip {
                            name,
                            flavor,
                            id,
                            start,
                            length,
                            source: _,
                        } = a;
                        name == &b.name
                            && flavor == &b.flavor
                            && id == &b.id
                            && start == &b.start
                            && (source_edit || length == &b.length)
                    })
            })
        }
        Location::Console { track, scene, clip } => {
            match (a.session_cell(track, scene), b.session_cell(track, scene)) {
                (Some(SessionCellContent::Midi(a)), Some(SessionCellContent::Midi(b))) => {
                    let crate::SessionMidiClip {
                        id,
                        name,
                        flavor,
                        source: _,
                    } = a;
                    *id == clip && b.id == clip && *name == b.name && *flavor == b.flavor
                }
                _ => false,
            }
        }
    }
}
fn same_shell(a: &Project, b: &Project) -> bool {
    let crate::SessionGrid {
        scenes: _,
        cells: _,
    } = &a.session;
    let Project {
        tuning,
        routing,
        audio_feeds,
        midi_routes,
        import_provenance,
        sample_rate,
        next_id: _,
        assets,
        tracks,
        returns,
        session: _,
        loop_region,
        master_gain,
        main_devices,
        tempo_map,
        meter_map,
    } = a;
    if tuning != &b.tuning
        || routing != &b.routing
        || audio_feeds != &b.audio_feeds
        || midi_routes != &b.midi_routes
        || import_provenance != &b.import_provenance
        || sample_rate != &b.sample_rate
        || assets != &b.assets
        || returns != &b.returns
        || loop_region != &b.loop_region
        || master_gain != &b.master_gain
        || main_devices != &b.main_devices
        || tempo_map != &b.tempo_map
        || meter_map != &b.meter_map
        || tracks.len() != b.tracks.len()
    {
        return false;
    }
    tracks.iter().zip(&b.tracks).all(|(a, b)| {
        let Track {
            color,
            gain_lane,
            id,
            name,
            gain,
            balance,
            muted,
            soloed,
            devices,
            instrument,
            instrument_position,
            content,
            sends,
        } = a;
        let GainLane {
            base,
            spans,
            recording_epoch: _,
        } = gain_lane;
        let TrackContent {
            audio_clips,
            midi_clips: _,
        } = content;
        color == &b.color
            && id == &b.id
            && name == &b.name
            && gain == &b.gain
            && balance == &b.balance
            && muted == &b.muted
            && soloed == &b.soloed
            && devices == &b.devices
            && instrument == &b.instrument
            && instrument_position == &b.instrument_position
            && sends == &b.sends
            && base == &b.gain_lane.base
            && spans == &b.gain_lane.spans
            && audio_clips == &b.content.audio_clips
    })
}

mod preflight;
#[cfg(test)]
mod tests;
