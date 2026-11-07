//! Metadata-only Arrangement loop edits. The existing source playback law owns
//! phase, note gates and elapsed windows; the occupancy planner owns replacement.
//! A preview never clones notes or becomes a caller-submitted replacement.
use crate::{
    BeatTime, Created, Invalid, MidiClip, MidiOneShotBoundary, MidiSourcePlayback, Project,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiViewResizeOperation {
    /// Explicit corner gesture: capture the visible one-shot range, or retain
    /// an authored loop, then enable Repeat and change only outer duration.
    RepeatOuterEnd { beat: BeatTime },
    /// Move the first visible repeat boundary, retaining outer occupancy and
    /// current source phase. In a directed view this authors the forward base
    /// boundary, then reapplies direction; it is not an inverse reflected-seam
    /// gesture. Notes/hidden source content are never rewritten.
    CycleEnd { beat: BeatTime },
    /// Move a forward one-shot's source start marker. Like Live's unlooped
    /// region, the Arrangement clip begins at the marker: its start moves by
    /// the same beats while its end and every note keep their song beat.
    /// Looped and directed clips keep their source controls instead.
    PlayStart { source_start: BeatTime },
}

/// Opaque Copy geometry for a loop gesture. The command recomputes against the
/// current canonical clip; this value grants no mutation or validation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiClipLoopResize {
    content_view: Option<crate::ContentWindow>,
    start: BeatTime,
    length: BeatTime,
    source_length: BeatTime,
    source_start: BeatTime,
    loop_start: BeatTime,
    loop_end: BeatTime,
    playback_offset: BeatTime,
}
impl MidiClipLoopResize {
    pub fn start(self) -> BeatTime {
        self.start
    }
    pub fn length(self) -> BeatTime {
        self.length
    }
    pub fn source_length(self) -> BeatTime {
        self.source_length
    }
    pub fn source_start(self) -> BeatTime {
        self.source_start
    }
    pub fn loop_start(self) -> BeatTime {
        self.loop_start
    }
    pub fn loop_end(self) -> BeatTime {
        self.loop_end
    }
    pub fn playback_offset(self) -> BeatTime {
        self.playback_offset
    }
    pub fn source_playback(self) -> Result<MidiSourcePlayback, Invalid> {
        let playback = MidiSourcePlayback::new(
            self.source_length,
            self.source_start,
            Some((self.loop_start, self.loop_end)),
            self.length,
        )?
        .with_playback_offset(self.playback_offset)?;
        self.content_view
            .map_or(Ok(playback), |view| playback.with_content_window(view))
    }
    /// Logical first boundary after this elapsed window's start. Consumers
    /// must range-check against outer occupancy before painting a handle.
    pub fn first_repeat_boundary(self) -> Option<BeatTime> {
        first_boundary(self.start, self.source_playback().ok()?)
    }
}

fn first_boundary(start: BeatTime, playback: MidiSourcePlayback) -> Option<BeatTime> {
    start
        .raw()
        .checked_add(playback.first_repeat_boundary()?.raw())
        .map(BeatTime::from_raw)
}

impl MidiSourcePlayback {
    /// Elapsed beats until the first repeat boundary after this window's start.
    /// This is not an absolute Arrangement beat. One-shots have no repeat marker.
    pub fn first_repeat_boundary(self) -> Option<BeatTime> {
        if self.unreversed().is_some() {
            return self
                .repeat_boundaries()
                .ok()?
                .and_then(|plan| plan.boundary(0))
                .map(BeatTime::from_raw);
        }
        let (_, end) = self.loop_range()?;
        let phase = self.source_phase_at_elapsed(BeatTime::ZERO)?;
        end.raw()
            .checked_sub(phase.raw())
            .filter(|n| *n > 0)
            .map(BeatTime::from_raw)
    }
}

impl MidiClip {
    /// Moved one-shot region for a new source start marker, without copying
    /// notes. A note at source beat n keeps its song beat
    /// `start + n - source_start - playback_offset`.
    pub fn with_play_start(&self, source_start: BeatTime) -> Result<MidiClip, Invalid> {
        if self.looping() || self.content_view.is_some() {
            return Err(Invalid(
                "looped and reversed midi clips keep their start in the source controls",
            ));
        }
        let moved = i128::from(source_start.raw()) - i128::from(self.source_start().raw());
        let start = u64::try_from(i128::from(self.start().raw()) + moved)
            .map_err(|_| Invalid("midi play start would move the clip before song start"))?;
        let length = u64::try_from(i128::from(self.length().raw()) - moved)
            .ok()
            .filter(|length| *length > 0)
            .ok_or(Invalid("midi play start must precede the clip end"))?;
        crate::midi::validate_source_boundary(
            self.source_length(),
            source_start,
            self.loop_start(),
            self.loop_end(),
            false,
            self.one_shot_boundary(),
        )?;
        let mut next = self.clone();
        next.start = BeatTime::from_raw(start);
        next.length = BeatTime::from_raw(length);
        crate::midi::validate_clip(next.start, next.length)?;
        if next.source_start() != source_start {
            std::sync::Arc::make_mut(&mut next.source).source_start = source_start;
        }
        next.source_playback()?;
        Ok(next)
    }

    /// Read-only counterpart of AudioViewProjection::logical_first_boundary.
    /// A one-shot has no repeat marker, even when it retains authored loop bounds.
    pub fn first_repeat_boundary(&self) -> Option<BeatTime> {
        self.looping().then_some(())?;
        first_boundary(self.start(), self.source_playback().ok()?)
    }

    /// Plan in constant metadata work, without enumerating notes/occurrences.
    /// Ordinary one-shots capture their current finite window, including a
    /// silent tail; previously captured Once sources retain their authored cycle.
    /// Repeats keep the canonical note-on law (no new chase at loop boundaries).
    pub fn preview_loop_resize(
        &self,
        operation: MidiViewResizeOperation,
    ) -> Result<MidiClipLoopResize, Invalid> {
        // An explicit loop gesture authors a forward cycle, then reapplies
        // the finite direction, matching audio's rebase_content contract.
        let directed = self.source_playback()?;
        let playback = directed.unreversed().unwrap_or(directed);
        let offset = playback.playback_offset();
        let beat = match operation {
            MidiViewResizeOperation::RepeatOuterEnd { beat }
            | MidiViewResizeOperation::CycleEnd { beat } => beat,
            MidiViewResizeOperation::PlayStart { .. } => {
                return Err(Invalid("midi play start is not a loop resize"));
            }
        };
        let delta = beat
            .raw()
            .checked_sub(self.start().raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("loop boundary must follow clip start"))?;
        let mut next = MidiClipLoopResize {
            content_view: self.content_view,
            start: self.start(),
            length: self.length(),
            source_length: self.source_length(),
            source_start: self.source_start(),
            loop_start: self.loop_start(),
            loop_end: self.loop_end(),
            playback_offset: offset,
        };
        match operation {
            MidiViewResizeOperation::RepeatOuterEnd { .. } => {
                if !self.looping() && self.one_shot_boundary() == MidiOneShotBoundary::SourceEnd {
                    let start = self
                        .source_start()
                        .raw()
                        .checked_add(offset.raw())
                        .ok_or(Invalid("loop source window start overflow"))?;
                    let end = start
                        .checked_add(self.length().raw())
                        .ok_or(Invalid("loop source window end overflow"))?;
                    next.loop_start = BeatTime::from_raw(start);
                    next.loop_end = BeatTime::from_raw(end);
                    next.source_length = next.source_length.max(next.loop_end);
                }
                next.length = BeatTime::from_raw(delta);
            }
            MidiViewResizeOperation::PlayStart { .. } => {
                return Err(Invalid("midi play start is not a loop resize"));
            }
            MidiViewResizeOperation::CycleEnd { .. } => {
                if !self.looping() {
                    return Err(Invalid("first repeat boundary requires an existing loop"));
                }
                let phase = playback
                    .source_phase_at_elapsed(BeatTime::ZERO)
                    .ok_or(Invalid("midi loop phase is unavailable"))?;
                next.loop_end = BeatTime::from_raw(
                    phase
                        .raw()
                        .checked_add(delta)
                        .ok_or(Invalid("midi loop end overflow"))?,
                );
                next.source_length = next.source_length.max(next.loop_end);
                // A retained window can start after many repetitions. Changing
                // the cycle must retain its source phase and iteration, rather
                // than reinterpret the old elapsed number under a new period.
                let old_first = self
                    .loop_end()
                    .raw()
                    .checked_sub(self.source_start().raw())
                    .filter(|n| *n > 0)
                    .ok_or(Invalid("midi first pass is empty"))?;
                if offset.raw() >= old_first {
                    let old_cycle = self
                        .loop_end()
                        .raw()
                        .checked_sub(self.loop_start().raw())
                        .filter(|n| *n > 0)
                        .ok_or(Invalid("midi loop is empty"))?;
                    let ordinal = (offset.raw() - old_first) / old_cycle;
                    let new_first = next
                        .loop_end
                        .raw()
                        .checked_sub(next.source_start.raw())
                        .filter(|n| *n > 0)
                        .ok_or(Invalid("midi loop end must follow source start"))?;
                    let new_cycle = next
                        .loop_end
                        .raw()
                        .checked_sub(next.loop_start.raw())
                        .filter(|n| *n > 0)
                        .ok_or(Invalid("midi loop end must follow loop start"))?;
                    let phase_offset = phase
                        .raw()
                        .checked_sub(next.loop_start.raw())
                        .ok_or(Invalid("midi loop phase precedes its loop"))?;
                    next.playback_offset = BeatTime::from_raw(
                        ordinal
                            .checked_mul(new_cycle)
                            .and_then(|n| n.checked_add(new_first))
                            .and_then(|n| n.checked_add(phase_offset))
                            .ok_or(Invalid("midi resized playback window overflow"))?,
                    );
                }
            }
        }
        if let Some(view) = next.content_view {
            next.content_view = Some(crate::ContentWindow::new(
                next.playback_offset.raw(),
                next.playback_offset
                    .raw()
                    .checked_add(next.length.raw())
                    .ok_or(Invalid("midi loop read window overflow"))?,
                view.reversed,
            )?);
        }
        crate::midi::validate_clip(next.start, next.length)?;
        crate::midi::validate_source_boundary(
            next.source_length,
            next.source_start,
            next.loop_start,
            next.loop_end,
            true,
            MidiOneShotBoundary::LoopEnd,
        )?;
        // Extent only grows. Existing note/volume bounds remain valid without
        // scanning either authored collection during a pointer preview.
        next.source_playback()?;
        Ok(next)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    clip: crate::ClipId,
    operation: MidiViewResizeOperation,
) -> Result<Created, Invalid> {
    let original = project
        .midi_clip(clip)
        .ok_or(Invalid("midi clip no longer exists"))?;
    if let MidiViewResizeOperation::PlayStart { source_start } = operation {
        if source_start == original.source_start() {
            return Ok(Created::None);
        }
        let candidate = original.with_play_start(source_start)?;
        return crate::clip_group::replace_midi(project, candidate);
    }
    let next = original.preview_loop_resize(operation)?;
    let mut candidate = original.clone();
    candidate.content_view = next.content_view;
    candidate.length = next.length;
    candidate.playback_offset = next.playback_offset;
    let source = std::sync::Arc::make_mut(&mut candidate.source);
    if source.source_length != next.source_length {
        source.source_length = next.source_length;
    }
    source.source_start = next.source_start;
    source.loop_start = next.loop_start;
    source.loop_end = next.loop_end;
    source.looping = true;
    source.one_shot_boundary = MidiOneShotBoundary::LoopEnd;
    crate::clip_group::replace_midi(project, candidate)
}
