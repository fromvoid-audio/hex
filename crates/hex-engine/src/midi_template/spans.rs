//! Prepared, stateless source-note spans for ongoing MIDI instruments.
use super::{
    MIDI_TEMPLATE_QUERY_FRAMES, MidiTemplateError, PreparedMidiSourceTemplate, SessionMidiOrigin,
    SourceSpan, require_positive_frame_advance,
};
mod current_sampler;
mod density;
mod gates;
mod volume;
use crate::{MidiTiming, PreparedTempoMap, PreparedTrackMix, midi_note_bank::PreparedMidiNote};
pub(crate) use current_sampler::CurrentSamplerEventCustody;
use density::{admit_held_voice_density, admit_voice_density};
pub(crate) use gates::PreparedMidiGateTemplate;
use hex_devices::SineTable;
use hex_lattice::{PreparedAnalyticNote, PreparedNativeInstrument};
use hex_project::{BeatTime, TempoMap};

/// Maximum sounding note spans returned for one 256-frame window.
pub const MIDI_SPAN_BUFFER_CAPACITY: usize = 192;
const MIDI_SPAN_MAX_SOURCE_NOTES: usize = 65_536;
// At most 192 admitted intervals can intersect a 256-frame raw query window.
// The balanced index has height <=17; 327 trees (intro + 326 repeat windows)
// charge one root-to-hit path per match plus two boundary paths per tree:
// 17 * (192 + 2 * 327) = 14,382, below this fixed query-work cap.
const MIDI_SPAN_MAX_QUERY_VISITS: usize = 16_384;
const MIDI_SPAN_MAX_PERIOD_WINDOWS: u128 = 326;
const Q32: u128 = 1_u128 << 32;
const FRAME_DENOMINATOR: u128 = 60_000_000;

pub(crate) use density::DensitySpan as TransportDensitySpan;

pub(crate) fn admit_transport_loop_density(
    intro: &[TransportDensitySpan],
    repeats: &[TransportDensitySpan],
    first_pass: u64,
    period: u64,
    release_raw: u64,
) -> Result<usize, MidiTemplateError> {
    density::admit_transport_loop_density(intro, repeats, first_pass, period, release_raw)
}

/// Held-only transport-loop density for instruments that steal releasing voices.
pub(crate) fn admit_held_transport_loop_density(
    intro: &[TransportDensitySpan],
    repeats: &[TransportDensitySpan],
    first_pass: u64,
    period: u64,
) -> Result<usize, MidiTemplateError> {
    density::admit_held_transport_loop_density(intro, repeats, first_pass, period)
}

#[derive(Clone, Copy, Debug)]
struct VoiceSpan {
    start: u64,
    gate_end: u64,
    raw_end: u128,
    note: hex_project::NoteId,
    pitch: u8,
    channel: u8,
    velocity: u8,
    sampler_index: u16,
}

#[derive(Clone, Copy, Debug)]
struct IntervalNode {
    span: usize,
    min_start: u64,
    max_end: u128,
    left: Option<usize>,
    right: Option<usize>,
}

/// An immutable ongoing instrument source. It retains no owning device handle;
/// rendering borrows the graph-owned sine table.
#[derive(Debug)]
pub struct PreparedMidiInstrumentTemplate {
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    sample_rate: u32,
    admitted_max_micro_bpm: u64,
    first_pass: u64,
    intro_iteration: u64,
    repeat_iteration: u64,
    period: Option<u64>,
    release_raw: u64,
    intro: Vec<VoiceSpan>,
    repeats: Vec<VoiceSpan>,
    intro_nodes: Vec<IntervalNode>,
    repeat_nodes: Vec<IntervalNode>,
    intro_root: Option<usize>,
    repeat_root: Option<usize>,
    max_active_voices: usize,
    admitted_voices: usize,
    max_query_onsets: usize,
    max_span_hits: usize,
    max_query_visits: usize,
    max_period_windows: usize,
    bank: std::sync::Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
    shared_contexts: std::sync::Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    release_frames: u64,
    event_source: bool,
}

struct SilentInstrument;
impl PreparedNativeInstrument for SilentInstrument {
    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Ok(PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            silent_note_sample,
        ))
    }
}

fn silent_note_sample(
    _payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _age_frames: u64,
    _gate_frames: u64,
    _release_frames: u64,
    _resources: &dyn hex_lattice::DeviceResources,
) -> [f32; 2] {
    [0.0; 2]
}

/// One immutable rendered-note interval in absolute project frames.
#[derive(Clone, Copy, Debug)]
pub struct MidiRenderSpan {
    pub start_frame: u64,
    pub gate_end_frame: u64,
    pub end_frame: u64,
    /// Absolute Q32.32 onset and gate boundary used to remap revisions.
    pub onset_beat: BeatTime,
    pub gate_end_beat: BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
    pub track: hex_project::TrackId,
    pub clip: hex_project::ClipId,
    pub note: hex_project::NoteId,
    pub iteration: u64,
    pub(crate) voice: Option<crate::midi_note_bank::PreparedMidiNote>,
}

impl MidiRenderSpan {
    pub(crate) fn prepared_voice(&self) -> Option<crate::midi_note_bank::PreparedMidiNote> {
        self.voice
    }
}

/// Identity of a carried occurrence that must not be emitted again by the
/// newly prepared source while its copied voice is still sounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct MidiOccurrenceSuppression {
    pub(crate) track: hex_project::TrackId,
    pub(crate) clip: hex_project::ClipId,
    pub(crate) note: hex_project::NoteId,
    pub(crate) iteration: u64,
    pub(crate) loop_pass: u64,
    pub(crate) until_frame: u64,
    // One key serves both identity lookup and reconstructed mutable signature.
    // Analytic records canonicalize every unused detail, including padding.
    onset_beat: BeatTime,
    gate_end_beat: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
    kind: u8,
    padding: [u8; 4],
    // Only kind2 carries an admitted retained-prefix gate cut. Separate from
    // suppression expiry: merging expiry must never delay the NoteOff.
    gate_cutoff_frame: u64,
}

impl MidiOccurrenceSuppression {
    pub(crate) const fn analytic(
        key: crate::mutable_instrument::MutableOccurrenceKey,
        until_frame: u64,
    ) -> Self {
        Self {
            track: key.track,
            clip: key.clip,
            note: key.note,
            iteration: key.iteration,
            loop_pass: key.loop_pass,
            until_frame,
            onset_beat: BeatTime::ZERO,
            gate_end_beat: BeatTime::ZERO,
            pitch: 0,
            channel: 0,
            velocity: 0,
            kind: 0,
            padding: [0; 4],
            gate_cutoff_frame: 0,
        }
    }

    pub(crate) const fn mutable(
        signature: crate::mutable_instrument::MutableOccurrenceSignature,
        until_frame: u64,
    ) -> Self {
        let (onset_beat, gate_end_beat, pitch, channel, velocity) = signature.details();
        Self {
            onset_beat,
            gate_end_beat,
            pitch,
            channel,
            velocity,
            kind: 1,
            ..Self::analytic(signature.key(), until_frame)
        }
    }

    pub(crate) const fn mutable_shortening(
        signature: crate::mutable_instrument::MutableOccurrenceSignature,
        until_frame: u64,
        gate_cutoff_frame: u64,
    ) -> Self {
        Self {
            kind: 2,
            gate_cutoff_frame,
            ..Self::mutable(signature, until_frame)
        }
    }
    pub(crate) const fn gate_cutoff_frame(self) -> Option<u64> {
        if self.kind == 2 {
            Some(self.gate_cutoff_frame)
        } else {
            None
        }
    }
    pub(crate) fn merge_expiry_and_cutoff(&mut self, entry: Self) {
        self.until_frame = self.until_frame.max(entry.until_frame);
        if let Some(cutoff) = entry.gate_cutoff_frame() {
            self.gate_cutoff_frame = self
                .gate_cutoff_frame()
                .map_or(cutoff, |old| old.min(cutoff));
            self.kind = 2;
        }
    }

    /// Exact original value, including every key/beat/MIDI bit. No allocation
    /// or provider lookup, and no integer/beat/velocity precision reduction.
    pub(crate) const fn mutable_signature(
        self,
    ) -> Option<crate::mutable_instrument::MutableOccurrenceSignature> {
        if self.kind == 1 || self.kind == 2 {
            Some(
                crate::mutable_instrument::MutableOccurrenceSignature::from_parts(
                    crate::mutable_instrument::MutableOccurrenceKey {
                        track: self.track,
                        clip: self.clip,
                        note: self.note,
                        iteration: self.iteration,
                        loop_pass: self.loop_pass,
                    },
                    self.onset_beat,
                    self.gate_end_beat,
                    self.pitch,
                    self.channel,
                    self.velocity,
                ),
            )
        } else {
            None
        }
    }
}

#[cfg(test)]
mod suppression_tests;

/// Opaque copied analytic voice retained after a route is replaced.
#[derive(Clone, Copy, Debug)]
pub struct MidiReleaseVoice {
    start_frame: u64,
    gate_end_frame: u64,
    end_frame: u64,
    voice: crate::midi_note_bank::PreparedMidiNote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReleaseVoiceError {
    ArithmeticOverflow,
}

impl MidiRenderSpan {
    pub(crate) fn release_at(
        self,
        cut_frame: u64,
    ) -> Result<Option<MidiReleaseVoice>, ReleaseVoiceError> {
        if cut_frame < self.start_frame || cut_frame >= self.end_frame {
            return Ok(None);
        }
        let Some(voice) = self.voice else {
            return Ok(None);
        };
        if cut_frame < self.gate_end_frame {
            let end_frame = cut_frame
                .checked_add(voice.release_frames())
                .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
            Ok(Some(MidiReleaseVoice {
                start_frame: self.start_frame,
                gate_end_frame: cut_frame,
                end_frame,
                voice,
            }))
        } else {
            Ok(Some(MidiReleaseVoice {
                start_frame: self.start_frame,
                gate_end_frame: self.gate_end_frame,
                end_frame: self.end_frame,
                voice,
            }))
        }
    }

    pub(crate) fn reconcile_at(
        self,
        cut_frame: u64,
        remapped_gate_end: Option<u64>,
    ) -> Result<Option<MidiReleaseVoice>, ReleaseVoiceError> {
        if cut_frame < self.start_frame || cut_frame >= self.end_frame {
            return Ok(None);
        }
        if cut_frame >= self.gate_end_frame {
            return self.release_at(cut_frame);
        }
        let Some(gate_end_frame) = remapped_gate_end.filter(|frame| *frame > cut_frame) else {
            return self.release_at(cut_frame);
        };
        let Some(voice) = self.voice else {
            return Ok(None);
        };
        let end_frame = gate_end_frame
            .checked_add(voice.release_frames())
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        Ok(Some(MidiReleaseVoice {
            start_frame: self.start_frame,
            gate_end_frame,
            end_frame,
            voice,
        }))
    }
}

impl MidiReleaseVoice {
    pub(crate) fn start_frame(&self) -> u64 {
        self.start_frame
    }

    pub(crate) fn context_ref(&self) -> Option<crate::midi_note_bank::SharedContextRef> {
        self.voice.context_ref()
    }

    pub(crate) fn set_context_ref(&mut self, context: crate::midi_note_bank::SharedContextRef) {
        self.voice = self.voice.with_context_ref(context);
    }

    pub(crate) fn sampler(&self) -> PreparedMidiNote {
        self.voice
    }

    pub(crate) fn set_sampler(&mut self, sampler: PreparedMidiNote) {
        self.voice = sampler;
    }

    pub(crate) fn detach(&mut self, transitions: &[Option<hex_lattice::AnalyticContinuation>]) {
        self.voice = self.voice.detach(transitions);
    }

    pub(crate) fn sample_at_with_continuation(
        &self,
        frame: u64,
        contexts: &crate::shared_analytic::SharedAnalyticContextTable,
        retained: &[Option<std::sync::Arc<crate::shared_analytic::SharedContextOwner>>],
        transitions: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<Option<[f32; 2]>, crate::Invalid> {
        self.sample_at_with_clock(frame, frame, contexts, retained, transitions)
    }

    /// Source age may advance while transport is paused. The parameter clock
    /// remains musical time so a held revision ramp freezes until Play resumes.
    pub(crate) fn sample_at_with_clock(
        &self,
        frame: u64,
        parameter_frame: u64,
        contexts: &crate::shared_analytic::SharedAnalyticContextTable,
        retained: &[Option<std::sync::Arc<crate::shared_analytic::SharedContextOwner>>],
        transitions: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<Option<[f32; 2]>, crate::Invalid> {
        if frame < self.start_frame || frame >= self.end_frame {
            return Ok(None);
        }
        self.voice
            .sample_at(
                contexts,
                retained,
                transitions,
                parameter_frame,
                frame - self.start_frame,
                self.gate_end_frame - self.start_frame,
            )
            .map(Some)
    }

    pub(crate) fn end_frame(&self) -> u64 {
        self.end_frame
    }

    pub(crate) fn gate_end_frame(&self) -> u64 {
        self.gate_end_frame
    }

    pub(crate) fn reconcile_at(
        self,
        cut_frame: u64,
        remapped_gate_end: Option<u64>,
    ) -> Result<Option<Self>, ReleaseVoiceError> {
        if cut_frame >= self.end_frame {
            return Ok(None);
        }
        if cut_frame >= self.gate_end_frame {
            return Ok(Some(self));
        }
        let Some(gate_end_frame) = remapped_gate_end.filter(|frame| *frame > cut_frame) else {
            return Ok(Some(Self {
                start_frame: self.start_frame,
                gate_end_frame: cut_frame,
                end_frame: cut_frame
                    .checked_add(self.voice.release_frames())
                    .ok_or(ReleaseVoiceError::ArithmeticOverflow)?,
                voice: self.voice,
            }));
        };
        let end_frame = gate_end_frame
            .checked_add(self.voice.release_frames())
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        Ok(Some(Self {
            start_frame: self.start_frame,
            gate_end_frame,
            end_frame,
            voice: self.voice,
        }))
    }

    /// End this voice no later than performance `frame`, a loop wrap.
    /// Unedited looped playback stops an occurrence at the wrap, so a voice
    /// released before it must not sound over the next pass.
    pub(crate) fn ending_by(self, frame: u64) -> Self {
        Self {
            gate_end_frame: self.gate_end_frame.min(frame),
            end_frame: self.end_frame.min(frame),
            ..self
        }
    }

    pub(crate) fn from_finite_span(
        start_frame: u64,
        gate_end_frame: u64,
        end_frame: u64,
        voice: PreparedMidiNote,
        source_cut: u64,
        performance_cut: u64,
    ) -> Result<Option<Self>, ReleaseVoiceError> {
        if source_cut < start_frame || source_cut >= end_frame {
            return Ok(None);
        }
        let released = if source_cut < gate_end_frame {
            Self {
                start_frame,
                gate_end_frame: source_cut,
                end_frame: source_cut
                    .checked_add(voice.release_frames())
                    .ok_or(ReleaseVoiceError::ArithmeticOverflow)?,
                voice,
            }
        } else {
            Self {
                start_frame,
                gate_end_frame,
                end_frame,
                voice,
            }
        };
        released.rebase(source_cut, performance_cut).map(Some)
    }

    pub(crate) fn from_finite_span_preserving_gate(
        start_frame: u64,
        gate_end_frame: u64,
        end_frame: u64,
        voice: PreparedMidiNote,
        source_cut: u64,
        performance_cut: u64,
    ) -> Result<Option<Self>, ReleaseVoiceError> {
        if source_cut < start_frame || source_cut >= end_frame {
            return Ok(None);
        }
        Self {
            start_frame,
            gate_end_frame,
            end_frame,
            voice,
        }
        .rebase(source_cut, performance_cut)
        .map(Some)
    }

    pub(crate) fn rebase(
        self,
        source_cut: u64,
        performance_cut: u64,
    ) -> Result<Self, ReleaseVoiceError> {
        let elapsed = source_cut
            .checked_sub(self.start_frame)
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        let start_frame = performance_cut
            .checked_sub(elapsed)
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        let gate_elapsed = source_cut.saturating_sub(self.gate_end_frame);
        let gate_end_frame = if self.gate_end_frame > source_cut {
            performance_cut
                .checked_add(self.gate_end_frame - source_cut)
                .ok_or(ReleaseVoiceError::ArithmeticOverflow)?
        } else {
            performance_cut
                .checked_sub(gate_elapsed)
                .ok_or(ReleaseVoiceError::ArithmeticOverflow)?
        };
        let end_elapsed = self
            .end_frame
            .checked_sub(source_cut)
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        let end_frame = performance_cut
            .checked_add(end_elapsed)
            .ok_or(ReleaseVoiceError::ArithmeticOverflow)?;
        Ok(Self {
            start_frame,
            gate_end_frame,
            end_frame,
            voice: self.voice,
        })
    }

    #[cfg(test)]
    pub(crate) fn sample_at(
        &self,
        frame: u64,
        contexts: &crate::shared_analytic::SharedAnalyticContextTable,
        retained: &[Option<std::sync::Arc<crate::shared_analytic::SharedContextOwner>>],
    ) -> Option<[f32; 2]> {
        if frame < self.start_frame || frame >= self.end_frame {
            return None;
        }
        self.voice.sample(
            contexts,
            retained,
            frame - self.start_frame,
            self.gate_end_frame - self.start_frame,
        )
    }
}

/// Borrowed timing and oscillator context for additive ongoing-source renders.
/// The context owns no buffers or graph state and performs no preparation.
pub struct MidiInstrumentRenderContext<'a, T: MidiTiming + ?Sized = PreparedTempoMap> {
    origin: SessionMidiOrigin,
    timing: &'a T,
    onset_floor_exclusive: Option<BeatTime>,
    suppressions: &'a [MidiOccurrenceSuppression],
    continuations: &'a [Option<hex_lattice::AnalyticContinuation>],
}

impl<T: MidiTiming + ?Sized> Copy for MidiInstrumentRenderContext<'_, T> {}

impl<T: MidiTiming + ?Sized> Clone for MidiInstrumentRenderContext<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: MidiTiming + ?Sized> MidiInstrumentRenderContext<'a, T> {
    pub fn new(launch: BeatTime, timing: &'a T, table: &'a SineTable) -> Self {
        Self::with_origin(SessionMidiOrigin::launch(launch), timing, table)
    }
    /// Keep timing and output span metadata on the global performance clock;
    /// only the elapsed source origin changes, with no historical-note chase.
    pub fn with_origin(origin: SessionMidiOrigin, timing: &'a T, _table: &'a SineTable) -> Self {
        Self {
            origin,
            timing,
            onset_floor_exclusive: None,
            suppressions: &[],
            continuations: &[],
        }
    }

    /// Skip source occurrences strictly before the handoff beat before converting
    /// them through an anchored clock. This prevents historical note onsets
    /// from underflowing when an edit shifts the effective tempo origin.
    pub fn after_onset(self, beat: BeatTime) -> Self {
        Self {
            onset_floor_exclusive: Some(beat),
            ..self
        }
    }

    pub(crate) fn with_continuations(
        self,
        continuations: &'a [Option<hex_lattice::AnalyticContinuation>],
    ) -> Self {
        Self {
            continuations,
            ..self
        }
    }

    pub(crate) fn with_suppressions(self, suppressions: &'a [MidiOccurrenceSuppression]) -> Self {
        Self {
            suppressions,
            ..self
        }
    }
}

/// Reusable fixed output storage for stateless MIDI span queries.
#[derive(Clone, Debug)]
pub struct MidiSpanBuffer {
    slots: [Option<MidiRenderSpan>; MIDI_SPAN_BUFFER_CAPACITY],
    len: usize,
}

impl Default for MidiSpanBuffer {
    fn default() -> Self {
        Self {
            slots: [None; MIDI_SPAN_BUFFER_CAPACITY],
            len: 0,
        }
    }
}

impl MidiSpanBuffer {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn spans(&self) -> impl Iterator<Item = &MidiRenderSpan> {
        self.slots[..self.len].iter().filter_map(Option::as_ref)
    }

    fn suppress_carried(&mut self, suppressions: &[MidiOccurrenceSuppression], frame: u64) {
        let mut write = 0;
        for read in 0..self.len {
            let Some(span) = self.slots[read] else {
                continue;
            };
            let suppressed = suppressions
                .binary_search_by(|entry| {
                    (entry.track, entry.clip, entry.note, entry.iteration).cmp(&(
                        span.track,
                        span.clip,
                        span.note,
                        span.iteration,
                    ))
                })
                .ok()
                .and_then(|index| suppressions.get(index))
                .is_some_and(|entry| entry.until_frame > frame);
            if !suppressed {
                self.slots[write] = Some(span);
                write += 1;
            }
        }
        for slot in &mut self.slots[write..self.len] {
            *slot = None;
        }
        self.len = write;
    }
}

impl PreparedMidiInstrumentTemplate {
    pub(crate) fn readmission_quote(&self) -> Result<(usize, usize), MidiTemplateError> {
        let spans = self
            .intro
            .len()
            .checked_add(self.repeats.len())
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let retained = std::mem::size_of::<Self>()
            .checked_add(
                spans
                    .checked_mul(
                        std::mem::size_of::<VoiceSpan>() + std::mem::size_of::<IntervalNode>(),
                    )
                    .ok_or(MidiTemplateError::ArithmeticOverflow)?,
            )
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        let scratch = density::current_sampler_workspace_quote(spans)
            .map_err(|_| MidiTemplateError::ArithmeticOverflow)?;
        Ok((retained, scratch))
    }

    pub(crate) fn retained_readmission_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.intro
                    .capacity()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.repeats
                    .capacity()
                    .checked_mul(std::mem::size_of::<VoiceSpan>())?,
            )?
            .checked_add(
                self.intro_nodes
                    .capacity()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )?
            .checked_add(
                self.repeat_nodes
                    .capacity()
                    .checked_mul(std::mem::size_of::<IntervalNode>())?,
            )
    }

    pub(crate) fn reprepare_for_source_on_control(
        &self,
        source: &PreparedMidiSourceTemplate,
        limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    ) -> Result<Self, MidiTemplateError> {
        let result = Self::prepare_with_bank_mode(
            source,
            std::sync::Arc::clone(&self.bank),
            std::sync::Arc::clone(&self.shared_contexts),
            self.event_source.then_some(self.release_frames),
            self.event_source,
            limits,
        )?;
        if result
            .retained_readmission_bytes()
            .is_none_or(|bytes| bytes > self.readmission_quote().map_or(0, |quote| quote.0))
        {
            return Err(MidiTemplateError::Allocation);
        }
        Ok(result)
    }
    /// Prepare original sine voices and interval indexes from an already
    /// admitted ongoing source template. Authored notes are never rescanned.
    pub fn prepare(
        source: &PreparedMidiSourceTemplate,
        table: &SineTable,
    ) -> Result<Self, MidiTemplateError> {
        let _ = table;
        let instrument = hex_devices::prepare_legacy_sine_instrument(source.sample_rate)
            .map_err(|_| MidiTemplateError::InvalidSource("sine instrument cannot be prepared"))?;
        Self::prepare_with_instrument(source, instrument.as_ref())
    }

    /// Prepare authored note spans through the selected registered instrument
    /// configuration. Each note retains its own bounded analytic sampler.
    pub fn prepare_with_instrument(
        source: &PreparedMidiSourceTemplate,
        instrument: &dyn PreparedNativeInstrument,
    ) -> Result<Self, MidiTemplateError> {
        let mut keys = crate::midi_note_bank::PreparedMidiNoteBank::empty_keys();
        for span in source.intro_spans.iter().chain(&source.repeat_spans) {
            crate::midi_note_bank::PreparedMidiNoteBank::add_key(
                &mut keys,
                span.pitch,
                span.velocity,
            )
            .map_err(|_| MidiTemplateError::InvalidSource("invalid midi note bank key"))?;
        }
        let bank =
            crate::midi_note_bank::PreparedMidiNoteBank::prepare(&mut keys, instrument, None, None)
                .map_err(|error| {
                    if error.0.contains("reserve") {
                        MidiTemplateError::Allocation
                    } else {
                        MidiTemplateError::InvalidSource(error.0)
                    }
                })?;
        Self::prepare_with_bank(source, bank)
    }

    /// Prepare source intervals by resolving compact note-bank indices. The
    /// caller owns a bank scoped to this track, state and sample rate.
    pub(crate) fn prepare_with_bank(
        source: &PreparedMidiSourceTemplate,
        bank: std::sync::Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
    ) -> Result<Self, MidiTemplateError> {
        Self::prepare_with_bank_and_contexts(
            source,
            bank,
            crate::shared_analytic::SharedAnalyticContextTable::empty(),
        )
    }

    pub(crate) fn prepare_with_bank_and_contexts(
        source: &PreparedMidiSourceTemplate,
        bank: std::sync::Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
        shared_contexts: std::sync::Arc<crate::shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<Self, MidiTemplateError> {
        Self::prepare_with_bank_mode(source, bank, shared_contexts, None, false, None)
    }

    /// Prepare a timing-only source for a mutable instrument event owner.
    /// This contains no analytic sampler and cannot be rendered by the legacy
    /// analytic path.
    pub(crate) fn prepare_events(
        source: &PreparedMidiSourceTemplate,
        limits: crate::midi_compile::MutableInstrumentLimits,
    ) -> Result<Self, MidiTemplateError> {
        let bank = crate::midi_note_bank::PreparedMidiNoteBank::empty();
        Self::prepare_with_bank_mode(
            source,
            bank,
            crate::shared_analytic::SharedAnalyticContextTable::empty(),
            Some(limits.max_release_tail_frames),
            true,
            Some(limits),
        )
    }

    pub(crate) fn is_event_source(&self) -> bool {
        self.event_source
    }

    fn prepare_with_bank_mode(
        source: &PreparedMidiSourceTemplate,
        bank: std::sync::Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
        shared_contexts: std::sync::Arc<crate::shared_analytic::SharedAnalyticContextTable>,
        event_release_frames: Option<u64>,
        event_source: bool,
        mutable_limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    ) -> Result<Self, MidiTemplateError> {
        let intro = prepare_spans(
            &source.intro_spans,
            source.sample_rate,
            source.admitted_max_micro_bpm,
            &bank,
            event_release_frames,
        )?;
        let repeats = prepare_spans(
            &source.repeat_spans,
            source.sample_rate,
            source.admitted_max_micro_bpm,
            &bank,
            event_release_frames,
        )?;
        let release_frames = event_release_frames.unwrap_or_else(|| {
            intro
                .iter()
                .chain(&repeats)
                .filter_map(|span| bank.sampler(span.sampler_index))
                .map(|sampler| sampler.release_frames())
                .max()
                .unwrap_or_else(|| u64::from(source.sample_rate / 20).max(1))
        });
        let release_raw = release_window_raw(
            release_frames,
            source.sample_rate,
            source.admitted_max_micro_bpm,
        )?;
        let mut intro = intro;
        let mut repeats = repeats;
        intro.sort_unstable_by_key(|span| (span.start, span.note));
        repeats.sort_unstable_by_key(|span| (span.start, span.note));
        if intro.len() > MIDI_SPAN_MAX_SOURCE_NOTES || repeats.len() > MIDI_SPAN_MAX_SOURCE_NOTES {
            return Err(MidiTemplateError::TooManySourceNotes);
        }
        let max_active_voices = admit_voice_density(
            &intro,
            &repeats,
            source.first_pass,
            source.period,
            release_raw,
        )?;
        let mut admitted_voices = max_active_voices;
        if let Some(limits) = mutable_limits {
            let max_events = limits
                .max_events_per_query
                .min(crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY);
            if source.max_query_edges() > max_events {
                return Err(MidiTemplateError::InvalidSource(
                    "mutable midi source exceeds instrument event capacity",
                ));
            }
            let max_voices = limits
                .max_active_occurrences
                .min(crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK);
            // A stealing instrument frees releasing slots on demand, so its
            // slots bound held density only; releases still bound the proof
            // ledger and span buffers through `max_active_voices`.
            if limits.steals_releasing_voices {
                admitted_voices =
                    admit_held_voice_density(&intro, &repeats, source.first_pass, source.period)?;
            }
            if admitted_voices > max_voices {
                return Err(MidiTemplateError::InvalidSource(
                    "mutable midi source exceeds instrument voice capacity",
                ));
            }
        }
        let max_query_onsets = source.max_query_onsets();
        let max_span_hits = max_active_voices
            .checked_add(max_query_onsets)
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        if max_span_hits > MIDI_SPAN_BUFFER_CAPACITY {
            return Err(MidiTemplateError::TooManySpans);
        }
        if let Some(period) = source.period {
            require_positive_frame_advance(
                period,
                source.sample_rate,
                source.admitted_max_micro_bpm,
            )
            .map_err(|_| MidiTemplateError::LoopPeriodTooShortForAdmission)?;
            if !repeats.is_empty() {
                require_period_query_bound(
                    period,
                    release_frames,
                    source.sample_rate,
                    source.admitted_max_micro_bpm,
                )?;
            }
        }
        let mut intro_nodes = Vec::new();
        intro_nodes
            .try_reserve_exact(intro.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        let intro_root = build_index(&intro, 0, intro.len(), &mut intro_nodes);
        let mut repeat_nodes = Vec::new();
        repeat_nodes
            .try_reserve_exact(repeats.len())
            .map_err(|_| MidiTemplateError::Allocation)?;
        let repeat_root = build_index(&repeats, 0, repeats.len(), &mut repeat_nodes);
        let has_sources = !intro.is_empty() || !repeats.is_empty();
        let max_period_windows = if repeats.is_empty() {
            0
        } else {
            usize::try_from(MIDI_SPAN_MAX_PERIOD_WINDOWS)
                .map_err(|_| MidiTemplateError::ArithmeticOverflow)?
        };
        Ok(Self {
            track: source.track,
            clip: source.clip,
            sample_rate: source.sample_rate,
            admitted_max_micro_bpm: source.admitted_max_micro_bpm,
            first_pass: source.first_pass,
            intro_iteration: source.intro_iteration,
            repeat_iteration: source.repeat_iteration,
            period: source.period,
            release_raw,
            intro,
            repeats,
            intro_nodes,
            repeat_nodes,
            intro_root,
            repeat_root,
            max_active_voices,
            admitted_voices,
            max_query_onsets,
            max_span_hits,
            max_query_visits: if has_sources {
                MIDI_SPAN_MAX_QUERY_VISITS
            } else {
                0
            },
            max_period_windows,
            bank,
            shared_contexts,
            release_frames,
            event_source,
        })
    }

    /// Prepare an explicit silent slot for a missing or bypassed instrument.
    pub fn prepare_silent(source: &PreparedMidiSourceTemplate) -> Result<Self, MidiTemplateError> {
        Self::prepare_with_instrument(source, &SilentInstrument)
    }

    pub fn max_active_voices(&self) -> usize {
        self.max_active_voices
    }

    /// Voice slots this template occupies in its admitted instrument: held
    /// density for an instrument that steals releasing voices, otherwise
    /// `max_active_voices` (releases included).
    pub(crate) fn admitted_voices(&self) -> usize {
        self.admitted_voices
    }

    pub(crate) fn note_bank(&self) -> &std::sync::Arc<crate::midi_note_bank::PreparedMidiNoteBank> {
        &self.bank
    }

    pub(crate) fn index_reference_count(&self) -> Result<usize, MidiTemplateError> {
        if self.event_source {
            return Ok(0);
        }
        self.intro
            .len()
            .checked_add(self.repeats.len())
            .ok_or(MidiTemplateError::ArithmeticOverflow)
    }

    pub fn max_query_onsets(&self) -> usize {
        self.max_query_onsets
    }

    /// Conservative bound on raw interval matches before exact frame filtering.
    pub fn max_span_hits(&self) -> usize {
        self.max_span_hits
    }

    /// Explicit per-query traversal guard ceiling; this is not a CPU budget.
    pub fn query_visit_ceiling(&self) -> usize {
        self.max_query_visits
    }

    /// Explicit repeat-window guard ceiling for this source.
    pub fn period_window_ceiling(&self) -> usize {
        self.max_period_windows
    }

    /// Fill caller storage with every gate/release interval intersecting the
    /// half-open frame window. Errors clear the visible span count.
    pub fn query<T: MidiTiming + ?Sized>(
        &self,
        launch: BeatTime,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        self.query_with_origin(
            SessionMidiOrigin::launch(launch),
            timing,
            start_frame,
            end_frame,
            buffer,
        )
    }

    /// Query spans from an inclusive elapsed cue without shifting MidiTiming.
    pub fn query_with_origin<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        self.query_with_onset_floor(origin, timing, start_frame, end_frame, buffer, None)
    }

    pub(crate) fn query_after_onset_with_origin<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
        onset_floor_exclusive: BeatTime,
    ) -> Result<(), MidiTemplateError> {
        self.query_with_onset_floor(
            origin,
            timing,
            start_frame,
            end_frame,
            buffer,
            Some(onset_floor_exclusive),
        )
    }

    fn query_with_onset_floor<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        start_frame: u64,
        end_frame: u64,
        buffer: &mut MidiSpanBuffer,
        onset_floor_exclusive: Option<BeatTime>,
    ) -> Result<(), MidiTemplateError> {
        buffer.len = 0;
        let result = self.query_view().query_inner(
            origin,
            timing,
            start_frame,
            end_frame,
            buffer,
            onset_floor_exclusive,
        );
        if result.is_err() {
            buffer.len = 0;
        }
        result
    }

    /// Overwrite a caller-owned output window with the exact existing
    /// sine-instrument render. A failed query zeros the output window.
    pub fn render<T: MidiTiming + ?Sized>(
        &self,
        launch: BeatTime,
        timing: &T,
        table: &SineTable,
        start_frame: u64,
        output: &mut [[f32; 2]],
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        self.render_with_origin(
            SessionMidiOrigin::launch(launch),
            timing,
            table,
            start_frame,
            output,
            buffer,
        )
    }

    /// Direct analytic render from a source-elapsed cue on global timing.
    pub fn render_with_origin<T: MidiTiming + ?Sized>(
        &self,
        origin: SessionMidiOrigin,
        timing: &T,
        _table: &SineTable,
        start_frame: u64,
        output: &mut [[f32; 2]],
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        if self.event_source {
            output.fill([0.0; 2]);
            buffer.len = 0;
            return Err(MidiTemplateError::MutableEventsNotAnalytic);
        }
        let width = match u64::try_from(output.len()) {
            Ok(width) => width,
            Err(_) => {
                output.fill([0.0; 2]);
                buffer.len = 0;
                return Err(MidiTemplateError::InvalidFrameRange);
            }
        };
        let Some(end) = start_frame.checked_add(width) else {
            output.fill([0.0; 2]);
            buffer.len = 0;
            return Err(MidiTemplateError::ArithmeticOverflow);
        };
        if let Err(error) = self.query_with_origin(origin, timing, start_frame, end, buffer) {
            output.fill([0.0; 2]);
            return Err(error);
        }
        output.fill([0.0; 2]);
        for (frame_offset, output_frame) in output.iter_mut().enumerate() {
            let frame = start_frame + frame_offset as u64;
            let mut sample = [0.0_f32; 2];
            for span in buffer.spans() {
                if frame >= span.start_frame && frame < span.end_frame {
                    let Some(voice) = span.voice else { continue };
                    let Some(contribution) = voice.sample(
                        &self.shared_contexts,
                        &[],
                        frame - span.start_frame,
                        span.gate_end_frame - span.start_frame,
                    ) else {
                        continue;
                    };
                    sample[0] += contribution[0];
                    sample[1] += contribution[1];
                }
            }
            *output_frame = sample;
        }
        Ok(())
    }

    /// Add this ongoing source to an existing mix using per-contribution track
    /// coefficients. The complete bounded query runs before output is touched;
    /// on failure the span count is cleared and the destination is unchanged.
    /// Muted tracks still validate timing and query bounds, then add nothing.
    /// No master stage is applied here.
    pub fn render_additive<T: MidiTiming + ?Sized>(
        &self,
        context: MidiInstrumentRenderContext<'_, T>,
        mix: PreparedTrackMix,
        start_frame: u64,
        output: &mut [[f32; 2]],
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        self.prepare_additive_spans(context, start_frame, output.len(), buffer)?;
        self.render_additive_spans(context, mix, start_frame, output, buffer, None)
    }

    /// Clip-owned gain uses the same effective global clock and source origin
    /// as the note query. Template-only public rendering deliberately stays unity.
    pub(crate) fn render_additive_with_volume<
        T: crate::prepared_tempo::MidiTempoBreaks + ?Sized,
    >(
        &self,
        context: MidiInstrumentRenderContext<'_, T>,
        mix: PreparedTrackMix,
        start_frame: u64,
        output: &mut [[f32; 2]],
        buffer: &mut MidiSpanBuffer,
        program: Option<&crate::midi_clip_volume::PreparedMidiClipVolume>,
    ) -> Result<(), MidiTemplateError> {
        self.prepare_additive_spans(context, start_frame, output.len(), buffer)?;
        if mix.muted() {
            return Ok(());
        }
        // Query only actual voice windows, before touching the destination.
        let gains = match program
            .map(|program| {
                volume::prepare_gains(
                    program,
                    context.origin,
                    context.timing,
                    start_frame,
                    output.len(),
                    buffer,
                )
            })
            .transpose()
        {
            Ok(gains) => gains.flatten(),
            Err(error) => {
                buffer.len = 0;
                return Err(error);
            }
        };
        self.render_additive_spans(context, mix, start_frame, output, buffer, gains.as_ref())
    }

    fn prepare_additive_spans<T: MidiTiming + ?Sized>(
        &self,
        context: MidiInstrumentRenderContext<'_, T>,
        start_frame: u64,
        frames: usize,
        buffer: &mut MidiSpanBuffer,
    ) -> Result<(), MidiTemplateError> {
        if self.event_source {
            buffer.len = 0;
            return Err(MidiTemplateError::MutableEventsNotAnalytic);
        }
        let width = match u64::try_from(frames) {
            Ok(width) => width,
            Err(_) => {
                buffer.len = 0;
                return Err(MidiTemplateError::InvalidFrameRange);
            }
        };
        let end_frame = match start_frame.checked_add(width) {
            Some(end) => end,
            None => {
                buffer.len = 0;
                return Err(MidiTemplateError::ArithmeticOverflow);
            }
        };
        self.query_with_onset_floor(
            context.origin,
            context.timing,
            start_frame,
            end_frame,
            buffer,
            context.onset_floor_exclusive,
        )?;
        buffer.suppress_carried(context.suppressions, start_frame);
        Ok(())
    }

    fn render_additive_spans<T: MidiTiming + ?Sized>(
        &self,
        context: MidiInstrumentRenderContext<'_, T>,
        mix: PreparedTrackMix,
        start_frame: u64,
        output: &mut [[f32; 2]],
        buffer: &MidiSpanBuffer,
        gains: Option<&[f64; MIDI_TEMPLATE_QUERY_FRAMES as usize]>,
    ) -> Result<(), MidiTemplateError> {
        if mix.muted() {
            return Ok(());
        }

        for (frame_offset, output_frame) in output.iter_mut().enumerate() {
            let frame = start_frame + frame_offset as u64;
            let gain = gains.map_or(1.0, |gains| gains[frame_offset]);
            for span in buffer.spans() {
                if frame >= span.start_frame && frame < span.end_frame {
                    let Some(voice) = span.voice else { continue };
                    let contribution = voice
                        .sample_at(
                            &self.shared_contexts,
                            &[],
                            context.continuations,
                            frame,
                            frame - span.start_frame,
                            span.gate_end_frame - span.start_frame,
                        )
                        .map_err(|_| {
                            MidiTemplateError::InvalidSource(
                                "analytic continuation sample was refused",
                            )
                        })?;
                    let contribution = if gain == 1.0 {
                        contribution
                    } else {
                        [
                            (f64::from(contribution[0]) * gain) as f32,
                            (f64::from(contribution[1]) * gain) as f32,
                        ]
                    };
                    mix.add_frame(output_frame, contribution);
                }
            }
        }
        Ok(())
    }
}

struct QueryContext<'a, T: MidiTiming + ?Sized> {
    origin: SessionMidiOrigin,
    timing: &'a T,
    start_frame: u64,
    end_frame: u64,
    buffer: &'a mut MidiSpanBuffer,
    visits: usize,
    onset_floor_exclusive: Option<u64>,
}

fn prepare_spans(
    source: &[SourceSpan],
    sample_rate: u32,
    admitted_max_micro_bpm: u64,
    bank: &crate::midi_note_bank::PreparedMidiNoteBank,
    event_release_frames: Option<u64>,
) -> Result<Vec<VoiceSpan>, MidiTemplateError> {
    if source.len() > MIDI_SPAN_MAX_SOURCE_NOTES {
        return Err(MidiTemplateError::TooManySourceNotes);
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(source.len())
        .map_err(|_| MidiTemplateError::Allocation)?;
    for span in source {
        let sampler_index = if event_release_frames.is_some() {
            0
        } else {
            bank.index(span.pitch, span.velocity)
                .ok_or(MidiTemplateError::InvalidSource(
                    "midi note sampler is missing from bank",
                ))?
        };
        let release_frames = if let Some(release_frames) = event_release_frames {
            release_frames
        } else {
            bank.sampler(sampler_index)
                .ok_or(MidiTemplateError::InvalidSource(
                    "midi note sampler index is invalid",
                ))?
                .release_frames()
        };
        let release_raw = release_window_raw(release_frames, sample_rate, admitted_max_micro_bpm)?;
        let raw_end = u128::from(span.gate_end)
            .checked_add(u128::from(release_raw))
            .ok_or(MidiTemplateError::ArithmeticOverflow)?;
        result.push(VoiceSpan {
            start: span.start,
            gate_end: span.gate_end,
            raw_end,
            note: span.note,
            pitch: span.pitch,
            channel: span.channel,
            velocity: span.velocity,
            sampler_index,
        });
    }
    Ok(result)
}

fn build_index(
    spans: &[VoiceSpan],
    start: usize,
    end: usize,
    nodes: &mut Vec<IntervalNode>,
) -> Option<usize> {
    if start == end {
        return None;
    }
    let middle = start + (end - start) / 2;
    let index = nodes.len();
    nodes.push(IntervalNode {
        span: middle,
        min_start: spans[middle].start,
        max_end: spans[middle].raw_end,
        left: None,
        right: None,
    });
    let left = build_index(spans, start, middle, nodes);
    let right = build_index(spans, middle + 1, end, nodes);
    let max_end = [
        Some(spans[middle].raw_end),
        left.map(|child| nodes[child].max_end),
        right.map(|child| nodes[child].max_end),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(spans[middle].raw_end);
    let min_start = [
        Some(spans[middle].start),
        left.map(|child| nodes[child].min_start),
        right.map(|child| nodes[child].min_start),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(spans[middle].start);
    nodes[index].left = left;
    nodes[index].right = right;
    nodes[index].min_start = min_start;
    nodes[index].max_end = max_end;
    Some(index)
}

fn release_window_raw(
    release_frames: u64,
    sample_rate: u32,
    max_micro_bpm: u64,
) -> Result<u64, MidiTemplateError> {
    let numerator = u128::from(release_frames)
        .checked_add(2)
        .and_then(|frames| frames.checked_mul(Q32))
        .and_then(|value| value.checked_mul(u128::from(max_micro_bpm)))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let denominator = FRAME_DENOMINATOR
        .checked_mul(u128::from(sample_rate))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let quotient = numerator / denominator;
    let rounded = quotient + u128::from(numerator % denominator != 0);
    u64::try_from(rounded).map_err(|_| MidiTemplateError::ArithmeticOverflow)
}

fn require_period_query_bound(
    period: u64,
    release_frames: u64,
    sample_rate: u32,
    max_micro_bpm: u64,
) -> Result<(), MidiTemplateError> {
    let required_frames = release_frames
        .checked_add(MIDI_TEMPLATE_QUERY_FRAMES + 2)
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let numerator = u128::from(required_frames)
        .checked_mul(Q32)
        .and_then(|value| value.checked_mul(u128::from(max_micro_bpm)))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let denominator = FRAME_DENOMINATOR
        .checked_mul(u128::from(sample_rate))
        .and_then(|value| value.checked_mul(MIDI_SPAN_MAX_PERIOD_WINDOWS - 2))
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let required_raw = numerator / denominator + u128::from(numerator % denominator != 0);
    if u128::from(period) <= required_raw + 1_024 {
        return Err(MidiTemplateError::LoopPeriodTooShortForSpanAdmission);
    }
    Ok(())
}

#[cfg(test)]
mod note_bank_lifetime_tests {
    use super::*;
    use crate::midi_note_bank::PreparedMidiNoteBank;
    use std::sync::{Arc, Weak};

    struct TestInstrument;

    impl PreparedNativeInstrument for TestInstrument {
        fn prepare_note(
            &self,
            _pitch: u8,
            _velocity: u8,
        ) -> Result<PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
            let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
            payload[hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS - 1] = u64::from(0.25_f32.to_bits());
            Ok(PreparedAnalyticNote::new(payload, 4, test_sample))
        }
    }

    fn test_sample(
        payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
        age: u64,
        gate: u64,
        release: u64,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> [f32; 2] {
        if age < gate.saturating_add(release) {
            [
                f32::from_bits(payload[hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS - 1] as u32),
                -0.5,
            ]
        } else {
            [0.0; 2]
        }
    }

    #[test]
    fn copied_release_voice_does_not_retain_its_note_bank() {
        let mut keys = PreparedMidiNoteBank::empty_keys();
        PreparedMidiNoteBank::add_key(&mut keys, 60, 100).unwrap();
        let bank = PreparedMidiNoteBank::prepare(&mut keys, &TestInstrument, None, None).unwrap();
        let sampler = bank.sampler(bank.index(60, 100).unwrap()).unwrap();
        let weak: Weak<PreparedMidiNoteBank> = Arc::downgrade(&bank);
        let release = MidiReleaseVoice::from_finite_span(0, 2, 6, sampler, 2, 2)
            .unwrap()
            .unwrap();
        drop(bank);
        assert!(weak.upgrade().is_none());
        assert_eq!(
            release.sample_at(
                3,
                &crate::shared_analytic::SharedAnalyticContextTable::default(),
                &[],
            ),
            Some([0.25, -0.5])
        );
    }
}
