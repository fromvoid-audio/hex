//! Validated MIDI clip source, arrangement and note metadata.
use crate::edit::{Command, MidiNoteUpdate, name_valid};
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MAX_TRACKS, MidiSourcePlayback,
    NoteId, Project, Track, TrackId,
};
use std::sync::Arc;

/// Independently-owned MIDI event source shared by Arrangement and Session clips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSource {
    pub(crate) source_length: BeatTime,
    pub(crate) source_start: BeatTime,
    pub(crate) loop_start: BeatTime,
    pub(crate) loop_end: BeatTime,
    pub(crate) looping: bool,
    pub(crate) notes: Vec<MidiNote>,
}

/// Stable owning key for detecting an unchanged copy-on-write source.
/// Holding the token prevents source-address reuse while cached artifacts use it.
#[derive(Debug, Clone)]
pub struct MidiSourceIdentity(Arc<MidiSource>);
impl MidiSourceIdentity {
    pub(crate) fn of(source: &Arc<MidiSource>) -> Self {
        Self(Arc::clone(source))
    }
}
impl PartialEq for MidiSourceIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for MidiSourceIdentity {}
impl std::hash::Hash for MidiSourceIdentity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.0), state);
    }
}

impl MidiSource {
    pub fn source_length(&self) -> BeatTime {
        self.source_length
    }
    pub fn source_start(&self) -> BeatTime {
        self.source_start
    }
    pub fn loop_start(&self) -> BeatTime {
        self.loop_start
    }
    pub fn loop_end(&self) -> BeatTime {
        self.loop_end
    }
    pub fn looping(&self) -> bool {
        self.looping
    }
    pub fn notes(&self) -> &[MidiNote] {
        &self.notes
    }
    pub(crate) fn defaults(length: BeatTime) -> Self {
        Self {
            source_length: length,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: length,
            looping: false,
            notes: Vec::new(),
        }
    }
}

/// A MIDI Arrangement clip has finite timeline placement and an independent source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClip {
    pub(crate) id: ClipId,
    pub(crate) start: BeatTime,
    pub(crate) length: BeatTime,
    pub(crate) source: Arc<MidiSource>,
}
impl MidiClip {
    pub fn id(&self) -> ClipId {
        self.id
    }
    pub fn start(&self) -> BeatTime {
        self.start
    }
    pub fn length(&self) -> BeatTime {
        self.length
    }
    /// Finite duration of this clip on the arrangement timeline.
    pub fn arrangement_length(&self) -> BeatTime {
        self.length
    }
    pub fn source(&self) -> &MidiSource {
        &self.source
    }
    pub fn source_identity(&self) -> MidiSourceIdentity {
        MidiSourceIdentity::of(&self.source)
    }
    pub fn source_length(&self) -> BeatTime {
        self.source.source_length
    }
    pub fn source_start(&self) -> BeatTime {
        self.source.source_start
    }
    pub fn loop_start(&self) -> BeatTime {
        self.source.loop_start
    }
    pub fn loop_end(&self) -> BeatTime {
        self.source.loop_end
    }
    pub fn looping(&self) -> bool {
        self.source.looping
    }
    pub fn notes(&self) -> &[MidiNote] {
        &self.source.notes
    }

    /// Return the validated finite source mapping used by playback compilers.
    pub fn source_playback(&self) -> Result<MidiSourcePlayback, Invalid> {
        MidiSourcePlayback::new(
            self.source.source_length,
            self.source.source_start,
            self.source
                .looping
                .then_some((self.source.loop_start, self.source.loop_end)),
            self.length,
        )
    }
}
impl std::ops::Deref for MidiClip {
    type Target = MidiSource;
    fn deref(&self) -> &Self::Target {
        &self.source
    }
}
impl std::ops::DerefMut for MidiClip {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.source)
    }
}

/// A note whose start is relative to its containing MIDI clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiNote {
    pub(crate) id: NoteId,
    pub(crate) start: BeatTime,
    pub(crate) duration: BeatTime,
    pub(crate) pitch: u8,
    pub(crate) channel: u8,
    pub(crate) velocity: u8,
}
impl MidiNote {
    pub fn id(&self) -> NoteId {
        self.id
    }
    pub fn start(&self) -> BeatTime {
        self.start
    }
    pub fn duration(&self) -> BeatTime {
        self.duration
    }
    pub fn pitch(&self) -> u8 {
        self.pitch
    }
    pub fn channel(&self) -> u8 {
        self.channel
    }
    pub fn velocity(&self) -> u8 {
        self.velocity
    }
}

/// Audio and MIDI arrangement content owned by one stable track identity.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackContent {
    pub(crate) audio_clips: Vec<crate::Clip>,
    pub(crate) midi_clips: Vec<MidiClip>,
}
impl TrackContent {
    pub fn audio_clips(&self) -> &[crate::Clip] {
        &self.audio_clips
    }
    pub fn midi_clips(&self) -> &[MidiClip] {
        &self.midi_clips
    }
}

pub(crate) fn validate_clip(start: BeatTime, length: BeatTime) -> Result<(), Invalid> {
    if length == BeatTime::ZERO {
        return Err(Invalid("midi clip length must be positive"));
    }
    start
        .raw()
        .checked_add(length.raw())
        .ok_or(Invalid("midi clip timeline end overflow"))?;
    Ok(())
}

pub(crate) fn validate_source(
    source_length: BeatTime,
    source_start: BeatTime,
    loop_start: BeatTime,
    loop_end: BeatTime,
    looping: bool,
) -> Result<(), Invalid> {
    if source_length == BeatTime::ZERO {
        return Err(Invalid("midi source length must be positive"));
    }
    if loop_start >= loop_end || loop_end > source_length {
        return Err(Invalid("midi source loop must be inside its source"));
    }
    if looping && source_start >= loop_end {
        return Err(Invalid("midi source start must precede its loop end"));
    }
    if !looping && source_start >= source_length {
        return Err(Invalid("midi source start must be inside its source"));
    }
    Ok(())
}

pub(crate) fn validate_note(
    start: BeatTime,
    duration: BeatTime,
    length: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
) -> Result<(), Invalid> {
    if duration == BeatTime::ZERO {
        return Err(Invalid("midi note duration must be positive"));
    }
    if pitch > 127 {
        return Err(Invalid("midi note pitch must be 0..127"));
    }
    if channel > 15 {
        return Err(Invalid("midi note channel must be 0..15"));
    }
    if !(1..=127).contains(&velocity) {
        return Err(Invalid("midi note velocity must be 1..127"));
    }
    let end = start
        .raw()
        .checked_add(duration.raw())
        .ok_or(Invalid("midi note end overflow"))?;
    if end > length.raw() {
        return Err(Invalid("midi note exceeds its clip"));
    }
    Ok(())
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<Created, Invalid> {
    match command {
        Command::AddMidiTrack { name } => {
            name_valid(&name)?;
            if project.tracks.len() >= MAX_TRACKS {
                return Err(Invalid("track limit reached"));
            }
            let id = TrackId(project.allocate_id()?);
            project.tracks.push(Track {
                id,
                name,
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                content: TrackContent::default(),
            });
            Ok(Created::Track(id))
        }
        Command::PlaceMidiClip {
            track,
            start,
            length,
        } => {
            validate_clip(start, length)?;
            project
                .track(track)
                .ok_or(Invalid("track no longer exists"))?;
            if clip_count(project) >= MAX_CLIPS {
                return Err(Invalid("clip limit reached"));
            }
            let id = ClipId(project.allocate_id()?);
            let clips = &mut project.track_mut(track)?.content.midi_clips;
            clips.push(MidiClip {
                id,
                start,
                length,
                source: Arc::new(MidiSource {
                    source_length: length,
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: length,
                    looping: false,
                    notes: Vec::new(),
                }),
            });
            Ok(Created::Clip(id))
        }
        Command::SetMidiClipSource {
            clip,
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping,
            length,
        } => {
            let target = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            validate_source(source_length, source_start, loop_start, loop_end, looping)?;
            validate_clip(target.start, length)?;
            for note in &target.notes {
                validate_note(
                    note.start,
                    note.duration,
                    source_length,
                    note.pitch,
                    note.channel,
                    note.velocity,
                )?;
            }
            let target = project
                .midi_clips_mut(clip)?
                .iter_mut()
                .find(|item| item.id == clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            target.length = length;
            target.source_length = source_length;
            target.source_start = source_start;
            target.loop_start = loop_start;
            target.loop_end = loop_end;
            target.looping = looping;
            Ok(Created::None)
        }
        Command::AddMidiNote {
            clip,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            let length = project
                .midi_source(clip)
                .ok_or(Invalid("midi clip no longer exists"))?
                .source_length;
            validate_note(start, duration, length, pitch, channel, velocity)?;
            if note_count(project) >= MAX_NOTES {
                return Err(Invalid("note limit reached"));
            }
            let id = NoteId(project.allocate_id()?);
            let target = project.midi_source_mut(clip)?;
            target
                .notes
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve midi note storage"))?;
            target.notes.push(MidiNote {
                id,
                start,
                duration,
                pitch,
                channel,
                velocity,
            });
            Ok(Created::Note(id))
        }
        Command::AddMidiNotes { clip, notes } => {
            crate::midi_batch::edit(project, clip, Vec::new(), notes)
        }
        Command::EditMidiNotes {
            clip,
            remove,
            insert,
        } => crate::midi_batch::edit(project, clip, remove, insert),
        Command::UpdateMidiNote {
            note,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            let (clip_id, length) =
                find_note_source(project, note).ok_or(Invalid("midi note no longer exists"))?;
            validate_note(start, duration, length, pitch, channel, velocity)?;
            let target = project
                .midi_source_mut(clip_id)?
                .notes
                .iter_mut()
                .find(|n| n.id == note)
                .ok_or(Invalid("midi note no longer exists"))?;
            target.start = start;
            target.duration = duration;
            target.pitch = pitch;
            target.channel = channel;
            target.velocity = velocity;
            Ok(Created::None)
        }
        Command::UpdateMidiNotes { clip, notes } => {
            let target = project
                .midi_source(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let indices = note_indices(target, notes.iter().map(|update| update.note))?;
            for update in &notes {
                validate_note(
                    update.start,
                    update.duration,
                    target.source_length,
                    update.pitch,
                    update.channel,
                    update.velocity,
                )?;
            }
            let target = project.midi_source_mut(clip)?;
            for (index, update) in indices.into_iter().zip(notes) {
                let note = target
                    .notes
                    .get_mut(index)
                    .ok_or(Invalid("midi note no longer exists in target clip"))?;
                apply_note_update(note, update);
            }
            Ok(Created::None)
        }
        Command::RemoveMidiNote(note) => {
            let (clip_id, _) =
                find_note_source(project, note).ok_or(Invalid("midi note no longer exists"))?;
            let source = project.midi_source_mut(clip_id)?;
            let index = source
                .notes
                .iter()
                .position(|n| n.id == note)
                .ok_or(Invalid("midi note no longer exists"))?;
            source.notes.remove(index);
            Ok(Created::None)
        }
        Command::RemoveMidiNotes { clip, notes } => {
            crate::midi_batch::edit(project, clip, notes, Vec::new())
        }
        Command::MoveMidiClip { clip, track, start } => {
            let (source_id, length, old_start) = project
                .tracks
                .iter()
                .find_map(|t| {
                    t.midi_clips()
                        .iter()
                        .find(|c| c.id == clip)
                        .map(|c| (t.id, c.length, c.start))
                })
                .ok_or(Invalid("midi clip no longer exists"))?;
            validate_clip(start, length)?;
            project
                .track(track)
                .ok_or(Invalid("destination track no longer exists"))?;
            if source_id == track && old_start == start {
                return Ok(Created::None);
            }
            let source = project.midi_clips_mut(clip)?;
            let index = source
                .iter()
                .position(|c| c.id == clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let mut moved = source.remove(index);
            moved.start = start;
            let dest = &mut project.track_mut(track)?.content.midi_clips;
            dest.push(moved);
            Ok(Created::None)
        }
        Command::RemoveMidiClip(clip) => {
            let clips = project.midi_clips_mut(clip)?;
            let index = clips
                .iter()
                .position(|c| c.id == clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            clips.remove(index);
            Ok(Created::None)
        }
        Command::DuplicateMidiClip(clip) => {
            let source = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?
                .clone();
            let start_raw = source
                .start
                .raw()
                .checked_add(source.length.raw())
                .ok_or(Invalid("midi clip timeline end overflow"))?;
            let start = BeatTime::from_raw(start_raw);
            validate_clip(start, source.length)?;
            if clip_count(project) >= MAX_CLIPS {
                return Err(Invalid("clip limit reached"));
            }
            if note_count(project)
                .checked_add(source.notes.len())
                .ok_or(Invalid("note count overflow"))?
                > MAX_NOTES
            {
                return Err(Invalid("note limit reached"));
            }
            let track = project
                .tracks
                .iter()
                .find(|t| t.midi_clips().iter().any(|c| c.id == clip))
                .ok_or(Invalid("midi clip track missing"))?
                .id;
            let id = ClipId(project.allocate_id()?);
            let mut notes = Vec::with_capacity(source.notes.len());
            for n in source.source.notes.clone() {
                notes.push(MidiNote {
                    id: NoteId(project.allocate_id()?),
                    ..n
                });
            }
            let clips = &mut project.track_mut(track)?.content.midi_clips;
            clips.push(MidiClip {
                id,
                start,
                length: source.length,
                source: Arc::new(MidiSource {
                    source_length: source.source_length,
                    source_start: source.source_start,
                    loop_start: source.loop_start,
                    loop_end: source.loop_end,
                    looping: source.looping,
                    notes,
                }),
            });
            Ok(Created::Clip(id))
        }
        _ => Err(Invalid("not a midi command")),
    }
}

pub(crate) fn note_indices(
    clip: &MidiSource,
    ids: impl Iterator<Item = NoteId> + Clone,
) -> Result<Vec<usize>, Invalid> {
    let count = ids.clone().count();
    if count == 0 {
        return Err(Invalid("midi note batch must not be empty"));
    }
    if count > MAX_NOTES {
        return Err(Invalid("midi note batch exceeds note limit"));
    }

    let mut sorted_ids = Vec::new();
    sorted_ids
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve midi note batch validation storage"))?;
    sorted_ids.extend(ids.clone());
    sorted_ids.sort_unstable();
    if sorted_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Invalid("midi note batch contains duplicate ids"));
    }

    let mut indices = Vec::new();
    indices
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve midi note batch indices"))?;
    if count <= 8 {
        for id in ids {
            let index = clip
                .notes
                .iter()
                .position(|note| note.id == id)
                .ok_or(Invalid("midi note no longer exists in target clip"))?;
            indices.push(index);
        }
        return Ok(indices);
    }

    let mut by_id = Vec::new();
    by_id
        .try_reserve_exact(clip.notes.len())
        .map_err(|_| Invalid("could not reserve midi note lookup storage"))?;
    by_id.extend(
        clip.notes
            .iter()
            .enumerate()
            .map(|(index, note)| (note.id, index)),
    );
    by_id.sort_unstable_by_key(|(id, _)| *id);
    for id in ids {
        let index = by_id
            .binary_search_by_key(&id, |(note_id, _)| *note_id)
            .map(|position| by_id[position].1)
            .map_err(|_| Invalid("midi note no longer exists in target clip"))?;
        indices.push(index);
    }
    Ok(indices)
}

fn apply_note_update(note: &mut MidiNote, update: MidiNoteUpdate) {
    note.start = update.start;
    note.duration = update.duration;
    note.pitch = update.pitch;
    note.channel = update.channel;
    note.velocity = update.velocity;
}

fn clip_count(project: &Project) -> usize {
    project
        .tracks
        .iter()
        .map(|t| t.clips().len() + t.midi_clips().len())
        .sum::<usize>()
        + project
            .session
            .cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.content,
                    crate::SessionCellContent::Midi(_) | crate::SessionCellContent::Audio(_)
                )
            })
            .count()
}
pub(crate) fn note_count(project: &Project) -> usize {
    project
        .tracks
        .iter()
        .flat_map(Track::midi_clips)
        .map(|c| c.source.notes.len())
        .sum::<usize>()
        + project
            .session
            .cells
            .iter()
            .map(|cell| match &cell.content {
                crate::SessionCellContent::Midi(clip) => clip.source.notes.len(),
                crate::SessionCellContent::Audio(_) => 0,
                crate::SessionCellContent::Stop => 0,
            })
            .sum::<usize>()
}

pub(crate) fn find_note_source(project: &Project, note: NoteId) -> Option<(ClipId, BeatTime)> {
    project
        .tracks
        .iter()
        .flat_map(Track::midi_clips)
        .find_map(|clip| {
            clip.source
                .notes
                .iter()
                .any(|item| item.id == note)
                .then_some((clip.id, clip.source.source_length))
        })
        .or_else(|| {
            project
                .session
                .cells
                .iter()
                .find_map(|cell| match &cell.content {
                    crate::SessionCellContent::Midi(clip)
                        if clip.source.notes.iter().any(|item| item.id == note) =>
                    {
                        Some((clip.id, clip.source.source_length))
                    }
                    _ => None,
                })
        })
}
