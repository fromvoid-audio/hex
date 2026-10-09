//! Validated MIDI clip source, arrangement and note metadata.
use crate::edit::{Command, MidiNoteUpdate, name_valid};
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MAX_TRACKS, MidiSourcePlayback,
    NoteId, Project, Track, TrackId,
};
use std::{collections::HashMap, sync::Arc};

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
/// Metadata-only edge candidate. Notes, IDs and source/loop bounds are retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiClipTrim {
    start: BeatTime,
    length: BeatTime,
    source_start: BeatTime,
}
impl MidiClipTrim {
    pub fn start(self) -> BeatTime {
        self.start
    }
    pub fn length(self) -> BeatTime {
        self.length
    }
    pub fn source_start(self) -> BeatTime {
        self.source_start
    }
}

impl MidiClip {
    /// Preview an edge edit in O(1) metadata work without copying authored notes.
    /// Right extension may expose silence for a one-shot. Left edits preserve the
    /// exclusive end and advance/rewind the source marker; loop-phase rewinds
    /// remain in the loop, while an intro can only rewind to source zero.
    pub fn preview_edge_trim(
        &self,
        edge: crate::TrimEdge,
        beat: BeatTime,
    ) -> Result<MidiClipTrim, Invalid> {
        validate_clip(self.start, self.length)?;
        self.source_playback()?;
        let start = self.start.raw();
        let end = start
            .checked_add(self.length.raw())
            .ok_or(Invalid("midi timeline end overflow"))?;
        let source_start = self.source_start().raw();
        let (new_start, new_end, new_source) = match edge {
            crate::TrimEdge::Right => (start, beat.raw().clamp(start + 1, u64::MAX), source_start),
            crate::TrimEdge::Left => {
                let in_loop = self.looping() && source_start >= self.loop_start().raw();
                let min_start = if in_loop {
                    0
                } else {
                    start.saturating_sub(source_start)
                };
                let max_start = if self.looping() {
                    end - 1
                } else {
                    let remaining = self.source_length().raw() - source_start - 1;
                    start
                        .checked_add(remaining.min(self.length.raw() - 1))
                        .ok_or(Invalid("midi timeline overflow"))?
                };
                let new_start = beat.raw().clamp(min_start, max_start);
                let source = if new_start == start {
                    source_start
                } else if in_loop {
                    let loop_start = self.loop_start().raw();
                    let period = self.loop_end().raw() - loop_start;
                    let phase = i128::from(source_start - loop_start) + i128::from(new_start)
                        - i128::from(start);
                    loop_start + phase.rem_euclid(i128::from(period)) as u64
                } else if new_start < start {
                    source_start - (start - new_start)
                } else {
                    let advanced = u128::from(source_start) + u128::from(new_start - start);
                    if self.looping() && advanced >= u128::from(self.loop_end().raw()) {
                        let period = self.loop_end().raw() - self.loop_start().raw();
                        self.loop_start().raw()
                            + ((advanced - u128::from(self.loop_end().raw())) % u128::from(period))
                                as u64
                    } else {
                        advanced as u64
                    }
                };
                (new_start, end, source)
            }
        };
        let preview = MidiClipTrim {
            start: BeatTime::from_raw(new_start),
            length: BeatTime::from_raw(new_end - new_start),
            source_start: BeatTime::from_raw(new_source),
        };
        validate_clip(preview.start, preview.length)?;
        validate_source(
            self.source_length(),
            preview.source_start,
            self.loop_start(),
            self.loop_end(),
            self.looping(),
        )?;
        Ok(preview)
    }
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
        Command::TrimMidiClipEdge { clip, edge, beat } => {
            let original = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let preview = original.preview_edge_trim(edge, beat)?;
            if preview.start == original.start
                && preview.length == original.length
                && preview.source_start == original.source_start()
            {
                return Ok(Created::None);
            }
            let target = project
                .midi_clips_mut(clip)?
                .iter_mut()
                .find(|item| item.id == clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            target.start = preview.start;
            target.length = preview.length;
            if target.source_start() != preview.source_start {
                target.source_start = preview.source_start;
            }
            Ok(Created::None)
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
        Command::MoveMidiClipToNewTrack {
            clip,
            name,
            before_track,
            start,
        } => {
            let length = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?
                .length;
            validate_clip(start, length)?;
            let track = super::edit::insert_track_before(project, name, before_track)?;
            apply(project, Command::MoveMidiClip { clip, track, start })?;
            Ok(Created::Track(track))
        }
        Command::MoveMidiClipWithOverwrite { clip, track, start } => {
            let (source_track, old_start, length) = project
                .tracks
                .iter()
                .find_map(|candidate| {
                    candidate
                        .midi_clips()
                        .iter()
                        .find(|item| item.id == clip)
                        .map(|item| (candidate.id, item.start, item.length))
                })
                .ok_or(Invalid("midi clip no longer exists"))?;
            if source_track == track && old_start == start {
                return Ok(Created::None);
            }
            let end = start
                .raw()
                .checked_add(length.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("midi timeline end overflow"))?;
            project
                .track(track)
                .ok_or(Invalid("destination track no longer exists"))?;
            crate::edit::overwrite_audio_in_beats(project, track, start, end, clip)?;
            overwrite_midi_interval(project, track, start, end, clip)?;
            apply(project, Command::MoveMidiClip { clip, track, start })
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

struct MidiOverwriteFragments {
    original_id: ClipId,
    left: Option<MidiClip>,
    right: Option<MidiClip>,
    right_id_is_new: bool,
}

pub(crate) fn overwrite_midi_in_frames(
    project: &mut Project,
    track: TrackId,
    start_frame: u64,
    end_frame: u64,
    excluded: ClipId,
) -> Result<(), Invalid> {
    if start_frame >= end_frame {
        return Ok(());
    }
    let tempo = project.tempo_map();
    let start_bound = if start_frame == u64::MAX {
        crate::TempoMap::BEAT_RANGE_SENTINEL
    } else {
        tempo.beat_lower_bound_for_frame(start_frame + 1)?
    };
    let left_raw = start_bound.saturating_sub(1).min(u128::from(u64::MAX)) as u64;
    let right_raw = tempo
        .beat_lower_bound_for_frame(end_frame)?
        .min(u128::from(u64::MAX)) as u64;
    overwrite_midi_interval(
        project,
        track,
        BeatTime::from_raw(left_raw),
        BeatTime::from_raw(right_raw),
        excluded,
    )
}

pub(crate) fn overwrite_midi_interval(
    project: &mut Project,
    track: TrackId,
    start: BeatTime,
    end: BeatTime,
    excluded: ClipId,
) -> Result<(), Invalid> {
    if start >= end {
        return Ok(());
    }
    let clips = project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?
        .midi_clips();
    let mut snapshot = Vec::new();
    snapshot
        .try_reserve_exact(clips.len())
        .map_err(|_| Invalid("could not reserve midi overwrite planning storage"))?;
    snapshot.extend(clips.iter().cloned());

    let mut plans = Vec::new();
    plans
        .try_reserve_exact(snapshot.len())
        .map_err(|_| Invalid("could not reserve midi overwrite fragment storage"))?;
    let mut occurrence_bound = 0_usize;
    for clip in snapshot {
        if clip.id == excluded {
            continue;
        }
        let clip_end_raw = clip
            .start
            .raw()
            .checked_add(clip.length.raw())
            .ok_or(Invalid("midi timeline end overflow"))?;
        if clip.start >= end || start.raw() >= clip_end_raw {
            continue;
        }
        let keep_left = clip.start < start;
        let keep_right = clip_end_raw > end.raw();
        let left = if keep_left {
            let clip_start_frame = project.tempo_map().beat_to_frame(clip.start)?;
            let left_end_frame = project.tempo_map().beat_to_frame(start)?;
            if clip_start_frame < left_end_frame {
                let mut left = clip.clone();
                left.length = BeatTime::from_raw(start.raw() - clip.start.raw());
                Some(left)
            } else {
                None
            }
        } else {
            None
        };
        let right_range = if keep_right {
            let right_start = BeatTime::from_raw(end.raw());
            let right_end = BeatTime::from_raw(clip_end_raw);
            let right_start_frame = project.tempo_map().beat_to_frame(right_start)?;
            let right_end_frame = project.tempo_map().beat_to_frame(right_end)?;
            (right_start_frame < right_end_frame).then_some((end.raw(), clip_end_raw))
        } else {
            None
        };
        let mut plan_occurrences = 0_usize;
        if right_range.is_some() {
            let playback = clip.source_playback()?;
            for note in clip.notes() {
                let plan = playback.note_occurrences(note.start, note.duration)?;
                let count = usize::try_from(plan.count()).map_err(|_| {
                    Invalid("midi overwrite occurrence count exceeds addressable range")
                })?;
                plan_occurrences = plan_occurrences
                    .checked_add(count)
                    .ok_or(Invalid("midi overwrite occurrence count overflow"))?;
            }
            occurrence_bound = occurrence_bound
                .checked_add(plan_occurrences)
                .ok_or(Invalid("midi overwrite occurrence count overflow"))?;
            if occurrence_bound > MAX_NOTES {
                return Err(Invalid(
                    "midi overwrite note expansion exceeds project limit",
                ));
            }
        }
        let right_id_is_new = left.is_some() && right_range.is_some();
        plans.push((
            clip.clone(),
            left,
            right_range,
            plan_occurrences,
            right_id_is_new,
        ));
    }
    if plans.is_empty() {
        return Ok(());
    }

    let total_clips = project.tracks.iter().try_fold(0_usize, |total, item| {
        total
            .checked_add(item.clips().len())
            .and_then(|value| value.checked_add(item.midi_clips().len()))
            .ok_or(Invalid("clip count overflow"))
    })?;
    let final_clips = plans
        .iter()
        .try_fold(total_clips, |count, (_, left, right, _, _)| {
            let retained = usize::from(left.is_some()) + usize::from(right.is_some());
            count
                .checked_sub(1)
                .and_then(|value| value.checked_add(retained))
                .ok_or(Invalid("clip count overflow"))
        })?;
    if final_clips > MAX_CLIPS {
        return Err(Invalid("clip limit reached"));
    }

    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(plans.len())
        .map_err(|_| Invalid("could not reserve midi overwrite candidate storage"))?;
    let mut generated_notes_total = 0_usize;
    let mut removed_notes_total = 0_usize;
    for (original, left, right_range, occurrence_count, right_id_is_new) in plans {
        if left.is_none() {
            removed_notes_total = removed_notes_total
                .checked_add(original.notes().len())
                .ok_or(Invalid("midi note count overflow"))?;
        }
        let right = if let Some((right_start, right_end)) = right_range {
            let playback = original.source_playback()?;
            let mut notes = Vec::new();
            notes
                .try_reserve_exact(occurrence_count)
                .map_err(|_| Invalid("could not reserve midi overwrite notes"))?;
            for note in original.notes() {
                let occurrences = playback.note_occurrences(note.start, note.duration)?;
                for occurrence in occurrences.iter() {
                    let absolute_start = original
                        .start
                        .raw()
                        .checked_add(occurrence.timeline_start().raw())
                        .ok_or(Invalid("midi note timeline overflow"))?;
                    let absolute_end = absolute_start
                        .checked_add(occurrence.gate_duration().raw())
                        .ok_or(Invalid("midi note gate overflow"))?;
                    let kept_start = absolute_start.max(right_start);
                    let kept_end = absolute_end.min(right_end);
                    if kept_start < kept_end {
                        notes.push(MidiNote {
                            id: NoteId(0),
                            start: BeatTime::from_raw(kept_start - right_start),
                            duration: BeatTime::from_raw(kept_end - kept_start),
                            pitch: note.pitch,
                            channel: note.channel,
                            velocity: note.velocity,
                        });
                    }
                }
            }
            generated_notes_total = generated_notes_total
                .checked_add(notes.len())
                .ok_or(Invalid("midi note count overflow"))?;
            let length = BeatTime::from_raw(right_end - right_start);
            Some(MidiClip {
                id: if right_id_is_new {
                    ClipId(0)
                } else {
                    original.id
                },
                start: BeatTime::from_raw(right_start),
                length,
                source: Arc::new(MidiSource {
                    source_length: length,
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: length,
                    looping: false,
                    notes,
                }),
            })
        } else {
            None
        };
        prepared.push(MidiOverwriteFragments {
            original_id: original.id,
            left,
            right,
            right_id_is_new,
        });
    }
    let current_notes = note_count(project);
    let final_notes = current_notes
        .checked_sub(removed_notes_total)
        .and_then(|value| value.checked_add(generated_notes_total))
        .ok_or(Invalid("midi note count overflow"))?;
    if final_notes > MAX_NOTES {
        return Err(Invalid("midi note limit reached"));
    }

    let old_len = project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?
        .midi_clips()
        .len();
    let replacement_count = prepared.iter().try_fold(0_usize, |count, plan| {
        count
            .checked_add(usize::from(plan.left.is_some()))
            .and_then(|value| value.checked_add(usize::from(plan.right.is_some())))
            .ok_or(Invalid("clip count overflow"))
    })?;
    let new_len = old_len
        .checked_sub(prepared.len())
        .and_then(|value| value.checked_add(replacement_count))
        .ok_or(Invalid("clip count overflow"))?;
    let mut replacements = Vec::new();
    replacements
        .try_reserve_exact(new_len)
        .map_err(|_| Invalid("could not reserve overwritten midi clips"))?;
    let mut by_id = HashMap::new();
    by_id
        .try_reserve(prepared.len())
        .map_err(|_| Invalid("could not reserve midi overwrite lookup"))?;
    for mut plan in prepared {
        let right = plan.right.as_mut();
        if let Some(right) = right {
            if plan.right_id_is_new {
                right.id = ClipId(project.allocate_id()?);
            }
            let source = Arc::make_mut(&mut right.source);
            for note in &mut source.notes {
                note.id = NoteId(project.allocate_id()?);
            }
        }
        by_id.insert(plan.original_id, plan);
    }
    let old = std::mem::take(&mut project.track_mut(track)?.content.midi_clips);
    for clip in old {
        if let Some(plan) = by_id.remove(&clip.id) {
            if let Some(left) = plan.left {
                replacements.push(left);
            }
            if let Some(right) = plan.right {
                replacements.push(right);
            }
        } else {
            replacements.push(clip);
        }
    }
    project.track_mut(track)?.content.midi_clips = replacements;
    Ok(())
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
