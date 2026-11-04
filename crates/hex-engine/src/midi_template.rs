//! Immutable beat-domain MIDI source templates for ongoing playback queries.
//!
//! Template construction and density admission are control-side operations.
//! Queries borrow a prepared tempo map and write into fixed caller-owned
//! storage. The template ignores finite Arrangement duration by design.
use crate::{MidiTiming, midi_compile::MidiEventKind};
mod current_sampler;
mod density;
mod origin;
mod readmission;
pub use origin::SessionMidiOrigin;
mod spans;
use density::{DensityEnvelope, DensityEvent, admit_density};
use hex_project::{
    BeatTime, ClipId, MidiClip, MidiNote, MidiSource, MidiSourcePlayback, NoteId, TrackId,
};
pub(crate) use spans::{MidiOccurrenceSuppression, PreparedMidiGateTemplate};

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
    MutableEventsNotAnalytic,
}

impl MidiTemplateError {
    pub(crate) fn diagnostic_message(self) -> &'static str {
        match self {
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
            Self::MutableEventsNotAnalytic => {
                "mutable midi event source cannot use analytic note rendering"
            }
        }
    }
}

impl std::fmt::Display for MidiTemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.diagnostic_message())
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

pub(crate) fn session_source_note_has_occurrence(
    source: &MidiSource,
    offset: BeatTime,
    note: &MidiNote,
) -> bool {
    note.active()
        && (source_note_has_repeat_occurrence(source, note)
            || source_note_has_intro_occurrence(source, note)
                && note
                    .start()
                    .raw()
                    .saturating_add(note.duration().raw())
                    .min(source.effective_end().raw())
                    .saturating_sub(source.source_start().raw())
                    > offset.raw())
}

fn source_note_has_intro_occurrence(source: &MidiSource, note: &MidiNote) -> bool {
    let intro_end = source.effective_end().raw();
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
    origin: SessionMidiOrigin,
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
    intro_iteration: u64,
    repeat_iteration: u64,
    period: Option<u64>,
    source: MidiSourcePlayback,
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
        Self::prepare_source_window_active(
            track,
            clip.id(),
            clip.source(),
            clip.playback_offset(),
            clip.active(),
            timing,
        )
    }

    /// Prepare an independently-owned MIDI source, including Session clips
    /// that have no finite Arrangement placement.
    pub fn prepare_source<T: MidiTiming + ?Sized>(
        track: TrackId,
        clip: ClipId,
        source_data: &MidiSource,
        timing: &T,
    ) -> Result<Self, MidiTemplateError> {
        Self::prepare_source_window(track, clip, source_data, BeatTime::ZERO, timing)
    }

    /// Prepare an ongoing source beginning at an elapsed source-law window.
    /// Crossing gates are chased at local zero with their remaining duration.
    /// Original repeat ordinals survive normalization; skipped loops are never
    /// expanded. Session launch still ignores finite Arrangement clip length.
    pub fn prepare_source_window<T: MidiTiming + ?Sized>(
        track: TrackId,
        clip: ClipId,
        source_data: &MidiSource,
        playback_offset: BeatTime,
        timing: &T,
    ) -> Result<Self, MidiTemplateError> {
        Self::prepare_source_window_active(track, clip, source_data, playback_offset, true, timing)
    }

    pub(crate) fn prepare_source_window_active<T: MidiTiming + ?Sized>(
        track: TrackId,
        clip: ClipId,
        source_data: &MidiSource,
        playback_offset: BeatTime,
        active: bool,
        timing: &T,
    ) -> Result<Self, MidiTemplateError> {
        Self::prepare_source_window_admitted(
            track,
            clip,
            source_data,
            playback_offset,
            active,
            timing.sample_rate(),
            timing.max_micro_bpm(),
        )
    }

    pub(crate) fn prepare_source_window_admitted(
        track: TrackId,
        clip: ClipId,
        source_data: &MidiSource,
        playback_offset: BeatTime,
        active: bool,
        sample_rate: u32,
        max_micro_bpm: u64,
    ) -> Result<Self, MidiTemplateError> {
        if sample_rate == 0 || max_micro_bpm == 0 {
            return Err(MidiTemplateError::InvalidSource(
                "invalid midi admission timing",
            ));
        }
        let horizon = u64::MAX
            .checked_sub(playback_offset.raw())
            .filter(|remaining| *remaining != 0)
            .ok_or(MidiTemplateError::BeatDomainExhausted)?;
        let source = source_data
            .playback_for_duration(BeatTime::from_raw(horizon))
            .and_then(|source| source.with_playback_offset(playback_offset))
            .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
        let source_start = source.source_start().raw();
        let first_pass = if let Some((_, loop_end)) = source.loop_range() {
            loop_end
                .raw()
                .checked_sub(source_start)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?
        } else {
            source
                .initial_end()
                .raw()
                .checked_sub(source_start)
                .ok_or(MidiTemplateError::ArithmeticOverflow)?
        };
        let period = active
            .then(|| source.loop_range())
            .flatten()
            .map(|(loop_start, loop_end)| loop_end.raw() - loop_start.raw());
        let elapsed = playback_offset.raw();
        let (first_pass, intro_iteration, repeat_iteration) = if elapsed < first_pass {
            (first_pass - elapsed, 0, 1)
        } else if let Some(period) = period {
            let iteration = (elapsed - first_pass) / period + 1;
            let remaining = period - (elapsed - first_pass) % period;
            (
                remaining.min(horizon),
                iteration,
                iteration
                    .checked_add(1)
                    .ok_or(MidiTemplateError::BeatDomainExhausted)?,
            )
        } else {
            (0, 0, 1)
        };
        let max_window_raw = conservative_window_raw(sample_rate, max_micro_bpm)?;

        if let Some(period) = period {
            require_positive_frame_advance(period, sample_rate, max_micro_bpm)
                .map_err(|_| MidiTemplateError::LoopPeriodTooShortForAdmission)?;
        }

        let note_count = if active {
            source_data
                .notes()
                .iter()
                .filter(|note| note.active())
                .count()
        } else {
            0
        };
        let max_edges = note_count
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
            .try_reserve_exact(note_count)
            .map_err(|_| MidiTemplateError::Allocation)?;
        repeat_spans
            .try_reserve_exact(note_count)
            .map_err(|_| MidiTemplateError::Allocation)?;
        let intro_window = if first_pass == 0 {
            None
        } else {
            Some(
                source_data
                    .playback_for_duration(BeatTime::from_raw(first_pass))
                    .and_then(|source| source.with_playback_offset(playback_offset))
                    .map_err(|error| MidiTemplateError::InvalidSource(error.0))?,
            )
        };
        for note in source_data
            .notes()
            .iter()
            .filter(|note| active && note.active())
        {
            let note_start = note.start().raw();
            let note_end = note_start
                .checked_add(note.duration().raw())
                .ok_or(MidiTemplateError::InvalidSource("midi note end overflow"))?;
            if let Some(window) = &intro_window {
                let plan = window
                    .note_occurrences(note.start(), note.duration())
                    .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
                for occurrence in plan.iter() {
                    let onset = occurrence.timeline_start().raw();
                    let offset = onset
                        .checked_add(occurrence.gate_duration().raw())
                        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
                    require_positive_frame_advance(offset - onset, sample_rate, max_micro_bpm)
                        .map_err(|_| MidiTemplateError::GateTooShortForAdmission)?;
                    intro_spans.push(SourceSpan::from_note(onset, offset, note));
                    intro.push(SourceEdge::from_note(onset, MidiEventKind::NoteOn, note));
                    intro.push(SourceEdge::from_note(offset, MidiEventKind::NoteOff, note));
                }
            }
            if source_note_has_repeat_occurrence(source_data, note) {
                if let Some((loop_start, loop_end)) = source.loop_range() {
                    let gate_end = note_end.min(loop_end.raw());
                    require_positive_frame_advance(
                        gate_end - note_start,
                        sample_rate,
                        max_micro_bpm,
                    )
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
            intro_iteration,
            repeat_iteration,
            period,
            source,
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

    /// Resolve one occurrence in global performance coordinates. Historical
    /// onsets are refused before gate projection, matching no-chase queries.
    pub(crate) fn occurrence_with_origin(
        &self,
        note: NoteId,
        iteration: u64,
        origin: SessionMidiOrigin,
    ) -> Option<SourceOccurrence> {
        let (span, base) = self.occurrence_span(note, iteration)?;
        let onset = origin
            .project(base.checked_add(u128::from(span.start))?)
            .ok()??;
        let gate_end = origin
            .project(base.checked_add(u128::from(span.gate_end))?)
            .ok()??;
        Some(SourceOccurrence {
            onset: BeatTime::from_raw(onset),
            gate_end: BeatTime::from_raw(gate_end),
            pitch: span.pitch,
            channel: span.channel,
            velocity: span.velocity,
        })
    }
    fn occurrence_span(&self, note: NoteId, iteration: u64) -> Option<(SourceSpan, u128)> {
        let (spans, base) = if iteration == self.intro_iteration {
            (&self.intro_spans, 0_u128)
        } else {
            let period = u128::from(self.period?);
            let repeat_index = u128::from(iteration.checked_sub(self.repeat_iteration)?);
            let base =
                u128::from(self.first_pass).checked_add(repeat_index.checked_mul(period)?)?;
            (&self.repeat_spans, base)
        };
        let span = spans
            .binary_search_by_key(&note, |span| span.note)
            .ok()
            .and_then(|index| spans.get(index))?;
        Some((*span, base))
    }

    /// Earliest elapsed occurrence of a source-local cue, even in blank source
    /// time. Canonical projection owns intro/repeat/skipped-source semantics.
    pub fn earliest_elapsed_for_source(
        &self,
        source_beat: BeatTime,
    ) -> Result<Option<BeatTime>, MidiTemplateError> {
        let plan = self
            .source
            .note_occurrences(source_beat, BeatTime::from_raw(1))
            .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
        Ok(plan
            .iter()
            .next()
            .map(|occurrence| occurrence.timeline_start()))
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
        self.query_with_origin(
            SessionMidiOrigin::launch(launch),
            timing,
            start_frame,
            end_frame,
            buffer,
        )
    }

    /// Query on the existing performance clock from an inclusive elapsed cue.
    /// Pre-cue onsets and their paired NoteOffs are excluded before conversion.
    pub fn query_with_origin<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiEdgeBuffer,
    ) -> Result<(), MidiTemplateError> {
        buffer.len = 0;
        buffer.onsets = 0;
        let result = self.query_inner(origin, timing, start_frame, end_frame, buffer);
        if result.is_err() {
            buffer.len = 0;
            buffer.onsets = 0;
        }
        result
    }

    fn query_inner<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
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
        if lower == hex_project::TempoMap::BEAT_RANGE_SENTINEL {
            return Err(MidiTemplateError::BeatDomainExhausted);
        }
        let Some((relative_start, relative_end)) = origin.query_bounds(lower, upper)? else {
            return Ok(());
        };

        let mut context = QueryContext {
            origin,
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
            self.push_edge(*edge, self.intro_iteration, u128::from(edge.phase), context)?;
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
                let repeat_number = u64::try_from(iteration)
                    .map_err(|_| MidiTemplateError::BeatDomainExhausted)?
                    .checked_add(self.repeat_iteration)
                    .ok_or(MidiTemplateError::BeatDomainExhausted)?;
                self.push_edge(*edge, repeat_number, relative, context)?;
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
        elapsed: u128,
        context: &mut QueryContext<'_, T>,
    ) -> Result<(), MidiTemplateError> {
        // NoteOff is admissible only if its paired onset belongs to this cue.
        // Ordinary launches need no lookup; cued offs reuse the prepared NoteId index.
        if context.origin.elapsed_at_anchor != BeatTime::ZERO && !edge.is_onset() {
            let (span, base) = self.occurrence_span(edge.note, iteration).ok_or(
                MidiTemplateError::InvalidSource("midi edge has no paired onset"),
            )?;
            let onset = base
                .checked_add(u128::from(span.start))
                .ok_or(MidiTemplateError::BeatDomainExhausted)?;
            if onset < u128::from(context.origin.elapsed_at_anchor.raw()) {
                return Ok(());
            }
        }
        let Some(raw) = context.origin.project(elapsed)? else {
            return Ok(());
        };
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

pub(crate) use spans::{
    CurrentSamplerEventCustody, MidiReleaseVoice, TransportDensitySpan,
    admit_held_transport_loop_density, admit_transport_loop_density,
};
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

#[cfg(test)]
mod window_tests;
