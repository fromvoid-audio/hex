//! Non-destructive, atomic same-track clip grouping. Source windows are
//! materialized through the canonical MIDI playback law, never by adding raw
//! authored onsets. Unselected occupancy in the outer span is a refusal: grouping
//! must not invoke the placement owner's destructive overlap replacement.
//!
//! Activation policy: the grouped container is always active, and each
//! source's deactivation moves into its material (inactive MIDI notes, inactive
//! audio events). Grouping therefore never makes a deactivated clip audible, and
//! the container stays editable as one active clip. The container takes its name
//! and flavor from the earliest selected clip.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiClip, MidiClipGain,
    MidiClipVolume, MidiClipVolumePoint, MidiSource, NoteId, Project,
};
use std::{collections::BTreeSet, sync::Arc};
pub(crate) mod render;

pub(crate) fn apply(project: &mut Project, ids: Vec<ClipId>) -> Result<Created, Invalid> {
    group(project, ids, None)
}

/// A time selection supplies the container bounds before source materialization,
/// so audio events, MIDI occurrences and volume points retain their song timing.
fn group(
    project: &mut Project,
    ids: Vec<ClipId>,
    range: Option<&crate::TimeRange>,
) -> Result<Created, Invalid> {
    let minimum = if range.is_some() { 1 } else { 2 };
    if !(minimum..=MAX_CLIPS).contains(&ids.len()) {
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
        return self::audio(project, track, &selected, range);
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
    // Use the largest scalar, keeping each scaled curve within its 0..=4 domain.
    let merged_gain = clips
        .iter()
        .map(|clip| clip.volume().clip_gain())
        .max_by_key(|gain| gain.millidb())
        .ok_or(Invalid("no midi clips selected"))?;
    let start = match range {
        Some(range) => project.tempo_map().frame_to_beat(range.start())?.raw(),
        None => first.start().raw(),
    };
    let end = clips.iter().try_fold(start, |end, c| {
        c.start()
            .raw()
            .checked_add(c.length().raw())
            .map(|n| end.max(n))
            .ok_or(Invalid("grouped midi end overflow"))
    })?;
    let end = match range {
        Some(range) => project.tempo_map().frame_to_beat(range.end())?.raw(),
        None => end,
    };
    let length = BeatTime::from_raw(
        end.checked_sub(start)
            .ok_or(Invalid("grouped midi bounds reversed"))?,
    );
    crate::midi::validate_clip(BeatTime::from_raw(start), length)?;
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
    let has_volume = clips
        .iter()
        .any(|clip| !clip.volume().points().is_empty() || clip.volume().clip_gain() != merged_gain);
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
                copy.active &= clip.active();
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
        let scale = f64::from(clip.volume().clip_gain().linear()) / f64::from(merged_gain.linear());
        let last = local.len() - 1;
        for (index, mut point) in local.into_iter().enumerate() {
            point.incoming = MidiClipGain::new(point.incoming.linear() * scale)?;
            point.value = MidiClipGain::new(point.value.linear() * scale)?;
            // The right side of the source's exclusive end is inaudible.
            if index == last {
                point.value = point.incoming;
            }
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
                // Hold the outgoing level across silence and jump at the next start.
                if index == 0 {
                    point.incoming = previous.value;
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
        content_view: None,
        active: true,
        playback_offset: BeatTime::ZERO,
        name: first.name().into(),
        flavor: first.flavor(),
        id: ClipId(0),
        start: BeatTime::from_raw(start),
        length,
        volume: MidiClipVolume::new(points, length)?.with_clip_gain(merged_gain),
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
    range: Option<&crate::TimeRange>,
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
    let start = range.map_or(first.start(), crate::TimeRange::start);
    let end = clips.iter().try_fold(start, |end, clip| {
        clip.start()
            .checked_add(clip.duration())
            .map(|n| end.max(n))
            .ok_or(Invalid("grouped audio end overflow"))
    })?;
    let end = range.map_or(end, crate::TimeRange::end);
    let duration = end
        .checked_sub(start)
        .ok_or(Invalid("grouped audio bounds reversed"))?;
    if duration > crate::MAX_AUDIO_DOMAIN_FRAMES {
        return Err(Invalid("grouped audio exceeds bounded preparation length"));
    }
    // Retained source events still have disjoint, replacement semantics.
    // Top-level crossfades must not be silently flattened into that program.
    let mut previous_end = start;
    for clip in &clips {
        let first = clip.start().max(start);
        let last = clip
            .start()
            .checked_add(clip.duration())
            .ok_or(Invalid("grouped audio end overflow"))?
            .min(end);
        if first < last {
            if first < previous_end {
                return Err(Invalid(
                    "grouping overlapping audio crossfades is unsupported; keep the clips separate",
                ));
            }
            previous_end = last;
        }
    }
    let mut sources = Vec::new();
    let mut events = Vec::new();
    let tempo = Arc::new(project.tempo_map().clone());
    for clip in &clips {
        let offset = clip.start() - start;
        let neutral_group = clip.has_retained_audio_roots()
            && clip.audio_edit().is_some_and(|edit| {
                edit.content_view.is_none()
                    && edit.placement.is_none()
                    && clip.gain() == ClipGain::UNITY
                    && edit.domain.fade.is_neutral()
            });
        if neutral_group {
            let edit = clip
                .audio_edit()
                .ok_or(Invalid("grouped audio root missing"))?;
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
                    active: event.active && clip.active(),
                    gain: event.gain,
                    fade: event.fade,
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
                active: clip.active(),
                gain: crate::ClipGain::UNITY,
                fade: None,
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
    merged.active = true;
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
    if root.content_view.is_some() {
        return Err(Invalid(
            "group source events before reversing their clip; directed range grouping is unsupported",
        ));
    }
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

/// Use the ordinary split owners so retained source windows, processing and MIDI
/// gates follow the same law as explicit cuts. Each populated row spans the full
/// selection, including empty margins. Publish only after every row groups.
pub(crate) fn time_range(
    project: &mut Project,
    range: &crate::TimeRange,
    create_if_empty: bool,
) -> Result<Created, Invalid> {
    range.validate(project)?;
    let left = project.tempo_map().frame_to_beat(range.start())?;
    let right = project.tempo_map().frame_to_beat(range.end())?;
    let mut candidate = project.clone();
    let mut result = None;
    for track in range.tracks() {
        for (frame, beat) in [(range.start(), left), (range.end(), right)] {
            let row = candidate
                .track(*track)
                .ok_or(Invalid("selected track no longer exists"))?;
            let mut cuts = Vec::new();
            for clip in row.clips() {
                let end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("clip end overflow"))?;
                if clip.start() < frame && frame < end {
                    cuts.push(crate::Command::SplitClip {
                        clip: clip.id(),
                        frame,
                    });
                }
            }
            for clip in row.midi_clips() {
                let end = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .ok_or(Invalid("midi clip end overflow"))?;
                if clip.start() < beat && beat.raw() < end {
                    cuts.push(crate::Command::SplitMidiClip {
                        clip: clip.id(),
                        beat,
                    });
                }
            }
            for cut in cuts {
                candidate.apply(cut)?;
            }
        }
        let row = candidate
            .track(*track)
            .ok_or(Invalid("selected track no longer exists"))?;
        let mut ids = Vec::new();
        for clip in row.clips() {
            if clip.start() >= range.start() && clip.start() < range.end() {
                ids.push(clip.id());
            }
        }
        for clip in row.midi_clips() {
            if clip.start() >= left && clip.start() < right {
                ids.push(clip.id());
            }
        }
        match ids.as_slice() {
            [] => (),
            // An exact single-clip selection already has the requested bounds.
            // Preserve its source representation, including directed/processed groups.
            [id] if row.clips().iter().any(|clip| {
                clip.id() == *id
                    && clip.start() == range.start()
                    && clip.duration() == range.end() - range.start()
            }) || row.midi_clips().iter().any(|clip| {
                clip.id() == *id
                    && clip.start() == left
                    && clip.length().raw() == right.raw() - left.raw()
            }) =>
            {
                result = Some(Created::Clip(*id));
            }
            _ => result = Some(group(&mut candidate, ids, Some(range))?),
        }
    }
    if result.is_none() && create_if_empty {
        let length = BeatTime::from_raw(right.raw() - left.raw());
        // Reuse the ordinary blank MIDI clip on hybrid lanes. Validate its
        // actual beat-to-frame projection before allowing placement to cut any
        // adjacent content, even when frame/beat conversion rounds an edge.
        let start = project.tempo_map().beat_to_frame(left)?;
        let end = project.tempo_map().beat_to_frame(right)?;
        for &track in range.tracks() {
            let row = project
                .track(track)
                .ok_or(Invalid("selected track no longer exists"))?;
            if row.is_group() {
                continue;
            }
            if row.clips().iter().any(|clip| {
                clip.start() < end && clip.start().saturating_add(clip.duration()) > start
            }) || row.midi_clips().iter().any(|clip| {
                clip.start() < right
                    && clip.start().raw().saturating_add(clip.length().raw()) > left.raw()
            }) {
                return Err(Invalid("selected time range overlaps a clip"));
            }
            result = Some(candidate.apply(crate::Command::PlaceMidiClip {
                track,
                start: left,
                length,
            })?);
        }
    }
    let result = result.ok_or(Invalid("selected time range contains no clips"))?;
    *project = candidate;
    Ok(result)
}
