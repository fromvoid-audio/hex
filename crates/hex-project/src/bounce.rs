//! Arrangement bounce intent and atomic adoption. Render post-device/pre-mixer;
//! mixer automation, sends, Returns and Main stay live. One Arrangement output
//! per source track spans selected clips (including gaps and release tails).
//! Whole-track bakes additionally print each Console source independently. Explicit ranges
//! have priority in the UI and adopt exactly their half-open frame interval.
//! In-place baking is restricted to complete tracks; clip and range targets
//! only render to new tracks. Audio grouping retains its separate pre-device
//! contract. Whole-track intent covers all
//! active Arrangement clips and every Console cell on one ordinary root track,
//! independent of selection. Inactive Arrangement clips, hierarchy and external
//! routing refuse; Console activation is retained on independently rendered cells.
//! Clip and whole-track bakes retain the bounded release-tail policy. New-track
//! clip bounces keep the selected span as the clip view; the tail stays in the
//! rendered file beyond the view end.
use crate::{ClipId, Command, Created, Invalid, Project, SessionCellContent, TimeRange, TrackId};
use std::{collections::BTreeSet, path::PathBuf};

pub const MAX_ARRANGEMENT_BOUNCE_FRAMES: u64 = 8_000_000;
mod console;
/// Stable output identity; Console cycles refer to the rendered file's frames,
/// not to the original source clock. Outputs are adopted in captured plan order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceSource {
    Arrangement,
    Console {
        clip: ClipId,
        loop_range: Option<crate::SessionAudioLoop>,
    },
}
#[derive(Clone, Debug)]
pub enum BounceTarget {
    Clips(Vec<ClipId>),
    /// Complete Arrangement/Console track bake. Requires in-place adoption; never
    /// derives its extent from a current clip or time selection.
    Track(TrackId),
    Range(TimeRange),
    /// Finite pre-device audio grouping, not a track/device bake. The range
    /// variant retains exterior fragments; the clip variant requires two clips.
    AudioGroup {
        clips: Vec<ClipId>,
        range: Option<TimeRange>,
    },
}
impl BounceTarget {
    pub fn is_audio_group(&self) -> bool {
        matches!(self, Self::AudioGroup { .. })
    }
    /// Clip and whole-track bakes include releases; ranges and grouping do not.
    pub fn includes_tails(&self) -> bool {
        matches!(self, Self::Clips(_) | Self::Track(_))
    }
    /// Only a device bake replaces live voices and needs the stopped fence.
    pub fn bakes_devices(&self, in_place: bool) -> bool {
        in_place && !self.is_audio_group()
    }
}
/// Where a new (not in-place) bounce lands and what happens to its sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceAdoption {
    /// One new track below each source track; sources stay unchanged.
    NewTracks,
    /// As `NewTracks`, then deactivate the bounced sources (contextual `c`/`0`).
    NewTracksMutingSources,
    /// Held-`b` clip drop: outputs keep their offsets from the dragged anchor and
    /// overwrite their destination span; sources stay unchanged. Clips only.
    Drop(BounceDrop),
}
/// Captured drop of a dragged anchor clip, in the drag's own clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BounceDrop {
    pub anchor: ClipId,
    pub track: TrackId,
    pub position: crate::ClipGroupPosition,
}
#[derive(Clone, Debug)]
pub struct BounceTrack {
    pub source: BounceSource,
    pub track: TrackId,
    pub start: u64,
    pub end: u64,
    pub clips: Vec<ClipId>,
}
#[derive(Clone, Debug)]
pub struct BounceFile {
    pub source: BounceSource,
    pub track: TrackId,
    pub start: u64,
    pub frames: u64,
    pub path: PathBuf,
}
impl Project {
    /// Validate a held-`b` clip drop before rendering: the same placement rules
    /// commit applies, so overlap and off-track drops refuse without a render.
    pub fn validate_bounce_drop(
        &self,
        target: &BounceTarget,
        drop: BounceDrop,
    ) -> Result<(), Invalid> {
        if !matches!(target, BounceTarget::Clips(_)) {
            return Err(Invalid("bounce drop requires dragged arrangement clips"));
        }
        let plan = self.plan_arrangement_bounce(target, false)?;
        drop_placements(self, &plan, drop).map(|_| ())
    }
    /// Validate captured intent before rendering. No file or history mutation.
    pub fn plan_arrangement_bounce(
        &self,
        target: &BounceTarget,
        in_place: bool,
    ) -> Result<Vec<BounceTrack>, Invalid> {
        if let BounceTarget::AudioGroup { clips, range } = target {
            if !in_place {
                return Err(Invalid("audio grouping replaces its selected clips"));
            }
            return Ok(vec![crate::clip_consolidate::render::plan(
                self,
                clips,
                range.as_ref(),
            )?]);
        }
        if in_place && !matches!(target, BounceTarget::Track(_)) {
            return Err(Invalid(
                "bounce in place requires a complete track · use grouping for clips or time selections",
            ));
        }
        if !self.audio_feeds.is_empty() || !self.midi_routes.is_empty() || !self.routing.is_empty()
        {
            return Err(Invalid(
                "bounce does not support external audio or midi routing",
            ));
        }
        if let BounceTarget::Track(id) = target {
            if !in_place {
                return Err(Invalid("whole-track bounce requires in-place adoption"));
            }
            let track = self
                .track(*id)
                .ok_or(Invalid("bounce track no longer exists"))?;
            if track.kind() != crate::TrackKind::Ordinary || track.parent_group().is_some() {
                return Err(Invalid(
                    "whole-track bounce requires an ordinary ungrouped track",
                ));
            }
            // Console cells are printed independently. A shared nonlinear source
            // cannot isolate those cells from another contributor's authored MIDI.
            let console = self.session_cells().iter().any(|cell| {
                cell.track() == *id && !matches!(cell.content(), SessionCellContent::Stop)
            });
            if console
                && track.source_id().is_some_and(|source| {
                    self.native_source(source)
                        .is_some_and(|definition| definition.receiver() == Some(*id))
                        && self.tracks().iter().any(|peer| {
                            peer.id() != *id
                                && peer.source_id() == Some(source)
                                && (!peer.midi_clips().is_empty()
                                    || self.session_cells().iter().any(|cell| {
                                        cell.track() == peer.id()
                                            && matches!(cell.content(), SessionCellContent::Midi(_))
                                    }))
                        })
                })
            {
                return Err(Invalid(
                    "cannot independently bounce console cells while another shared-source contributor has midi",
                ));
            }
            // Clearing a receiver would silence every other linked track even
            // when it currently has no clips. Keep their source route intact.
            if track.source_id().is_some_and(|source| {
                self.native_source(source)
                    .is_some_and(|definition| definition.receiver() == Some(*id))
                    && self
                        .tracks()
                        .iter()
                        .any(|peer| peer.id() != *id && peer.source_id() == Some(source))
            }) {
                return Err(Invalid(
                    "cannot bounce this shared source receiver · unlink or reroute linked tracks first",
                ));
            }
            if track.clips().iter().any(|c| !c.active())
                || track.midi_clips().iter().any(|c| !c.active())
            {
                return Err(Invalid(
                    "activate all arrangement clips before whole-track bounce",
                ));
            }
        }
        let selected = match target {
            BounceTarget::Clips(ids) => {
                if ids.is_empty() || ids.len() > crate::MAX_CLIPS {
                    return Err(Invalid("select arrangement clips to bounce"));
                }
                let set = ids.iter().copied().collect::<BTreeSet<_>>();
                if set.len() != ids.len() {
                    return Err(Invalid("bounce contains duplicate clips"));
                }
                set
            }
            BounceTarget::Track(id) => {
                let track = self
                    .track(*id)
                    .ok_or(Invalid("bounce track no longer exists"))?;
                track
                    .clips()
                    .iter()
                    .map(|c| c.id())
                    .chain(track.midi_clips().iter().map(|c| c.id()))
                    .collect()
            }
            BounceTarget::Range(range) => {
                range.validate(self)?;
                BTreeSet::new()
            }
            BounceTarget::AudioGroup { .. } => {
                return Err(Invalid("audio grouping plan was not resolved"));
            }
        };
        let mut found = 0;
        let mut result = Vec::new();
        for track in self.tracks() {
            let mut clips = Vec::new();
            let mut start = u64::MAX;
            let mut end = 0;
            for (id, first, last) in track
                .clips()
                .iter()
                .map(|c| (c.id(), c.start(), c.start().checked_add(c.duration())))
                .chain(track.midi_clips().iter().map(|c| {
                    let range = midi_frames(self, c);
                    (
                        c.id(),
                        range.as_ref().map_or(0, |r| r.0),
                        range.ok().map(|r| r.1),
                    )
                }))
            {
                let last = last.ok_or(Invalid("bounce source interval overflows"))?;
                if selected.contains(&id) {
                    found += 1;
                    start = start.min(first);
                    end = end.max(last);
                    clips.push(id);
                }
            }
            if let BounceTarget::Range(range) = target {
                if !range.tracks().contains(&track.id()) {
                    continue;
                }
                start = range.start();
                end = range.end();
            } else if clips.is_empty() {
                continue;
            }
            if end
                .checked_sub(start)
                .is_none_or(|n| n == 0 || n > MAX_ARRANGEMENT_BOUNCE_FRAMES)
            {
                return Err(Invalid("bounce exceeds its finite frame budget"));
            }
            result.push(BounceTrack {
                source: crate::BounceSource::Arrangement,
                track: track.id(),
                start,
                end,
                clips,
            });
        }
        if let BounceTarget::Track(track) = target {
            result.extend(console::plan(self, *track)?);
        }
        if matches!(target, BounceTarget::Clips(_)) && found != selected.len() {
            return Err(Invalid("bounce clip no longer exists in arrangement"));
        }
        if result.is_empty() {
            return Err(Invalid("select arrangement or console material to bounce"));
        }
        if !in_place
            && self
                .tracks()
                .len()
                .checked_add(result.len())
                .is_none_or(|n| n > crate::MAX_TRACKS)
        {
            return Err(Invalid("bounce exceeds track limit"));
        }
        Ok(result)
    }
}

pub(crate) fn commit(
    project: &mut Project,
    target: BounceTarget,
    in_place: bool,
    adoption: crate::BounceAdoption,
    files: Vec<BounceFile>,
) -> Result<Created, Invalid> {
    match adoption {
        BounceAdoption::NewTracks => {}
        BounceAdoption::NewTracksMutingSources
            if !in_place && matches!(target, BounceTarget::Clips(_) | BounceTarget::Range(_)) => {}
        BounceAdoption::NewTracksMutingSources => {
            return Err(Invalid(
                "bounce and mute requires selected clips or a time selection on new tracks",
            ));
        }
        BounceAdoption::Drop(_) if !in_place && matches!(target, BounceTarget::Clips(_)) => {}
        BounceAdoption::Drop(_) => {
            return Err(Invalid("bounce drop requires dragged arrangement clips"));
        }
    }
    if let BounceTarget::AudioGroup { clips, range } = &target {
        if !in_place {
            return Err(Invalid("audio grouping replaces its selected clips"));
        }
        return crate::clip_consolidate::render::commit(project, clips, range.as_ref(), files);
    }
    let plan = project.plan_arrangement_bounce(&target, in_place)?;
    if files.len() != plan.len() {
        return Err(Invalid("bounce output count changed"));
    }
    let mut paths = std::collections::BTreeSet::new();
    for file in &files {
        if !paths.insert(&file.path) {
            return Err(Invalid("bounce outputs must have distinct media paths"));
        }
    }
    let placements = match adoption {
        BounceAdoption::Drop(drop) => Some(drop_placements(project, &plan, drop)?),
        BounceAdoption::NewTracks | BounceAdoption::NewTracksMutingSources => None,
    };
    let mut first = None;
    let mut last = None;
    for (index, (item, file)) in plan.into_iter().zip(files).enumerate() {
        if file.source != item.source
            || file.track != item.track
            || file.start != item.start
            || file.frames == 0
            || file.frames > MAX_ARRANGEMENT_BOUNCE_FRAMES
            || file.start.checked_add(file.frames).is_none()
            || match target {
                BounceTarget::Range(_) => file.frames != item.end - item.start,
                BounceTarget::Clips(_) | BounceTarget::Track(_) => {
                    file.frames < item.end - item.start
                }
                BounceTarget::AudioGroup { .. } => true,
            }
        {
            return Err(Invalid("bounce output differs from captured interval"));
        }
        if matches!(item.source, BounceSource::Console { .. }) {
            if !in_place || !matches!(target, BounceTarget::Track(id) if id == item.track) {
                return Err(Invalid(
                    "console bounce requires complete in-place track intent",
                ));
            }
            let clip = console::adopt(project, &item, file)?;
            first.get_or_insert(clip);
            last = Some(clip);
            continue;
        }
        let original = project
            .track(item.track)
            .ok_or(Invalid("bounce track changed"))?
            .clone();
        if in_place && target.includes_tails() {
            let output_end = file
                .start
                .checked_add(file.frames)
                .ok_or(Invalid("bounce end overflows"))?;
            let overlaps = original.clips().iter().any(|c| {
                !item.clips.contains(&c.id())
                    && c.start() < output_end
                    && c.start()
                        .checked_add(c.duration())
                        .is_none_or(|end| end > file.start)
            }) || original.midi_clips().iter().any(|c| {
                !item.clips.contains(&c.id())
                    && midi_frames(project, c)
                        .map_or(true, |(s, e)| s < output_end && e > file.start)
            });
            if overlaps {
                return Err(Invalid(
                    "bounce tail overlaps unselected material · use b to bounce to a new track",
                ));
            }
        }
        let span = item.end - item.start;
        let (destination, start) = if let Some(placements) = &placements {
            let (track, start) = placements[index];
            project.apply(Command::DeleteTimeRange {
                range: TimeRange::new(vec![track], start, start + span)?,
            })?;
            (track, start)
        } else if in_place {
            match &target {
                BounceTarget::Clips(_) | BounceTarget::Track(_) => {
                    for clip in &item.clips {
                        project.apply(if project.midi_clip(*clip).is_some() {
                            Command::RemoveMidiClip(*clip)
                        } else {
                            Command::RemoveClip(*clip)
                        })?;
                    }
                }
                BounceTarget::Range(_) => {
                    project.apply(Command::DeleteTimeRange {
                        range: TimeRange::new(vec![item.track], item.start, item.end)?,
                    })?;
                }
                BounceTarget::AudioGroup { .. } => {
                    return Err(Invalid("audio grouping plan was not resolved"));
                }
            }
            if !matches!(target, BounceTarget::Track(_)) {
                if project
                    .track(item.track)
                    .is_some_and(|owner| owner.instrument().is_some())
                {
                    project.apply(Command::RemoveInstrument { track: item.track })?;
                }
                let owner = project
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == item.track)
                    .ok_or(Invalid("bounce track changed"))?;
                owner.devices.clear();
                owner.instrument = None;
                owner.instrument_position = None;
            }
            (item.track, file.start)
        } else {
            let index = project
                .tracks
                .iter()
                .position(|t| t.id == item.track)
                .ok_or(Invalid("bounce track changed"))?;
            let before_track = project.tracks.get(index + 1).map(|t| t.id);
            let Created::Track(id) = project.apply(Command::InsertTrack {
                name: format!("{} bounce", original.name),
                before_track,
            })?
            else {
                return Err(Invalid("bounce track was not created"));
            };
            let owner = project
                .tracks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or(Invalid("bounce track missing"))?;
            owner.color = original.color;
            owner.gain = original.gain;
            owner.balance = original.balance;
            owner.gain_lane = original.gain_lane;
            owner.sends = original.sends;
            owner.muted = original.muted;
            owner.soloed = original.soloed;
            (id, file.start)
        };
        // No overwrite: a release tail intersecting unselected material refuses the
        // entire private candidate instead of erasing or truncating either source.
        let Created::Imported { clip, .. } = project.apply(Command::ImportPreparedAudioClip {
            track: destination,
            start,
            path: file.path,
            source_sample_rate: project.sample_rate(),
            source_frames: file.frames,
            playback_frames: file.frames,
            conversion_version: crate::IDENTITY_CONVERSION,
        })?
        else {
            return Err(Invalid("bounce output was not adopted"));
        };
        // A new-track clip bounce keeps the selected span; the rendered release
        // tail stays in the file beyond the view end, revealed by extending it.
        if !in_place && matches!(target, BounceTarget::Clips(_)) && file.frames > span {
            project.apply(Command::TrimClip {
                clip,
                start,
                duration: span,
                source_offset: 0,
                source_length: span,
            })?;
        }
        first.get_or_insert(clip);
        last = Some(clip);
    }
    // Same canonical deactivation as contextual `c`/`0`, but always inactive:
    // a toggle could reactivate material that was already partly muted.
    if adoption == BounceAdoption::NewTracksMutingSources {
        match &target {
            BounceTarget::Clips(clips) => project.apply(Command::EditClips {
                clips: clips.clone(),
                edit: crate::ClipEdit::Active { active: false },
            })?,
            BounceTarget::Range(range) => project.apply(Command::SetTimeRangeActive {
                range: range.clone(),
                active: false,
            })?,
            BounceTarget::Track(_) | BounceTarget::AudioGroup { .. } => {
                return Err(Invalid("bounce and mute target changed"));
            }
        };
    }
    if let BounceTarget::Track(track) = target {
        if project
            .track(track)
            .is_some_and(|owner| owner.instrument().is_some())
        {
            project.apply(Command::RemoveInstrument { track })?;
        }
        let owner = project
            .tracks
            .iter_mut()
            .find(|t| t.id == track)
            .ok_or(Invalid("bounce track changed"))?;
        owner.devices.clear();
        owner.instrument = None;
        owner.instrument_position = None;
    }
    match (first, last) {
        (Some(first), Some(last)) => Ok(Created::Clips { first, last }),
        _ => Err(Invalid("bounce produced no clips")),
    }
}

/// Resolve each output's destination track and start from the anchor's drop.
/// Track order and frames shift by the anchor's own offset; any destination
/// outside the track list, or overlapping a bounced source, refuses.
fn drop_placements(
    project: &Project,
    plan: &[BounceTrack],
    drop: BounceDrop,
) -> Result<Vec<(TrackId, u64)>, Invalid> {
    let index_of = |id| project.tracks().iter().position(|t| t.id() == id);
    let interval = |id| -> Result<Option<(u64, u64)>, Invalid> {
        if let Some(clip) = project.clip(id) {
            return Ok(Some((clip.start(), clip.start() + clip.duration())));
        }
        project
            .midi_clip(id)
            .map(|clip| midi_frames(project, clip))
            .transpose()
    };
    let anchor = plan
        .iter()
        .find(|item| item.clips.contains(&drop.anchor))
        .ok_or(Invalid("bounce drop anchor is not a dragged clip"))?;
    let (anchor_start, _) =
        interval(drop.anchor)?.ok_or(Invalid("bounce drop anchor no longer exists"))?;
    let destination = match drop.position {
        crate::ClipGroupPosition::Audio(frame) => frame,
        crate::ClipGroupPosition::Midi(beat) => project.tempo_map().beat_to_frame(beat)?,
    };
    let track_shift = index_of(drop.track)
        .zip(index_of(anchor.track))
        .map(|(to, from)| to as i128 - from as i128)
        .ok_or(Invalid("bounce drop track no longer exists"))?;
    let frame_shift = i128::from(destination) - i128::from(anchor_start);
    let mut sources = Vec::new();
    for item in plan {
        for id in &item.clips {
            if let Some(range) = interval(*id)? {
                sources.push((item.track, range));
            }
        }
    }
    plan.iter()
        .map(|item| {
            let track = index_of(item.track)
                .and_then(|i| usize::try_from(i as i128 + track_shift).ok())
                .and_then(|i| project.tracks().get(i))
                .filter(|t| t.kind() == crate::TrackKind::Ordinary)
                .ok_or(Invalid("bounce drop lands outside the ordinary tracks"))?
                .id();
            let start = u64::try_from(i128::from(item.start) + frame_shift)
                .map_err(|_| Invalid("bounce drop starts before the arrangement"))?;
            let end = start
                .checked_add(item.end - item.start)
                .ok_or(Invalid("bounce drop end overflows"))?;
            if sources
                .iter()
                .any(|(owner, (s, e))| *owner == track && *s < end && *e > start)
            {
                return Err(Invalid(
                    "bounce drop would overwrite its own source · drop elsewhere",
                ));
            }
            Ok((track, start))
        })
        .collect()
}

fn midi_frames(project: &Project, clip: &crate::MidiClip) -> Result<(u64, u64), Invalid> {
    let end = clip
        .start()
        .raw()
        .checked_add(clip.length().raw())
        .ok_or(Invalid("midi bounce interval overflows"))?;
    Ok((
        project.tempo_map().beat_to_frame(clip.start())?,
        project
            .tempo_map()
            .beat_to_frame(crate::BeatTime::from_raw(end))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, TrackDeviceSlot, default_instrument_state};

    #[test]
    fn whole_track_bounce_refuses_to_disconnect_other_source_contributors() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        editor
            .execute(Command::PlaceMidiClip {
                track: receiver,
                start: BeatTime::ONE,
                length: BeatTime::ONE,
            })
            .unwrap();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let item = editor
            .project()
            .plan_arrangement_bounce(&BounceTarget::Track(receiver), true)
            .unwrap()
            .remove(0);
        let Created::Track(peer) = editor
            .execute(Command::AddMidiTrack {
                name: "linked".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        editor
            .execute(Command::Gain {
                track: peer,
                gain: 0.5,
            })
            .unwrap();
        assert!(editor.undo());
        let original = editor.project().clone();
        let err = editor
            .project()
            .plan_arrangement_bounce(&BounceTarget::Track(receiver), true)
            .unwrap_err();
        assert!(err.to_string().contains("linked tracks"), "{err}");
        let err = editor
            .execute(Command::CommitArrangementBounce {
                target: BounceTarget::Track(receiver),
                in_place: true,
                adoption: BounceAdoption::NewTracks,
                files: vec![BounceFile {
                    source: BounceSource::Arrangement,
                    track: receiver,
                    start: item.start,
                    frames: item.end - item.start,
                    path: "bounced.wav".into(),
                }],
            })
            .unwrap_err();
        assert!(err.to_string().contains("linked tracks"), "{err}");
        assert_eq!(editor.project(), &original);
        assert!(editor.can_redo());
        assert_eq!(
            editor.project().native_source(source).unwrap().receiver(),
            Some(receiver)
        );
        assert_eq!(
            editor.project().track(peer).unwrap().source_id(),
            Some(source)
        );
    }

    #[test]
    fn in_place_bounce_unlinks_receiver_but_retains_source_across_history() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "source".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("source track")
        };
        let Created::Clip(_clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        let source = editor.project().track(track).unwrap().source_id().unwrap();
        let instrument = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();
        let original = editor.project().clone();
        let item = original
            .plan_arrangement_bounce(&BounceTarget::Track(track), true)
            .unwrap()
            .remove(0);
        let after_command = Command::CommitArrangementBounce {
            target: BounceTarget::Track(track),
            in_place: true,
            adoption: BounceAdoption::NewTracks,
            files: vec![BounceFile {
                source: BounceSource::Arrangement,
                track,
                start: item.start,
                frames: item.end - item.start,
                path: "bounced.wav".into(),
            }],
        };

        editor.execute(after_command).unwrap();
        let after = editor.project().clone();
        assert!(after.track(track).unwrap().instrument().is_none());
        let retained = after.native_source(source).unwrap();
        assert_eq!(retained.instrument().id(), instrument);
        assert_eq!(retained.receiver(), None);
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);

        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        restored.next_id = original.snapshot().next_id;
        assert_eq!(restored, original.snapshot());
        assert_eq!(
            editor.project().native_source(source).unwrap().receiver(),
            Some(track)
        );
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
        assert_eq!(
            editor
                .project()
                .native_source(source)
                .unwrap()
                .instrument()
                .id(),
            instrument
        );
    }

    #[test]
    fn retired_clip_and_range_in_place_refuse_without_history_mutation() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, clip, asset } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!()
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 100,
                source_offset: 0,
                source_length: 100,
                duration: 100,
                looping: false,
            })
            .unwrap();
        let original = editor.project().clone();
        let output = |frames| BounceFile {
            source: crate::BounceSource::Arrangement,
            track,
            start: 0,
            frames,
            path: "generated.wav".into(),
        };
        let error = editor
            .execute(Command::CommitArrangementBounce {
                target: BounceTarget::Clips(vec![clip]),
                in_place: true,
                adoption: BounceAdoption::NewTracks,
                files: vec![output(150)],
            })
            .unwrap_err();
        assert!(error.to_string().contains("complete track"));
        assert_eq!(editor.project(), &original);
        assert!(
            editor
                .execute(Command::CommitArrangementBounce {
                    target: BounceTarget::Range(TimeRange::new(vec![track], 0, 100).unwrap()),
                    in_place: true,
                    adoption: BounceAdoption::NewTracks,
                    files: vec![output(101)]
                })
                .is_err()
        );
        assert_eq!(editor.project(), &original);
        assert!(
            editor
                .execute(Command::CommitArrangementBounce {
                    target: BounceTarget::Clips(vec![clip]),
                    in_place: false,
                    adoption: BounceAdoption::NewTracks,
                    files: vec![output(150)]
                })
                .is_ok()
        );
        assert_eq!(editor.project().tracks()[0], original.tracks()[0]);
        // The view keeps the selected span; the 50-frame tail stays in the file.
        let bounced = &editor.project().tracks()[1].clips()[0];
        assert_eq!((bounced.duration(), bounced.source_length()), (100, 100));
        let asset = editor.project().asset(bounced.asset()).unwrap();
        assert_eq!(asset.playback_frames(), 150);
        let bounced = bounced.id();
        editor
            .execute(Command::TrimClipEdge {
                clip: bounced,
                edge: crate::TrimEdge::Right,
                frame: 150,
            })
            .unwrap();
        let extended = editor.project().clip(bounced).unwrap();
        assert_eq!((extended.duration(), extended.source_length()), (150, 150));
        assert!(editor.undo());
        assert!(editor.undo());
        assert_eq!(
            editor.project().snapshot().tracks,
            original.snapshot().tracks
        );
    }
    #[test]
    fn whole_track_refuses_incomplete_content_without_mutating_history() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let target = BounceTarget::Track(track);
        assert!(
            editor
                .project()
                .plan_arrangement_bounce(&target, false)
                .is_err()
        );
        editor
            .execute(Command::SetClipActive {
                clip,
                active: false,
            })
            .unwrap();
        assert!(
            editor
                .project()
                .plan_arrangement_bounce(&target, true)
                .unwrap_err()
                .to_string()
                .contains("activate all")
        );
        editor
            .execute(Command::SetClipActive { clip, active: true })
            .unwrap();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        editor
            .execute(Command::CopyArrangementAudioClipToSession {
                source: clip,
                scene,
            })
            .unwrap();
        let before = editor.project().clone();
        assert_eq!(
            editor
                .project()
                .plan_arrangement_bounce(&target, true)
                .unwrap()
                .len(),
            2
        );
        assert!(
            editor
                .execute(Command::CommitArrangementBounce {
                    target,
                    in_place: true,
                    adoption: BounceAdoption::NewTracks,
                    files: vec![BounceFile {
                        source: crate::BounceSource::Arrangement,
                        track,
                        start: 0,
                        frames: 100,
                        path: "bounce.wav".into()
                    }],
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        let plan = editor
            .project()
            .plan_arrangement_bounce(&BounceTarget::Track(track), true)
            .unwrap();
        let mut files: Vec<_> = plan
            .iter()
            .enumerate()
            .map(|(i, item)| BounceFile {
                source: item.source,
                track: item.track,
                start: item.start,
                frames: item.end - item.start,
                path: format!("bounce-{i}.wav").into(),
            })
            .collect();
        // A second output from the same track must still identify its Console
        // cell; failure after the first private replacement cannot leak edits.
        files[1].source = BounceSource::Arrangement;
        assert!(
            editor
                .execute(Command::CommitArrangementBounce {
                    target: BounceTarget::Track(track),
                    in_place: true,
                    adoption: BounceAdoption::NewTracks,
                    files
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .plan_arrangement_bounce(&BounceTarget::Track(track), true)
                .is_ok()
        );
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: crate::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        for id in [track, group] {
            assert!(
                editor
                    .project()
                    .plan_arrangement_bounce(&BounceTarget::Track(id), true)
                    .unwrap_err()
                    .to_string()
                    .contains("ungrouped")
            );
        }
    }

    #[test]
    fn bounce_and_mute_deactivates_exact_sources_in_one_history_step() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let original = editor.project().clone();
        let output = |start, frames| BounceFile {
            source: BounceSource::Arrangement,
            track,
            start,
            frames,
            path: "generated.wav".into(),
        };
        let range = TimeRange::new(vec![track], 25, 75).unwrap();
        for (target, in_place) in [
            (BounceTarget::Clips(vec![clip]), true),
            (BounceTarget::Track(track), true),
            (
                BounceTarget::AudioGroup {
                    clips: vec![clip],
                    range: None,
                },
                true,
            ),
        ] {
            assert!(
                editor
                    .execute(Command::CommitArrangementBounce {
                        target,
                        in_place,
                        adoption: BounceAdoption::NewTracksMutingSources,
                        files: vec![output(0, 100)],
                    })
                    .is_err()
            );
            assert_eq!(editor.project(), &original);
        }

        editor
            .execute(Command::CommitArrangementBounce {
                target: BounceTarget::Clips(vec![clip]),
                in_place: false,
                adoption: BounceAdoption::NewTracksMutingSources,
                files: vec![output(0, 100)],
            })
            .unwrap();
        let muted = editor.project().clone();
        assert!(!muted.clip(clip).unwrap().active());
        assert!(muted.tracks()[1].clips()[0].active());
        assert!(editor.undo());
        assert_eq!(
            editor.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert!(editor.redo());
        assert_eq!(editor.project(), &muted);
        assert!(editor.undo());

        editor
            .execute(Command::CommitArrangementBounce {
                target: BounceTarget::Range(range.clone()),
                in_place: false,
                adoption: BounceAdoption::NewTracksMutingSources,
                files: vec![output(25, 50)],
            })
            .unwrap();
        let fragments = editor.project().tracks()[0]
            .clips()
            .iter()
            .map(|c| (c.start(), c.duration(), c.active()))
            .collect::<Vec<_>>();
        assert_eq!(fragments, [(0, 25, true), (25, 50, false), (75, 25, true)]);
        let bounced = &editor.project().tracks()[1].clips()[0];
        assert_eq!((bounced.start(), bounced.duration()), (25, 50));
        assert!(editor.undo());
        assert_eq!(
            editor.project().snapshot().tracks,
            original.snapshot().tracks
        );
    }

    #[test]
    fn bounce_drop_lands_at_the_anchor_offset_and_keeps_sources() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let Created::Track(other) = editor
            .execute(Command::InsertTrack {
                name: "other".into(),
                before_track: None,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let original = editor.project().clone();
        let commit = |track_to, start: u64| Command::CommitArrangementBounce {
            target: BounceTarget::Clips(vec![clip]),
            in_place: false,
            adoption: BounceAdoption::Drop(BounceDrop {
                anchor: clip,
                track: track_to,
                position: crate::ClipGroupPosition::Audio(start),
            }),
            files: vec![BounceFile {
                source: BounceSource::Arrangement,
                track,
                start: 0,
                frames: 150,
                path: "generated.wav".into(),
            }],
        };
        // Overlapping the source on its own track refuses without any edit.
        let error = editor.execute(commit(track, 50)).unwrap_err();
        assert!(error.to_string().contains("own source"), "{error}");
        assert_eq!(editor.project(), &original);

        editor.execute(commit(other, 300)).unwrap();
        let p = editor.project();
        assert_eq!(p.tracks().len(), 2);
        assert_eq!(p.tracks()[0], original.tracks()[0]);
        let bounced = &p.track(other).unwrap().clips()[0];
        assert_eq!((bounced.start(), bounced.duration()), (300, 100));
        assert_eq!(p.asset(bounced.asset()).unwrap().playback_frames(), 150);
        let dropped = p.clone();
        assert!(editor.undo());
        assert_eq!(
            editor.project().snapshot().tracks,
            original.snapshot().tracks
        );
        assert!(editor.redo());
        assert_eq!(editor.project(), &dropped);
        assert!(editor.undo());

        // A same-track drop clear of the source is allowed; the source stays.
        editor.execute(commit(track, 100)).unwrap();
        let clips = editor.project().track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[0].id(), clips[0].start()), (clip, 0));
        assert_eq!((clips[1].start(), clips[1].duration()), (100, 100));
        assert!(editor.undo());
        for target in [
            BounceTarget::Range(TimeRange::new(vec![track], 0, 100).unwrap()),
            BounceTarget::Track(track),
        ] {
            let Command::CommitArrangementBounce {
                adoption, files, ..
            } = commit(other, 300)
            else {
                unreachable!()
            };
            assert!(
                editor
                    .execute(Command::CommitArrangementBounce {
                        target,
                        in_place: false,
                        adoption,
                        files,
                    })
                    .is_err()
            );
            assert_eq!(
                editor.project().snapshot().tracks,
                original.snapshot().tracks
            );
        }
    }
}
