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
    /// Existing identities only, with the exact collateral retirement count.
    /// A group may retire selected members after deterministic author priority;
    /// a single update additionally proves every victim's exact left prefix.
    OverwriteUpdate {
        only: Option<NoteId>,
        removed: usize,
    },
    /// One canonical composite authoring edge. Only existing positive prefixes
    /// and an exact new-ID range survive; counts bind all collateral retirement
    /// and truncation. Original input footprints can outlive an erased author,
    /// so a multi-author edge cannot be rederived from final authors alone.
    AuthorEdit {
        first: u64,
        inserted: usize,
        removed: usize,
        truncated: usize,
        single: bool,
    },
    Edit {
        first: u64,
        inserted: usize,
        removed: usize,
        only_removed: Option<NoteId>,
    },
}
impl Notes {
    fn authoring(first: u64, inserted: usize, removed: usize) -> Self {
        if inserted == 0 {
            Self::Edit {
                first,
                inserted,
                removed,
                only_removed: None,
            }
        } else {
            Self::AuthorEdit {
                first,
                inserted,
                removed: 0,
                truncated: 0,
                single: inserted == 1 && removed == 0,
            }
        }
    }
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
        source_length: Option<BeatTime>,
        volume_range: Option<(BeatTime, BeatTime)>,
    },
    Source {
        location: Location,
    },
    Volume {
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
                volume_range: None,
                location: location(*clip)?,
                source_length: None,
                edit: Notes::authoring(source.next_id, 1, 0),
            },
            Command::AddMidiNotes { clip, notes } => Kind::Notes {
                volume_range: None,
                location: location(*clip)?,
                source_length: None,
                edit: Notes::authoring(source.next_id, notes.len(), 0),
            },
            Command::EditMidiNotesWithSourceExtent {
                clip,
                remove,
                insert,
            } => Kind::Notes {
                volume_range: None,
                location: location(*clip)?,
                source_length: Some(crate::midi_batch::extended_source_length(
                    source.midi_source(*clip).ok_or(MISSING)?.source_length(),
                    insert,
                )?),
                edit: Notes::authoring(source.next_id, insert.len(), remove.len()),
            },
            Command::EditMidiNotes {
                clip,
                insert,
                remove,
            } => Kind::Notes {
                volume_range: None,
                location: location(*clip)?,
                source_length: None,
                edit: Notes::authoring(source.next_id, insert.len(), remove.len()),
            },
            Command::DuplicateMidiSourceRange { clip, start, end } => {
                let range = crate::midi_source_range::Range::capture(
                    source.midi_source(*clip).ok_or(MISSING)?,
                    *start,
                    *end,
                )?;
                Kind::Notes {
                    volume_range: Some((*start, *end)),
                    location: location(*clip)?,
                    source_length: Some(range.source_length),
                    edit: Notes::authoring(source.next_id, range.count, 0),
                }
            }
            Command::RemoveMidiNotes { clip, notes } => Kind::Notes {
                volume_range: None,
                location: location(*clip)?,
                source_length: None,
                edit: Notes::authoring(source.next_id, 0, notes.len()),
            },
            Command::UpdateMidiNotes { clip, .. } => Kind::Notes {
                volume_range: None,
                location: location(*clip)?,
                source_length: None,
                edit: Notes::OverwriteUpdate {
                    only: None,
                    removed: 0,
                },
            },
            Command::UpdateMidiNote { note, .. } | Command::RemoveMidiNote(note) => {
                let (clip, _) = crate::midi::find_note_source(source, *note).ok_or(MISSING)?;
                Kind::Notes {
                    volume_range: None,
                    location: location(clip)?,
                    source_length: None,
                    edit: if matches!(command, Command::UpdateMidiNote { .. }) {
                        Notes::OverwriteUpdate {
                            only: Some(*note),
                            removed: 0,
                        }
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
            Command::ReplaceMidiClipVolume { clip, .. } => Kind::Volume {
                location: location(*clip)?,
            },
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
            edit: Notes::OverwriteUpdate { removed, .. },
            ..
        } = &mut self.kind
        {
            *removed = source
                .midi_source(location.clip())?
                .notes
                .len()
                .checked_sub(target.midi_source(location.clip())?.notes.len())?;
        }
        if let Kind::Notes {
            location,
            edit:
                Notes::Edit {
                    first, inserted, ..
                }
                | Notes::AuthorEdit {
                    first, inserted, ..
                },
            ..
        } = &mut self.kind
        {
            let count = match created {
                Created::None if *inserted == 0 => 0,
                Created::Note(id) if *inserted == 1 && id.value() == *first => 1,
                Created::Notes {
                    clip,
                    first: a,
                    last: b,
                } if clip == location.clip() && a.value() == *first => {
                    usize::try_from(b.value().checked_sub(a.value())?.checked_add(1)?).ok()?
                }
                _ => return None,
            };
            if count > *inserted || (*inserted > 0 && count == 0) {
                return None;
            }
            // Allocate only actual surviving authors; input members erased by
            // later members consume neither IDs nor capacity.
            *inserted = count;
        }
        if let Kind::Notes {
            location,
            edit:
                Notes::AuthorEdit {
                    first,
                    inserted,
                    removed,
                    truncated,
                    ..
                },
            ..
        } = &mut self.kind
        {
            let (_, retired, shortened) = author_counts(
                source.midi_source(location.clip())?,
                target.midi_source(location.clip())?,
                *first,
                *inserted,
            )?;
            *removed = retired;
            *truncated = shortened;
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
            Kind::Notes { location, .. }
            | Kind::Source { location }
            | Kind::Volume { location } => match location {
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
                                    && birth_volume(source, copied, &c.volume)
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
                            && matches!(target.session_cell(track,scene), Some(SessionCellContent::Midi(c)) if c.id == clip && birth_volume(source,copied,&c.volume) && birth_source(source,&c.name,c.flavor,&c.source,clip,copied,length))
                    }
                    Kind::ConsoleDeath { track, scene, clip } => {
                        matches!(source.session_cell(track,scene),Some(SessionCellContent::Midi(c)) if c.id == clip)
                            && target.session_cell(track, scene).is_none()
                    }
                    Kind::Notes {
                        location,
                        edit,
                        source_length,
                        volume_range,
                    } => {
                        matching_clip_shell(source, target, location, false, volume_range.is_some())
                            && volume_range.is_none_or(|(start, end)| {
                                source
                                    .midi_clip_volume(location.clip())
                                    .zip(target.midi_clip_volume(location.clip()))
                                    .zip(source_length)
                                    .is_some_and(|((a, b), length)| {
                                        a.duplicated_range(start, end, length)
                                            .is_ok_and(|v| &v == b)
                                    })
                            })
                            && source
                                .midi_source(location.clip())
                                .zip(target.midi_source(location.clip()))
                                .is_some_and(|(a, b)| {
                                    source_shell_with_extent(a, b, source_length)
                                        && notes_match(a, b, edit)
                                })
                    }
                    Kind::Source { location } => {
                        matching_clip_shell(source, target, location, true, false)
                            && source
                                .midi_source(location.clip())
                                .zip(target.midi_source(location.clip()))
                                .is_some_and(|(a, b)| a.notes == b.notes)
                    }
                    Kind::Volume { location } => {
                        matching_clip_shell(source, target, location, false, true)
                            && source.midi_source(location.clip())
                                == target.midi_source(location.clip())
                    }
                    Kind::AppendScene { .. } => false,
                }
            }
        }
    }
}
fn birth_volume(source: &Project, copied: Option<ClipId>, volume: &crate::MidiClipVolume) -> bool {
    match copied {
        Some(clip) => source.midi_clip_volume(clip) == Some(volume),
        None => volume.points().is_empty(),
    }
}
fn source_shell_with_extent(a: &MidiSource, b: &MidiSource, length: Option<BeatTime>) -> bool {
    match length {
        None => source_shell(a, b),
        Some(length) => {
            a.source_length() <= length
                && b.source_length() == length
                && a.source_start() == b.source_start()
                && a.loop_start() == b.loop_start()
                && a.loop_end() == b.loop_end()
                && a.looping() == b.looping()
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
        Notes::AuthorEdit {
            first,
            inserted,
            removed,
            truncated,
            single,
        } => {
            let Some((split, retired, shortened)) = author_counts(a, b, first, inserted) else {
                return false;
            };
            retired == removed
                && shortened == truncated
                && (!single || exact_prefixes(a, &b.notes[..split], &b.notes[split..], 0))
        }
        Notes::OverwriteUpdate { only, removed } => {
            if b.notes.len().checked_add(removed) != Some(a.notes.len()) {
                return false;
            }
            let gates = if let Some(id) = only {
                let Some(author) = b.notes.iter().find(|note| note.id == id) else {
                    return false;
                };
                let Ok(gates) =
                    crate::midi_note_overwrite::prefixes(&a.notes, std::slice::from_ref(author))
                else {
                    return false;
                };
                Some(gates)
            } else {
                None
            };
            let mut current = b.notes.iter().peekable();
            for (index, old) in a.notes.iter().enumerate() {
                if let Some(new) = current.peek().filter(|new| new.id == old.id) {
                    if let Some(id) = only
                        && old.id != id
                        && !prefix_matches(
                            old,
                            new,
                            gates.as_ref().and_then(|g| g.get(index)).copied().flatten(),
                        )
                    {
                        return false;
                    }
                    current.next();
                } else if let Some(id) = only
                    && (old.id == id
                        || gates
                            .as_ref()
                            .and_then(|g| g.get(index))
                            .is_some_and(Option::is_some))
                {
                    return false;
                }
            }
            // Group authoring can retire selected members, but cannot introduce
            // identities, reorder survivors or touch any other source/owner.
            current.next().is_none()
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
            if a.notes
                .len()
                .checked_sub(split)
                .is_none_or(|missing| missing < removed)
                || only_removed.is_some_and(|id| {
                    removed != 1
                        || !a.notes.iter().any(|n| n.id == id)
                        || b.notes[..split].iter().any(|n| n.id == id)
                })
            {
                return false;
            }
            exact_prefixes(a, &b.notes[..split], &b.notes[split..], removed)
        }
    }
}
fn author_counts(
    a: &MidiSource,
    b: &MidiSource,
    first: u64,
    inserted: usize,
) -> Option<(usize, usize, usize)> {
    first.checked_add(u64::try_from(inserted).ok()?)?;
    let split = b.notes.partition_point(|note| note.id.value() < first);
    if b.notes.len().checked_sub(split)? != inserted
        || b.notes[split..]
            .iter()
            .enumerate()
            .any(|(index, note)| note.id.value() != first + index as u64)
    {
        return None;
    }
    let mut old = a.notes.iter();
    let mut truncated = 0;
    for note in &b.notes[..split] {
        let previous = old.find(|previous| previous.id == note.id)?;
        if note.duration == BeatTime::ZERO
            || note.duration > previous.duration
            || !prefix_matches(previous, note, Some(note.duration))
        {
            return None;
        }
        truncated += usize::from(note.duration != previous.duration);
    }
    Some((split, a.notes.len().checked_sub(split)?, truncated))
}

fn exact_prefixes(
    a: &MidiSource,
    retained: &[crate::MidiNote],
    authored: &[crate::MidiNote],
    removed: usize,
) -> bool {
    let Ok(gates) = crate::midi_note_overwrite::prefixes(&a.notes, authored) else {
        return false;
    };
    let mut kept = retained.iter().peekable();
    let mut explicit_removals = 0;
    for (old, gate) in a.notes.iter().zip(gates) {
        if let Some(new) = kept.peek().filter(|new| new.id == old.id) {
            if !prefix_matches(old, new, gate) {
                return false;
            }
            kept.next();
        } else if gate.is_some() {
            explicit_removals += 1;
        }
    }
    kept.next().is_none() && explicit_removals <= removed
}

fn prefix_matches(old: &crate::MidiNote, new: &crate::MidiNote, gate: Option<BeatTime>) -> bool {
    gate == Some(new.duration)
        && old.id == new.id
        && old.start == new.start
        && old.pitch == new.pitch
        && old.channel == new.channel
        && old.velocity == new.velocity
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
fn matching_clip_shell(
    a: &Project,
    b: &Project,
    location: Location,
    source_edit: bool,
    volume_edit: bool,
) -> bool {
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
                            volume,
                        } = a;
                        name == &b.name
                            && flavor == &b.flavor
                            && (volume_edit || volume == &b.volume)
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
                        volume,
                    } = a;
                    *id == clip
                        && b.id == clip
                        && *name == b.name
                        && *flavor == b.flavor
                        && (volume_edit || volume == &b.volume)
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
