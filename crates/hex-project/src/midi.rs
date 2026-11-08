//! Validated MIDI source, Arrangement placement and note metadata.
//!
//! MIDI timing uses checked Q32.32 quarter-note beats, independent of audio frame anchors. Source
//! extent/start and retained loop markers are independent of a clip's finite Arrangement length.
//! Notes on the same pitch cannot overlap inside one source, irrespective of channel.
//! New/moved/resized authors win, retaining only an older left prefix with its ID and
//! expression; no right fragment is resurrected. Batch originals are excluded
//! together, then authors resolve in input order, later members winning. Other pitches remain
//! polyphonic. Current-format loading refuses overlapping authored notes without
//! rewriting files; loop occurrences and DSP releases retain their separate semantics.
//! Channel, pitch, velocity and positive duration are validated before mutation.
//! Source shrink refuses notes it would invalidate. Nonrepeating sources select their
//! exclusive audible end independently from retained note/source extent; repeating
//! playback always follows the existing loop interval. Source edits preserve that policy.
//! Strict note edits remain inside the source; the explicit authoring command may grow
//! extent to inserted note ends in the same transaction, without moving placement or loops.
//! A split retains finite source windows when materializing a clipped sub-frame
//! gate would lose its provenance; playable edges and authored notes remain intact.
//! Copy-on-write identity tokens retain the actual source, preventing address reuse from making
//! stale prepared artifacts appear current. The project Editor owns atomic history, not this view.
use crate::edit::{Command, MidiNoteUpdate};
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiSourcePlayback, NoteId, Project,
    Track,
};
use std::sync::Arc;

/// Exclusive audible end of a nonrepeating source; later authored notes remain retained.
/// Repeating sources always use their loop interval, independently of this policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MidiOneShotBoundary {
    #[default]
    SourceEnd,
    LoopEnd,
}

/// Authored notes and their finite source extent. Playback windows belong to
/// `MidiSource`, so changing a window can retain this immutable content backing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiContent {
    pub(crate) source_length: BeatTime,
    pub(crate) notes: Vec<MidiNote>,
}
impl MidiContent {
    pub fn source_length(&self) -> BeatTime {
        self.source_length
    }
    pub fn notes(&self) -> &[MidiNote] {
        &self.notes
    }
}

/// Effective MIDI source: immutable content with independent playback markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiSource {
    pub(crate) content: Arc<MidiContent>,
    pub(crate) source_start: BeatTime,
    pub(crate) loop_start: BeatTime,
    pub(crate) loop_end: BeatTime,
    pub(crate) looping: bool,
    pub(crate) one_shot_boundary: MidiOneShotBoundary,
}

impl std::ops::Deref for MidiSource {
    type Target = MidiContent;
    fn deref(&self) -> &Self::Target {
        &self.content
    }
}
impl std::ops::DerefMut for MidiSource {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.content)
    }
}

/// Stable owning key for detecting an unchanged copy-on-write source.
/// Holding the token prevents source-address reuse while cached artifacts use it.
#[derive(Debug, Clone)]
pub struct MidiSourceIdentity(Arc<MidiSource>);
#[path = "midi_source_weak_identity.rs"]
mod view_identity;
pub use view_identity::MidiSourceWeakIdentity;
impl MidiSourceIdentity {
    pub(crate) fn of(source: &Arc<MidiSource>) -> Self {
        Self(Arc::clone(source))
    }
    /// Conservative retained control-metadata backing, including the actual source's note
    /// capacity and Arc header/alignment. Shared clones may be charged repeatedly. This is not
    /// PCM, allocation identity, exclusive ownership, or a closed-storage proof.
    pub fn retained_control_metadata_bytes(&self) -> Option<usize> {
        std::mem::size_of::<MidiSource>()
            .checked_add(std::mem::size_of::<MidiContent>())?
            .checked_add(
                self.0
                    .notes
                    .capacity()
                    .checked_mul(std::mem::size_of::<MidiNote>())?,
            )?
            .checked_add(4usize.checked_mul(std::mem::size_of::<usize>())?)?
            .checked_add(
                std::mem::align_of::<usize>()
                    .max(std::mem::align_of::<MidiSource>())
                    .checked_sub(1)?,
            )
            .and_then(|bytes| {
                bytes.checked_add(
                    std::mem::align_of::<usize>()
                        .max(std::mem::align_of::<MidiContent>())
                        .checked_sub(1)?,
                )
            })
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
    pub fn one_shot_boundary(&self) -> MidiOneShotBoundary {
        self.one_shot_boundary
    }
    /// Exclusive end of the initial pass. Notes outside it stay in the authored source.
    pub fn effective_end(&self) -> BeatTime {
        if self.looping || self.one_shot_boundary == MidiOneShotBoundary::LoopEnd {
            self.loop_end
        } else {
            self.source_length
        }
    }
    /// Shared finite projection for Arrangement and Session preparation.
    pub fn playback_for_duration(&self, duration: BeatTime) -> Result<MidiSourcePlayback, Invalid> {
        validate_source_boundary(
            self.source_length,
            self.source_start,
            self.loop_start,
            self.loop_end,
            self.looping,
            self.one_shot_boundary,
        )?;
        MidiSourcePlayback::new_with_initial_end(
            self.source_length,
            self.source_start,
            self.looping.then_some((self.loop_start, self.loop_end)),
            duration,
            self.effective_end(),
        )
    }
    pub(crate) fn defaults(length: BeatTime) -> Self {
        Self {
            content: Arc::new(MidiContent {
                source_length: length,
                notes: Vec::new(),
            }),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: length,
            looping: false,
            one_shot_boundary: MidiOneShotBoundary::SourceEnd,
        }
    }
}

/// A MIDI Arrangement clip has finite timeline placement, independent note/source
/// identity, and a separately shared clip-owned source-beat volume curve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClip {
    pub(crate) content_view: Option<crate::ContentWindow>,
    pub(crate) active: bool,
    pub(crate) playback_offset: BeatTime,
    pub(crate) name: String,
    pub(crate) flavor: crate::ClipFlavor,
    pub(crate) id: ClipId,
    pub(crate) start: BeatTime,
    pub(crate) length: BeatTime,
    pub(crate) volume: crate::MidiClipVolume,
    pub(crate) source: Arc<MidiSource>,
}
/// Metadata-only edge candidate. A silent source prefix is materialized only on commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiClipTrim {
    content_view: Option<crate::ContentWindow>,
    playback_offset: BeatTime,
    start: BeatTime,
    length: BeatTime,
    source_start: BeatTime,
    source_prefix: BeatTime,
}
impl MidiClipTrim {
    pub fn playback_offset(self) -> BeatTime {
        self.playback_offset
    }
    pub fn start(self) -> BeatTime {
        self.start
    }
    pub fn length(self) -> BeatTime {
        self.length
    }
    pub fn source_start(self) -> BeatTime {
        self.source_start
    }
    /// Silent time prepended to a one-shot source; authored source coordinates
    /// move by this amount while their absolute musical placement stays fixed.
    pub fn source_prefix(self) -> BeatTime {
        self.source_prefix
    }
}

impl MidiClip {
    /// Authored clip activation, independent of track mute or selection.
    pub fn active(&self) -> bool {
        self.active
    }
    /// Elapsed source-playback window origin in quarter-note beats.
    pub fn playback_offset(&self) -> BeatTime {
        self.playback_offset
    }
    pub fn volume(&self) -> &crate::MidiClipVolume {
        &self.volume
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn flavor(&self) -> crate::ClipFlavor {
        self.flavor
    }
    /// Preview an edge edit in O(1) metadata work without copying authored notes.
    /// Right extension may expose silence for a one-shot. Left edits preserve the
    /// exclusive end. Inward left trims advance an elapsed playback window,
    /// retaining authored markers/notes and chasing a crossing gate for its
    /// remaining duration. Left extension consumes that window first, then uses
    /// the source-marker/blank-prefix rewind law. Silent one-shot tails remain
    /// valid windows; loop phase is projected from the retained source law.
    pub fn preview_edge_trim(
        &self,
        edge: crate::TrimEdge,
        beat: BeatTime,
    ) -> Result<MidiClipTrim, Invalid> {
        validate_clip(self.start, self.length)?;
        self.source_playback()?;
        let start = self.start.raw();
        if let Some(view) = self.content_view {
            let end = start
                .checked_add(self.length.raw())
                .ok_or(Invalid("midi end overflow"))?;
            let (new_start, new_end) = match edge {
                crate::TrimEdge::Left => (beat.raw(), end),
                crate::TrimEdge::Right => (start, beat.raw()),
            };
            let length = new_end
                .checked_sub(new_start)
                .filter(|n| *n > 0)
                .ok_or(Invalid("midi trim must retain positive length"))?;
            let delta = i128::from(new_start) - i128::from(start);
            let raw_end = if view.reversed {
                i128::from(view.end) - delta
            } else {
                i128::from(view.start) + delta + i128::from(length)
            };
            let raw_start = raw_end - i128::from(length);
            let rewind = u64::try_from((-raw_start).max(0))
                .map_err(|_| Invalid("midi directed trim overflow"))?;
            let source_start=self.source_start().raw().checked_sub(rewind).ok_or(Invalid("midi extension precedes retained source; a forward clip can add a blank prefix"))?;
            let window = crate::ContentWindow::new(
                u64::try_from(raw_start + i128::from(rewind))
                    .map_err(|_| Invalid("midi directed trim overflow"))?,
                u64::try_from(raw_end + i128::from(rewind))
                    .map_err(|_| Invalid("midi directed trim overflow"))?,
                view.reversed,
            )?;
            let offset = self
                .playback_offset
                .raw()
                .checked_add(rewind)
                .ok_or(Invalid("midi directed trim offset overflow"))?;
            return Ok(MidiClipTrim {
                content_view: Some(window),
                playback_offset: BeatTime::from_raw(offset),
                start: BeatTime::from_raw(new_start),
                length: BeatTime::from_raw(length),
                source_start: BeatTime::from_raw(source_start),
                source_prefix: BeatTime::ZERO,
            });
        }
        let end = start
            .checked_add(self.length.raw())
            .ok_or(Invalid("midi timeline end overflow"))?;
        if self.playback_offset != BeatTime::ZERO
            || matches!(edge, crate::TrimEdge::Left) && beat.raw() >= start
        {
            let offset = self.playback_offset.raw();
            if matches!(edge, crate::TrimEdge::Left)
                && offset <= start
                && beat.raw() < start - offset
            {
                // Consume the elapsed window first, then apply the established
                // source-prefix/loop rewind law before the original origin.
                let mut origin = self.clone();
                origin.start = BeatTime::from_raw(start - offset);
                origin.length = BeatTime::from_raw(
                    self.length
                        .raw()
                        .checked_add(offset)
                        .ok_or(Invalid("midi trim origin overflow"))?,
                );
                origin.playback_offset = BeatTime::ZERO;
                return origin.preview_edge_trim(edge, beat);
            }
            let new_start = match edge {
                crate::TrimEdge::Left => beat.raw().clamp(start.saturating_sub(offset), end - 1),
                crate::TrimEdge::Right => start,
            };
            let new_end = match edge {
                crate::TrimEdge::Right => beat.raw().max(start + 1),
                crate::TrimEdge::Left => end,
            };
            let new_offset =
                u64::try_from(i128::from(offset) + i128::from(new_start) - i128::from(start))
                    .map_err(|_| Invalid("midi playback trim offset overflow"))?;
            new_offset
                .checked_add(new_end - new_start)
                .ok_or(Invalid("midi playback window end overflow"))?;
            return Ok(MidiClipTrim {
                content_view: None,
                playback_offset: BeatTime::from_raw(new_offset),
                start: BeatTime::from_raw(new_start),
                length: BeatTime::from_raw(new_end - new_start),
                source_start: self.source_start(),
                source_prefix: BeatTime::ZERO,
            });
        }
        let source_start = self.source_start().raw();
        let (new_start, new_end, new_source, prefix) = match edge {
            crate::TrimEdge::Right => (
                start,
                beat.raw().clamp(start + 1, u64::MAX),
                source_start,
                0,
            ),
            crate::TrimEdge::Left => {
                let in_loop = self.looping() && source_start >= self.loop_start().raw();
                let min_start = if in_loop {
                    0
                } else if self.looping() {
                    start.saturating_sub(source_start)
                } else {
                    // Reserve room for the entire retained source, including
                    // notes currently outside the visible one-shot window.
                    start
                        .saturating_sub(source_start)
                        .saturating_sub(u64::MAX - self.source_length().raw())
                };
                let max_start = if self.looping() {
                    end - 1
                } else {
                    let remaining = self.source.effective_end().raw() - source_start - 1;
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
                    source_start.saturating_sub(start - new_start)
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
                let prefix = if self.looping() {
                    0
                } else {
                    start.saturating_sub(new_start).saturating_sub(source_start)
                };
                (new_start, end, source, prefix)
            }
        };
        let shifted = |value: BeatTime| {
            value
                .raw()
                .checked_add(prefix)
                .map(BeatTime::from_raw)
                .ok_or(Invalid("midi source prefix overflow"))
        };
        let preview = MidiClipTrim {
            content_view: None,
            playback_offset: BeatTime::ZERO,
            start: BeatTime::from_raw(new_start),
            length: BeatTime::from_raw(new_end - new_start),
            source_start: BeatTime::from_raw(new_source),
            source_prefix: BeatTime::from_raw(prefix),
        };
        validate_clip(preview.start, preview.length)?;
        validate_source_boundary(
            shifted(self.source_length())?,
            preview.source_start,
            shifted(self.loop_start())?,
            shifted(self.loop_end())?,
            self.looping(),
            self.source.one_shot_boundary,
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
    pub fn content_view(&self) -> Option<crate::ContentWindow> {
        self.content_view
    }
    pub fn source_playback(&self) -> Result<MidiSourcePlayback, Invalid> {
        let playback = self
            .source
            .playback_for_duration(self.length)?
            .with_playback_offset(self.playback_offset)?;
        self.content_view
            .map_or(Ok(playback), |view| playback.with_content_window(view))
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
    pub(crate) active: bool,
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
    pub fn active(&self) -> bool {
        self.active
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

pub(crate) fn validate_source_boundary(
    source_length: BeatTime,
    source_start: BeatTime,
    loop_start: BeatTime,
    loop_end: BeatTime,
    looping: bool,
    boundary: MidiOneShotBoundary,
) -> Result<(), Invalid> {
    validate_source(source_length, source_start, loop_start, loop_end, looping)?;
    if boundary == MidiOneShotBoundary::LoopEnd && source_start >= loop_end {
        return Err(Invalid(
            "bounded midi source start must precede its selected end",
        ));
    }
    Ok(())
}

/// Bounded preflight shared with the history proof and sampler no-op gate.
pub(crate) fn extended_update_source_length(
    current: BeatTime,
    updates: &[MidiNoteUpdate],
) -> Result<BeatTime, Invalid> {
    if updates.is_empty() || updates.len() > MAX_NOTES {
        return Err(Invalid("invalid midi update batch size"));
    }
    updates.iter().try_fold(current, |length, update| {
        let end = update
            .start
            .raw()
            .checked_add(update.duration.raw())
            .ok_or(Invalid("midi note end overflow"))?;
        Ok(BeatTime::from_raw(length.raw().max(end)))
    })
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
    let grow_source = matches!(&command, Command::UpdateMidiNotesWithSourceExtent { .. });
    match command {
        Command::ResizeMidiClipView { clip, operation } => {
            crate::midi_loop_resize::apply(project, clip, operation)
        }
        Command::AddMidiTrack { name } => {
            let id = crate::edit::insert_track_before(project, name, None)?;
            Ok(Created::Track(id))
        }
        Command::PlaceMidiClipWithOverwrite {
            track,
            start,
            length,
        } => apply(
            project,
            Command::PlaceMidiClip {
                track,
                start,
                length,
            },
        ),
        Command::PlaceMidiClip {
            track,
            start,
            length,
        } => {
            validate_clip(start, length)?;
            project
                .track(track)
                .ok_or(Invalid("track no longer exists"))?;
            let incoming = MidiClip {
                content_view: None,
                active: true,
                playback_offset: crate::BeatTime::ZERO,
                name: "clip".into(),
                flavor: crate::ClipFlavor::default(),
                id: ClipId(0),
                start,
                length,
                volume: crate::MidiClipVolume::default(),
                source: Arc::new(MidiSource::defaults(length)),
            };
            crate::clip_group::place_one(
                project,
                track,
                crate::ClipGroupContent::Midi(incoming),
                true,
            )
        }
        Command::StretchMidiClipEdge { clip, edge, beat } => {
            crate::midi_stretch::apply(project, clip, edge, beat)
        }
        Command::TrimMidiClipEdge { clip, edge, beat } => {
            let original = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let preview = original.preview_edge_trim(edge, beat)?;
            if preview.start == original.start
                && preview.length == original.length
                && preview.source_start == original.source_start()
                && preview.playback_offset == original.playback_offset
            {
                return Ok(Created::None);
            }
            let mut target = original.clone();
            if preview.source_prefix != BeatTime::ZERO {
                let shifted = |value: BeatTime| {
                    value
                        .raw()
                        .checked_add(preview.source_prefix.raw())
                        .map(BeatTime::from_raw)
                        .ok_or(Invalid("midi source prefix overflow"))
                };
                // This is the existing private history/preparation candidate.
                // Detach only this source; other Arrangement/Session owners and
                // the previous history state keep their original coordinates.
                let source = Arc::make_mut(&mut target.source);
                source.source_length = shifted(source.source_length)?;
                target.volume = target
                    .volume
                    .shifted(preview.source_prefix, source.source_length)?;
                source.loop_start = shifted(source.loop_start)?;
                source.loop_end = shifted(source.loop_end)?;
                for note in &mut source.notes {
                    note.start = shifted(note.start)?;
                }
            }
            target.content_view = preview.content_view;
            target.start = preview.start;
            target.length = preview.length;
            target.playback_offset = preview.playback_offset;
            if target.source_start() != preview.source_start {
                target.source_start = preview.source_start;
            }
            crate::clip_group::replace_midi(project, target)
        }
        Command::SplitMidiClip { clip, beat } => {
            let (track, clip_index) = project
                .tracks
                .iter()
                .find_map(|track| {
                    track
                        .midi_clips()
                        .iter()
                        .position(|item| item.id == clip)
                        .map(|index| (track.id, index))
                })
                .ok_or(Invalid("midi clip no longer exists"))?;
            let original = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?
                .clone();
            if original.content_view.is_some() {
                return crate::clip_reverse::split_midi(project, track, original, beat);
            }
            let end = original
                .start
                .raw()
                .checked_add(original.length.raw())
                .ok_or(Invalid("midi clip timeline end overflow"))?;
            if beat.raw() <= original.start.raw() || beat.raw() >= end {
                return Err(Invalid("split cursor must be inside the midi clip"));
            }
            if clip_count(project) >= MAX_CLIPS {
                return Err(Invalid("clip limit reached"));
            }
            let left_length = crate::BeatTime::from_raw(beat.raw() - original.start.raw());
            let right_length = crate::BeatTime::from_raw(end - beat.raw());
            validate_clip(original.start, left_length)?;
            validate_clip(beat, right_length)?;

            // Materialize the finite playback that belongs in the right
            // region. This includes a clipped occurrence already sounding at
            // the cut and every later loop onset, without repeating the carry
            // note at subsequent loop iterations.
            let playback = original.source_playback()?;
            let mut occurrence_bound = 0_usize;
            for note in original.notes() {
                let count = usize::try_from(
                    playback
                        .note_occurrences(note.start, note.duration)?
                        .count(),
                )
                .map_err(|_| Invalid("midi split occurrence count exceeds addressable range"))?;
                occurrence_bound = occurrence_bound
                    .checked_add(count)
                    .ok_or(Invalid("midi split occurrence count overflow"))?;
                if occurrence_bound > MAX_NOTES {
                    return Err(Invalid("midi split note expansion exceeds project limit"));
                }
            }

            project
                .track_mut(track)?
                .content
                .midi_clips
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve midi clip storage"))?;
            let mut notes = Vec::new();
            notes
                .try_reserve_exact(occurrence_bound)
                .map_err(|_| Invalid("could not reserve split midi notes"))?;
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
                    let kept_start = absolute_start.max(beat.raw());
                    let kept_end = absolute_end.min(end);
                    if kept_start < kept_end {
                        // Materializing a clipped carry as a new authored note
                        // loses the fact that its gate was clipped. A positive
                        // sub-frame fragment would then be rejected as an
                        // unplayable authored note. Retain the existing finite
                        // source window instead, so playback can distinguish
                        // the clipped occurrence without dropping its data.
                        let clipped = kept_end - kept_start < note.duration.raw();
                        if note.active
                            && clipped
                            && project
                                .tempo_map()
                                .beat_to_frame(BeatTime::from_raw(kept_start))?
                                == project
                                    .tempo_map()
                                    .beat_to_frame(BeatTime::from_raw(kept_end))?
                        {
                            let mut retained = original.clone();
                            let window_start = retained.playback_offset.raw();
                            let window_end = window_start
                                .checked_add(retained.length.raw())
                                .ok_or(Invalid("midi split source window overflow"))?;
                            retained.content_view =
                                Some(crate::ContentWindow::new(window_start, window_end, false)?);
                            return crate::clip_reverse::split_midi(project, track, retained, beat);
                        }
                        notes.push(MidiNote {
                            id: NoteId(0),
                            active: note.active,
                            start: BeatTime::from_raw(kept_start - beat.raw()),
                            duration: BeatTime::from_raw(kept_end - kept_start),
                            pitch: note.pitch,
                            channel: note.channel,
                            velocity: note.velocity,
                        });
                    }
                }
            }
            let final_notes = note_count(project)
                .checked_add(notes.len())
                .ok_or(Invalid("midi note count overflow"))?;
            if final_notes > MAX_NOTES {
                return Err(Invalid("midi note limit reached"));
            }
            let id_count = notes
                .len()
                .checked_add(1)
                .ok_or(Invalid("midi split id count overflow"))?;
            let (first_id, _) = project.allocate_id_range(id_count)?;
            let right_id = ClipId(first_id);
            for (index, note) in notes.iter_mut().enumerate() {
                let id = first_id
                    .checked_add(1)
                    .and_then(|base| base.checked_add(index as u64))
                    .ok_or(Invalid("project id space exhausted"))?;
                note.id = NoteId(id);
                validate_note(
                    note.start,
                    note.duration,
                    right_length,
                    note.pitch,
                    note.channel,
                    note.velocity,
                )?;
            }

            let right_volume = original.volume.materialized(
                original.source_playback()?,
                left_length,
                original.length,
            )?;
            let mut left = original;
            left.length = left_length;
            let right = MidiClip {
                content_view: None,
                active: left.active,
                playback_offset: crate::BeatTime::ZERO,
                name: left.name.clone(),
                flavor: left.flavor,
                id: right_id,
                start: beat,
                length: right_length,
                volume: right_volume,
                source: Arc::new(MidiSource {
                    content: std::sync::Arc::new(crate::MidiContent {
                        source_length: right_length,
                        notes,
                    }),
                    one_shot_boundary: crate::MidiOneShotBoundary::SourceEnd,
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: right_length,
                    looping: false,
                }),
            };
            let clips = &mut project.track_mut(track)?.content.midi_clips;
            clips[clip_index] = left;
            clips.insert(clip_index + 1, right);
            Ok(Created::Clip(right_id))
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
            validate_source_boundary(
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                target.one_shot_boundary(),
            )?;
            target.volume.validate_source_length(source_length)?;
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
            let mut target = target.clone();
            target.content_view = target
                .content_view
                .map(|view| view.resized(0, length.raw()))
                .transpose()?;
            target.length = length;
            if target.source_length != source_length {
                target.source_length = source_length;
            }
            target.source_start = source_start;
            target.loop_start = loop_start;
            target.loop_end = loop_end;
            target.looping = looping;
            target.source_playback()?;
            crate::clip_group::replace_midi(project, target)
        }
        Command::AddMidiNote {
            clip,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            match crate::midi_batch::edit(
                project,
                clip,
                Vec::new(),
                vec![crate::MidiNoteInsert {
                    active: true,
                    start,
                    duration,
                    pitch,
                    channel,
                    velocity,
                }],
            )? {
                Created::Notes { first, .. } => Ok(Created::Note(first)),
                _ => Err(Invalid("single midi note authoring returned no note")),
            }
        }
        Command::AddMidiNotes { clip, notes } => {
            crate::midi_batch::edit(project, clip, Vec::new(), notes)
        }
        Command::EditMidiNotesWithSourceExtent {
            clip,
            remove,
            insert,
        } => crate::midi_batch::edit_with_source_extent(project, clip, remove, insert),
        Command::EditMidiNotes {
            clip,
            remove,
            insert,
        } => crate::midi_batch::edit(project, clip, remove, insert),
        Command::DuplicateMidiSourceRange { clip, start, end } => {
            crate::midi_source_range::duplicate(project, clip, start, end)
        }
        Command::UpdateMidiNote {
            note,
            start,
            duration,
            pitch,
            channel,
            velocity,
        } => {
            let (clip_id, _) =
                find_note_source(project, note).ok_or(Invalid("midi note no longer exists"))?;
            apply(
                project,
                Command::UpdateMidiNotes {
                    clip: clip_id,
                    notes: vec![MidiNoteUpdate {
                        note,
                        start,
                        duration,
                        pitch,
                        channel,
                        velocity,
                    }],
                },
            )
        }
        Command::UpdateMidiNotes { clip, notes }
        | Command::UpdateMidiNotesWithSourceExtent { clip, notes } => {
            let target = project
                .midi_source(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let source_length = if grow_source {
                extended_update_source_length(target.source_length(), &notes)?
            } else {
                target.source_length()
            };
            let indices = note_indices(target, notes.iter().map(|update| update.note))?;
            for update in &notes {
                validate_note(
                    update.start,
                    update.duration,
                    source_length,
                    update.pitch,
                    update.channel,
                    update.velocity,
                )?;
            }
            let mut removed = Vec::new();
            let mut authored = Vec::new();
            removed
                .try_reserve_exact(notes.len())
                .map_err(|_| Invalid("could not reserve selected midi identities"))?;
            authored
                .try_reserve_exact(notes.len())
                .map_err(|_| Invalid("could not reserve authored midi geometry"))?;
            for update in &notes {
                removed.push(update.note);
                authored.push(crate::MidiNoteInsert {
                    active: true,
                    start: update.start,
                    duration: update.duration,
                    pitch: update.pitch,
                    channel: update.channel,
                    velocity: update.velocity,
                });
            }
            let mut plan = target.preview_note_overwrite(&removed, &authored)?;
            for (&index, gate) in indices.iter().zip(&plan.authored) {
                let retained = plan
                    .retained
                    .get_mut(index)
                    .ok_or(Invalid("midi note no longer exists in target clip"))?;
                *retained = *gate;
            }
            let target = project.midi_source_mut(clip)?;
            target.source_length = source_length;
            for (index, update) in indices.into_iter().zip(notes) {
                let note = target
                    .notes
                    .get_mut(index)
                    .ok_or(Invalid("midi note no longer exists in target clip"))?;
                apply_note_update(note, update);
            }
            crate::midi_note_overwrite::retain_gates(target, &plan.retained)?;
            Ok(Created::None)
        }
        Command::SetMidiNotesActive {
            clip,
            notes,
            active,
        } => {
            let source = project
                .midi_source(clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let indices = note_indices(source, notes.iter().copied())?;
            let source = project.midi_source_mut(clip)?;
            for index in indices {
                let note = source
                    .notes
                    .get_mut(index)
                    .ok_or(Invalid("midi note no longer exists"))?;
                note.active = active;
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
            initial_gain,
        } => {
            let length = project
                .midi_clip(clip)
                .ok_or(Invalid("midi clip no longer exists"))?
                .length;
            validate_clip(start, length)?;
            let track = super::edit::insert_track_before_with_gain(
                project,
                name,
                before_track,
                initial_gain,
            )?;
            apply(project, Command::MoveMidiClip { clip, track, start })?;
            Ok(Created::Track(track))
        }
        Command::MoveMidiClipWithOverwrite { clip, track, start } => {
            apply(project, Command::MoveMidiClip { clip, track, start })
        }
        Command::CopyMidiClipTo { clip, track, start } => {
            crate::clip_copy::midi_to(project, clip, track, start)
        }
        Command::MoveMidiClip { clip, track, start } => {
            let source_id = project
                .tracks
                .iter()
                .find(|t| t.midi_clips().iter().any(|c| c.id == clip))
                .map(|t| t.id)
                .ok_or(Invalid("midi clip no longer exists"))?;
            let mut incoming = project
                .midi_clip(clip)
                .cloned()
                .ok_or(Invalid("midi clip no longer exists"))?;
            validate_clip(start, incoming.length)?;
            project
                .track(track)
                .ok_or(Invalid("destination track no longer exists"))?;
            if source_id == track && incoming.start == start {
                return Ok(Created::None);
            }
            incoming.start = start;
            crate::clip_group::place_one(
                project,
                track,
                crate::ClipGroupContent::Midi(incoming),
                false,
            )
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
                .ok_or(Invalid("midi clip no longer exists"))?;
            let start = source
                .start
                .raw()
                .checked_add(source.length.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("midi clip timeline end overflow"))?;
            let track = project
                .tracks
                .iter()
                .find(|t| t.midi_clips().iter().any(|c| c.id == clip))
                .map(|t| t.id)
                .ok_or(Invalid("midi clip track missing"))?;
            crate::clip_copy::midi_to(project, clip, track, start)
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

pub(crate) fn clip_count(project: &Project) -> usize {
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
    let shared: usize = project
        .aliases
        .sources
        .values()
        .map(|content| match content {
            crate::clip_alias::Content::Midi(content) => content.notes.len(),
            _ => 0,
        })
        .sum();
    shared
        + project
            .tracks
            .iter()
            .flat_map(Track::midi_clips)
            .filter(|clip| project.clip_alias(clip.id).is_none())
            .map(|clip| clip.notes().len())
            .sum::<usize>()
        + project
            .session
            .cells
            .iter()
            .map(|cell| match &cell.content {
                crate::SessionCellContent::Midi(clip) if project.clip_alias(clip.id).is_none() => {
                    clip.source.notes.len()
                }
                _ => 0,
            })
            .sum::<usize>()
}

pub(crate) fn midi_overwrite_frame_bounds(
    tempo: &crate::TempoMap,
    start_frame: u64,
    end_frame: u64,
) -> Result<(BeatTime, BeatTime), Invalid> {
    let start_bound = if start_frame == u64::MAX {
        crate::TempoMap::BEAT_RANGE_SENTINEL
    } else {
        tempo.beat_lower_bound_for_frame(start_frame + 1)?
    };
    let left_raw = start_bound.saturating_sub(1).min(u128::from(u64::MAX)) as u64;
    let right_raw = tempo
        .beat_lower_bound_for_frame(end_frame)?
        .min(u128::from(u64::MAX)) as u64;
    Ok((BeatTime::from_raw(left_raw), BeatTime::from_raw(right_raw)))
}

pub(crate) fn overwrite_occurrence_count(clip: &MidiClip) -> Result<usize, Invalid> {
    let playback = clip.source_playback()?;
    clip.notes().iter().try_fold(0_usize, |total, note| {
        let plan = playback.note_occurrences(note.start, note.duration)?;
        let count = usize::try_from(plan.count())
            .map_err(|_| Invalid("midi overwrite occurrence count exceeds addressable range"))?;
        total
            .checked_add(count)
            .ok_or(Invalid("midi overwrite occurrence count overflow"))
    })
}

/// Materialize all retained non-prefix ranges in one bounded occurrence pass.
/// The caller accounts enumeration separately from notes actually retained.
pub(crate) fn materialize_overwrite_ranges(
    original: &MidiClip,
    ranges: &[(u64, u64)],
) -> Result<Vec<MidiClip>, Invalid> {
    materialize_overwrite_ranges_inner(original, ranges, None)
}

/// Frame-based overwrite retains only the audible part of a clipped occurrence.
/// A generated remainder with both edges in the same frame has no surviving
/// playback. Unclipped authored notes remain intact, including invalid gates
/// that the engine must refuse. Beat-only section edits keep their existing law.
pub(crate) fn materialize_overwrite_frame_ranges(
    original: &MidiClip,
    ranges: &[(u64, u64)],
    tempo: &crate::TempoMap,
) -> Result<Vec<MidiClip>, Invalid> {
    materialize_overwrite_ranges_inner(original, ranges, Some(tempo))
}

fn materialize_overwrite_ranges_inner(
    original: &MidiClip,
    ranges: &[(u64, u64)],
    frame_clipping: Option<&crate::TempoMap>,
) -> Result<Vec<MidiClip>, Invalid> {
    let mut fragments = Vec::new();
    fragments
        .try_reserve_exact(ranges.len())
        .map_err(|_| Invalid("could not reserve midi overwrite fragment storage"))?;
    for &(start, end) in ranges {
        let length = BeatTime::from_raw(
            end.checked_sub(start)
                .filter(|n| *n > 0)
                .ok_or(Invalid("midi retained range must be positive"))?,
        );
        fragments.push(MidiClip {
            content_view: None,
            active: original.active,
            playback_offset: crate::BeatTime::ZERO,
            name: original.name.clone(),
            flavor: original.flavor,
            id: original.id,
            start: BeatTime::from_raw(start),
            length,
            volume: original.volume.materialized(
                original.source_playback()?,
                BeatTime::from_raw(
                    start
                        .checked_sub(original.start.raw())
                        .ok_or(Invalid("midi volume fragment precedes clip"))?,
                ),
                BeatTime::from_raw(
                    end.checked_sub(original.start.raw())
                        .ok_or(Invalid("midi volume fragment precedes clip"))?,
                ),
            )?,
            source: Arc::new(MidiSource {
                content: std::sync::Arc::new(crate::MidiContent {
                    source_length: length,
                    notes: Vec::new(),
                }),
                one_shot_boundary: crate::MidiOneShotBoundary::SourceEnd,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: length,
                looping: false,
            }),
        });
    }
    let playback = original.source_playback()?;
    let mut generated = 0_usize;
    for note in original.notes() {
        for occurrence in playback.note_occurrences(note.start, note.duration)?.iter() {
            let absolute_start = original
                .start
                .raw()
                .checked_add(occurrence.timeline_start().raw())
                .ok_or(Invalid("midi note timeline overflow"))?;
            let absolute_end = absolute_start
                .checked_add(occurrence.gate_duration().raw())
                .ok_or(Invalid("midi note gate overflow"))?;
            let first = ranges.partition_point(|(_, end)| *end <= absolute_start);
            for (&(start, end), fragment) in ranges[first..].iter().zip(&mut fragments[first..]) {
                if start >= absolute_end {
                    break;
                }
                let kept_start = absolute_start.max(start);
                let kept_end = absolute_end.min(end);
                if kept_start < kept_end {
                    if (kept_start != absolute_start || kept_end != absolute_end)
                        && let Some(tempo) = frame_clipping
                        && tempo.beat_to_frame(BeatTime::from_raw(kept_start))?
                            == tempo.beat_to_frame(BeatTime::from_raw(kept_end))?
                    {
                        continue;
                    }
                    generated = generated
                        .checked_add(1)
                        .filter(|n| *n <= MAX_NOTES)
                        .ok_or(Invalid("midi note limit reached"))?;
                    let source = Arc::get_mut(&mut fragment.source)
                        .ok_or(Invalid("midi overwrite source was unexpectedly shared"))?;
                    source
                        .notes
                        .try_reserve(1)
                        .map_err(|_| Invalid("could not reserve midi overwrite notes"))?;
                    source.notes.push(MidiNote {
                        id: NoteId(0),
                        active: note.active,
                        start: BeatTime::from_raw(kept_start - start),
                        duration: BeatTime::from_raw(kept_end - kept_start),
                        pitch: note.pitch,
                        channel: note.channel,
                        velocity: note.velocity,
                    });
                }
            }
        }
    }
    Ok(fragments)
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

#[cfg(test)]
mod retained_metadata_tests {
    use super::*;
    #[test]
    fn source_quote_keeps_unused_note_capacity_and_shared_clone_backing() {
        let mut source = MidiSource::defaults(BeatTime::ONE);
        let empty = MidiSourceIdentity(Arc::new(source.clone()))
            .retained_control_metadata_bytes()
            .unwrap();
        source.notes.reserve_exact(16);
        assert!(source.notes.is_empty());
        let expected_extra = source.notes.capacity() * std::mem::size_of::<MidiNote>();
        let identity = MidiSourceIdentity(Arc::new(source));
        assert_eq!(
            identity.retained_control_metadata_bytes().unwrap() - empty,
            expected_extra
        );
        assert_eq!(
            identity.clone().retained_control_metadata_bytes(),
            identity.retained_control_metadata_bytes()
        );
    }
}

#[cfg(test)]
#[path = "midi_content_tests.rs"]
mod content_tests;
