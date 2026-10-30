//! Borrowed command routing and no-op preflight before the native fence.
use super::*;

impl Command {
    /// Closed native sampler composition family. This is only routing; actual
    /// command provenance and adjacent snapshots are checked by Editor.
    pub fn supports_sampler_midi_edit(&self) -> bool {
        matches!(
            self,
            Self::AddScene { .. }
                | Self::PlaceMidiClip { .. }
                | Self::PlaceMidiClipWithOverwrite { .. }
                | Self::DuplicateMidiClip(_)
                | Self::RemoveMidiClip(_)
                | Self::CreateSessionMidiClip { .. }
                | Self::CopyArrangementMidiToSession { .. }
                | Self::ClearSessionCell { .. }
                | Self::AddMidiNote { .. }
                | Self::AddMidiNotes { .. }
                | Self::EditMidiNotes { .. }
                | Self::DuplicateMidiSourceRange { .. }
                | Self::UpdateMidiNote { .. }
                | Self::UpdateMidiNotes { .. }
                | Self::SetMidiNotesActive { .. }
                | Self::RemoveMidiNote(_)
                | Self::RemoveMidiNotes { .. }
                | Self::ReplaceMidiClipVolume { .. }
                | Self::SetMidiClipGain { .. }
                | Self::SetMidiClipSource { .. }
                | Self::SetSessionMidiClipSource { .. }
        )
    }
    /// Borrowed preflight without a project clone. Normalize owned batch updates
    /// in place so no-op detection is bounded and does not allocate before admission.
    pub fn prepare_sampler_midi_edit(&mut self, project: &Project) -> Result<bool, Invalid> {
        const MISSING: Invalid = Invalid("midi edit target is unavailable");
        match self {
            Self::SetMidiNotesActive {
                clip,
                notes,
                active,
            } => {
                if notes.is_empty() || notes.len() > crate::MAX_NOTES {
                    return Err(Invalid("invalid midi activation batch size"));
                }
                notes.sort_unstable();
                if notes.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(Invalid("duplicate midi activation note"));
                }
                let source = project.midi_source(*clip).ok_or(MISSING)?;
                let mut matched = 0usize;
                let mut changed = false;
                for note in &source.notes {
                    if notes.binary_search(&note.id).is_ok() {
                        matched += 1;
                        changed |= note.active != *active;
                    }
                }
                if matched != notes.len() {
                    return Err(MISSING);
                }
                return Ok(changed);
            }
            Self::ReplaceMidiClipVolume { clip, volume } => {
                let source = project.midi_source(*clip).ok_or(MISSING)?;
                volume.validate_source_length(source.source_length())?;
                let original = project.midi_clip_volume(*clip).ok_or(MISSING)?;
                return Ok(original != &volume.clone().with_clip_gain(original.clip_gain()));
            }
            Self::SetMidiClipGain { clip, gain } => {
                return Ok(project.midi_clip_volume(*clip).ok_or(MISSING)?.clip_gain() != *gain);
            }
            Self::ClearSessionCell { track, scene }
                if project.session_cell(*track, *scene).is_none() =>
            {
                if project.track(*track).is_none()
                    || !project.session.scenes.iter().any(|s| s.id == *scene)
                {
                    return Err(MISSING);
                }
                return Ok(false);
            }
            Self::DuplicateMidiSourceRange { clip, start, end } => {
                let source = project.midi_source(*clip).ok_or(MISSING)?;
                let range = crate::midi_source_range::Range::capture(source, *start, *end)?;
                let volume = project.midi_clip_volume(*clip).ok_or(MISSING)?;
                let copied = volume.duplicated_range(*start, *end, range.source_length)?;
                return Ok(range.count > 0
                    || range.source_length != source.source_length()
                    || &copied != volume);
            }
            Self::UpdateMidiNote {
                note,
                start,
                duration,
                pitch,
                channel,
                velocity,
            } => {
                let (clip, length) =
                    crate::midi::find_note_source(project, *note).ok_or(MISSING)?;
                crate::midi::validate_note(*start, *duration, length, *pitch, *channel, *velocity)?;
                let old = project
                    .midi_source(clip)
                    .and_then(|s| s.notes.iter().find(|n| n.id == *note))
                    .ok_or(MISSING)?;
                return Ok((
                    old.start,
                    old.duration,
                    old.pitch,
                    old.channel,
                    old.velocity,
                ) != (*start, *duration, *pitch, *channel, *velocity));
            }
            Self::UpdateMidiNotes { clip, notes } => {
                if notes.is_empty() || notes.len() > crate::MAX_NOTES {
                    return Err(Invalid("invalid midi update batch size"));
                }
                notes.sort_unstable_by_key(|n| n.note);
                if notes.windows(2).any(|n| n[0].note == n[1].note) {
                    return Err(Invalid("duplicate midi note update"));
                }
                let source = project.midi_source(*clip).ok_or(MISSING)?;
                for n in notes.iter() {
                    crate::midi::validate_note(
                        n.start,
                        n.duration,
                        source.source_length,
                        n.pitch,
                        n.channel,
                        n.velocity,
                    )?;
                }
                let mut matched = 0;
                let mut changed = false;
                for old in &source.notes {
                    if let Ok(index) = notes.binary_search_by_key(&old.id, |n| n.note) {
                        let n = &notes[index];
                        matched += 1;
                        changed |= (
                            old.start,
                            old.duration,
                            old.pitch,
                            old.channel,
                            old.velocity,
                        ) != (n.start, n.duration, n.pitch, n.channel, n.velocity);
                    }
                }
                if matched != notes.len() {
                    return Err(MISSING);
                }
                return Ok(changed);
            }
            Self::SetMidiClipSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                length,
            } => {
                let old = project.midi_clip(*clip).ok_or(MISSING)?;
                crate::midi::validate_clip(old.start, *length)?;
                validate_source_edit(
                    &old.source,
                    *source_length,
                    *source_start,
                    *loop_start,
                    *loop_end,
                    *looping,
                )?;
                return Ok(old.length != *length
                    || source_values(&old.source)
                        != (
                            *source_length,
                            *source_start,
                            *loop_start,
                            *loop_end,
                            *looping,
                        ));
            }
            Self::SetSessionMidiClipSource {
                clip,
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
            } => {
                let old = project.session_midi_clip(*clip).ok_or(MISSING)?;
                validate_source_edit(
                    &old.source,
                    *source_length,
                    *source_start,
                    *loop_start,
                    *loop_end,
                    *looping,
                )?;
                return Ok(source_values(&old.source)
                    != (
                        *source_length,
                        *source_start,
                        *loop_start,
                        *loop_end,
                        *looping,
                    ));
            }
            _ => {}
        }
        Plan::capture(self, project)?.ok_or(Invalid("unsupported midi edit command"))?;
        Ok(true)
    }
}
fn source_values(s: &MidiSource) -> (BeatTime, BeatTime, BeatTime, BeatTime, bool) {
    (
        s.source_length,
        s.source_start,
        s.loop_start,
        s.loop_end,
        s.looping,
    )
}
fn validate_source_edit(
    s: &MidiSource,
    length: BeatTime,
    start: BeatTime,
    loop_start: BeatTime,
    loop_end: BeatTime,
    looping: bool,
) -> Result<(), Invalid> {
    crate::midi::validate_source_boundary(
        length,
        start,
        loop_start,
        loop_end,
        looping,
        s.one_shot_boundary(),
    )?;
    for n in &s.notes {
        crate::midi::validate_note(n.start, n.duration, length, n.pitch, n.channel, n.velocity)?;
    }
    Ok(())
}
