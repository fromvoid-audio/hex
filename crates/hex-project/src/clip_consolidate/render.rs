//! Explicit bounded render intent for groups whose outer processing cannot be
//! distributed through leaf events. Canonical planning/adoption performs no I/O;
//! Session owns the existing render/media transaction. Saved clip/asset formats
//! are unchanged and retained roots never become recursive.
use crate::{
    BounceFile, BounceTarget, BounceTrack, ClipId, Command, Created, Invalid, Project, TimeRange,
};
use std::collections::BTreeSet;

impl Project {
    /// Resolve the legacy explicit rendered-group intent. Interactive grouping
    /// uses the canonical non-destructive edit path so its sources remain editable.
    /// This pure query never changes IDs.
    pub fn audio_group_render_target(
        &self,
        command: &Command,
    ) -> Result<Option<BounceTarget>, Invalid> {
        let (clips, range) = match command {
            Command::GroupClips(ids) => (ids.clone(), None),
            Command::GroupTimeRange(range) => {
                range.validate(self)?;
                let ids = self
                    .tracks()
                    .iter()
                    .filter(|t| range.tracks().contains(&t.id()))
                    .flat_map(|t| t.clips())
                    .filter(|c| {
                        c.start() < range.end()
                            && c.start()
                                .checked_add(c.duration())
                                .is_some_and(|end| end > range.start())
                    })
                    .map(|c| c.id())
                    .collect();
                (ids, Some(range.clone()))
            }
            _ => return Ok(None),
        };
        let requires_render = clips.iter().filter_map(|id| self.clip(*id)).any(|c| {
            c.has_retained_audio_roots()
                && c.audio_edit().is_some_and(|edit| {
                    edit.content_view.is_some()
                        || edit.placement.is_some()
                        || c.gain() != crate::ClipGain::UNITY
                        || !edit.domain.fade.is_neutral()
                })
        });
        if !requires_render {
            return Ok(None);
        }
        // An exact one-clip time selection already is one clip; preserve its
        // editable root rather than rendering an otherwise no-op grouping.
        if let (Some(range), [id]) = (&range, clips.as_slice())
            && let Some(c) = self.clip(*id)
            && range.tracks().len() == 1
            && c.start() == range.start()
            && c.duration() == range.end() - range.start()
        {
            return Ok(None);
        }
        plan(self, &clips, range.as_ref())?;
        Ok(Some(BounceTarget::AudioGroup { clips, range }))
    }
}

pub(crate) fn plan(
    project: &Project,
    clips: &[ClipId],
    range: Option<&TimeRange>,
) -> Result<BounceTrack, Invalid> {
    let minimum = if range.is_some() { 1 } else { 2 };
    if clips.len() < minimum || clips.len() > crate::MAX_CLIPS {
        return Err(Invalid("select at least two audio clips to group"));
    }
    let ids: BTreeSet<_> = clips.iter().copied().collect();
    if ids.len() != clips.len() {
        return Err(Invalid("clip grouping contains duplicate ids"));
    }
    let first = project
        .clip(clips[0])
        .ok_or(Invalid("selected audio clip no longer exists"))?;
    // Rendering a processed group cannot retain muted constituent sources.
    // Refuse rather than replace editable inactive material with silence.
    for id in clips {
        let clip = project
            .clip(*id)
            .ok_or(Invalid("selected audio clip no longer exists"))?;
        let events = clip.audio_events();
        if events.events.iter().any(|event| !event.active)
            || events.sources.iter().any(|source| {
                source
                    .clip
                    .audio_events()
                    .events
                    .iter()
                    .any(|event| !event.active)
            })
        {
            return Err(Invalid(
                "processed audio grouping cannot retain inactive events; keep the clips separate",
            ));
        }
    }
    let owner = project
        .tracks()
        .iter()
        .find(|t| t.clips().iter().any(|c| c.id() == first.id()))
        .ok_or(Invalid("selected audio track no longer exists"))?;
    let mut start = u64::MAX;
    let mut end = 0;
    for id in clips {
        let c = owner
            .clips()
            .iter()
            .find(|c| c.id() == *id)
            .ok_or(Invalid("group audio clips from one track"))?;
        if c.active() != first.active() {
            return Err(Invalid("grouped clips must have the same activation state"));
        }
        start = start.min(c.start());
        end = end.max(
            c.start()
                .checked_add(c.duration())
                .ok_or(Invalid("grouped audio end overflow"))?,
        );
    }
    if let Some(range) = range {
        range.validate(project)?;
        if range.tracks() != [owner.id()] {
            return Err(Invalid(
                "rendered audio grouping supports one selected track",
            ));
        }
        start = range.start();
        end = range.end();
        if clips.iter().any(|id| {
            project.clip(*id).is_none_or(|c| {
                c.start() >= end
                    || c.start()
                        .checked_add(c.duration())
                        .is_none_or(|last| last <= start)
            })
        }) {
            return Err(Invalid("grouping selection no longer intersects its range"));
        }
    }
    if end
        .checked_sub(start)
        .is_none_or(|n| n == 0 || n > crate::MAX_ARRANGEMENT_BOUNCE_FRAMES)
    {
        return Err(Invalid(
            "audio grouping exceeds its finite render frame budget",
        ));
    }
    if owner.clips().iter().any(|c| {
        !ids.contains(&c.id())
            && c.start() < end
            && c.start()
                .checked_add(c.duration())
                .is_none_or(|last| last > start)
    }) {
        return Err(Invalid("unselected clips occupy the audio grouping span"));
    }
    for c in owner.midi_clips() {
        let a = project.tempo_map().beat_to_frame(c.start())?;
        let b = project
            .tempo_map()
            .beat_to_frame(crate::BeatTime::from_raw(
                c.start()
                    .raw()
                    .checked_add(c.length().raw())
                    .ok_or(Invalid("midi clip end overflow"))?,
            ))?;
        if a < end && b > start {
            return Err(Invalid("audio and midi cannot share one grouped clip"));
        }
    }
    Ok(BounceTrack {
        source: crate::BounceSource::Arrangement,
        track: owner.id(),
        start,
        end,
        clips: clips.to_vec(),
    })
}

pub(crate) fn commit(
    project: &mut Project,
    clips: &[ClipId],
    range: Option<&TimeRange>,
    files: Vec<BounceFile>,
) -> Result<Created, Invalid> {
    let item = plan(project, clips, range)?;
    let [file] = files.as_slice() else {
        return Err(Invalid("audio grouping requires one rendered output"));
    };
    if file.source != crate::BounceSource::Arrangement
        || file.track != item.track
        || file.start != item.start
        || file.frames != item.end - item.start
    {
        return Err(Invalid(
            "grouped render differs from its captured audio span",
        ));
    }
    let original = item
        .clips
        .iter()
        .filter_map(|id| project.clip(*id))
        .min_by_key(|c| (c.start(), c.id()))
        .ok_or(Invalid("audio grouping source missing"))?
        .clone();
    if let Some(range) = range {
        project.apply(Command::DeleteTimeRange {
            range: range.clone(),
        })?;
    } else {
        for id in clips {
            project.apply(Command::RemoveClip(*id))?;
        }
    }
    let Created::Imported { clip, .. } = project.apply(Command::ImportPreparedAudioClip {
        track: item.track,
        start: item.start,
        path: file.path.clone(),
        source_sample_rate: project.sample_rate(),
        source_frames: file.frames,
        playback_frames: file.frames,
        conversion_version: crate::IDENTITY_CONVERSION,
    })?
    else {
        return Err(Invalid("grouped render was not adopted"));
    };
    let result = project.clip_mut(clip)?;
    result.active = original.active;
    result.name = original.name;
    result.flavor = original.flavor;
    Ok(Created::Clip(clip))
}
