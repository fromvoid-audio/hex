//! Bounded control-side compilation of MIDI edges and stateless note spans.
use crate::Invalid;
use hex_project::{BeatTime, ClipId, MidiClip, MidiNote, NoteId, Project, Track, TrackId};

pub const MAX_MIDI_EVENT_EDGES: usize = 131_072;
/// Maximum prepared MIDI note occurrences across the entire project.
pub const MAX_COMPILED_MIDI_OCCURRENCES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEventKind {
    NoteOff,
    NoteOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledMidiEvent {
    pub frame: u64,
    pub kind: MidiEventKind,
    pub track: TrackId,
    pub clip: ClipId,
    pub note: NoteId,
    /// Zero for the initial source pass, then one per loop repetition.
    pub iteration: u64,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

/// Resolve all note boundaries through the project's tempo map. This allocates
/// on the control side and does not schedule MIDI in the audio callback.
pub fn midi_events(project: &Project) -> Result<Vec<CompiledMidiEvent>, Invalid> {
    let occurrence_count = count_project_occurrences(project)?;
    let edge_count = occurrence_count
        .checked_mul(2)
        .ok_or(Invalid("midi event count overflow"))?;
    if edge_count > MAX_MIDI_EVENT_EDGES {
        return Err(Invalid("midi event edge limit reached"));
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(edge_count)
        .map_err(|_| Invalid("could not reserve midi event storage"))?;
    for (track_index, track) in project.tracks().iter().enumerate() {
        for clip in track.midi_clips() {
            append_clip(project, track_index, track.id(), clip, &mut ordered)?;
        }
    }
    ordered.sort_by_key(|(track_index, event)| {
        (
            event.frame,
            if event.kind == MidiEventKind::NoteOff {
                0_u8
            } else {
                1
            },
            *track_index,
            event.clip.value(),
            event.note.value(),
            event.iteration,
        )
    });
    let mut result = Vec::new();
    result
        .try_reserve_exact(edge_count)
        .map_err(|_| Invalid("could not reserve midi event storage"))?;
    result.extend(ordered.into_iter().map(|(_, event)| event));
    Ok(result)
}

/// Count every authored or expanded occurrence before any compiler allocates a
/// project-wide MIDI event or instrument span buffer.
pub(crate) fn count_project_occurrences(project: &Project) -> Result<usize, Invalid> {
    let mut total = 0_usize;
    for clip in project.tracks().iter().flat_map(Track::midi_clips) {
        let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
        for note in clip.notes() {
            let count = source
                .note_occurrences(note.start(), note.duration())
                .map_err(|error| Invalid(error.0))?
                .count();
            let count = usize::try_from(count)
                .map_err(|_| Invalid("compiled midi occurrence count overflow"))?;
            total = total
                .checked_add(count)
                .ok_or(Invalid("compiled midi occurrence count overflow"))?;
            if total > MAX_COMPILED_MIDI_OCCURRENCES {
                return Err(Invalid("compiled midi occurrence limit reached"));
            }
        }
    }
    Ok(total)
}

fn append_clip(
    project: &Project,
    track_index: usize,
    track: TrackId,
    clip: &MidiClip,
    events: &mut Vec<(usize, CompiledMidiEvent)>,
) -> Result<(), Invalid> {
    for_each_clip_occurrence(project, clip, |note, iteration, onset, off, _, _| {
        for (frame, kind) in [
            (onset, MidiEventKind::NoteOn),
            (off, MidiEventKind::NoteOff),
        ] {
            events.push((
                track_index,
                CompiledMidiEvent {
                    frame,
                    kind,
                    track,
                    clip: clip.id(),
                    note: note.id(),
                    iteration,
                    pitch: note.pitch(),
                    channel: note.channel(),
                    velocity: note.velocity(),
                },
            ));
        }
        Ok(())
    })
}

fn for_each_clip_occurrence(
    project: &Project,
    clip: &MidiClip,
    mut visit: impl FnMut(&MidiNote, u64, u64, u64, BeatTime, BeatTime) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
    for note in clip.notes() {
        let plan = source
            .note_occurrences(note.start(), note.duration())
            .map_err(|error| Invalid(error.0))?;
        for occurrence in plan.iter() {
            let onset_raw = clip
                .start()
                .raw()
                .checked_add(occurrence.timeline_start().raw())
                .ok_or(Invalid("midi note onset beat overflow"))?;
            let off_raw = onset_raw
                .checked_add(occurrence.gate_duration().raw())
                .ok_or(Invalid("midi note offset beat overflow"))?;
            let onset = project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(onset_raw))
                .map_err(|_| Invalid("midi note onset exceeds frame range"))?;
            let off = project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(off_raw))
                .map_err(|_| Invalid("midi note offset exceeds frame range"))?;
            if off <= onset {
                return Err(Invalid("midi note duration quantizes to zero frames"));
            }
            visit(
                note,
                occurrence.iteration(),
                onset,
                off,
                BeatTime::from_raw(onset_raw),
                BeatTime::from_raw(off_raw),
            )?;
        }
    }
    Ok(())
}

/// Compile-time limit on notes sounding at once on each instrument track, including release tails.
pub const MAX_SIMULTANEOUS_VOICES_PER_TRACK: usize = 64;
/// Maximum note onsets in any half-open 256-frame window on each instrument track.
pub const MAX_ONSETS_PER_WINDOW: usize = 128;
/// Renderer queries instrument intervals in chunks no larger than this frame count.
pub const RENDER_QUERY_FRAMES: u64 = 256;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompiledNoteSpan {
    pub(crate) start: u64,
    pub(crate) gate_end: u64,
    pub(crate) end: u64,
    pub(crate) voice: hex_devices::PreparedVoice,
    pub(crate) clip: ClipId,
    pub(crate) note: hex_project::NoteId,
    pub(crate) iteration: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SpanNode {
    pub(crate) span_index: usize,
    pub(crate) span_start: u64,
    pub(crate) max_end: u64,
    pub(crate) left: Option<usize>,
    pub(crate) right: Option<usize>,
}

pub(crate) fn compile_instrument_track(
    project: &Project,
    track: &Track,
    table: std::sync::Arc<hex_devices::SineTable>,
    mut previews: Option<&mut Vec<crate::MidiClipPreview>>,
) -> Result<Option<crate::InstrumentTrack>, Invalid> {
    if track.midi_clips().is_empty() {
        return Ok(None);
    }
    let sample_rate = project.sample_rate();
    let mut clip_counts = Vec::new();
    clip_counts
        .try_reserve_exact(track.midi_clips().len())
        .map_err(|_| Invalid("could not reserve instrument clip counts"))?;
    let mut span_count = 0_usize;
    for clip in track.midi_clips() {
        let mut clip_count = 0_usize;
        let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
        for note in clip.notes() {
            let count = source
                .note_occurrences(note.start(), note.duration())
                .map_err(|error| Invalid(error.0))?
                .count();
            let count = usize::try_from(count)
                .map_err(|_| Invalid("compiled midi occurrence count overflow"))?;
            clip_count = clip_count
                .checked_add(count)
                .ok_or(Invalid("compiled midi occurrence count overflow"))?;
        }
        span_count = span_count
            .checked_add(clip_count)
            .ok_or(Invalid("compiled midi occurrence count overflow"))?;
        clip_counts.push(clip_count);
    }
    let mut spans = Vec::new();
    spans
        .try_reserve_exact(span_count)
        .map_err(|_| Invalid("could not reserve instrument note storage"))?;
    let mut timeline_end = None;
    for (clip, occurrence_count) in track.midi_clips().iter().zip(clip_counts) {
        let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
        let clip_start_frame = project
            .tempo_map()
            .beat_to_frame(clip.start())
            .map_err(|_| Invalid("midi clip start exceeds frame range"))?;
        let clip_end_raw = clip
            .start()
            .raw()
            .checked_add(source.arrangement_duration().raw())
            .ok_or(Invalid("midi clip end beat overflow"))?;
        let clip_end = project
            .tempo_map()
            .beat_to_frame(hex_project::BeatTime::from_raw(clip_end_raw))
            .map_err(|_| Invalid("midi clip end exceeds frame range"))?;
        timeline_end = Some(timeline_end.map_or(clip_end, |current: u64| current.max(clip_end)));
        let mut preview = if previews.is_some() {
            let ordinals = crate::midi_preview::sample_ordinals(occurrence_count)?;
            let mut samples = Vec::new();
            samples
                .try_reserve_exact(ordinals.len())
                .map_err(|_| Invalid("could not reserve midi preview samples"))?;
            Some((ordinals, samples))
        } else {
            None
        };
        let mut emitted = 0_usize;
        for_each_clip_occurrence(project, clip, |note, iteration, start, gate_end, _, _| {
            if let Some((ordinals, samples)) = preview.as_mut() {
                if ordinals.get(samples.len()).copied() == Some(emitted) {
                    samples.push(crate::MidiPreviewSample {
                        frame_start: start,
                        frame_gate_end: gate_end,
                        pitch: note.pitch(),
                    });
                }
            }
            emitted = emitted
                .checked_add(1)
                .ok_or(Invalid("compiled midi occurrence count overflow"))?;
            let voice = table
                .prepare_voice(sample_rate, note.pitch(), note.velocity())
                .map_err(|_| Invalid("midi pitch cannot be rendered at project sample rate"))?;
            let end = gate_end
                .checked_add(voice.release_frames())
                .ok_or(Invalid("midi release end frame overflow"))?;
            spans.push(CompiledNoteSpan {
                start,
                gate_end,
                end,
                voice,
                clip: clip.id(),
                note: note.id(),
                iteration,
            });
            Ok(())
        })?;
        if emitted != occurrence_count {
            return Err(Invalid(
                "midi preview occurrence count changed during compile",
            ));
        }
        if let (Some(previews), Some((_, samples))) = (previews.as_deref_mut(), preview) {
            previews.push(crate::MidiClipPreview {
                clip_id: clip.id(),
                frame_start: clip_start_frame,
                frame_end: clip_end,
                authored_note_count: clip.notes().len(),
                occurrence_count,
                samples,
            });
        }
    }
    spans.sort_by_key(|span| {
        (
            span.start,
            span.clip.value(),
            span.note.value(),
            span.iteration,
        )
    });
    validate_density(&spans)?;
    let mut nodes = Vec::with_capacity(spans.len());
    let root = build_interval_tree(&spans, 0, spans.len(), &mut nodes);
    Ok(Some(crate::InstrumentTrack {
        spans,
        nodes,
        root,
        table,
        timeline_end,
    }))
}

fn validate_density(spans: &[CompiledNoteSpan]) -> Result<(), Invalid> {
    let onset_frames = spans.iter().map(|span| span.start).collect::<Vec<_>>();
    validate_onset_density(&onset_frames)?;

    let mut edges = Vec::with_capacity(spans.len() * 2);
    for span in spans {
        edges.push((span.start, true));
        edges.push((span.end, false));
    }
    edges.sort_unstable_by_key(|(frame, is_start)| (*frame, *is_start));
    let mut active = 0_usize;
    for (_, is_start) in edges {
        if is_start {
            active += 1;
            if active > MAX_SIMULTANEOUS_VOICES_PER_TRACK {
                return Err(Invalid(
                    "midi voice density exceeds 64 simultaneous voices per track",
                ));
            }
        } else {
            active -= 1;
        }
    }
    Ok(())
}

fn validate_onset_density(starts: &[u64]) -> Result<(), Invalid> {
    for onset_end in 0..starts.len() {
        let window_end = starts[onset_end]
            .checked_add(RENDER_QUERY_FRAMES)
            .ok_or(Invalid("midi onset window exceeds frame range"))?;
        let mut count = 0;
        for start in &starts[onset_end..] {
            if *start >= window_end {
                break;
            }
            count += 1;
            if count > MAX_ONSETS_PER_WINDOW {
                return Err(Invalid("midi onset density exceeds 128 per 256 frames"));
            }
        }
    }
    Ok(())
}

fn build_interval_tree(
    spans: &[CompiledNoteSpan],
    start: usize,
    end: usize,
    nodes: &mut Vec<SpanNode>,
) -> Option<usize> {
    if start == end {
        return None;
    }
    let middle = start + (end - start) / 2;
    let index = nodes.len();
    nodes.push(SpanNode {
        span_index: middle,
        span_start: spans[middle].start,
        max_end: spans[middle].end,
        left: None,
        right: None,
    });
    let left = build_interval_tree(spans, start, middle, nodes);
    let right = build_interval_tree(spans, middle + 1, end, nodes);
    let max_end = [
        Some(spans[middle].end),
        left.map(|child| nodes[child].max_end),
        right.map(|child| nodes[child].max_end),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(spans[middle].end);
    nodes[index].left = left;
    nodes[index].right = right;
    nodes[index].max_end = max_end;
    Some(index)
}

#[cfg(test)]
mod density_tests {
    use super::*;

    #[test]
    fn onset_window_is_half_open_and_accepts_exact_capacity() {
        assert_eq!(MAX_ONSETS_PER_WINDOW, 128);
        assert!(validate_onset_density(&[0; 128]).is_ok());
        assert_eq!(
            validate_onset_density(&[0; 129]),
            Err(Invalid("midi onset density exceeds 128 per 256 frames"))
        );
        let mut boundary = vec![0; 128];
        boundary.extend([RENDER_QUERY_FRAMES; 128]);
        assert!(validate_onset_density(&boundary).is_ok());
    }
}
