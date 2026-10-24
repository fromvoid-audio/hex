//! Non-destructive, atomic same-track clip grouping. Source windows are
//! materialized through the canonical MIDI playback law, never by adding raw
//! authored onsets. Unselected occupancy in the outer span is a refusal: grouping
//! must not invoke the placement owner's destructive overlap replacement.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiClip, MidiClipGain,
    MidiClipVolume, MidiClipVolumePoint, MidiSource, NoteId, Project,
};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) fn apply(project: &mut Project, ids: Vec<ClipId>) -> Result<Created, Invalid> {
    if !(2..=MAX_CLIPS).contains(&ids.len()) {
        return Err(Invalid("select at least two clips to group"));
    }
    let selected: BTreeSet<_> = ids.iter().copied().collect();
    if selected.len() != ids.len() {
        return Err(Invalid("clip grouping contains duplicate ids"));
    }
    let mut owner = None;
    let mut found = 0;
    let mut audio = false;
    let mut midi = false;
    for track in project.tracks() {
        let a = track
            .clips()
            .iter()
            .filter(|c| selected.contains(&c.id()))
            .count();
        let m = track
            .midi_clips()
            .iter()
            .filter(|c| selected.contains(&c.id()))
            .count();
        if a + m != 0 {
            if owner.is_some() {
                return Err(Invalid(
                    "grouping clips across tracks would change routing; select one track",
                ));
            }
            owner = Some(track.id());
            found += a + m;
            audio |= a != 0;
            midi |= m != 0;
        }
    }
    if found != ids.len() {
        return Err(Invalid("selected arrangement clip no longer exists"));
    }
    if audio && midi {
        return Err(Invalid(
            "audio and midi cannot share one clip; select one kind",
        ));
    }
    let track = owner.ok_or(Invalid("selected track no longer exists"))?;
    if audio {
        return self::audio(project, track, &selected);
    }
    let row = project
        .track(track)
        .ok_or(Invalid("selected track no longer exists"))?;
    let mut clips: Vec<_> = row
        .midi_clips()
        .iter()
        .filter(|c| selected.contains(&c.id()))
        .collect();
    clips.sort_by_key(|c| (c.start(), c.id()));
    let first = clips.first().ok_or(Invalid("no midi clips selected"))?;
    let start = first.start().raw();
    let end = clips.iter().try_fold(start, |end, c| {
        c.start()
            .raw()
            .checked_add(c.length().raw())
            .map(|n| end.max(n))
            .ok_or(Invalid("grouped midi end overflow"))
    })?;
    let length = BeatTime::from_raw(end - start);
    let active = first.active();
    if clips.iter().any(|c| c.active() != active) {
        return Err(Invalid("grouped clips must have the same activation state"));
    }
    // Preflight occurrence expansion before allocating or enumerating any loop.
    let count = clips.iter().try_fold(0usize, |count, c| {
        count
            .checked_add(crate::midi::overwrite_occurrence_count(c)?)
            .filter(|n| *n <= MAX_NOTES)
            .ok_or(Invalid("grouped midi note expansion exceeds project limit"))
    })?;
    let removed: usize = clips.iter().map(|c| c.notes().len()).sum();
    if crate::midi::note_count(project)
        .checked_sub(removed)
        .and_then(|n| n.checked_add(count))
        .is_none_or(|n| n > MAX_NOTES)
    {
        return Err(Invalid("midi note limit reached"));
    }
    let mut notes = Vec::new();
    notes
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve grouped midi notes"))?;
    let mut points = Vec::new();
    let has_volume = clips.iter().any(|clip| !clip.volume().points().is_empty());
    for clip in &clips {
        let offset = clip.start().raw() - start;
        let playback = clip.source_playback()?;
        for note in clip.notes() {
            for occurrence in playback
                .note_occurrences(note.start(), note.duration())?
                .iter()
            {
                let mut copy = note.clone();
                copy.id = NoteId(0);
                copy.start = BeatTime::from_raw(
                    offset
                        .checked_add(occurrence.timeline_start().raw())
                        .ok_or(Invalid("grouped note start overflow"))?,
                );
                copy.duration = occurrence.gate_duration();
                notes.push(copy);
            }
        }
        // Implicit unity must stay implicit: ordinary MIDI preparation does
        // not require automation support when no source authored a curve.
        if !has_volume {
            continue;
        }
        let curve = clip
            .volume()
            .materialized(playback, BeatTime::ZERO, clip.length())?;
        let mut local = curve.points().to_vec();
        if local.is_empty() {
            local.extend([
                MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::UNITY),
                MidiClipVolumePoint::linear(clip.length(), MidiClipGain::UNITY),
            ]);
        }
        for mut point in local {
            point.beat = BeatTime::from_raw(
                offset
                    .checked_add(point.beat.raw())
                    .ok_or(Invalid("grouped volume point overflow"))?,
            );
            if let Some(previous) = points.last_mut() {
                let previous: &mut MidiClipVolumePoint = previous;
                if previous.beat == point.beat {
                    previous.value = point.value;
                    continue;
                }
            }
            if points.len() == crate::MAX_MIDI_VOLUME_POINTS_PER_CLIP {
                return Err(Invalid("grouped clip volume point limit reached"));
            }
            points.push(point);
        }
    }
    // Existing sources have disjoint authored pitches, but loop seams may
    // retrigger. They are clipped by the playback owner before concatenation.
    notes.sort_by_key(|n| (n.start(), n.pitch(), n.channel()));
    let mut merged = MidiClip {
        active,
        playback_offset: BeatTime::ZERO,
        name: first.name().into(),
        flavor: first.flavor(),
        id: ClipId(0),
        start: BeatTime::from_raw(start),
        length,
        volume: MidiClipVolume::new(points, length)?,
        source: Arc::new(MidiSource::defaults(length)),
    };
    // Preserve unrelated material exactly; the canonical occupancy validator
    // refuses a new container spanning any nonselected clip, including audio.
    let row = project.track_mut(track)?;
    row.content
        .midi_clips
        .retain(|c| !selected.contains(&c.id()));
    merged.id = ClipId(project.allocate_id()?);
    for note in &mut notes {
        note.id = NoteId(project.allocate_id()?);
    }
    Arc::make_mut(&mut merged.source).notes = notes;
    let result = merged.id;
    project.track_mut(track)?.content.midi_clips.push(merged);
    project.validate_arrangement_occupancy()?;
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::Clip(result))
}

fn audio(
    project: &mut Project,
    track: crate::TrackId,
    selected: &BTreeSet<ClipId>,
) -> Result<Created, Invalid> {
    use crate::{
        AudioClipEdit, AudioEvent, AudioEvents, ClipFade, ClipGain, MAX_AUDIO_EVENTS,
        RetainedAudioRoot,
    };
    let row = project
        .track(track)
        .ok_or(Invalid("selected track no longer exists"))?;
    let mut clips: Vec<_> = row
        .clips()
        .iter()
        .filter(|c| selected.contains(&c.id()))
        .collect();
    clips.sort_by_key(|c| (c.start(), c.id()));
    let first = clips.first().ok_or(Invalid("no audio clips selected"))?;
    let start = first.start();
    let end = clips.iter().try_fold(start, |end, clip| {
        clip.start()
            .checked_add(clip.duration())
            .map(|n| end.max(n))
            .ok_or(Invalid("grouped audio end overflow"))
    })?;
    let duration = end - start;
    if duration > crate::MAX_AUDIO_DOMAIN_FRAMES {
        return Err(Invalid("grouped audio exceeds bounded preparation length"));
    }
    if clips.iter().any(|c| c.active() != first.active()) {
        return Err(Invalid("grouped clips must have the same activation state"));
    }
    let mut sources = Vec::new();
    let mut events = Vec::new();
    let tempo = Arc::new(project.tempo_map().clone());
    for clip in &clips {
        let offset = clip.start() - start;
        if clip.has_retained_audio_roots() {
            let edit = clip
                .audio_edit()
                .ok_or(Invalid("grouped audio root missing"))?;
            if edit.placement.is_some()
                || clip.gain() != ClipGain::UNITY
                || !edit.domain.fade.is_neutral()
            {
                return Err(Invalid(
                    "grouped audio with outer processing or repeats cannot be nested; group unprocessed source clips",
                ));
            }
            let sequence = edit
                .domain
                .events
                .as_ref()
                .ok_or(Invalid("grouped audio events missing"))?;
            let mut remap = std::collections::BTreeMap::new();
            for root in &sequence.sources {
                if sources.len() == MAX_AUDIO_EVENTS {
                    return Err(Invalid("grouped audio source limit reached"));
                }
                let id = sources.len() as u64 + 1;
                let mut retained = root.clone();
                retained.id = id;
                sources
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve grouped audio roots"))?;
                sources.push(retained);
                remap.insert(root.id, id);
            }
            for event in &sequence.events {
                let a = event.start.max(edit.view_start);
                let b = event.end.min(edit.view_end);
                if a >= b {
                    continue;
                }
                let source = *remap
                    .get(&event.source)
                    .ok_or(Invalid("grouped audio contains an unsupported self source"))?;
                if events.len() == MAX_AUDIO_EVENTS {
                    return Err(Invalid("grouped audio event limit reached"));
                }
                events
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve grouped audio events"))?;
                events.push(AudioEvent {
                    source,
                    id: events.len() as u64 + 1,
                    start: offset + a - edit.view_start,
                    end: offset + b - edit.view_start,
                    source_start: event
                        .source_start
                        .checked_add(a - event.start)
                        .ok_or(Invalid("grouped audio source overflow"))?,
                });
            }
        } else {
            if sources.len() == MAX_AUDIO_EVENTS {
                return Err(Invalid("grouped audio source limit reached"));
            }
            let id = sources.len() as u64 + 1;
            sources
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve grouped audio roots"))?;
            sources.push(RetainedAudioRoot {
                id,
                clip: (*clip).clone(),
                tempo: Arc::clone(&tempo),
            });
            if events.len() == MAX_AUDIO_EVENTS {
                return Err(Invalid("grouped audio event limit reached"));
            }
            events
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve grouped audio events"))?;
            events.push(AudioEvent {
                source: id,
                id: events.len() as u64 + 1,
                start: offset,
                end: offset + clip.duration(),
                source_start: 0,
            });
        }
        if events.len() > MAX_AUDIO_EVENTS {
            return Err(Invalid("grouped audio event limit reached"));
        }
    }
    let mut merged = (*first).clone();
    merged.start = start;
    merged.duration = duration;
    merged.source_offset = 0;
    merged.source_length = project
        .asset(merged.asset())
        .ok_or(Invalid("grouped audio media missing"))?
        .playback_frames();
    merged.source_phase = 0;
    merged.looping = false;
    merged.gain = ClipGain::UNITY;
    merged.fade = ClipFade::neutral(duration)?;
    let mut edit = AudioClipEdit::capture(&merged);
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(AudioEvents {
        next_id: events.len() as u64 + 1,
        events,
        sources,
    }));
    merged.edit = Some(Arc::new(edit));
    merged.id = ClipId(project.allocate_id()?);
    let result = merged.id;
    let row = project.track_mut(track)?;
    row.content
        .audio_clips
        .retain(|c| !selected.contains(&c.id()));
    row.content.audio_clips.push(merged);
    project.validate_arrangement_occupancy()?;
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::Clip(result))
}

pub(crate) fn event_range(
    project: &mut Project,
    clip: ClipId,
    start: u64,
    end: u64,
) -> Result<Created, Invalid> {
    let original = project
        .clip(clip)
        .ok_or(Invalid("audio clip no longer exists"))?
        .clone();
    let root = original.audio_event_root();
    if root.placement.is_some() || start >= end || start < root.view_start || end > root.view_end {
        return Err(Invalid(
            "select one positive audio span inside a single placed occurrence",
        ));
    }
    if !original
        .audio_events()
        .events
        .iter()
        .any(|event| event.start < end && start < event.end)
    {
        return Err(Invalid("selected range contains no audio events"));
    }
    if start == root.view_start && end == root.view_end {
        return Err(Invalid("selected audio is already one clip"));
    }
    let visible = root.root_frame_at(root.view_start, project.tempo_map())?;
    let first = original
        .start()
        .checked_add(
            root.root_frame_at(start, project.tempo_map())?
                .checked_sub(visible)
                .ok_or(Invalid("audio selection precedes its view"))?,
        )
        .ok_or(Invalid("audio selection start overflow"))?;
    let last = original
        .start()
        .checked_add(
            root.root_frame_at(end, project.tempo_map())?
                .checked_sub(visible)
                .ok_or(Invalid("audio selection precedes its view"))?,
        )
        .ok_or(Invalid("audio selection end overflow"))?;
    let outer_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("audio clip end overflow"))?;
    if first >= last || last > outer_end {
        return Err(Invalid("audio selection has no placed sample span"));
    }
    let track = project
        .tracks()
        .iter()
        .find(|t| t.clips().iter().any(|c| c.id() == clip))
        .map(|t| t.id())
        .ok_or(Invalid("audio clip track missing"))?;
    let mut fragments = Vec::new();
    for (a, b) in [(original.start(), first), (first, last), (last, outer_end)] {
        if a == b {
            continue;
        }
        let mut fragment =
            crate::preview_clip_fragment_with_tempo(&original, a, b, a, project.tempo_map())?;
        if a != original.start() || a == first {
            fragment.id = ClipId(project.allocate_id()?);
        }
        if fragment.edit.is_none() {
            fragment.edit = Some(Arc::new(crate::AudioClipEdit::capture(&fragment)));
        }
        fragments.push(fragment);
    }
    let result = fragments
        .iter()
        .find(|c| c.start() == first)
        .map(|c| c.id())
        .ok_or(Invalid("grouped audio selection missing"))?;
    let row = project.track_mut(track)?;
    row.content.audio_clips.retain(|c| c.id() != clip);
    row.content.audio_clips.extend(fragments);
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::Clip(result))
}
