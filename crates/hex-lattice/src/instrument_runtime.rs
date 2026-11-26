//! Optional bounded mutable note runtimes for instruments with voice history.
mod automation;
pub use automation::{InstrumentParameterRuntime, MAX_INSTRUMENT_AUTOMATION_FRAMES};

use super::StereoFrame;
use hex_lattice_contract::DeviceContractError;
use std::{any::Any, fmt, num::NonZeroU64};

/// Borrowed, immutable host musical clock. Q32.32 quarter-note beats at frame
/// centers, including the exclusive segment end. Implementations must return
/// deterministic, nondecreasing values throughout a preflight/process call.
/// No device owns this clock or constructs a project tempo map on the callback.
pub trait NativeInstrumentClock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64>;
    /// Exact local tempo at the content origin, in integer micro-BPM. This
    /// optional scalar is borrowed from the host map, including through latency
    /// compensation; devices must define an explicit unavailable-clock law.
    fn tempo_micro_bpm_at_offset(&self, _offset: usize) -> Option<u32> {
        None
    }
    /// Explicit Arrangement position in Q32.32 quarter-note beats, including
    /// loop normalization. Performance beats may continue across a loop and
    /// are not a substitute. Unknown/warmup timing remains explicit None.
    fn arrangement_beat_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    /// Optional exact content-frame origin and loop pass at an aligned input.
    /// None is explicit warmup/unavailable timing, never a guessed frame zero.
    fn arrangement_frame_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    /// Identity of the authored Arrangement frame coordinates at this
    /// origin. Changes only when that clock changes. Delayed content keeps
    /// the identity it entered with, including around tempo edits.
    fn authored_clock_epoch_at_offset(&self, _offset: usize) -> Option<NonZeroU64> {
        None
    }
    /// Exact Arrangement content origin eligible for authored foreign
    /// automation. Stopped monitoring, audition and uncompensated warmup
    /// return None even if they have a cursor or a musical clock.
    fn authored_arrangement_frame_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    fn loop_pass_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    /// Canonical time signature at the aligned Arrangement position.
    fn arrangement_meter_at_offset(&self, _offset: usize) -> Option<(u8, u8)> {
        None
    }
    /// Active Arrangement cycle bounds in Q32.32 quarter-note beats.
    fn arrangement_cycle_at_offset(&self, _offset: usize) -> Option<(u64, u64)> {
        None
    }
    /// Optional exact host performance-frame origin, independent of arrangement
    /// loop wraps. Unknown during warmup or stopped monitoring is explicit None.
    fn performance_frame_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    /// Transport state at the input origin. An advancing beat alone does not
    /// imply playback: stopped audition can have a running musical clock.
    fn transport_is_running(&self) -> Option<bool> {
        None
    }
    /// Optional block-level fast query: returns `Some((first_arrangement, first_performance))`
    /// only when `transport_is_running() == Some(true)` and every offset
    /// `first_offset + i` for `i in 0..len` has exact consecutive arrangement
    /// and performance frames `(first_arrangement + i, first_performance + i)`.
    fn contiguous_running_frames(&self, _first_offset: usize, _len: usize) -> Option<(u64, u64)> {
        None
    }
    /// Optional block-level fast query: returns `Some(tempo)` only when every
    /// offset `first_offset + i` for `i in 0..len` reports `Some(tempo)`.
    fn constant_tempo_micro_bpm(&self, _first_offset: usize, _len: usize) -> Option<u32> {
        None
    }
}

/// A controller value on an instrument's logical MIDI channel. Notes retain
/// their occurrence identity independently of this shared controller state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeInstrumentExpression {
    ChannelPressure(u8),
    PitchBend(i16),
    ModWheel(u8),
}
impl NativeInstrumentExpression {
    pub const fn is_valid(self) -> bool {
        match self {
            Self::ChannelPressure(value) | Self::ModWheel(value) => value <= 127,
            Self::PitchBend(value) => value >= -8192 && value <= 8191,
        }
    }
    /// Pressure, bipolar bend and wheel. MIDI's asymmetric bend endpoints map
    /// exactly to -1 and +1; center maps exactly to zero.
    pub fn normalized(self) -> f32 {
        match self {
            Self::ChannelPressure(value) | Self::ModWheel(value) => f32::from(value) / 127.0,
            Self::PitchBend(value) => f32::from(value) / if value < 0 { 8192.0 } else { 8191.0 },
        }
    }
}

/// Sample-offset note/controller event in caller-owned bounded block storage.
/// Equal offsets retain the caller's order. Per-note expression (`NotePressure`,
/// `NotePitchBend`, `NoteTimbre`) addresses a held occurrence, including an
/// onset earlier at the same offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeInstrumentEvent {
    Expression {
        frame_offset: usize,
        expression: NativeInstrumentExpression,
    },
    /// Channel-wide MIDI Control Change (`cc` 0..=127, `value` 0..=127).
    ControlChange {
        frame_offset: usize,
        cc: u8,
        value: u8,
    },
    /// Channel-wide 14-bit MIDI Pitch Bend (`-8192..=8191`, 0 = center).
    PitchBend { frame_offset: usize, value: i16 },
    NotePressure {
        frame_offset: usize,
        occurrence_id: NonZeroU64,
        value: u8,
    },
    /// Per-note MPE pitch offset in cents (`-4800..=4800`, 0 = unbent).
    NotePitchBend {
        frame_offset: usize,
        occurrence_id: NonZeroU64,
        cents: i16,
    },
    /// Per-note MPE slide / timbre (`0..=127`, CC74).
    NoteTimbre {
        frame_offset: usize,
        occurrence_id: NonZeroU64,
        value: u8,
    },
    NoteOn {
        frame_offset: usize,
        occurrence_id: u64,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        frame_offset: usize,
        occurrence_id: u64,
    },
}

impl NativeInstrumentEvent {
    pub const fn frame_offset(self) -> usize {
        match self {
            Self::NoteOn { frame_offset, .. }
            | Self::NoteOff { frame_offset, .. }
            | Self::NotePressure { frame_offset, .. }
            | Self::NotePitchBend { frame_offset, .. }
            | Self::NoteTimbre { frame_offset, .. }
            | Self::ControlChange { frame_offset, .. }
            | Self::PitchBend { frame_offset, .. }
            | Self::Expression { frame_offset, .. } => frame_offset,
        }
    }

    const fn occurrence_id(self) -> Option<u64> {
        match self {
            Self::NoteOn { occurrence_id, .. } | Self::NoteOff { occurrence_id, .. } => {
                Some(occurrence_id)
            }
            Self::NotePressure { occurrence_id, .. }
            | Self::NotePitchBend { occurrence_id, .. }
            | Self::NoteTimbre { occurrence_id, .. } => Some(occurrence_id.get()),
            Self::Expression { .. } | Self::ControlChange { .. } | Self::PitchBend { .. } => None,
        }
    }
}

/// A sounding shared analytic occurrence whose payload was prepared on control.
/// The host proves source/context/owner continuity; imports never synthesize
/// pitch or replay old audio on the callback. Gate age is note-relative.
#[derive(Clone, Copy, Debug)]
pub struct NativeInstrumentAnalyticSeed {
    pub occurrence_id: u64,
    pub note: super::PreparedSharedAnalyticNote,
    pub age_frames: u64,
    pub gate_frame: Option<u64>,
}
impl Default for NativeInstrumentAnalyticSeed {
    fn default() -> Self {
        Self {
            occurrence_id: 0,
            note: super::PreparedSharedAnalyticNote::new(
                [0; super::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
                0,
            ),
            age_frames: 0,
            gate_frame: None,
        }
    }
}

/// Finite ceilings supplied by the caller before either runtime twin is
/// constructed. The lattice contract deliberately does not copy engine voice,
/// onset or block-event limits into generic policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeInstrumentRuntimeLimits {
    /// Ceiling for the checked combined declaration of committed and scratch
    /// runtime storage.
    pub max_pair_bytes: usize,
    /// Maximum caller-supplied event count in one processed block or segment.
    pub max_events_per_block: usize,
    /// Maximum simultaneous occurrence identities supported by the runtime.
    pub max_active_occurrences: usize,
    /// Maximum declared frames until quiescence after the final gated note-off,
    /// including envelope release and internal output.
    pub max_tail_frames: u64,
}

/// Immutable control-prepared factory for a mutable instrument runtime.
/// `declared_twin_bytes` includes the fixed runtime object and all owned fixed
/// buffers for one twin. The pair checks its caller-supplied combined ceiling
/// before creating either twin. `create_runtime` must copy/own every immutable
/// value needed by sampling; the pair does not retain this factory, and its
/// eventual destruction belongs to the control side. `max_release_tail_frames`
/// is a finite conservative bound after the final gated note-off, including
/// envelope release and internal feedback/output state; the runtime must
/// become quiescent by that bound.
pub trait PreparedMutableInstrumentRuntime: Send + Sync {
    /// Exact sorted, unique parameter grants for the borrowed automation view.
    /// Empty keeps the existing unsupported path. Both actual twins must expose
    /// these same immutable descriptors for their entire lifetime.
    fn parameter_automation_descriptors(&self) -> &[hex_lattice_contract::ParameterDescriptor] {
        &[]
    }
    /// Exact sample-rate semantics for one parameter of this prepared instance.
    /// The parameter must also appear in the borrowed automation descriptors,
    /// and both actual twins must agree with the factory at admission.
    fn parameter_modulation_semantics(
        &self,
        _parameter: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        None
    }

    /// Immutable opt-in for exact read-only active occurrence IDs. Both actual
    /// twins must agree before the pair is admitted; the default is unsupported.
    fn supports_active_occurrence_ids(&self) -> bool {
        false
    }
    /// Both runtime twins must agree. Providers preflight pressure against
    /// held occurrences in event order before changing audio or history.
    fn supports_note_pressure(&self) -> bool {
        false
    }
    /// Immutable opt-in for borrowed per-occurrence output gain. The pair checks
    /// this against both actual twins before admission; default providers refuse
    /// nonempty gain batches while retaining their ordinary unity path.
    fn supports_occurrence_gain(&self) -> bool {
        false
    }
    /// Immutable opt-in that releasing (note-off'd) voices are stealable. A
    /// note-on that finds all `max_active_occurrences` slots occupied then
    /// takes the slot of the runtime's oldest releasing occurrence instead of
    /// refusing; it still refuses with `EventCapacityExceeded` when every slot
    /// is held. A steal never hard-cuts a sounding voice: if the provider's
    /// fixed declick storage is exhausted, it drops the incoming onset instead
    /// (that occurrence never becomes active and its NoteOff is ignored), and
    /// the rest of the block, including every NoteOff, still applies. The stolen occurrence leaves the active set at that frame:
    /// `active_occurrence_ids` no longer reports it, it does not count toward
    /// `max_active_occurrences`, it ignores later events, and hosts must not
    /// send it occurrence gains (its gain is frozen at the steal frame). Its
    /// audio continues in provider-owned fixed storage with a bounded declick
    /// fade and never outlasts `max_release_tail_frames` after its note-off.
    /// Hosts may therefore admit note density by held voices and must take
    /// per-occurrence gains from `active_occurrence_ids`, not their own
    /// release deadlines. The default keeps the strict release-occupies-a-slot
    /// contract.
    fn steals_releasing_voices(&self) -> bool {
        false
    }
    /// Opt-in declaration that the immutable factory owns only its inline Box payload, with no
    /// owned heap backing. The host cross-checks this against dynamic `size_of_val` for resource
    /// admission. Default unknown; twin-runtime storage never proves factory storage ownership.
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        None
    }
    /// Fixed algorithmic buffering latency at this prepared sample rate.
    /// Unknown is explicit; envelope attack, filter phase and musical release
    /// are not buffering latency. A mutable factory must agree with its source.
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        None
    }

    /// Opt-in, allocation-free exact source-law authorization. Other providers
    /// retain the no-chase policy. The host also proves source/route identity.
    fn can_import_analytic_context(
        &self,
        _source: &dyn super::PreparedSharedAnalyticContext,
    ) -> bool {
        false
    }
    fn runtime_version(&self) -> u32;
    fn declared_twin_bytes(&self) -> usize;
    fn max_events_per_block(&self) -> usize;
    fn max_active_occurrences(&self) -> usize;
    fn max_release_tail_frames(&self) -> u64;
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime>;
    fn as_any(&self) -> &dyn Any;

    /// Prepare an opt-in proof for this exact target configuration. The proof
    /// is source-independent: at adoption, the runtime checks the actual
    /// committed source law and derives the ramp from its current state. This
    /// allows skipped canonical revisions without weakening source validation.
    /// Preparation runs on control and returned proof storage must be bounded.
    /// The default preserves exact-copy and detached-tail behavior.
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Unchanged)
    }
}

/// Control-side admission policy authored by one candidate target factory.
/// Applicability to the actual committed source is checked at handoff, not
/// inferred from a potentially skipped canonical predecessor.
pub enum InstrumentHistoryRetargetPreparation {
    /// Preserve the existing exact-copy and detached-tail compatibility policy.
    Unchanged,
    /// A bounded proof authorizes this exact candidate target for a supported
    /// retarget law; the runtime validates the actual source at handoff.
    Prepared(Box<dyn PreparedInstrumentHistoryRetarget>),
    /// This target cannot retarget active history; replacing it requires the
    /// old same-owner runtime to be quiescent.
    RequiresQuiescence,
}

/// Opaque immutable control-prepared authorization for one precise mutable
/// instrument history transition. The candidate graph owns the proof and keeps
/// it alive until that graph is retired on control. The declared payload size
/// excludes allocator bookkeeping.
pub trait PreparedInstrumentHistoryRetarget: Any + Send + Sync {
    /// An explicit provider proof may bridge two formats after validating every
    /// source field. Default implementations retain exact-version matching.
    fn allows_runtime_versions(&self, source: u32, target: u32) -> bool {
        source == target
    }
    fn declared_fixed_storage_bytes(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
}

/// Actual source context and independent transition boundary, together with
/// sounding notes. Providers validate this before either candidate twin changes.
pub struct AnalyticInstrumentHistory<'a> {
    pub source: &'a dyn super::PreparedSharedAnalyticContext,
    pub source_id: crate::AnalyticContinuationId,
    pub transition: Option<crate::AnalyticContinuation>,
    pub performance_frame: u64,
    pub voices: &'a [NativeInstrumentAnalyticSeed],
}

/// Mutable per-note state. Compatibility must include every immutable law
/// parameter that affects future samples, as well as runtime version and any
/// implementation-specific rate/configuration key. Compatibility must be
/// invariant for the runtime's lifetime: `can_copy_history_from` may not
/// change its answer after processing or reset. Compatibility must also include
/// layout and capacity when they affect safe history transfer. Preflight must
/// be read-only and verify event semantics against current active occurrences
/// and fixed capacity. Once it succeeds, `process_block`, history copies and
/// reset must be infallible,
/// allocation-free and bounded by the declared event and occurrence limits.
pub trait MutableInstrumentRuntime: Any + Send {
    /// Optional borrowed view into this same voice/runtime owner. Its immutable
    /// grants must match the prepared factory; no new resource custody is created.
    fn parameter_automation(&mut self) -> Option<&mut dyn InstrumentParameterRuntime> {
        None
    }

    /// Optional read-only view of the same parameter owner for complete canonical
    /// target snapshots. Both twins must opt in with immutable factory-matching
    /// grants and support bounded copying throughout reset/history transfer.
    /// No voice history, resource custody or future program evaluation is added.
    fn parameter_automation_readonly(&self) -> Option<&dyn InstrumentParameterRuntime> {
        None
    }

    /// Must agree with the prepared factory and remain invariant for this runtime.
    fn supports_active_occurrence_ids(&self) -> bool {
        false
    }
    /// An immutable grant matching the factory. Preflight refuses unknown,
    /// released and retired occurrences before processing any event.
    fn supports_note_pressure(&self) -> bool {
        false
    }
    /// Report exact live occurrence identities, including released voices that
    /// have not retired. No host deadline or whole-runtime quiescence estimate
    /// substitutes for this provider-owned observation. Write a strictly ascending,
    /// nonzero, unique prefix and return its length. A short buffer must refuse
    /// without truncation. The pair bounds output by its admitted occurrence cap.
    /// On any error all output is unspecified and must be ignored. This read-only
    /// operation must allocate/free nothing and retain no caller storage/resources.
    fn active_occurrence_ids(
        &self,
        _output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        Err(NativeInstrumentRuntimeError::UnsupportedActiveOccurrenceIds)
    }
    /// Must agree with the prepared factory and remain invariant for this runtime.
    fn supports_occurrence_gain(&self) -> bool {
        false
    }
    /// Read-only preflight against the current speculative voice/event state.
    /// Generic sorted-ID, gain range and complete segment bounds are already
    /// checked by the pair. Providers must reject unknown occurrence identities.
    fn preflight_occurrence_gains(
        &self,
        _events: &[NativeInstrumentEvent],
        gains: &[crate::NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if gains.is_empty() {
            Ok(())
        } else {
            Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain)
        }
    }
    /// Process only after complete event/clock/gain preflight. Empty gain batches
    /// use the ordinary clock path. Nonempty support must be explicitly declared
    /// and implemented; gain is applied per occurrence before shared summing.
    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _gains: &[crate::NativeInstrumentOccurrenceGain],
        resources: &dyn crate::DeviceResources,
    ) {
        self.process_block_with_clock(events, frames, clock, resources);
    }
    fn can_import_analytic_history(&self, _history: &AnalyticInstrumentHistory<'_>) -> bool {
        false
    }
    /// Inactive candidate only, after both twins and the whole graph preflight.
    fn import_analytic_history(&mut self, _history: &AnalyticInstrumentHistory<'_>) {}

    fn can_import_analytic(&self, _notes: &[NativeInstrumentAnalyticSeed]) -> bool {
        false
    }
    /// Apply only after both twins pass preflight; no allocation or final drop.
    fn import_analytic(&mut self, _notes: &[NativeInstrumentAnalyticSeed]) {}
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError>;

    /// Apply a previously preflighted event slice and add generated audio to
    /// `frames`. Events at an equal frame offset are observed in slice order.
    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    );

    /// Opt-in musical processing; legacy devices retain their existing path.
    fn preflight_clock(
        &self,
        _clock: Option<&dyn NativeInstrumentClock>,
        _frames: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        self.process_block(events, frames, _resources);
    }

    fn reset(&mut self);
    /// True only when no existing note or internal state can produce future
    /// output without another event. The runtime must satisfy its declared
    /// release-tail bound after the final note-off.
    fn is_quiescent(&self) -> bool;
    /// Whether all state in `previous` can be copied with equivalent future
    /// samples under this runtime's immutable configuration. This includes
    /// gated and released voices, envelopes, phase, feedback/filter memory,
    /// random-generator state, and any other state that can affect output.
    /// The answer must remain invariant for each runtime's lifetime.
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool;
    /// Copy the complete future-sample state from a compatible runtime.
    /// After copying, processing the same events with the same frame
    /// partitioning must produce sample-identical output. Implementations must
    /// include held/released voices and every envelope, phase, feedback and
    /// random-generator state value; this operation is bounded and alloc-free.
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime);
    /// Read-only opt-in preflight for the exact proof prepared by this
    /// candidate's factory. It must validate complete source and target laws,
    /// every carried voice and all bounded runtime state.
    fn can_retarget_history_from(
        &self,
        _previous: &dyn MutableInstrumentRuntime,
        _proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        false
    }
    /// Apply a previously preflighted history retarget. Both twins must have
    /// passed `can_retarget_history_from` before either is changed. This is
    /// infallible, bounded and allocation-free.
    fn retarget_history_from(
        &mut self,
        _previous: &dyn MutableInstrumentRuntime,
        _proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
    }
    fn fixed_storage_bytes(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
}

/// Errors found before mutable runtime state or caller PCM is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeInstrumentRuntimeError {
    UnsupportedActiveOccurrenceIds,
    ActiveOccurrenceCapacityExceeded,
    InvalidActiveOccurrenceIds,
    InvalidOccurrenceGain,
    UnsupportedOccurrenceGain,
    UnsupportedNotePressure,
    UnsupportedParameterAutomation,
    UnsupportedParameterModulation,
    InvalidParameterAutomation,
    RuntimeBudgetExceeded,
    InvalidEventBatch,
    EventCapacityExceeded,
    IncompatibleHistory,
    IncompatibleTwins,
    BlockNotStarted,
    ResourceUnavailable,
}

impl NativeInstrumentRuntimeError {
    /// Allocation-free cause for callback diagnostics.
    pub fn diagnostic_message(self) -> &'static str {
        match self {
            Self::UnsupportedActiveOccurrenceIds => {
                "native instrument active occurrence ids are unsupported"
            }
            Self::ActiveOccurrenceCapacityExceeded => {
                "native instrument active occurrence output capacity is exceeded"
            }
            Self::InvalidActiveOccurrenceIds => {
                "native instrument active occurrence ids are invalid"
            }
            Self::InvalidOccurrenceGain => "native instrument occurrence gain is invalid",
            Self::UnsupportedOccurrenceGain => "native instrument occurrence gain is unsupported",
            Self::UnsupportedNotePressure => "native instrument note pressure is unsupported",
            Self::UnsupportedParameterModulation => {
                "native instrument sample-rate modulation is unsupported"
            }
            Self::UnsupportedParameterAutomation => {
                "native instrument parameter automation is unsupported"
            }
            Self::InvalidParameterAutomation => "native instrument parameter automation is invalid",
            Self::RuntimeBudgetExceeded => "native instrument runtime exceeds caller limits",
            Self::InvalidEventBatch => "native instrument event batch is invalid",
            Self::EventCapacityExceeded => "native instrument event or voice capacity is exceeded",
            Self::IncompatibleHistory => "native instrument runtime histories are incompatible",
            Self::IncompatibleTwins => "native instrument runtime twins are incompatible",
            Self::BlockNotStarted => "native instrument runtime block was not started",
            Self::ResourceUnavailable => "native instrument resource inventory is unavailable",
        }
    }
}

impl fmt::Display for NativeInstrumentRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_message())
    }
}

impl std::error::Error for NativeInstrumentRuntimeError {}

mod pair;
pub use pair::NativeInstrumentRuntimePair;
pub(super) use pair::validate_event_batch;

#[cfg(test)]
mod tests;
