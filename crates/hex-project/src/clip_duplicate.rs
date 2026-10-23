//! Whole-selection duplication beside the canonical group placement owner.
//!
//! The earliest selected start lands at the latest selected end. MIDI-only
//! groups translate in exact beats; audio/mixed groups retain the existing
//! frame-displacement and warped-audio move policy. Gaps do not overwrite.
use crate::{
    BeatTime, ClipGroupContent, ClipGroupPosition, ClipGroupPreview, ClipId, Created, Invalid,
    MAX_CLIPS, Project,
};
use std::collections::BTreeSet;

pub(crate) fn apply(project: &mut Project, ids: Vec<ClipId>) -> Result<Created, Invalid> {
    if ids.is_empty() || ids.len() > MAX_CLIPS {
        return Err(Invalid("clip duplication exceeds project limits"));
    }
    let selected = ids.iter().copied().collect::<BTreeSet<_>>();
    if selected.len() != ids.len() {
        return Err(Invalid("clip duplication contains duplicate ids"));
    }
    let mut found = 0;
    let mut audio_anchor = None;
    let mut frame_start = u64::MAX;
    let mut frame_end = 0;
    let mut beat_start = u64::MAX;
    let mut beat_end = 0;
    for track in project.tracks() {
        for clip in track.clips().iter().filter(|c| selected.contains(&c.id())) {
            found += 1;
            frame_start = frame_start.min(clip.start());
            frame_end = frame_end.max(
                clip.start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("duplicate audio end overflow"))?,
            );
            audio_anchor.get_or_insert((clip.id(), track.id(), clip.start()));
        }
        for clip in track
            .midi_clips()
            .iter()
            .filter(|c| selected.contains(&c.id()))
        {
            found += 1;
            beat_start = beat_start.min(clip.start().raw());
            beat_end = beat_end.max(
                clip.start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .ok_or(Invalid("duplicate midi end overflow"))?,
            );
        }
    }
    if found != ids.len() {
        return Err(Invalid("selected arrangement clip no longer exists"));
    }
    let incoming = if let Some((anchor, track, start)) = audio_anchor {
        for row in project.tracks() {
            for clip in row
                .midi_clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
            {
                frame_start = frame_start.min(project.tempo_map().beat_to_frame(clip.start())?);
                let end = BeatTime::from_raw(
                    clip.start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("duplicate midi end overflow"))?,
                );
                frame_end = frame_end.max(project.tempo_map().beat_to_frame(end)?);
            }
        }
        let span = frame_end
            .checked_sub(frame_start)
            .filter(|span| *span > 0)
            .ok_or(Invalid("duplicate selection has no positive extent"))?;
        let destination = start
            .checked_add(span)
            .ok_or(Invalid("duplicate destination overflow"))?;
        project.preview_clip_group_move(
            &ids,
            anchor,
            track,
            ClipGroupPosition::Audio(destination),
        )?
    } else {
        let span = beat_end
            .checked_sub(beat_start)
            .filter(|span| *span > 0)
            .ok_or(Invalid("duplicate selection has no positive beat extent"))?;
        let mut incoming = Vec::new();
        incoming
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve duplicate clip metadata"))?;
        for track in project.tracks() {
            for clip in track
                .midi_clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
            {
                let start = BeatTime::from_raw(
                    clip.start()
                        .raw()
                        .checked_add(span)
                        .ok_or(Invalid("duplicate midi start overflow"))?,
                );
                crate::midi::validate_clip(start, clip.length())?;
                let end = BeatTime::from_raw(
                    start
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("duplicate midi end overflow"))?,
                );
                project.tempo_map().beat_to_frame(end)?;
                let mut copy = clip.clone();
                copy.start = start;
                incoming.push(ClipGroupPreview {
                    track: track.id(),
                    content: ClipGroupContent::Midi(copy),
                });
            }
        }
        incoming
    };
    let created = crate::clip_group::place_batch(project, incoming, &[])?;
    Project::from_snapshot(project.snapshot())?;
    Ok(created)
}
