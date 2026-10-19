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
                let start = if clip.id() == anchor && delta == requested_delta {
                    let ClipGroupPosition::Midi(beat) = position else {
                        return Err(Invalid("midi anchor kind changed"));
                    };
                    beat
                } else if delta == 0 {
                    clip.start()
                } else {
                    self.tempo_map().frame_to_beat(frame)?
                };
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
fn translated(frame: u64, delta: i128) -> Result<u64, Invalid> {
    u64::try_from(i128::from(frame) + delta)
        .map_err(|_| Invalid("whole clip group exceeds timeline bounds"))
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
    // Protect ALL moving originals before carving any destination. Sequential
    // single-clip overwrite commands could erase a later selected member.
    let selected = ids.into_iter().collect::<BTreeSet<_>>();
    if !copy {
        for row in &mut project.tracks {
            row.content
                .audio_clips
                .retain(|c| !selected.contains(&c.id()));
            row.content
                .midi_clips
                .retain(|c| !selected.contains(&c.id()));
        }
    }
    // Copies keep selected originals physically present for capacity/ID checks.
    // Protect both victim kinds while still carving unselected destinations.
    let protected = |id| copy && selected.contains(&id);
    // Clear old destination content first; incoming members may overlap each
    // other in an existing valid project and must never erase one another.
    for member in &planned {
        match &member.content {
            ClipGroupContent::Audio(c) => {
                let end = c
                    .start()
                    .checked_add(c.duration())
                    .ok_or(Invalid("group audio end overflow"))?;
                crate::edit::overwrite_audio_interval_excluding(
                    project,
                    member.track,
                    c.start(),
                    end,
                    protected,
                )?;
                crate::midi::overwrite_midi_in_frames_excluding(
                    project,
                    member.track,
                    c.start(),
                    end,
                    protected,
                )?;
            }
            ClipGroupContent::Midi(c) => {
                let end = BeatTime::from_raw(
                    c.start()
                        .raw()
                        .checked_add(c.length().raw())
                        .ok_or(Invalid("group midi end overflow"))?,
                );
                crate::edit::overwrite_audio_in_beats_excluding(
                    project,
                    member.track,
                    c.start(),
                    end,
                    protected,
                )?;
                crate::midi::overwrite_midi_interval_excluding(
                    project,
                    member.track,
                    c.start(),
                    end,
                    protected,
                )?;
            }
        }
    }
    for member in planned {
        match member.content {
            ClipGroupContent::Audio(c) => {
                if copy {
                    crate::clip_copy::append_audio(project, c, member.track)?;
                } else {
                    let clips = project.audio_clips_mut(member.track)?;
                    clips
                        .try_reserve(1)
                        .map_err(|_| Invalid("could not reserve moved audio clips"))?;
                    clips.push(c);
                }
            }
            ClipGroupContent::Midi(c) => {
                if copy {
                    let start = c.start();
                    crate::clip_copy::append_midi(project, c, member.track, start)?;
                } else {
                    let clips = &mut project.track_mut(member.track)?.content.midi_clips;
                    clips
                        .try_reserve(1)
                        .map_err(|_| Invalid("could not reserve moved midi clips"))?;
                    clips.push(c);
                }
            }
        }
    }
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::None)
}

#[cfg(test)]
#[path = "clip_group_tests.rs"]
mod tests;
