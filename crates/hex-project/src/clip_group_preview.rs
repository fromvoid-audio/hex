//! Drag-local metadata reuse, correlated only to one immutable Project allocation.
use crate::{
    BeatTime, Clip, ClipGroupContent, ClipGroupPosition, ClipGroupPreview, ClipId, Invalid,
    MAX_CLIPS, Project, TrackId,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Weak},
};

#[derive(Debug)]
struct Original {
    track: usize,
    clip: usize,
    midi: bool,
    frame: u64,
    length: u64,
    beat: BeatTime,
}

/// Opaque UI projection storage. It never supplies a canonical edit or native
/// ownership proof. Project's private fields have no interior mutation: the
/// Weak witness forces get_mut/make_mut to refuse/dissociate in-place changes.
/// Storage and the Weak source witness cannot be extracted or replaced.
/// ```compile_fail
/// fn extract(capture: hex_project::ClipGroupMovePreview) {
///     let _ = capture.source;
/// }
/// ```
/// ```compile_fail
/// fn replace(capture: &mut hex_project::ClipGroupMovePreview) {
///     capture.storage.clear();
/// }
/// ```
/// ```compile_fail
/// fn mutate(capture: &mut hex_project::ClipGroupMovePreview, track: hex_project::TrackId) {
///     capture.previews()[0].track = track;
/// }
/// ```
#[derive(Debug)]
pub struct ClipGroupMovePreview {
    source: Weak<Project>,
    originals: Vec<Original>,
    storage: Vec<ClipGroupPreview>,
    anchor: usize,
    earliest: u64,
    visible: bool,
}

impl ClipGroupMovePreview {
    pub fn capture(
        project: &Arc<Project>,
        ids: &[ClipId],
        anchor: ClipId,
    ) -> Result<Self, Invalid> {
        if ids.is_empty() || ids.len() > MAX_CLIPS {
            return Err(Invalid("clip group exceeds project limits"));
        }
        let selected: BTreeSet<_> = ids.iter().copied().collect();
        if selected.len() != ids.len() || !selected.contains(&anchor) {
            return Err(Invalid("clip group contains duplicate ids or no anchor"));
        }
        let mut originals = Vec::new();
        let mut storage = Vec::new();
        originals
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve group source indices"))?;
        storage
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve group preview storage"))?;
        let mut anchor_index = None;
        let mut earliest = u64::MAX;
        for (track_index, track) in project.tracks().iter().enumerate() {
            for (clip_index, clip) in track
                .clips()
                .iter()
                .enumerate()
                .filter(|(_, c)| selected.contains(&c.id()))
            {
                if clip.id() == anchor {
                    anchor_index = Some(storage.len());
                }
                earliest = earliest.min(clip.start());
                let mut preview = clip.clone();
                if let Some(edit) = preview
                    .edit
                    .as_mut()
                    .filter(|edit| edit.live_musical_start().is_some())
                {
                    // Detach only the placement shell ONCE. The retained domain
                    // (processing recipe, events, fades) remains shared.
                    Arc::make_mut(edit);
                }
                originals.push(Original {
                    track: track_index,
                    clip: clip_index,
                    midi: false,
                    frame: clip.start(),
                    length: clip.duration(),
                    beat: BeatTime::ZERO,
                });
                storage.push(ClipGroupPreview {
                    track: track.id(),
                    content: ClipGroupContent::Audio(preview),
                });
            }
            for (clip_index, clip) in track
                .midi_clips()
                .iter()
                .enumerate()
                .filter(|(_, c)| selected.contains(&c.id()))
            {
                if clip.id() == anchor {
                    anchor_index = Some(storage.len());
                }
                let frame = project.tempo_map().beat_to_frame(clip.start())?;
                earliest = earliest.min(frame);
                originals.push(Original {
                    track: track_index,
                    clip: clip_index,
                    midi: true,
                    frame,
                    length: clip.length().raw(),
                    beat: clip.start(),
                });
                storage.push(ClipGroupPreview {
                    track: track.id(),
                    content: ClipGroupContent::Midi(clip.clone()),
                });
            }
        }
        if storage.len() != ids.len() {
            return Err(Invalid("selected arrangement clip no longer exists"));
        }
        Ok(Self {
            source: Arc::downgrade(project),
            originals,
            storage,
            anchor: anchor_index.ok_or(Invalid("group anchor no longer exists"))?,
            earliest,
            visible: false,
        })
    }

    pub fn matches_source(&self, project: &Arc<Project>) -> bool {
        Weak::ptr_eq(&self.source, &Arc::downgrade(project))
    }

    pub fn invalidate(&mut self) {
        self.visible = false;
    }

    pub fn previews(&self) -> &[ClipGroupPreview] {
        if self.visible { &self.storage } else { &[] }
    }

    /// Borrow the actual indexed original under the caller's current strong
    /// source borrow. Neither the indices nor an owning source handle escape.
    pub fn original_audio<'a>(&self, project: &'a Arc<Project>, member: usize) -> Option<&'a Clip> {
        if !self.matches_source(project) {
            return None;
        }
        let source = self.originals.get(member)?;
        if source.midi {
            return None;
        }
        project.tracks().get(source.track)?.clips().get(source.clip)
    }

    pub fn anchor_matches(
        &self,
        project: &Arc<Project>,
        id: ClipId,
        track: TrackId,
        position: ClipGroupPosition,
        length: u64,
    ) -> bool {
        if !self.matches_source(project) || self.storage[self.anchor].id() != id {
            return false;
        }
        let original = &self.originals[self.anchor];
        if project.tracks()[original.track].id() != track || original.length != length {
            return false;
        }
        match position {
            ClipGroupPosition::Audio(start) => !original.midi && original.frame == start,
            ClipGroupPosition::Midi(start) => original.midi && original.beat == start,
        }
    }

    pub fn update(
        &mut self,
        project: &Arc<Project>,
        target: TrackId,
        position: ClipGroupPosition,
    ) -> Result<(), Invalid> {
        self.invalidate();
        if !self.matches_source(project) {
            return Err(Invalid("clip group source allocation changed"));
        }
        let target_index = project
            .tracks()
            .iter()
            .position(|t| t.id() == target)
            .ok_or(Invalid("clip group target no longer exists"))?;
        let anchor = &self.originals[self.anchor];
        let destination = match position {
            ClipGroupPosition::Audio(frame) if !anchor.midi => frame,
            ClipGroupPosition::Midi(beat) if anchor.midi => {
                project.tempo_map().beat_to_frame(beat)?
            }
            _ => return Err(Invalid("clip group anchor kind changed")),
        };
        let requested = i128::from(destination) - i128::from(anchor.frame);
        let delta = requested.max(-i128::from(self.earliest));
        let track_delta = target_index as i128 - anchor.track as i128;
        // Must match `Project::preview_clip_group_move`: all-MIDI groups use one
        // exact beat delta rather than a frame round trip.
        let beat_delta = match position {
            ClipGroupPosition::Midi(beat) if self.originals.iter().all(|o| o.midi) => {
                let earliest = self
                    .originals
                    .iter()
                    .map(|o| o.beat)
                    .min()
                    .unwrap_or(anchor.beat);
                Some(super::clip_group::midi_beat_delta(
                    anchor.beat,
                    beat,
                    earliest,
                ))
            }
            _ => None,
        };
        for (index, (original, preview)) in self.originals.iter().zip(&mut self.storage).enumerate()
        {
            let target = usize::try_from(original.track as i128 + track_delta)
                .ok()
                .and_then(|i| project.tracks().get(i))
                .ok_or(Invalid("whole clip group does not fit destination tracks"))?;
            let frame = super::clip_group::translated(original.frame, delta)?;
            match &mut preview.content {
                ClipGroupContent::Audio(clip) => {
                    if let Some(edit) = clip
                        .edit
                        .as_mut()
                        .filter(|edit| edit.live_musical_start().is_some())
                    {
                        // A borrowed preview may be cloned by a caller. Never
                        // silently allocate another shell on the pointer path.
                        if Arc::get_mut(edit).is_none() {
                            return Err(Invalid("group preview placement metadata is shared"));
                        }
                    }
                    crate::audio_edit::move_clip(clip, frame, project.tempo_map())?;
                    clip.start()
                        .checked_add(clip.duration())
                        .ok_or(Invalid("group audio end overflow"))?;
                }
                ClipGroupContent::Midi(clip) => {
                    clip.start = match beat_delta {
                        Some(beat_delta) => {
                            super::clip_group::shifted_beat(original.beat, beat_delta)?
                        }
                        None => super::clip_group::midi_start(
                            project,
                            original.beat,
                            frame,
                            delta,
                            requested,
                            index == self.anchor,
                            position,
                        )?,
                    };
                    let end = BeatTime::from_raw(
                        clip.start()
                            .raw()
                            .checked_add(clip.length().raw())
                            .ok_or(Invalid("midi group end overflow"))?,
                    );
                    crate::midi::validate_clip(clip.start(), clip.length())?;
                    project.tempo_map().beat_to_frame(end)?;
                }
            }
            preview.track = target.id();
        }
        self.visible = true;
        Ok(())
    }
}
