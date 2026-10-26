//! Arrangement bounce intent and atomic adoption. Render post-device/pre-mixer;
//! mixer automation, sends, Returns and Main stay live. One Arrangement output
//! per source track spans selected clips (including gaps and release tails).
//! Whole-track bakes additionally print each Console source independently. Explicit ranges
//! have priority in the UI and adopt exactly their half-open frame interval.
//! Exact in-place ranges retain all original input and devices, adopt a
//! post-device print and suppress original output over precisely that interval.
//! Deleting the print retains suppression. Clip/whole-track baking removes the
//! baked device chain only when no unselected
//! Arrangement or Console material can be affected. Whole-track intent covers all
//! active Arrangement clips and every Console cell on one ordinary root track,
//! independent of selection. Inactive Arrangement clips, hierarchy and external
//! routing refuse; Console activation is retained on independently rendered cells.
//! Clip and whole-track bakes retain the bounded release-tail policy.
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
            if in_place
                && !matches!(target, BounceTarget::Range(_))
                && (track.instrument().is_some() || !track.devices().is_empty())
            {
                let unrelated = match target {
                    BounceTarget::Clips(_) | BounceTarget::Track(_) => {
                        clips.len() != track.clips().len() + track.midi_clips().len()
                    }
                    BounceTarget::Range(_) => {
                        track.clips().iter().any(|c| {
                            c.start() < start
                                || c.start().checked_add(c.duration()).is_none_or(|e| e > end)
                        }) || track.midi_clips().iter().any(|c| {
                            midi_frames(self, c).map_or(true, |(s, e)| s < start || e > end)
                        })
                    }
                    BounceTarget::AudioGroup { .. } => {
                        return Err(Invalid("audio grouping plan was not resolved"));
                    }
                };
                let console = self.session_cells().iter().any(|c| {
                    c.track() == track.id() && !matches!(c.content(), SessionCellContent::Stop)
                });
                if unrelated || (console && !matches!(target, BounceTarget::Track(_))) {
                    return Err(Invalid(
                        "in-place bounce would change unselected material through baked devices · use command-b",
                    ));
                }
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
    files: Vec<BounceFile>,
) -> Result<Created, Invalid> {
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
    let mut first = None;
    let mut last = None;
    for (item, file) in plan.into_iter().zip(files) {
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
        if in_place && matches!(target, BounceTarget::Range(_)) {
            let clip = crate::arrangement_print::adopt(project, file)?;
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
                    "bounce tail overlaps unselected material · use command-b",
                ));
            }
        }
        let destination = if in_place {
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
            item.track
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
            id
        };
        // No overwrite: a release tail intersecting unselected material refuses the
        // entire private candidate instead of erasing or truncating either source.
        let Created::Imported { clip, .. } = project.apply(Command::ImportPreparedAudioClip {
            track: destination,
            start: file.start,
            path: file.path,
            source_sample_rate: project.sample_rate(),
            source_frames: file.frames,
            playback_frames: file.frames,
            conversion_version: crate::IDENTITY_CONVERSION,
        })?
        else {
            return Err(Invalid("bounce output was not adopted"));
        };
        first.get_or_insert(clip);
        last = Some(clip);
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
        let Created::Clip(clip) = editor
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
            .plan_arrangement_bounce(&BounceTarget::Clips(vec![clip]), true)
            .unwrap()
            .remove(0);
        let after_command = Command::CommitArrangementBounce {
            target: BounceTarget::Clips(vec![clip]),
            in_place: true,
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
    fn tail_overlap_and_wrong_exact_range_refuse_without_history_mutation() {
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
                files: vec![output(150)],
            })
            .unwrap_err();
        assert!(error.to_string().contains("overlaps unselected"));
        assert_eq!(editor.project(), &original);
        assert!(
            editor
                .execute(Command::CommitArrangementBounce {
                    target: BounceTarget::Range(TimeRange::new(vec![track], 0, 100).unwrap()),
                    in_place: true,
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
                    files: vec![output(150)]
                })
                .is_ok()
        );
        assert_eq!(editor.project().tracks()[0], original.tracks()[0]);
        assert_eq!(editor.project().tracks()[1].clips()[0].duration(), 150);
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
}
