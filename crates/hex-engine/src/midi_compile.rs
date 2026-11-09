//! Bounded control-side compilation of MIDI edges and prepared note spans.
//!
//! Validate/count projected occurrences before allocating banks. Convert absolute on/off beats
//! independently through canonical timing, refuse zero-frame gates and sort equal-frame note-offs
//! before note-ons with stable authored identity ordering. Prepared event edges alone produce no
//! audio. Rendering borrows admitted fixed span/query storage; no per-sample beat conversion,
//! voice preparation or note bank allocation belongs on callback. Named occurrence/edge/runtime
//! limits here and in mutable_instrument are independent bounds, not physical device-deadline
//! evidence.
use crate::Invalid;
use hex_lattice::PreparedNativeInstrument;
use hex_project::{BeatTime, ClipId, MidiClip, MidiNote, NoteId, Project, Track, TrackId};
use std::sync::Arc;

pub const MAX_MIDI_EVENT_EDGES: usize = 131_072;
/// Maximum prepared MIDI note occurrences across the entire project.
pub const MAX_COMPILED_MIDI_OCCURRENCES: usize = 65_536;

/// Device-specific limits copied from the prepared mutable factory before
/// Arrangement span compilation. The copy keeps admission independent of
/// runtime construction and avoids preparing the factory twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MutableInstrumentLimits {
    pub(crate) max_events_per_query: usize,
    pub(crate) max_active_occurrences: usize,
    pub(crate) max_release_tail_frames: u64,
    /// Both immutable host declarations are required for exact per-note volume.
    pub(crate) supports_clip_volume: bool,
    /// The factory's `steals_releasing_voices` declaration: its voice slots
    /// bound held density only, while releases still bound the proof ledger.
    pub(crate) steals_releasing_voices: bool,
}

impl MutableInstrumentLimits {
    pub(crate) fn from_factory(
        factory: &dyn hex_lattice::PreparedMutableInstrumentRuntime,
    ) -> Result<Self, Invalid> {
        let max_events_per_query = factory.max_events_per_block();
        let max_active_occurrences = factory.max_active_occurrences();
        if max_events_per_query == 0
            || max_events_per_query > crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY
            || max_active_occurrences == 0
            || max_active_occurrences > MAX_SIMULTANEOUS_VOICES_PER_TRACK
        {
            return Err(Invalid(
                "mutable instrument capacity declaration is unsupported",
            ));
        }
        Ok(Self {
            max_events_per_query,
            max_active_occurrences,
            max_release_tail_frames: factory.max_release_tail_frames(),
            supports_clip_volume: factory.supports_occurrence_gain()
                && factory.supports_active_occurrence_ids(),
            steals_releasing_voices: factory.steals_releasing_voices(),
        })
    }
}

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
        for clip in track.midi_clips().iter().filter(|clip| clip.active()) {
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
    for clip in project
        .tracks()
        .iter()
        .flat_map(Track::midi_clips)
        .filter(|clip| clip.active())
    {
        let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
        for note in clip.notes().iter().filter(|note| note.active()) {
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
    for_each_clip_occurrence(
        project,
        clip,
        |note, iteration, onset, off, _, _, playable| {
            if !note.active() || !playable {
                return Ok(());
            }
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
        },
    )
}

fn for_each_clip_occurrence(
    project: &Project,
    clip: &MidiClip,
    mut visit: impl FnMut(&MidiNote, u64, u64, u64, BeatTime, BeatTime, bool) -> Result<(), Invalid>,
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
            // An inactive authored note remains available to the editor but
            // produces no gate. Its valid beat duration may quantize to zero
            // frames without making the playable project unpreparable.
            let clamped = occurrence.gate_duration() < note.duration();
            if off < onset || (off == onset && note.active() && !clamped) {
                return Err(Invalid("midi note duration quantizes to zero frames"));
            }
            // A clamp can leave a positive beat gate with no representable
            // frame. Keep it in preview accounting, but schedule no note edges.
            let playable = off > onset;
            visit(
                note,
                occurrence.iteration(),
                onset,
                off,
                BeatTime::from_raw(onset_raw),
                BeatTime::from_raw(off_raw),
                playable,
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
    pub(crate) onset_beat: BeatTime,
    pub(crate) gate_end_beat: BeatTime,
    pub(crate) sampler_index: u16,
    pub(crate) pitch: u8,
    pub(crate) channel: u8,
    pub(crate) velocity: u8,
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
    instrument: Option<&dyn PreparedNativeInstrument>,
    mutable_limits: Option<MutableInstrumentLimits>,
    shared_context: Option<(u16, &crate::shared_analytic::SharedContextOwner)>,
    shared_contexts: Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    previews: Option<&mut Vec<crate::MidiClipPreview>>,
) -> Result<Option<crate::InstrumentTrack>, Invalid> {
    compile_instrument_track_with_empty_bank(
        project,
        track,
        instrument,
        mutable_limits,
        shared_context,
        shared_contexts,
        previews,
        crate::midi_note_bank::PreparedMidiNoteBank::empty,
    )
}

#[cfg(test)]
thread_local! { pub(crate) static CURRENT_SAMPLER_COMPILER_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// The actual native constructor owns the bank declaration at allocation birth.
/// Reuse the same validated occurrence compiler; no alternate MIDI scheduler.
pub(crate) fn compile_current_sampler_track(
    project: &Project,
    track: &Track,
    prepared: &hex_devices::sampler::PreparedSamplerRuntime,
    contexts: Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    declaration: &mut Option<crate::owned_shared::SharedDeclaration>,
) -> Result<Option<crate::InstrumentTrack>, Invalid> {
    #[cfg(test)]
    CURRENT_SAMPLER_COMPILER_ENTRIES.with(|n| n.set(n.get() + 1));
    if declaration.is_some() {
        return Err(Invalid("native midi bank destination is occupied"));
    }
    let limits = MutableInstrumentLimits::from_factory(prepared)?;
    let instrument =
        hex_devices::sampler::catalog::PreparedSamplerInstrument::from_runtime_on_control(
            prepared.clone(),
        );
    compile_instrument_track_with_empty_bank(
        project,
        track,
        Some(&instrument),
        Some(limits),
        None,
        contexts,
        None,
        || {
            let bank = crate::owned_shared::OwnedShared::new_owned(
                crate::midi_note_bank::PreparedMidiNoteBank::empty_value(),
            );
            let (bank, paired) = bank.into_paired_parts_on_control();
            *declaration = Some(paired);
            bank
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_instrument_track_with_empty_bank(
    project: &Project,
    track: &Track,
    instrument: Option<&dyn PreparedNativeInstrument>,
    mutable_limits: Option<MutableInstrumentLimits>,
    shared_context: Option<(u16, &crate::shared_analytic::SharedContextOwner)>,
    shared_contexts: Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    mut previews: Option<&mut Vec<crate::MidiClipPreview>>,
    empty_bank: impl FnOnce() -> Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
) -> Result<Option<crate::InstrumentTrack>, Invalid> {
    // MIDI clip timing and editor previews are project data even when the
    // selected instrument is absent or bypassed; only note sampling is silent.
    if track.midi_clips().is_empty() {
        return Ok(None);
    }
    let volumes = if track
        .midi_clips()
        .iter()
        .any(|clip| !clip.volume().is_unity())
    {
        let max =
            crate::PreparedTempoMap::validate_canonical(project.tempo_map(), project.sample_rate())
                .map_err(|_| Invalid("invalid clip volume preparation timing"))?;
        let timing =
            crate::prepared_tempo::CanonicalMidiTiming::for_validated(project.tempo_map(), max);
        crate::midi_clip_volume::arrangement::PreparedArrangementVolumes::prepare(
            project, track, &timing,
        )?
    } else {
        None
    };
    if volumes.is_some() && mutable_limits.is_some_and(|limits| !limits.supports_clip_volume) {
        return Err(Invalid(
            "clip volume is unsupported by this mutable instrument mode",
        ));
    }
    let mut keys = crate::midi_note_bank::PreparedMidiNoteBank::empty_keys();
    let mut clip_counts = Vec::new();
    clip_counts
        .try_reserve_exact(track.midi_clips().len())
        .map_err(|_| Invalid("could not reserve instrument clip counts"))?;
    let mut span_count = 0_usize;
    for clip in track.midi_clips() {
        let mut clip_count = 0_usize;
        let mut playable_count = 0_usize;
        let source = clip.source_playback().map_err(|error| Invalid(error.0))?;
        for note in clip.notes() {
            let count = source
                .note_occurrences(note.start(), note.duration())
                .map_err(|error| Invalid(error.0))?
                .count();
            let count = usize::try_from(count)
                .map_err(|_| Invalid("compiled midi occurrence count overflow"))?;
            if clip.active() && note.active() && count != 0 && instrument.is_some() {
                crate::midi_note_bank::PreparedMidiNoteBank::add_key(
                    &mut keys,
                    note.pitch(),
                    note.velocity(),
                )?;
            }
            clip_count = clip_count
                .checked_add(count)
                .ok_or(Invalid("compiled midi occurrence count overflow"))?;
            if note.active() {
                playable_count = playable_count
                    .checked_add(count)
                    .ok_or(Invalid("compiled midi occurrence count overflow"))?;
            }
        }
        span_count = span_count
            .checked_add(if clip.active() { playable_count } else { 0 })
            .ok_or(Invalid("compiled midi occurrence count overflow"))?;
        clip_counts.push(clip_count);
    }
    let bank = if instrument.is_some() && mutable_limits.is_none() {
        instrument
            .map(|instrument| {
                crate::midi_note_bank::PreparedMidiNoteBank::prepare(
                    &mut keys,
                    instrument,
                    None,
                    shared_context,
                )
            })
            .transpose()?
    } else {
        None
    };
    let mutable = mutable_limits.is_some();
    let has_playable_source = instrument.is_some() || mutable;
    let mut spans = Vec::new();
    spans
        .try_reserve_exact(if has_playable_source { span_count } else { 0 })
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
        if !clip.active() {
            // Inactive sources retain bounded editor samples, but do not
            // enumerate their skipped loops or prepare playable occurrences.
            if let (Some(previews), Some((ordinals, mut samples))) =
                (previews.as_deref_mut(), preview)
            {
                let mut base = 0usize;
                let mut selected = 0usize;
                for note in clip.notes() {
                    let plan = source
                        .note_occurrences(note.start(), note.duration())
                        .map_err(|e| Invalid(e.0))?;
                    let count = usize::try_from(plan.count())
                        .map_err(|_| Invalid("midi preview occurrence count overflow"))?;
                    let limit = base
                        .checked_add(count)
                        .ok_or(Invalid("midi preview occurrence count overflow"))?;
                    while let Some(&ordinal) =
                        ordinals.get(selected).filter(|&&ordinal| ordinal < limit)
                    {
                        let hit = plan
                            .iter()
                            .nth(ordinal - base)
                            .ok_or(Invalid("midi preview occurrence is missing"))?;
                        let onset = clip
                            .start()
                            .raw()
                            .checked_add(hit.timeline_start().raw())
                            .ok_or(Invalid("midi preview onset overflow"))?;
                        let end = onset
                            .checked_add(hit.gate_duration().raw())
                            .ok_or(Invalid("midi preview gate overflow"))?;
                        samples.push(crate::MidiPreviewSample {
                            frame_start: project
                                .tempo_map()
                                .beat_to_frame(BeatTime::from_raw(onset))
                                .map_err(|e| Invalid(e.0))?,
                            frame_gate_end: project
                                .tempo_map()
                                .beat_to_frame(BeatTime::from_raw(end))
                                .map_err(|e| Invalid(e.0))?,
                            pitch: note.pitch(),
                        });
                        selected += 1;
                    }
                    base = limit;
                }
                previews.push(crate::MidiClipPreview {
                    clip_id: clip.id(),
                    frame_start: clip_start_frame,
                    frame_end: clip_end,
                    authored_note_count: clip.notes().len(),
                    occurrence_count,
                    samples,
                });
            }
            continue;
        }
        let mut emitted = 0_usize;
        for_each_clip_occurrence(
            project,
            clip,
            |note, iteration, start, gate_end, onset_beat, off_beat, playable| {
                if let Some((ordinals, samples)) = preview.as_mut()
                    && ordinals.get(samples.len()).copied() == Some(emitted)
                {
                    samples.push(crate::MidiPreviewSample {
                        frame_start: start,
                        frame_gate_end: gate_end,
                        pitch: note.pitch(),
                    });
                }
                emitted = emitted
                    .checked_add(1)
                    .ok_or(Invalid("compiled midi occurrence count overflow"))?;
                if clip.active() && note.active() && playable && has_playable_source {
                    let (sampler_index, end) = if mutable {
                        let tail = mutable_limits
                            .ok_or(Invalid("mutable instrument limits are missing"))?
                            .max_release_tail_frames;
                        (
                            u16::MAX,
                            gate_end
                                .checked_add(tail)
                                .ok_or(Invalid("midi release end frame overflow"))?,
                        )
                    } else {
                        let bank = bank
                            .as_ref()
                            .ok_or(Invalid("midi note bank is missing after preparation"))?;
                        let sampler_index = bank
                            .index(note.pitch(), note.velocity())
                            .ok_or(Invalid("midi note sampler key is missing from its bank"))?;
                        let voice = bank
                            .sampler(sampler_index)
                            .ok_or(Invalid("midi note sampler index is invalid"))?;
                        (
                            sampler_index,
                            gate_end
                                .checked_add(voice.release_frames())
                                .ok_or(Invalid("midi release end frame overflow"))?,
                        )
                    };
                    spans.push(CompiledNoteSpan {
                        start,
                        gate_end,
                        end,
                        onset_beat,
                        gate_end_beat: off_beat,
                        sampler_index,
                        pitch: note.pitch(),
                        channel: note.channel(),
                        velocity: note.velocity(),
                        clip: clip.id(),
                        note: note.id(),
                        iteration,
                    });
                }
                Ok(())
            },
        )?;
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
    validate_density(&spans, mutable_limits)?;
    let mut nodes = Vec::with_capacity(spans.len());
    let root = build_interval_tree(&spans, 0, spans.len(), &mut nodes);
    Ok(Some(crate::InstrumentTrack {
        spans,
        nodes,
        root,
        timeline_end,
        bank: bank.unwrap_or_else(empty_bank),
        shared_contexts,
        mutable,
        volumes,
    }))
}

trait DensitySpan {
    fn start(&self) -> u64;
    fn gate_end(&self) -> u64;
    fn end(&self) -> u64;
}

impl DensitySpan for CompiledNoteSpan {
    fn start(&self) -> u64 {
        self.start
    }
    fn gate_end(&self) -> u64 {
        self.gate_end
    }
    fn end(&self) -> u64 {
        self.end
    }
}

fn validate_density<S: DensitySpan>(
    spans: &[S],
    mutable_limits: Option<MutableInstrumentLimits>,
) -> Result<(), Invalid> {
    let onset_frames = spans.iter().map(DensitySpan::start).collect::<Vec<_>>();
    validate_onset_density(&onset_frames)?;

    let max_voices = mutable_limits.map_or(MAX_SIMULTANEOUS_VOICES_PER_TRACK, |limits| {
        limits
            .max_active_occurrences
            .min(MAX_SIMULTANEOUS_VOICES_PER_TRACK)
    });
    let message = if mutable_limits.is_some() {
        "midi voice density exceeds the instrument capacity per track"
    } else {
        "midi voice density exceeds 64 simultaneous voices per track"
    };
    if mutable_limits.is_some_and(|limits| limits.steals_releasing_voices) {
        // Stolen releases leave the instrument's slots but keep their proofs:
        // slots bound held density, the per-track ledger bounds releases.
        if maximum_overlap(spans, DensitySpan::gate_end) > max_voices {
            return Err(Invalid(message));
        }
        if maximum_overlap(spans, DensitySpan::end) > MAX_SIMULTANEOUS_VOICES_PER_TRACK {
            return Err(Invalid(
                "midi voice density exceeds 64 simultaneous voices per track",
            ));
        }
    } else if maximum_overlap(spans, DensitySpan::end) > max_voices {
        return Err(Invalid(message));
    }
    if let Some(limits) = mutable_limits {
        validate_event_density(
            spans,
            limits
                .max_events_per_query
                .min(crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY),
        )?;
    }
    Ok(())
}

/// Greatest number of half-open `[start, end)` spans sounding together; an
/// end at a frame precedes a start at the same frame.
fn maximum_overlap<S: DensitySpan>(spans: &[S], end: fn(&S) -> u64) -> usize {
    let mut edges = Vec::with_capacity(spans.len() * 2);
    for span in spans {
        edges.push((span.start(), true));
        edges.push((end(span), false));
    }
    edges.sort_unstable_by_key(|(frame, is_start)| (*frame, *is_start));
    let mut active = 0_usize;
    let mut maximum = 0_usize;
    for (_, is_start) in edges {
        if is_start {
            active += 1;
            maximum = maximum.max(active);
        } else {
            active = active.saturating_sub(1);
        }
    }
    maximum
}

fn validate_event_density<S: DensitySpan>(spans: &[S], max_events: usize) -> Result<(), Invalid> {
    let mut edges = Vec::new();
    let edge_count = spans
        .len()
        .checked_mul(2)
        .ok_or(Invalid("mutable midi event edge count overflow"))?;
    edges
        .try_reserve_exact(edge_count)
        .map_err(|_| Invalid("could not reserve mutable midi edge density"))?;
    for span in spans {
        edges.push((span.start(), true));
        edges.push((span.gate_end(), false));
    }
    edges.sort_unstable_by_key(|(frame, is_on)| (*frame, *is_on));
    for (start_index, (start, _)) in edges.iter().copied().enumerate() {
        let window_end = start
            .checked_add(RENDER_QUERY_FRAMES)
            .ok_or(Invalid("mutable midi event window exceeds frame range"))?;
        let end_index = edges.partition_point(|(frame, _)| *frame < window_end);
        if end_index - start_index > max_events {
            return Err(Invalid(
                "mutable midi event density exceeds the instrument capacity per query",
            ));
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

#[cfg(test)]
mod declared_capacity_tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct Span {
        start: u64,
        gate_end: u64,
        end: u64,
    }

    impl DensitySpan for Span {
        fn start(&self) -> u64 {
            self.start
        }
        fn gate_end(&self) -> u64 {
            self.gate_end
        }
        fn end(&self) -> u64 {
            self.end
        }
    }

    fn limits(events: usize, voices: usize) -> MutableInstrumentLimits {
        MutableInstrumentLimits {
            max_events_per_query: events,
            max_active_occurrences: voices,
            max_release_tail_frames: 0,
            supports_clip_volume: false,
            steals_releasing_voices: false,
        }
    }

    #[test]
    fn aggregate_gateoffs_and_short_note_edges_are_counted_per_half_open_query() {
        let mut spans = Vec::new();
        spans.extend((0..64).map(|_| Span {
            start: 0,
            gate_end: 256,
            end: 256,
        }));
        // At frame 256 the old gates end before these new gates begin. Their
        // 64 NoteOffs and 128 adjacent one-frame notes produce 320 edges in
        // one query without exceeding one simultaneously active voice.
        spans.extend((0..128).map(|index| {
            let start = 256 + index * 2;
            Span {
                start,
                gate_end: start + 1,
                end: start + 1,
            }
        }));
        assert_eq!(
            validate_density(&spans, Some(limits(256, 64))),
            Err(Invalid(
                "mutable midi event density exceeds the instrument capacity per query",
            ))
        );

        spans.truncate(64 + 96);
        assert_eq!(validate_density(&spans, Some(limits(256, 64))), Ok(()));
    }

    #[test]
    fn voice_capacity_uses_release_end_not_noteoff_edges() {
        let overlapping_release_spans = (0..65)
            .map(|_| Span {
                start: 0,
                gate_end: 256,
                end: 300,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_density(&overlapping_release_spans, Some(limits(256, 64))),
            Err(Invalid(
                "midi voice density exceeds the instrument capacity per track",
            ))
        );

        // A NoteOff exactly at the boundary is processed before a new NoteOn
        // there, so 64 adjacent releases and 64 fresh gates remain within the
        // fixed host voice bound. The 256-frame event windows are half-open.
        let adjacent_boundary_spans = (0..64)
            .map(|_| Span {
                start: 0,
                gate_end: 256,
                end: 256,
            })
            .chain((0..64).map(|_| Span {
                start: 256,
                gate_end: 512,
                end: 512,
            }))
            .collect::<Vec<_>>();
        assert_eq!(
            validate_density(&adjacent_boundary_spans, Some(limits(256, 64))),
            Ok(())
        );
    }

    #[test]
    fn stealing_capacity_bounds_held_gates_and_the_ledger_bounds_releases() {
        let stealing = |voices| MutableInstrumentLimits {
            steals_releasing_voices: true,
            ..limits(256, voices)
        };
        // One-voice retrigger: each release overlaps the next gate.
        let retrigger = (0..64)
            .map(|index| Span {
                start: index * 100,
                gate_end: index * 100 + 100,
                end: index * 100 + 150,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_density(&retrigger, Some(limits(256, 1))),
            Err(Invalid(
                "midi voice density exceeds the instrument capacity per track",
            ))
        );
        assert_eq!(validate_density(&retrigger, Some(stealing(1))), Ok(()));

        let mut held = retrigger.clone();
        held.push(Span {
            start: 50,
            gate_end: 60,
            end: 60,
        });
        assert_eq!(
            validate_density(&held, Some(stealing(1))),
            Err(Invalid(
                "midi voice density exceeds the instrument capacity per track",
            ))
        );

        // 65 short gates whose releases all overlap exceed the proof ledger.
        let releases = (0..65)
            .map(|index| Span {
                start: index,
                gate_end: index + 1,
                end: 1000,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_density(&releases, Some(stealing(1))),
            Err(Invalid(
                "midi voice density exceeds 64 simultaneous voices per track",
            ))
        );
        assert_eq!(validate_density(&releases[..64], Some(stealing(1))), Ok(()));
    }
}
