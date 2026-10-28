//! Non-destructive, atomic same-track clip grouping. Source windows are
//! materialized through the canonical MIDI playback law, never by adding raw
//! authored onsets. Unselected occupancy in the outer span is a refusal: grouping
//! must not invoke the placement owner's destructive overlap replacement.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MidiClip, MidiClipGain,
    MidiClipVolume, MidiClipVolumePoint, MidiSource, NoteId, Project,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
pub(crate) mod render;

// A flattened curve belongs to the new clip, not to each outgoing voice. Only
// a shared constant is known to preserve every source's overlapping tail.
fn constant_clip_volume(clip: &MidiClip) -> Option<MidiClipGain> {
    let points = clip.volume().points();
    let gain = points
        .first()
        .map_or(MidiClipGain::UNITY, |point| point.value);
    points
        .iter()
        .all(|point| point.incoming == gain && point.value == gain)
        .then_some(gain)
}

// A source can share one immutable law across many notes. Project it once per
// clip view, retaining its original owner so the identity cannot be reused.
struct VoiceProjectionCache {
    playback: crate::MidiSourcePlayback,
    offset: BeatTime,
    length: BeatTime,
    clip_owner: Option<crate::MidiVoiceGainLayer>,
    empty_projection: Option<crate::MidiVoiceGain>,
    entries: HashMap<crate::MidiVoiceGainIdentity, crate::MidiVoiceGain>,
}
impl VoiceProjectionCache {
    fn new(
        playback: crate::MidiSourcePlayback,
        offset: BeatTime,
        length: BeatTime,
        clip_volume: Option<&MidiClipVolume>,
    ) -> Result<Self, Invalid> {
        let clip_owner = if let Some(volume) = clip_volume.filter(|volume| !volume.is_unity()) {
            let duration = playback.arrangement_duration();
            let end = offset
                .raw()
                .checked_add(duration.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("grouped voice gain end overflow"))?;
            let volume = volume.materialized(playback, BeatTime::ZERO, duration)?;
            Some(crate::MidiVoiceGainLayer::new(offset, end, length, volume)?)
        } else {
            None
        };
        Ok(Self {
            playback,
            offset,
            length,
            clip_owner,
            empty_projection: None,
            entries: HashMap::new(),
        })
    }
    fn project(&mut self, gain: &crate::MidiVoiceGain) -> Result<crate::MidiVoiceGain, Invalid> {
        let Some(identity) = gain.identity() else {
            let Some(layer) = &self.clip_owner else {
                return Ok(gain.clone());
            };
            if let Some(projected) = &self.empty_projection {
                return Ok(projected.clone());
            }
            let projected = gain.with_layer(layer.clone())?;
            self.empty_projection = Some(projected.clone());
            return Ok(projected);
        };
        if let Some(projected) = self.entries.get(&identity) {
            return Ok(projected.clone());
        }
        let mut projected = gain.materialized(self.playback, self.offset, self.length)?;
        if let Some(layer) = &self.clip_owner {
            projected = projected.with_layer(layer.clone())?;
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| Invalid("could not reserve grouped voice gain identities"))?;
        self.entries.insert(identity, projected.clone());
        Ok(projected)
    }
}

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
    let merged_gain = first.volume().clip_gain();
    // One consolidated MIDI clip cannot give overlapping release tails
    // different clip scalars without changing their audio.
    if clips
        .iter()
        .any(|clip| clip.volume().clip_gain() != merged_gain)
    {
        return Err(Invalid(
            "cannot group midi clips with different clip gains; release tails would change",
        ));
    }
    if clips.len() > 1
        && constant_clip_volume(first).is_none_or(|gain| {
            clips
                .iter()
                .any(|clip| constant_clip_volume(clip) != Some(gain))
        })
    {
        return Err(Invalid(
            "cannot group midi clip volume curves without changing release tails",
        ));
    }
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
    let has_volume = clips.iter().any(|clip| !clip.volume().points().is_empty());
    for clip in &clips {
        let offset = clip.start().raw() - start;
        let playback = clip.source_playback()?;
        // The current admission guard only permits the shared-volume path.
        // Per-voice capture is enabled after all playback consumers support it.
        let mut projected_gains =
            VoiceProjectionCache::new(playback, BeatTime::from_raw(offset), length, None)?;
        for note in clip.notes() {
            let voice_gain = projected_gains.project(&note.voice_gain)?;
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
                copy.voice_gain = voice_gain.clone();
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
    Arc::make_mut(&mut merged.source).replace_notes(notes);
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
    let result = result.ok_or(Invalid("selected time range contains no clips"))?;
    *project = candidate;
    Ok(result)
}

#[cfg(test)]
mod voice_projection_tests {
    use super::*;

    #[test]
    fn captured_clip_curve_follows_repeated_and_reversed_source_windows() {
        let q = |n: u64| BeatTime::from_raw(n << 32);
        let volume = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(q(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(q(8), MidiClipGain::new(2.0).unwrap()),
            ],
            q(8),
        )
        .unwrap();
        for reversed in [false, true] {
            let playback = crate::MidiSourcePlayback::new(q(8), q(0), Some((q(2), q(6))), q(12))
                .unwrap()
                .with_content_window(crate::ContentWindow::new(0, q(12).raw(), reversed).unwrap())
                .unwrap();
            let mut cache =
                VoiceProjectionCache::new(playback, q(2), q(14), Some(&volume)).unwrap();
            let owner = cache.project(&crate::MidiVoiceGain::default()).unwrap();
            let layer = &owner.layers()[0];
            for quarter in 0..12 {
                let elapsed = BeatTime::from_raw(q(quarter).raw() + (1_u64 << 31));
                let phase = playback.source_phase_at_elapsed(elapsed).unwrap();
                let destination = BeatTime::from_raw(q(2).raw() + elapsed.raw());
                let expected = volume.gain_at(phase).linear();
                let actual = layer.curve_gain_at(destination).linear();
                assert!(
                    (expected - actual).abs() < 1e-9,
                    "reverse={reversed}, quarter={quarter}: {actual} != {expected}"
                );
            }
            if !reversed {
                // The seam jumps from source beat six back to beat two.
                assert_eq!(layer.volume().gain_before(q(6)), volume.gain_before(q(6)));
                assert_eq!(layer.curve_gain_at(q(8)), volume.gain_at(q(2)));
            }
        }
    }

    #[test]
    fn capture_keeps_independent_clip_curves_and_release_endpoints_on_each_note() {
        let b = BeatTime::from_raw;
        let make_volume = |start, end, scalar| {
            MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(b(0), MidiClipGain::new(start).unwrap()),
                    MidiClipVolumePoint {
                        beat: b(4),
                        incoming: MidiClipGain::new(end).unwrap(),
                        value: MidiClipGain::new(3.5).unwrap(),
                    },
                ],
                b(4),
            )
            .unwrap()
            .with_clip_gain(crate::ClipGain::new(scalar).unwrap())
        };
        let first_volume = make_volume(0.5, 1.0, -6_000);
        let second_volume = make_volume(2.0, 3.0, 6_000);
        let playback = crate::MidiSourcePlayback::one_shot(b(4)).unwrap();
        let mut first =
            VoiceProjectionCache::new(playback, b(0), b(8), Some(&first_volume)).unwrap();
        let mut second =
            VoiceProjectionCache::new(playback, b(4), b(8), Some(&second_volume)).unwrap();
        let unity = crate::MidiVoiceGain::default();
        let old = first.project(&unity).unwrap();
        let old_again = first.project(&unity).unwrap();
        let next = second.project(&unity).unwrap();
        assert_eq!(old.owner_identity(), old_again.owner_identity());
        assert_ne!(old.owner_identity(), next.owner_identity());
        assert_eq!(
            old.layers()[0].volume().clip_gain(),
            first_volume.clip_gain()
        );
        assert_eq!(
            next.layers()[0].volume().clip_gain(),
            second_volume.clip_gain()
        );
        assert_eq!(old.layers()[0].curve_gain_at(b(2)).linear(), 0.75);
        assert_eq!(old.layers()[0].curve_gain_at(b(4)).linear(), 1.0);
        assert_eq!(old.layers()[0].curve_gain_at(b(6)).linear(), 1.0);
        assert_eq!(next.layers()[0].curve_gain_at(b(6)).linear(), 2.5);
        assert_eq!(next.layers()[0].curve_gain_at(b(8)).linear(), 3.0);
        let nested = first.project(&old).unwrap();
        assert_eq!(nested.layers().len(), 2);
        assert_eq!(
            nested.layers()[1].volume().clip_gain(),
            first_volume.clip_gain()
        );
        let full = crate::MidiVoiceGain::new(vec![
            old.layers()[0].clone();
            crate::MAX_MIDI_VOICE_GAIN_LAYERS
        ])
        .unwrap();
        assert!(first.project(&full).is_err());
    }

    #[test]
    fn shared_source_law_keeps_one_projected_owner_and_its_phase() {
        let b = BeatTime::from_raw;
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(b(8), MidiClipGain::new(1.0).unwrap()),
            ],
            b(8),
        )
        .unwrap();
        let layer = crate::MidiVoiceGainLayer::new(b(0), b(8), b(8), curve).unwrap();
        let gain = crate::MidiVoiceGain::new(vec![layer.clone()]).unwrap();
        let mut cache = VoiceProjectionCache::new(
            crate::MidiSourcePlayback::one_shot(b(8)).unwrap(),
            b(2),
            b(10),
            None,
        )
        .unwrap();
        assert!(
            cache
                .project(&crate::MidiVoiceGain::default())
                .unwrap()
                .layers()
                .is_empty()
        );
        assert!(cache.entries.is_empty());
        let first = cache.project(&gain).unwrap();
        let second = cache.project(&gain.clone()).unwrap();
        assert_eq!(first.owner_identity(), second.owner_identity());
        assert_eq!(cache.entries.len(), 1);
        for at in 0..=8 {
            assert_eq!(
                first.layers()[0].curve_gain_at(b(at + 2)),
                layer.curve_gain_at(b(at))
            );
        }
    }
}
