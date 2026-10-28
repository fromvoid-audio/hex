//! Arrangement edge stretching scales retained MIDI source coordinates, independently
//! of edge trimming. Pointer candidates contain only checked constant-size metadata;
//! canonical application validates every note and volume boundary before detaching
//! the edited source. It uses existing occupancy replacement and global history.
use crate::{BeatTime, Created, Invalid, MidiClip, MidiClipVolume, Project, TrimEdge};
use std::sync::Arc;

/// Metadata-only rational source scaling for one Arrangement edge. Construction is
/// O(1), with no note/curve clones. Full contents can still refuse a compression
/// whose integer mapping collapses an unsampled note or volume interval; application
/// validates those contents atomically before changing project/history.
///
/// All source boundaries use floor(old * new_length / old_length), including note
/// ends (duration is the difference of mapped endpoints), retained loop markers and
/// clip volume points. A candidate preserves the source's one-shot/loop policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiClipStretch {
    playback_offset: BeatTime,
    start: BeatTime,
    length: BeatTime,
    original_length: BeatTime,
    source_start: BeatTime,
    source_length: BeatTime,
    loop_start: BeatTime,
    loop_end: BeatTime,
}

impl MidiClipStretch {
    /// Elapsed source-law window scaled by the same checked ratio as notes.
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
    pub fn source_length(self) -> BeatTime {
        self.source_length
    }
    pub fn loop_start(self) -> BeatTime {
        self.loop_start
    }
    pub fn loop_end(self) -> BeatTime {
        self.loop_end
    }

    /// Map a retained source-beat boundary without copying any source contents.
    /// The widened product is exact; refusal preserves representable Q32.32 beats.
    pub fn scale(self, beat: BeatTime) -> Result<BeatTime, Invalid> {
        let raw = u128::from(beat.raw())
            .checked_mul(u128::from(self.length.raw()))
            .ok_or(Invalid("midi stretch product overflow"))?
            / u128::from(self.original_length.raw());
        u64::try_from(raw)
            .map(BeatTime::from_raw)
            .map_err(|_| Invalid("midi stretch source overflow"))
    }

    fn note_interval(self, note: &crate::MidiNote) -> Result<(BeatTime, BeatTime), Invalid> {
        let end = note
            .start()
            .raw()
            .checked_add(note.duration().raw())
            .map(BeatTime::from_raw)
            .ok_or(Invalid("midi note end overflow"))?;
        let start = self.scale(note.start())?;
        let end = self.scale(end)?;
        let duration = BeatTime::from_raw(
            end.raw()
                .checked_sub(start.raw())
                .filter(|duration| *duration != 0)
                .ok_or(Invalid("midi stretch collapses a note"))?,
        );
        Ok((start, duration))
    }

    fn validate_contents(self, clip: &MidiClip) -> Result<(), Invalid> {
        for note in clip.notes() {
            let (start, duration) = self.note_interval(note)?;
            crate::midi::validate_note(
                start,
                duration,
                self.source_length,
                note.pitch(),
                note.channel(),
                note.velocity(),
            )?;
        }
        let mut previous = None;
        for point in clip.volume().points() {
            let beat = self.scale(point.beat)?;
            if beat > self.source_length || previous.is_some_and(|old| old >= beat) {
                return Err(Invalid("midi stretch collapses volume boundaries"));
            }
            previous = Some(beat);
        }
        Ok(())
    }
}

impl MidiClip {
    /// Stretch an Arrangement edge around its opposite fixed edge. Unlike trim,
    /// this scales source-relative timing, retaining every authored note and marker.
    /// Invalid crossing/zero length and unrepresentable source geometry are refused;
    /// full note/volume validation belongs to canonical application, not pointer work.
    pub fn preview_edge_stretch(
        &self,
        edge: TrimEdge,
        beat: BeatTime,
    ) -> Result<MidiClipStretch, Invalid> {
        crate::midi::validate_clip(self.start(), self.length())?;
        self.source_playback()?;
        let end = self
            .start()
            .raw()
            .checked_add(self.length().raw())
            .ok_or(Invalid("midi clip timeline end overflow"))?;
        let (start, end) = match edge {
            TrimEdge::Right => (self.start(), beat.raw()),
            TrimEdge::Left => (beat, end),
        };
        let length = end
            .checked_sub(start.raw())
            .filter(|length| *length != 0)
            .map(BeatTime::from_raw)
            .ok_or(Invalid("midi stretch edge must preserve positive length"))?;
        crate::midi::validate_clip(start, length)?;
        let mut preview = MidiClipStretch {
            playback_offset: BeatTime::ZERO,
            start,
            length,
            original_length: self.length(),
            source_start: BeatTime::ZERO,
            source_length: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ZERO,
        };
        preview.playback_offset = preview.scale(self.playback_offset())?;
        preview
            .playback_offset
            .raw()
            .checked_add(length.raw())
            .ok_or(Invalid("midi stretched playback window overflow"))?;
        preview.source_start = preview.scale(self.source_start())?;
        preview.source_length = preview.scale(self.source_length())?;
        preview.loop_start = preview.scale(self.loop_start())?;
        preview.loop_end = preview.scale(self.loop_end())?;
        crate::midi::validate_source_boundary(
            preview.source_length,
            preview.source_start,
            preview.loop_start,
            preview.loop_end,
            self.looping(),
            self.one_shot_boundary(),
        )?;
        Ok(preview)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    clip: crate::ClipId,
    edge: TrimEdge,
    beat: BeatTime,
) -> Result<Created, Invalid> {
    let original = project
        .midi_clip(clip)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let preview = original.preview_edge_stretch(edge, beat)?;
    if preview.start == original.start() && preview.length == original.length() {
        return Ok(Created::None);
    }
    preview.validate_contents(original)?;
    let volume = MidiClipVolume::new(
        original
            .volume()
            .points()
            .iter()
            .map(|point| {
                Ok(crate::MidiClipVolumePoint {
                    beat: preview.scale(point.beat)?,
                    ..*point
                })
            })
            .collect::<Result<Vec<_>, Invalid>>()?,
        preview.source_length,
    )?
    .with_clip_gain(original.volume().clip_gain());
    let mut target = original.clone();
    let source = Arc::make_mut(&mut target.source);
    source.source_start = preview.source_start;
    source.source_length = preview.source_length;
    source.loop_start = preview.loop_start;
    source.loop_end = preview.loop_end;
    for note in &mut source.notes {
        let (start, duration) = preview.note_interval(note)?;
        note.start = start;
        note.duration = duration;
    }
    target.content_view = original
        .content_view()
        .map(|view| -> Result<_, Invalid> {
            crate::ContentWindow::new(
                preview.scale(BeatTime::from_raw(view.start))?.raw(),
                preview.scale(BeatTime::from_raw(view.end))?.raw(),
                view.reversed,
            )
        })
        .transpose()?;
    target.start = preview.start;
    target.length = preview.length;
    target.playback_offset = preview.playback_offset;
    target
        .playback_offset
        .raw()
        .checked_add(target.length.raw())
        .ok_or(Invalid("midi stretched playback window overflow"))?;
    target.volume = volume;
    crate::clip_group::replace_midi(project, target)
}

#[cfg(test)]
mod tests;
