//! Bounded frame-based gap insertion on exactly the destination clipboard rows.
use super::{RangeClipboard, adjacent_rows, paste_rows, source_rows_or_single_target};
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_NOTES, MidiClip, MidiNote, MidiSource, NoteId, Project,
    TrackId,
};
use std::sync::Arc;

/// Open a frame-exact gap on the clipboard's row count, then place its complete
/// rectangle into that gap. The caller applies this on an isolated project
/// candidate, so any failed validation leaves the public project untouched.
pub(crate) fn insert(
    project: &mut Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    let rows = adjacent_rows(project, clipboard, track, "insert")?;
    insert_rows(project, clipboard, &rows, frame)
}

pub(crate) fn insert_on_source_tracks(
    project: &mut Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    let rows = source_rows_or_single_target(project, clipboard, track, "insert")?;
    insert_rows(project, clipboard, &rows, frame)
}

fn insert_rows(
    project: &mut Project,
    clipboard: &RangeClipboard,
    rows: &[TrackId],
    frame: u64,
) -> Result<Created, Invalid> {
    if project.sample_rate() != clipboard.sample_rate {
        return Err(Invalid("time clipboard belongs to another sample rate"));
    }
    for (id, expected) in &clipboard.assets {
        if project.asset(*id) != Some(expected) {
            return Err(Invalid("copied media no longer matches the project"));
        }
    }
    if rows.len() != clipboard.rows || rows.iter().any(|track| project.track(*track).is_none()) {
        return Err(Invalid("insert destination track mapping is invalid"));
    }
    frame
        .checked_add(clipboard.duration)
        .ok_or(Invalid("insert end overflow"))?;

    project.refuse_automation_transform(rows)?;
    for destination in rows {
        open_audio_gap(project, *destination, frame, clipboard.duration)?;
        open_midi_gap(project, *destination, frame, clipboard.duration)?;
    }
    paste_rows(project, clipboard, rows, frame)?;
    // `paste` validates the complete candidate snapshot after shifting and copy.
    Ok(Created::None)
}

fn open_audio_gap(
    project: &mut Project,
    track: TrackId,
    frame: u64,
    duration: u64,
) -> Result<(), Invalid> {
    let clips = project
        .track(track)
        .ok_or(Invalid("insert track no longer exists"))?
        .clips();
    let mut original = Vec::new();
    original
        .try_reserve_exact(clips.len())
        .map_err(|_| Invalid("could not reserve insert audio planning storage"))?;
    original.extend(clips.iter().cloned());
    let mut replacement = Vec::new();
    replacement
        .try_reserve(original.len().saturating_add(1))
        .map_err(|_| Invalid("could not reserve inserted audio clips"))?;
    for clip in original {
        let end = clip
            .start
            .checked_add(clip.duration)
            .ok_or(Invalid("clip end overflow"))?;
        if end <= frame {
            replacement.push(clip);
        } else if clip.start >= frame {
            let mut shifted = clip;
            shifted.start = shifted
                .start
                .checked_add(duration)
                .ok_or(Invalid("shifted audio clip start overflow"))?;
            shifted
                .start
                .checked_add(shifted.duration)
                .ok_or(Invalid("shifted audio clip end overflow"))?;
            let target_frame = shifted.start;
            crate::audio_edit::move_clip(&mut shifted, target_frame, project.tempo_map())?;
            replacement.push(shifted);
        } else {
            let (left, mut right) =
                crate::edit::split_clip_at_tempo(&clip, frame, ClipId(0), project.tempo_map())?;
            right.id = ClipId(project.allocate_id()?);
            right.start = right
                .start
                .checked_add(duration)
                .ok_or(Invalid("shifted audio tail start overflow"))?;
            right
                .start
                .checked_add(right.duration)
                .ok_or(Invalid("shifted audio tail end overflow"))?;
            let target_frame = right.start;
            crate::audio_edit::move_clip(&mut right, target_frame, project.tempo_map())?;
            replacement.push(left);
            replacement.push(right);
        }
    }
    *project.audio_clips_mut(track)? = replacement;
    Ok(())
}

fn open_midi_gap(
    project: &mut Project,
    track: TrackId,
    frame: u64,
    duration: u64,
) -> Result<(), Invalid> {
    let clips = project
        .track(track)
        .ok_or(Invalid("insert track no longer exists"))?
        .midi_clips();
    let mut originals = Vec::new();
    originals
        .try_reserve_exact(clips.len())
        .map_err(|_| Invalid("could not reserve insert midi planning storage"))?;
    originals.extend(clips.iter().cloned());
    let mut replacement = Vec::new();
    replacement
        .try_reserve(originals.len().saturating_mul(2))
        .map_err(|_| Invalid("could not reserve inserted midi clips"))?;
    let mut expanded_notes = 0_usize;
    for original in originals {
        let clip_end_beat = original
            .start
            .raw()
            .checked_add(original.length.raw())
            .ok_or(Invalid("midi clip end overflow"))?;
        let original_start_frame = project.tempo_map().beat_to_frame(original.start)?;
        let original_end_frame = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(clip_end_beat))?;
        if original_end_frame <= frame {
            replacement.push(original);
            continue;
        }

        if original_start_frame >= frame {
            let new_start_frame = original_start_frame
                .checked_add(duration)
                .ok_or(Invalid("shifted midi clip start overflow"))?;
            let new_end_frame = original_end_frame
                .checked_add(duration)
                .ok_or(Invalid("shifted midi clip end overflow"))?;
            if project.tempo_map().points().len() == 1
                && let Some(shifted) = shift_midi_source_exact(
                    project,
                    &original,
                    original_start_frame,
                    original_end_frame,
                    new_start_frame,
                    new_end_frame,
                    duration,
                    &mut expanded_notes,
                )? {
                    replacement.push(shifted);
                    continue;
            }
            expanded_notes = expanded_notes
                .checked_add(materialized_note_bound(&original)?)
                .ok_or(Invalid("midi insert expansion overflow"))?;
            if expanded_notes > MAX_NOTES {
                return Err(Invalid("midi insert expansion exceeds note limit"));
            }
            replacement.push(materialize_shifted_midi(
                project,
                &original,
                original_start_frame,
                original_end_frame,
                new_start_frame,
                duration,
                original.id,
            )?);
        } else {
            let split_beat = project.tempo_map().frame_to_beat(frame)?;
            let shifted_start = frame
                .checked_add(duration)
                .ok_or(Invalid("shifted midi tail start overflow"))?;
            let shifted_end = original_end_frame
                .checked_add(duration)
                .ok_or(Invalid("shifted midi tail end overflow"))?;
            if split_beat > original.start {
                let mut left = original.clone();
                left.length = BeatTime::from_raw(split_beat.raw() - original.start.raw());
                replacement.push(left);
            }
            let right_count = materialized_note_bound(&original)?;
            expanded_notes = expanded_notes
                .checked_add(right_count)
                .ok_or(Invalid("midi insert expansion overflow"))?;
            if expanded_notes > MAX_NOTES {
                return Err(Invalid("midi insert expansion exceeds note limit"));
            }
            if shifted_start < shifted_end {
                let right_id = ClipId(project.allocate_id()?);
                replacement.push(materialize_shifted_midi(
                    project,
                    &original,
                    frame,
                    original_end_frame,
                    shifted_start,
                    duration,
                    right_id,
                )?);
            }
        }
    }
    let track_mut = project.track_mut(track)?;
    track_mut.content.midi_clips = replacement;
    Ok(())
}

fn materialized_note_bound(clip: &MidiClip) -> Result<usize, Invalid> {
    let playback = clip.source_playback()?;
    clip.notes().iter().try_fold(0_usize, |total, note| {
        let occurrences = playback.note_occurrences(note.start(), note.duration())?;
        let count = usize::try_from(occurrences.count())
            .map_err(|_| Invalid("midi insert expansion overflow"))?;
        total
            .checked_add(count)
            .ok_or(Invalid("midi insert expansion overflow"))
    })
}

#[allow(clippy::too_many_arguments)]
fn shift_midi_source_exact(
    project: &mut Project,
    original: &MidiClip,
    old_start_frame: u64,
    old_end_frame: u64,
    new_start_frame: u64,
    new_end_frame: u64,
    frame_delta: u64,
    expanded_notes: &mut usize,
) -> Result<Option<MidiClip>, Invalid> {
    let map = project.tempo_map();
    let start = map.frame_to_beat(new_start_frame)?;
    let delta_raw = start
        .raw()
        .checked_sub(original.start.raw())
        .ok_or(Invalid("shifted midi beat precedes source"))?;
    if map.beat_to_frame(BeatTime::from_raw(
        original
            .start
            .raw()
            .checked_add(original.length.raw())
            .and_then(|raw| raw.checked_add(delta_raw))
            .ok_or(Invalid("shifted midi end overflow"))?,
    ))? != new_end_frame
    {
        return Ok(None);
    }
    let playback = original.source_playback()?;
    let clip_end_raw = original
        .start
        .raw()
        .checked_add(original.length.raw())
        .ok_or(Invalid("shifted midi end overflow"))?;
    for note in original.notes() {
        let occurrences = playback.note_occurrences(note.start(), note.duration())?;
        let count = usize::try_from(occurrences.count())
            .map_err(|_| Invalid("midi insert expansion overflow"))?;
        let projected = expanded_notes
            .checked_add(count)
            .ok_or(Invalid("midi insert expansion overflow"))?;
        if projected > MAX_NOTES {
            return Err(Invalid("midi insert expansion exceeds note limit"));
        }
        for occurrence in occurrences.iter() {
            let onset = original
                .start
                .raw()
                .checked_add(occurrence.timeline_start().raw())
                .ok_or(Invalid("midi note onset overflow"))?;
            let gate = onset
                .checked_add(occurrence.gate_duration().raw())
                .ok_or(Invalid("midi note gate overflow"))?;
            for beat in [onset, gate] {
                let shifted = beat
                    .checked_add(delta_raw)
                    .ok_or(Invalid("shifted midi note overflow"))?;
                if map.beat_to_frame(BeatTime::from_raw(shifted))?
                    != map
                        .beat_to_frame(BeatTime::from_raw(beat))?
                        .checked_add(frame_delta)
                        .ok_or(Invalid("shifted midi event frame overflow"))?
                {
                    return Ok(None);
                }
            }
        }
    }
    if map.beat_to_frame(original.start)? != old_start_frame
        || map.beat_to_frame(BeatTime::from_raw(clip_end_raw))? != old_end_frame
    {
        return Err(Invalid("midi clip frame bounds changed during insertion"));
    }
    *expanded_notes = expanded_notes
        .checked_add(materialized_note_bound(original)?)
        .ok_or(Invalid("midi insert expansion overflow"))?;
    if *expanded_notes > MAX_NOTES {
        return Err(Invalid("midi insert expansion exceeds note limit"));
    }
    let mut shifted = original.clone();
    shifted.start = start;
    Ok(Some(shifted))
}

fn materialize_shifted_midi(
    project: &mut Project,
    original: &MidiClip,
    source_start_frame: u64,
    source_end_frame: u64,
    destination_start_frame: u64,
    frame_delta: u64,
    id: ClipId,
) -> Result<MidiClip, Invalid> {
    let destination_end_frame = source_end_frame
        .checked_add(frame_delta)
        .ok_or(Invalid("shifted midi end overflow"))?;
    let start = project.tempo_map().frame_to_beat(destination_start_frame)?;
    let end = project.tempo_map().frame_to_beat(destination_end_frame)?;
    let length = BeatTime::from_raw(
        end.raw()
            .checked_sub(start.raw())
            .filter(|length| *length > 0)
            .ok_or(Invalid("shifted midi interval collapsed"))?,
    );
    let playback = original.source_playback()?;
    let mut notes = Vec::new();
    notes
        .try_reserve(materialized_note_bound(original)?)
        .map_err(|_| Invalid("could not reserve shifted midi notes"))?;
    let clip_end = original
        .start
        .raw()
        .checked_add(original.length.raw())
        .ok_or(Invalid("midi clip end overflow"))?;
    for note in original.notes() {
        let occurrences = playback.note_occurrences(note.start(), note.duration())?;
        for occurrence in occurrences.iter() {
            let onset = original
                .start
                .raw()
                .checked_add(occurrence.timeline_start().raw())
                .ok_or(Invalid("midi note onset overflow"))?;
            let gate = onset
                .checked_add(occurrence.gate_duration().raw())
                .ok_or(Invalid("midi note gate overflow"))?
                .min(clip_end);
            let old_onset_frame = project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(onset))?;
            let old_gate_frame = project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(gate))?;
            let kept_start = old_onset_frame.max(source_start_frame);
            let kept_end = old_gate_frame.min(source_end_frame);
            if kept_start >= kept_end {
                continue;
            }
            let shifted_start = kept_start
                .checked_add(frame_delta)
                .ok_or(Invalid("shifted midi note onset overflow"))?;
            let shifted_end = kept_end
                .checked_add(frame_delta)
                .ok_or(Invalid("shifted midi note gate overflow"))?;
            let note_start = project.tempo_map().frame_to_beat(shifted_start)?;
            let note_end = project.tempo_map().frame_to_beat(shifted_end)?;
            let relative = note_start
                .raw()
                .checked_sub(start.raw())
                .ok_or(Invalid("shifted midi note precedes clip"))?;
            let duration = note_end
                .raw()
                .checked_sub(note_start.raw())
                .filter(|duration| *duration > 0)
                .ok_or(Invalid("shifted midi note collapsed"))?;
            notes.push(MidiNote {
                id: NoteId(project.allocate_id()?),
                active: note.active(),
                start: BeatTime::from_raw(relative),
                duration: BeatTime::from_raw(duration),
                pitch: note.pitch(),
                channel: note.channel(),
                velocity: note.velocity(),
            });
        }
    }
    let volume = super::midi_volume::FrameVolume::capture(
        original,
        project.tempo_map(),
        source_start_frame,
        source_end_frame,
    )?
    .paste(
        project.tempo_map(),
        destination_start_frame,
        destination_end_frame,
    )?;
    Ok(MidiClip {
        content_view: None,
        active: original.active,
        playback_offset: crate::BeatTime::ZERO,
        volume,
        name: original.name.clone(),
        flavor: original.flavor,
        id,
        start,
        length,
        source: Arc::new(MidiSource {
            content: std::sync::Arc::new(crate::MidiContent {
                source_length: length,
                notes,
            }),
            one_shot_boundary: crate::MidiOneShotBoundary::SourceEnd,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: length,
            looping: false,
        }),
    })
}
