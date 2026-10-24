//! One bounded whole-selection move, using canonical projection and overwrite.
use crate::{BeatTime, Clip, ClipId, Created, Invalid, MAX_CLIPS, MidiClip, Project, TrackId};
use std::collections::BTreeSet;

/// The picked clip's single snapped destination. Other members share its delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipGroupPosition {
    Audio(u64),
    Midi(BeatTime),
}
/// Metadata-only projected member; source data and stable identities are retained.
#[derive(Clone, Debug)]
pub enum ClipGroupContent {
    Audio(Clip),
    Midi(MidiClip),
}
#[derive(Clone, Debug)]
pub struct ClipGroupPreview {
    pub track: TrackId,
    pub content: ClipGroupContent,
}
impl ClipGroupPreview {
    pub fn id(&self) -> ClipId {
        match &self.content {
            ClipGroupContent::Audio(c) => c.id(),
            ClipGroupContent::Midi(c) => c.id(),
        }
    }
}

/// One prepared incoming placement uses the same complete final-row plan as a
/// group. Fresh material receives its identity only after final-count admission;
/// a move/resize excludes its old identity without exposing an intermediate edit.
pub(crate) fn place_one(
    project: &mut Project,
    track: TrackId,
    content: ClipGroupContent,
    fresh: bool,
) -> Result<Created, Invalid> {
    let member = ClipGroupPreview { track, content };
    let selected = if fresh {
        BTreeSet::new()
    } else {
        BTreeSet::from([member.id()])
    };
    let mut incoming = Vec::new();
    incoming
        .try_reserve_exact(1)
        .map_err(|_| Invalid("could not reserve incoming placement"))?;
    incoming.push(member);
    match (fresh, batch::apply(project, incoming, &selected, fresh)?) {
        (true, Created::Clips { first, last }) if first == last => Ok(Created::Clip(first)),
        (false, Created::None) => Ok(Created::None),
        _ => Err(Invalid(
            "single placement returned an invalid identity range",
        )),
    }
}

/// Admit a complete clipboard/rectangle replacement against final row counts.
/// Cuts include empty rectangle cells; ordinary clip paste supplies no cuts.
pub(crate) fn place_batch(
    project: &mut Project,
    incoming: Vec<ClipGroupPreview>,
    cuts: &[(TrackId, u64, u64)],
) -> Result<Created, Invalid> {
    batch::apply_with_cuts(project, incoming, &BTreeSet::new(), true, cuts)
}

/// Scalar/source edits retain collection order and never run overwrite work.
/// A prepared extent change uses the same final-row admission as a move.
pub(crate) fn replace_audio(project: &mut Project, incoming: Clip) -> Result<Created, Invalid> {
    let id = incoming.id();
    let old = project
        .clip(incoming.id())
        .ok_or(Invalid("audio clip no longer exists"))?;
    if (old.start(), old.duration()) == (incoming.start(), incoming.duration()) {
        *project.clip_mut(id)? = incoming;
        return Ok(Created::None);
    }
    let track = project
        .tracks()
        .iter()
        .find(|t| t.clips().iter().any(|c| c.id() == incoming.id()))
        .map(|t| t.id())
        .ok_or(Invalid("audio clip track missing"))?;
    place_one(project, track, ClipGroupContent::Audio(incoming), false)
}

pub(crate) fn replace_midi(project: &mut Project, incoming: MidiClip) -> Result<Created, Invalid> {
    let track = project
        .tracks()
        .iter()
        .find(|t| t.midi_clips().iter().any(|c| c.id() == incoming.id()))
        .map(|t| t.id())
        .ok_or(Invalid("midi clip track missing"))?;
    let old = project
        .midi_clip(incoming.id())
        .ok_or(Invalid("midi clip no longer exists"))?;
    if (old.start(), old.length()) == (incoming.start(), incoming.length()) {
        let target = project
            .midi_clips_mut(incoming.id())?
            .iter_mut()
            .find(|c| c.id() == incoming.id())
            .ok_or(Invalid("midi clip no longer exists"))?;
        *target = incoming;
        return Ok(Created::None);
    }
    place_one(project, track, ClipGroupContent::Midi(incoming), false)
}
impl Project {
    /// Validate the complete group before returning any preview. All ordinary
    /// tracks are hybrid; vertical movement preserves their relative indices.
    pub fn preview_clip_group_move(
        &self,
        ids: &[ClipId],
        anchor: ClipId,
        target: TrackId,
        position: ClipGroupPosition,
    ) -> Result<Vec<ClipGroupPreview>, Invalid> {
        if ids.is_empty() || ids.len() > MAX_CLIPS {
            return Err(Invalid("clip group exceeds project limits"));
        }
        let selected = ids.iter().copied().collect::<BTreeSet<_>>();
        if selected.len() != ids.len() || !selected.contains(&anchor) {
            return Err(Invalid("clip group contains duplicate ids or no anchor"));
        }
        let source_index = self
            .tracks()
            .iter()
            .position(|t| {
                t.clips().iter().any(|c| c.id() == anchor)
                    || t.midi_clips().iter().any(|c| c.id() == anchor)
            })
            .ok_or(Invalid("clip group anchor no longer exists"))?;
        let target_index = self
            .tracks()
            .iter()
            .position(|t| t.id() == target)
            .ok_or(Invalid("clip group target no longer exists"))?;
        let original_anchor = match position {
            ClipGroupPosition::Audio(_) => self
                .clip(anchor)
                .ok_or(Invalid("audio anchor is stale"))?
                .start(),
            ClipGroupPosition::Midi(_) => self.tempo_map().beat_to_frame(
                self.midi_clip(anchor)
                    .ok_or(Invalid("midi anchor is stale"))?
                    .start(),
            )?,
        };
        let destination_anchor = match position {
            ClipGroupPosition::Audio(frame) => frame,
            ClipGroupPosition::Midi(beat) => self.tempo_map().beat_to_frame(beat)?,
        };
        let requested_delta = i128::from(destination_anchor) - i128::from(original_anchor);
        let mut earliest = original_anchor;
        for row in self.tracks() {
            for c in row.clips().iter().filter(|c| selected.contains(&c.id())) {
                earliest = earliest.min(c.start());
            }
            for c in row
                .midi_clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
            {
                earliest = earliest.min(self.tempo_map().beat_to_frame(c.start())?);
            }
        }
        // Clamp one shared delta at time zero, never individual members.
        let delta = requested_delta.max(-i128::from(earliest));
        let mut result = Vec::new();
        result
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve clip group preview"))?;
        for (index, track) in self.tracks().iter().enumerate() {
            let destination = index as i128 + target_index as i128 - source_index as i128;
            let destination_track = || {
                usize::try_from(destination)
                    .ok()
                    .and_then(|i| self.tracks().get(i))
                    .map(|t| t.id())
                    .ok_or(Invalid("whole clip group does not fit destination tracks"))
            };
            for clip in track.clips().iter().filter(|c| selected.contains(&c.id())) {
                let start = translated(clip.start(), delta)?;
                let mut projected = clip.clone();
                crate::audio_edit::move_clip(&mut projected, start, self.tempo_map())?;
                projected
                    .start()
                    .checked_add(projected.duration())
                    .ok_or(Invalid("group audio end overflow"))?;
                result.push(ClipGroupPreview {
                    track: destination_track()?,
                    content: ClipGroupContent::Audio(projected),
                });
            }
            for clip in track
                .midi_clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
            {
                let frame = translated(self.tempo_map().beat_to_frame(clip.start())?, delta)?;
                let start = midi_start(
                    self,
                    clip.start(),
                    frame,
                    delta,
                    requested_delta,
                    clip.id() == anchor,
                    position,
                )?;
                let end = BeatTime::from_raw(
                    start
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("midi group end overflow"))?,
                );
                crate::midi::validate_clip(start, clip.length())?;
                // A canonical positive beat extent may be smaller than one
                // integer frame. Retain it just as single-clip movement does;
                // projection still refuses an unrepresentable endpoint.
                self.tempo_map().beat_to_frame(end)?;
                let mut projected = clip.clone();
                projected.start = start;
                result.push(ClipGroupPreview {
                    track: destination_track()?,
                    content: ClipGroupContent::Midi(projected),
                });
            }
        }
        if result.len() != ids.len() {
            return Err(Invalid("selected arrangement clip no longer exists"));
        }
        Ok(result)
    }
}
pub(super) fn translated(frame: u64, delta: i128) -> Result<u64, Invalid> {
    u64::try_from(i128::from(frame) + delta)
        .map_err(|_| Invalid("whole clip group exceeds timeline bounds"))
}

pub(super) fn midi_start(
    project: &Project,
    original: BeatTime,
    frame: u64,
    delta: i128,
    requested: i128,
    anchor: bool,
    position: ClipGroupPosition,
) -> Result<BeatTime, Invalid> {
    if anchor && delta == requested {
        let ClipGroupPosition::Midi(beat) = position else {
            return Err(Invalid("midi anchor kind changed"));
        };
        Ok(beat)
    } else if delta == 0 {
        Ok(original)
    } else {
        project.tempo_map().frame_to_beat(frame)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    ids: Vec<ClipId>,
    anchor: ClipId,
    track: TrackId,
    position: ClipGroupPosition,
    copy: bool,
) -> Result<Created, Invalid> {
    let planned = project.preview_clip_group_move(&ids, anchor, track, position)?;
    let noop = planned.iter().all(|p| {
        project.track(p.track).is_some_and(|t| match &p.content {
            ClipGroupContent::Audio(c) => t.clips().iter().any(|old| old == c),
            ClipGroupContent::Midi(c) => t.midi_clips().iter().any(|old| old == c),
        })
    });
    if noop {
        return Ok(Created::None);
    }
    let selected = ids.into_iter().collect::<BTreeSet<_>>();
    let created = batch::apply(project, planned, &selected, copy)?;
    Project::from_snapshot(project.snapshot())?;
    Ok(created)
}

#[cfg(test)]
#[path = "clip_group_tests.rs"]
mod tests;

#[path = "clip_group_batch.rs"]
mod batch;
