//! Immutable beat-domain MIDI source templates for ongoing playback queries.
//!
//! Template construction and density admission are control-side operations.
//! Queries borrow a prepared tempo map and write into fixed caller-owned
//! storage. The template ignores finite Arrangement duration by design.
use crate::{MidiTiming, midi_compile::MidiEventKind};
mod density;
mod spans;
use density::{DensityEnvelope, DensityEvent, admit_density};
use hex_project::{
    BeatTime, ClipId, MidiClip, MidiNote, MidiSource, MidiSourcePlayback, NoteId, TrackId,
};
pub(crate) use spans::MidiOccurrenceSuppression;

/// Maximum frame width accepted by one ongoing MIDI edge query.
pub const MIDI_TEMPLATE_QUERY_FRAMES: u64 = 256;
/// Fixed maximum edge count written into one caller-owned query buffer.
pub const MIDI_TEMPLATE_MAX_EDGES: usize = 256;
/// Fixed maximum onset count admitted in one query window.
pub const MIDI_TEMPLATE_MAX_ONSETS: usize = 128;
const MIDI_TEMPLATE_MAX_PERIOD_WINDOWS: u128 = 260;
const FRAME_FACTOR_NUMERATOR: u128 = 60_000_000;
const FRAME_Q32: u128 = 1_u128 << 32;
const MAX_PREFIX_TRUNCATION_Q32: u128 = 1_024;

/// A static, nonallocating template preparation or query failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiTemplateError {
    InvalidSource(&'static str),
    Allocation,
    ArithmeticOverflow,
    TempoRateMismatch,
    TempoMapTooFast,
    InvalidFrameRange,
    QueryTooWide,
    BeatDomainExhausted,
    GateTooShortForAdmission,
    LoopPeriodTooShortForAdmission,
    TooManyEdges,
    TooManyOnsets,
    PeriodWorkBoundExceeded,
    TooManySourceNotes,
    LoopPeriodTooShortForSpanAdmission,
    SpanAdmissionWorkBoundExceeded,
    SpanPeriodWorkBoundExceeded,
    SpanVisitBoundExceeded,
    TooManySpans,
    TooManyVoices,
    TempoConversion,
}

impl std::fmt::Display for MidiTemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidSource(message) => message,
            Self::Allocation => "could not reserve ongoing midi template storage",
            Self::ArithmeticOverflow => "ongoing midi template arithmetic overflow",
            Self::TempoRateMismatch => "ongoing midi template tempo sample rate mismatch",
            Self::TempoMapTooFast => "tempo map exceeds ongoing midi template admission tempo",
            Self::InvalidFrameRange => "ongoing midi query frame range is invalid",
            Self::QueryTooWide => "ongoing midi query exceeds 256 frames",
            Self::BeatDomainExhausted => "ongoing midi query exceeds the beat domain",
            Self::GateTooShortForAdmission => {
                "cannot guarantee a one-frame midi note gate at the admitted tempo"
            }
            Self::LoopPeriodTooShortForAdmission => {
                "cannot guarantee a one-frame midi loop period at the admitted tempo"
            }
            Self::TooManyEdges => "ongoing midi query exceeds 256 edges",
            Self::TooManyOnsets => "ongoing midi query exceeds 128 onsets",
            Self::PeriodWorkBoundExceeded => "ongoing midi query exceeds its bounded period search",
            Self::TooManySourceNotes => "ongoing midi instrument source exceeds 65,536 notes",
            Self::LoopPeriodTooShortForSpanAdmission => {
                "midi loop period is too short for bounded instrument span queries"
            }
            Self::SpanAdmissionWorkBoundExceeded => {
                "midi instrument voice admission exceeds its bounded first-tail sweep"
            }
            Self::SpanPeriodWorkBoundExceeded => {
                "ongoing midi instrument query exceeds its bounded period search"
            }
            Self::SpanVisitBoundExceeded => {
                "ongoing midi instrument query exceeds its interval visit bound"
            }
            Self::TooManySpans => "ongoing midi instrument query exceeds 192 sounding spans",
            Self::TooManyVoices => "ongoing midi source exceeds 64 simultaneous sounding voices",
            Self::TempoConversion => "ongoing midi edge exceeds tempo conversion range",
        })
    }
}

impl std::error::Error for MidiTemplateError {}

#[derive(Clone, Copy, Debug)]
struct SourceEdge {
    phase: u64,
    kind: MidiEventKind,
    note: NoteId,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SourceSpan {
    pub(super) start: u64,
    pub(super) gate_end: u64,
    pub(super) note: NoteId,
    pub(super) pitch: u8,
    pub(super) channel: u8,
    pub(super) velocity: u8,
}

pub(crate) fn session_source_note_has_occurrence(source: &MidiSource, note: &MidiNote) -> bool {
    source_note_has_intro_occurrence(source, note)
        || source_note_has_repeat_occurrence(source, note)
}

fn source_note_has_intro_occurrence(source: &MidiSource, note: &MidiNote) -> bool {
    let intro_end = if source.looping() {
        source.loop_end().raw()
    } else {
        source.source_length().raw()
    };
    note.start().raw() >= source.source_start().raw() && note.start().raw() < intro_end
}

fn source_note_has_repeat_occurrence(source: &MidiSource, note: &MidiNote) -> bool {
    source.looping()
        && note.start().raw() >= source.loop_start().raw()
        && note.start().raw() < source.loop_end().raw()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceOccurrence {
    pub(crate) onset: BeatTime,
    pub(crate) gate_end: BeatTime,
    pub(crate) pitch: u8,
    pub(crate) channel: u8,
    pub(crate) velocity: u8,
}

struct QueryContext<'a, T: MidiTiming + ?Sized> {
    launch_raw: u128,
    timing: &'a T,
    start_frame: u64,
    end_frame: u64,
    buffer: &'a mut MidiEdgeBuffer,
}

impl SourceEdge {
    fn from_note(phase: u64, kind: MidiEventKind, note: &MidiNote) -> Self {
        Self {
            phase,
            kind,
            note: note.id(),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        }
    }

    fn is_onset(self) -> bool {
        self.kind == MidiEventKind::NoteOn
    }
}

/// A source template that can produce ongoing events without expanding loops.
///
/// It is prepared from canonical clip metadata and note identity. Arrangement
/// duration is intentionally ignored; one-shot sources stop at source end and
/// looping sources repeat their loop interval indefinitely. The template owns
/// sorted intro/repeat edge arrays, and query-time work visits only binary-
/// searched candidate ranges plus at most 260 overlapping period windows.
#[derive(Debug)]
pub struct PreparedMidiSourceTemplate {
    track: TrackId,
    clip: ClipId,
    sample_rate: u32,
    admitted_max_micro_bpm: u64,
    first_pass: u64,
    period: Option<u64>,
    intro: Vec<SourceEdge>,
    repeats: Vec<SourceEdge>,
    intro_spans: Vec<SourceSpan>,
    repeat_spans: Vec<SourceSpan>,
    density: DensityEnvelope,
}

impl PreparedMidiSourceTemplate {
    /// Prepare an ongoing source from a validated canonical clip and timing map.
    ///
    /// Admission bounds every closed beat interval that can map into one
    /// 256-frame query. At most 1,024 Q32 prefix truncations contribute less
    /// than 1,024 Q32 frame units; the extra two frames cover nearest-frame
    /// endpoint rounding. Intro, cyclic periodic and first-seam edge density
    /// are checked independently for both all edges and onsets. The seam bound
    /// sums the final intro window and first periodic tail, so it is conservative.
    pub fn prepare<T: MidiTiming + ?Sized>(
        track: TrackId,
        clip: &MidiClip,
        timing: &T,
    ) -> Result<Self, MidiTemplateError> {
        clip.source_playback()
            .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
        Self::prepare_source(track, clip.id(), clip.source(), timing)
    }

    /// Prepare an independently-owned MIDI source, including Session clips
    /// that have no finite Arrangement placement.
    pub fn prepare_source<T: MidiTiming + ?Sized>(
        track: TrackId,
        clip: ClipId,
        source_data: &MidiSource,
        timing: &T,
    ) -> Result<Self, MidiTemplateError> {
        let sample_rate = timing.sample_rate();
        let max_micro_bpm = timing.max_micro_bpm();
        let source = MidiSourcePlayback::new(
            source_data.source_length(),
            source_data.source_start(),
            source_data
                .looping()
                .then_some((source_data.loop_start(), source_data.loop_end())),
            source_data.source_length(),
        )
        .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
        let source_start = source.source_start().raw();
        let first_pass = if let Some((_, loop_end)) = source.loop_range() {
            loop_end
                .raw()
                .checked_sub(source_start)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?
        } else {
            source
                .source_length()
                .raw()
                .checked_sub(source_start)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?
        };
        let period = source
            .loop_range()
            .map(|(loop_start, loop_end)| loop_end.raw() - loop_start.raw());
        let max_window_raw = conservative_window_raw(sample_rate, max_micro_bpm)?;

        if let Some(period) = period {
            require_positive_frame_advance(period, sample_rate, max_micro_bpm)
                .map_err(|_| MidiTemplateError::LoopPeriodTooShortForAdmission)?;
        }

        let max_edges = source_data
            .notes()
            .len()
            .checked_mul(4)
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let mut intro = Vec::new();
        let mut repeats = Vec::new();
        let mut intro_spans = Vec::new();
        let mut repeat_spans = Vec::new();
        intro
            .try_reserve_exact(max_edges / 2)
            .map_err(|_| MidiTemplateError::Allocation)?;
        repeats
            .try_reserve_exact(max_edges / 2)
            .map_err(|_| MidiTemplateError::Allocation)?;
        intro_spans
            .try_reserve_exact(source_data.notes().len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        repeat_spans
            .try_reserve_exact(source_data.notes().len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        let intro_end = source
            .loop_range()
            .map_or(source.source_length().raw(), |(_, loop_end)| loop_end.raw());

        for note in source_data.notes() {
            let note_start = note.start().raw();
            let note_end = note_start
                .checked_add(note.duration().raw())
                .ok_or(MidiTemplateError::InvalidSource("midi note end overflow"))?;

            if source_note_has_intro_occurrence(source_data, note) {
                let gate_end = note_end.min(intro_end);
                let gate = gate_end
                    .checked_sub(note_start)
                    .ok_or(MidiTemplateError::ArithmeticOverflow)?;
                require_positive_frame_advance(gate, sample_rate, max_micro_bpm)
                    .map_err(|_| MidiTemplateError::GateTooShortForAdmission)?;
                let onset = note_start - source_start;
                let offset = gate_end - source_start;
                intro_spans.push(SourceSpan::from_note(onset, offset, note));
                intro.push(SourceEdge::from_note(onset, MidiEventKind::NoteOn, note));
                intro.push(SourceEdge::from_note(offset, MidiEventKind::NoteOff, note));
            }

            if source_note_has_repeat_occurrence(source_data, note) {
                if let Some((loop_start, loop_end)) = source.loop_range() {
                    let gate_end = note_end.min(loop_end.raw());
                    let gate = gate_end
                        .checked_sub(note_start)
                        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
                    require_positive_frame_advance(gate, sample_rate, max_micro_bpm)
                        .map_err(|_| MidiTemplateError::GateTooShortForAdmission)?;
                    let onset = note_start - loop_start.raw();
                    let offset = gate_end - loop_start.raw();
                    repeat_spans.push(SourceSpan::from_note(onset, offset, note));
                    repeats.push(SourceEdge::from_note(onset, MidiEventKind::NoteOn, note));
                    repeats.push(SourceEdge::from_note(offset, MidiEventKind::NoteOff, note));
                }
            }
        }
        sort_edges(&mut intro);
        sort_edges(&mut repeats);
        intro_spans.sort_unstable_by_key(|span| span.note);
        repeat_spans.sort_unstable_by_key(|span| span.note);
        let mut intro_density = Vec::new();
        let mut repeat_density = Vec::new();
        intro_density
            .try_reserve_exact(intro.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        repeat_density
            .try_reserve_exact(repeats.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        intro_density.extend(intro.iter().map(|edge| DensityEvent {
            phase: edge.phase,
            onset: edge.is_onset(),
        }));
        repeat_density.extend(repeats.iter().map(|edge| DensityEvent {
            phase: edge.phase,
            onset: edge.is_onset(),
        }));
        let density = admit_density(
            &intro_density,
            &repeat_density,
            first_pass,
            period,
            max_window_raw,
        )?;

        Ok(Self {
            track,
            clip,
            sample_rate,
            admitted_max_micro_bpm: max_micro_bpm,
            first_pass,
            period,
            intro,
            repeats,
            intro_spans,
            repeat_spans,
            density,
        })
    }

    pub fn clip_id(&self) -> ClipId {
        self.clip
    }

    pub fn track_id(&self) -> TrackId {
        self.track
    }

    /// Resolve one source-relative note occurrence using the preparation-time
    /// NoteId index. The handoff path uses this to compare a carried voice with
    /// its candidate revision without rescanning authored notes.
    pub(crate) fn occurrence(
        &self,
        note: NoteId,
        iteration: u64,
        launch: BeatTime,
    ) -> Option<SourceOccurrence> {
        let (spans, base) = if iteration == 0 {
            (&self.intro_spans, 0_u128)
        } else {
            let period = u128::from(self.period?);
            let repeat_index = u128::from(iteration.checked_sub(1)?);
            let base =
                u128::from(self.first_pass).checked_add(repeat_index.checked_mul(period)?)?;
            (&self.repeat_spans, base)
        };
        let span = spans
            .binary_search_by_key(&note, |span| span.note)
            .ok()
            .and_then(|index| spans.get(index))?;
        let onset = u128::from(launch.raw())
            .checked_add(base)?
            .checked_add(u128::from(span.start))?;
        let gate_end = u128::from(launch.raw())
            .checked_add(base)?
            .checked_add(u128::from(span.gate_end))?;
        Some(SourceOccurrence {
            onset: BeatTime::from_raw(u64::try_from(onset).ok()?),
            gate_end: BeatTime::from_raw(u64::try_from(gate_end).ok()?),
            pitch: span.pitch,
            channel: span.channel,
            velocity: span.velocity,
        })
    }

    /// Conservative admitted maximum of edges in one bounded query window.
    pub fn max_query_edges(&self) -> usize {
        self.density.max_edges
    }

    /// Conservative admitted maximum of note onsets in one bounded query window.
    pub fn max_query_onsets(&self) -> usize {
        self.density.max_onsets
    }

    /// Fill the caller-owned fixed buffer with edges in `[start_frame, end_frame)`.
    ///
    /// Query length is at most 256 frames. On error `buffer.len` is reset to zero
    /// and all slot contents are ignored. No allocation, history replay, or scan
    /// across all authored notes/edges occurs here.
    pub fn query<T: MidiTiming + ?Sized>(
        &self,
        launch: BeatTime,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiEdgeBuffer,
    ) -> Result<(), MidiTemplateError> {
        buffer.len = 0;
        buffer.onsets = 0;
        let result = self.query_inner(launch, timing, start_frame, end_frame, buffer);
        if result.is_err() {
            buffer.len = 0;
            buffer.onsets = 0;
        }
        result
    }

    fn query_inner<T: MidiTiming + ?Sized>(
        &self,
        launch: BeatTime,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiEdgeBuffer,
    ) -> Result<(), MidiTemplateError> {
        if start_frame > end_frame {
            return Err(MidiTemplateError::InvalidFrameRange);
        }
        if end_frame - start_frame > MIDI_TEMPLATE_QUERY_FRAMES {
            return Err(MidiTemplateError::QueryTooWide);
        }
        if timing.sample_rate() != self.sample_rate {
            return Err(MidiTemplateError::TempoRateMismatch);
        }
        if timing.max_micro_bpm() > self.admitted_max_micro_bpm {
            return Err(MidiTemplateError::TempoMapTooFast);
        }
        if start_frame == end_frame {
            return Ok(());
        }

        let lower = timing
            .beat_lower_bound_for_frame(start_frame)
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        let upper = timing
            .beat_lower_bound_for_frame(end_frame)
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        if upper <= u128::from(launch.raw()) {
            return Ok(());
        }
        if lower == hex_project::TempoMap::BEAT_RANGE_SENTINEL {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }
        let launch_raw = u128::from(launch.raw());
        let relative_start = lower.max(launch_raw) - launch_raw;
        let relative_end = upper.saturating_sub(launch_raw);
        if relative_start >= relative_end {
            return Ok(());
        }
        if relative_start > u128::from(u64::MAX) || relative_end > u128::from(u64::MAX) + 1 {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }

        let mut context = QueryContext {
            launch_raw,
            timing,
            start_frame,
            end_frame,
            buffer,
        };
        self.append_intro(&mut context, relative_start, relative_end)?;
        if let Some(period) = self.period {
            self.append_repeats(&mut context, relative_start, relative_end, period)?;
        }
        context.buffer.slots[..context.buffer.len].sort_unstable_by_key(|slot| {
            slot.map_or(
                (u64::MAX, u8::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX),
                event_sort_key,
            )
        });
        Ok(())
    }

    fn append_intro<T: MidiTiming + ?Sized>(
        &self,
        context: &mut QueryContext<'_, T>,
        relative_start: u128,
        relative_end: u128,
    ) -> Result<(), MidiTemplateError> {
        let begin = self
            .intro
            .partition_point(|edge| u128::from(edge.phase) < relative_start);
        let end = self
            .intro
            .partition_point(|edge| u128::from(edge.phase) < relative_end);
        for edge in &self.intro[begin..end] {
            let absolute = context
                .launch_raw
                .checked_add(u128::from(edge.phase))
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            self.push_edge(*edge, 0, absolute, context)?;
        }
        Ok(())
    }

    fn append_repeats<T: MidiTiming + ?Sized>(
        &self,
        context: &mut QueryContext<'_, T>,
        relative_start: u128,
        relative_end: u128,
        period: u64,
    ) -> Result<(), MidiTemplateError> {
        if self.repeats.is_empty() || relative_end <= u128::from(self.first_pass) {
            return Ok(());
        }
        let period = u128::from(period);
        let first_pass = u128::from(self.first_pass);
        let earliest_base = relative_start.saturating_sub(period);
        let first_iteration = if earliest_base <= first_pass {
            0
        } else {
            ceil_div(earliest_base - first_pass, period)?
        };
        let last_iteration = (relative_end - 1 - first_pass) / period;
        if first_iteration > last_iteration {
            return Ok(());
        }
        let iteration_count = last_iteration
            .checked_sub(first_iteration)
            .and_then(|count| count.checked_add(1))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        if iteration_count > MIDI_TEMPLATE_MAX_PERIOD_WINDOWS {
            return Err(MidiTemplateError::PeriodWorkBoundExceeded);
        }

        let mut iteration = first_iteration;
        while iteration <= last_iteration {
            let base = first_pass
                .checked_add(
                    iteration
                        .checked_mul(period)
                        .ok_or(MidiTemplateError::BeatDomainExhausted)?,
                )
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            let local_start = relative_start.saturating_sub(base);
            let local_end = relative_end.saturating_sub(base);
            let begin = self
                .repeats
                .partition_point(|edge| u128::from(edge.phase) < local_start);
            let end = self
                .repeats
                .partition_point(|edge| u128::from(edge.phase) < local_end);
            for edge in &self.repeats[begin..end] {
                let relative = base
                    .checked_add(u128::from(edge.phase))
                    .ok_or(MidiTemplateError::BeatDomainExhausted)?;
                let absolute = context
                    .launch_raw
                    .checked_add(relative)
                    .ok_or(MidiTemplateError::BeatDomainExhausted)?;
                let repeat_number = u64::try_from(iteration)
                    .map_err(|_| MidiTemplateError::BeatDomainExhausted)?
                    .checked_add(1)
                    .ok_or(MidiTemplateError::BeatDomainExhausted)?;
                self.push_edge(*edge, repeat_number, absolute, context)?;
            }
            iteration = iteration
                .checked_add(1)
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        }
        Ok(())
    }

    fn push_edge<T: MidiTiming + ?Sized>(
        &self,
        edge: SourceEdge,
        iteration: u64,
        absolute_raw: u128,
        context: &mut QueryContext<'_, T>,
    ) -> Result<(), MidiTemplateError> {
        let raw =
            u64::try_from(absolute_raw).map_err(|_| MidiTemplateError::BeatDomainExhausted)?;
        let frame = context
            .timing
            .beat_to_frame(BeatTime::from_raw(raw))
            .map_err(|_| MidiTemplateError::TempoConversion)?;
        if frame < context.start_frame || frame >= context.end_frame {
            return Ok(());
        }
        let next_len = context
            .buffer
            .len
            .checked_add(1)
            .ok_or(MidiTemplateError::TooManyEdges)?;
        if next_len > MIDI_TEMPLATE_MAX_EDGES {
            return Err(MidiTemplateError::TooManyEdges);
        }
        if edge.is_onset() && context.buffer.onsets >= MIDI_TEMPLATE_MAX_ONSETS {
            return Err(MidiTemplateError::TooManyOnsets);
        }
        context.buffer.slots[context.buffer.len] = Some(crate::midi_compile::CompiledMidiEvent {
            frame,
            kind: edge.kind,
            track: self.track,
            clip: self.clip,
            note: edge.note,
            iteration,
            pitch: edge.pitch,
            channel: edge.channel,
            velocity: edge.velocity,
        });
        context.buffer.len = next_len;
        if edge.is_onset() {
            context.buffer.onsets += 1;
        }
        Ok(())
    }
}

impl SourceSpan {
    fn from_note(start: u64, gate_end: u64, note: &MidiNote) -> Self {
        Self {
            start,
            gate_end,
            note: note.id(),
            pitch: note.pitch(),
            channel: note.channel(),
            velocity: note.velocity(),
        }
    }
}

pub(crate) use spans::MidiReleaseVoice;
pub use spans::{
    MIDI_SPAN_BUFFER_CAPACITY, MidiInstrumentRenderContext, MidiRenderSpan, MidiSpanBuffer,
    PreparedMidiInstrumentTemplate,
};

/// Fixed-capacity output storage reusable across stateless edge queries.
#[derive(Clone, Debug)]
pub struct MidiEdgeBuffer {
    slots: [Option<crate::midi_compile::CompiledMidiEvent>; MIDI_TEMPLATE_MAX_EDGES],
    len: usize,
    onsets: usize,
}

impl Default for MidiEdgeBuffer {
    fn default() -> Self {
        Self {
            slots: [None; MIDI_TEMPLATE_MAX_EDGES],
            len: 0,
            onsets: 0,
        }
    }
}

impl MidiEdgeBuffer {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn events(&self) -> impl Iterator<Item = &crate::midi_compile::CompiledMidiEvent> {
        self.slots[..self.len].iter().filter_map(Option::as_ref)
    }
}

fn sort_edges(edges: &mut [SourceEdge]) {
    edges.sort_unstable_by_key(|edge| {
        (
            edge.phase,
            if edge.kind == MidiEventKind::NoteOff {
                0_u8
            } else {
                1
            },
            edge.note.value(),
        )
    });
}

fn event_sort_key(event: crate::midi_compile::CompiledMidiEvent) -> (u64, u8, u64, u64, u64, u64) {
    (
        event.frame,
        if event.kind == MidiEventKind::NoteOff {
            0
        } else {
            1
        },
        event.track.value(),
        event.clip.value(),
        event.note.value(),
        event.iteration,
    )
}

fn conservative_window_raw(sample_rate: u32, max_micro_bpm: u64) -> Result<u64, MidiTemplateError> {
    let numerator = 258_u128
        .checked_mul(FRAME_Q32)
        .and_then(|value| value.checked_mul(u128::from(max_micro_bpm)))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let denominator = FRAME_FACTOR_NUMERATOR
        .checked_mul(u128::from(sample_rate))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    u64::try_from(ceil_div(numerator, denominator)?)
        .map_err(|_| MidiTemplateError::ArithmeticOverflow)
}

fn require_positive_frame_advance(
    raw_beat_advance: u64,
    sample_rate: u32,
    max_micro_bpm: u64,
) -> Result<(), MidiTemplateError> {
    let factor = FRAME_FACTOR_NUMERATOR
        .checked_mul(u128::from(sample_rate))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let q32 = u128::from(raw_beat_advance)
        .checked_mul(factor)
        .ok_or(MidiTemplateError::ArithmeticOverflow)?
        / u128::from(max_micro_bpm);
    let conservative = q32
        .checked_sub(MAX_PREFIX_TRUNCATION_Q32)
        .ok_or(MidiTemplateError::GateTooShortForAdmission)?;
    if conservative < FRAME_Q32 {
        return Err(MidiTemplateError::GateTooShortForAdmission);
    }
    Ok(())
}

fn ceil_div(numerator: u128, denominator: u128) -> Result<u128, MidiTemplateError> {
    if denominator == 0 {
        return Err(MidiTemplateError::ArithmeticOverflow);
    }
    let quotient = numerator / denominator;
    if numerator % denominator == 0 {
        Ok(quotient)
    } else {
        quotient
            .checked_add(1)
            .ok_or(MidiTemplateError::ArithmeticOverflow)
    }
}
