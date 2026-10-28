//! Source-local range duplication. Validate the complete destination, then reuse
//! canonical batch admission/overwrite/ID allocation in one atomic model edit.
//! No playback folding, source-start/loop changes or Arrangement placement edits.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_NOTES, MidiNote, MidiNoteInsert, MidiSource, Project,
};

#[derive(Clone, Copy)]
pub(crate) struct Range {
    start: u64,
    end: u64,
    delta: u64,
    pub(crate) source_length: BeatTime,
    pub(crate) count: usize,
}
impl Range {
    pub(crate) fn capture(
        source: &MidiSource,
        start: BeatTime,
        end: BeatTime,
    ) -> Result<Self, Invalid> {
        let delta = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("midi source range must be positive"))?;
        let destination_end = end
            .raw()
            .checked_add(delta)
            .ok_or(Invalid("midi source range destination overflow"))?;
        if source.notes().len() > MAX_NOTES {
            return Err(Invalid("midi note limit reached"));
        }
        let mut range = Self {
            start: start.raw(),
            end: end.raw(),
            delta,
            source_length: BeatTime::from_raw(source.source_length().raw().max(destination_end)),
            count: 0,
        };
        for note in source.notes() {
            if range.copy(note)?.is_some() {
                range.count += 1;
            }
        }
        Ok(range)
    }
    pub(crate) fn copy(self, note: &MidiNote) -> Result<Option<MidiNoteInsert>, Invalid> {
        let end = note
            .start()
            .raw()
            .checked_add(note.duration().raw())
            .ok_or(Invalid("midi note end overflow"))?
            .min(self.end);
        let start = note.start().raw().max(self.start);
        if start >= end {
            return Ok(None);
        }
        Ok(Some(MidiNoteInsert {
            active: note.active(),
            start: BeatTime::from_raw(
                start
                    .checked_add(self.delta)
                    .ok_or(Invalid("midi source range note overflow"))?,
            ),
            duration: BeatTime::from_raw(end - start),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        }))
    }
}
pub(crate) fn duplicate(
    project: &mut Project,
    clip: ClipId,
    start: BeatTime,
    end: BeatTime,
) -> Result<Created, Invalid> {
    let source = project
        .midi_source(clip)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let range = Range::capture(source, start, end)?;
    let volume = project
        .midi_clip_volume(clip)
        .ok_or(Invalid("midi clip no longer exists"))?
        .duplicated_range(start, end, range.source_length)?;
    let mut insert = Vec::new();
    let mut gains: Option<Vec<crate::MidiVoiceGain>> = None;
    insert
        .try_reserve_exact(range.count)
        .map_err(|_| Invalid("could not reserve midi range notes"))?;
    let playback =
        crate::MidiSourcePlayback::new(source.source_length(), BeatTime::ZERO, None, end)?;
    for note in source.notes() {
        if let Some(copy) = range.copy(note)? {
            if !note.voice_gain().layers().is_empty() && gains.is_none() {
                let mut owners = Vec::new();
                owners
                    .try_reserve_exact(range.count)
                    .map_err(|_| Invalid("could not reserve midi range gains"))?;
                owners.resize_with(insert.len(), crate::MidiVoiceGain::default);
                gains = Some(owners);
            }
            if let Some(gains) = &mut gains {
                let gain = if note.voice_gain().layers().is_empty() {
                    note.voice_gain().clone()
                } else {
                    note.voice_gain().materialized_range(
                        playback,
                        start,
                        end,
                        end,
                        range.source_length,
                    )?
                };
                gains.push(gain);
            }
            insert.push(copy);
        }
    }
    let created = crate::midi_batch::edit_range(project, clip, insert, range.source_length, gains)?;
    project.replace_midi_clip_volume(clip, volume)?;
    Ok(created)
}
