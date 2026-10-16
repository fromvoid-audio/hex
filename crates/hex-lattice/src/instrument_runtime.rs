//! Optional bounded mutable note runtimes for instruments with voice history.
use super::StereoFrame;
use hex_lattice_contract::DeviceContractError;
use std::{any::Any, fmt};

/// Borrowed, immutable host musical clock. Q32.32 quarter-note beats at frame
/// centers, including the exclusive segment end. Implementations must return
/// deterministic, nondecreasing values throughout a preflight/process call.
/// No device owns this clock or constructs a project tempo map on the callback.
pub trait NativeInstrumentClock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64>;
    /// Optional exact content-frame origin and loop pass at an aligned input.
    /// None is explicit warmup/unavailable timing, never a guessed frame zero.
    fn arrangement_frame_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    fn loop_pass_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
}

/// One note-on or note-off event in caller-owned block storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeInstrumentEvent {
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
    const fn frame_offset(self) -> usize {
        match self {
            Self::NoteOn { frame_offset, .. } | Self::NoteOff { frame_offset, .. } => frame_offset,
        }
    }

    const fn occurrence_id(self) -> u64 {
        match self {
            Self::NoteOn { occurrence_id, .. } | Self::NoteOff { occurrence_id, .. } => {
                occurrence_id
            }
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
    ) -> Result<(), NativeInstrumentRuntimeError>;

    /// Apply a previously preflighted event slice and add generated audio to
    /// `frames`. Events at an equal frame offset are observed in slice order.
    fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]);

    /// Opt-in musical processing; legacy devices retain their existing path.
    fn preflight_clock(
        &self,
        _clock: Option<&dyn NativeInstrumentClock>,
        _frames: usize,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Ok(())
    }
    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
    ) {
        self.process_block(events, frames);
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
    RuntimeBudgetExceeded,
    InvalidEventBatch,
    EventCapacityExceeded,
    IncompatibleHistory,
    IncompatibleTwins,
    BlockNotStarted,
}

impl NativeInstrumentRuntimeError {
    /// Allocation-free cause for callback diagnostics.
    pub fn diagnostic_message(self) -> &'static str {
        match self {
            Self::RuntimeBudgetExceeded => "native instrument runtime exceeds caller limits",
            Self::InvalidEventBatch => "native instrument event batch is invalid",
            Self::EventCapacityExceeded => "native instrument event or voice capacity is exceeded",
            Self::IncompatibleHistory => "native instrument runtime histories are incompatible",
            Self::IncompatibleTwins => "native instrument runtime twins are incompatible",
            Self::BlockNotStarted => "native instrument runtime block was not started",
        }
    }
}

impl fmt::Display for NativeInstrumentRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_message())
    }
}

impl std::error::Error for NativeInstrumentRuntimeError {}

/// Paired mutable runtime with committed primary history and speculative
/// scratch history. Construction, compatibility decisions and final retirement
/// belong on the control side; processing only borrows caller-owned event/PCM
/// slices and performs no allocation or final drop.
pub struct NativeInstrumentRuntimePair {
    runtime_version: u32,
    declared_pair_bytes: usize,
    max_events_per_block: usize,
    max_active_occurrences: usize,
    max_release_tail_frames: u64,
    primary: Box<dyn MutableInstrumentRuntime>,
    scratch: Box<dyn MutableInstrumentRuntime>,
    block_started: bool,
}

impl NativeInstrumentRuntimePair {
    /// Validate declarations against explicit caller ceilings before asking
    /// the implementation to allocate either committed/scratch twin.
    pub fn new(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        let runtime_version = prepared.runtime_version();
        let declared_twin_bytes = prepared.declared_twin_bytes();
        let max_events_per_block = prepared.max_events_per_block();
        let max_active_occurrences = prepared.max_active_occurrences();
        let max_release_tail_frames = prepared.max_release_tail_frames();
        let declared_pair_bytes = declared_twin_bytes
            .checked_mul(2)
            .ok_or(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)?;
        if runtime_version == 0
            || declared_twin_bytes == 0
            || declared_pair_bytes > limits.max_pair_bytes
            || max_events_per_block == 0
            || max_events_per_block > limits.max_events_per_block
            || max_active_occurrences == 0
            || max_active_occurrences > limits.max_active_occurrences
            || max_release_tail_frames > limits.max_tail_frames
        {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }

        let primary = prepared.create_runtime();
        let scratch = prepared.create_runtime();
        if !primary.can_copy_history_from(scratch.as_ref())
            || !scratch.can_copy_history_from(primary.as_ref())
        {
            return Err(NativeInstrumentRuntimeError::IncompatibleTwins);
        }
        if primary.fixed_storage_bytes() > declared_twin_bytes
            || scratch.fixed_storage_bytes() > declared_twin_bytes
        {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }

        Ok(Self {
            runtime_version,
            declared_pair_bytes,
            max_events_per_block,
            max_active_occurrences,
            max_release_tail_frames,
            primary,
            scratch,
            block_started: false,
        })
    }

    pub const fn runtime_version(&self) -> u32 {
        self.runtime_version
    }

    /// Checked combined declaration for both fixed runtime twins.
    pub const fn declared_pair_bytes(&self) -> usize {
        self.declared_pair_bytes
    }

    pub const fn max_events_per_block(&self) -> usize {
        self.max_events_per_block
    }

    pub const fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    pub const fn max_release_tail_frames(&self) -> u64 {
        self.max_release_tail_frames
    }

    /// Quiescence of committed state only. An uncommitted block is never
    /// eligible for retirement even if its primary twin is quiet.
    pub fn committed_is_quiescent(&self) -> bool {
        !self.block_started && self.primary.is_quiescent()
    }

    /// Quiescence of the current speculative history inside an open block.
    /// This is used only to preflight a route boundary before rendering its
    /// next source segment; it never observes or mutates committed history.
    pub fn speculative_is_quiescent(&self) -> bool {
        self.block_started && self.scratch.is_quiescent()
    }

    /// Whether committed history is available for a control-side checkpoint.
    /// An open speculative block is never observable or checkpointable.
    pub const fn committed_state_available(&self) -> bool {
        !self.block_started
    }

    /// Measured sum reported by both runtime implementations, excluding
    /// allocator bookkeeping. Implementations remain responsible for honest
    /// accounting of their fixed internal storage.
    pub fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        self.primary
            .fixed_storage_bytes()
            .checked_add(self.scratch.fixed_storage_bytes())
    }

    /// Begin a speculative block by restoring scratch from committed primary.
    pub fn begin_block(&mut self) {
        self.scratch.copy_history_from(self.primary.as_ref());
        self.block_started = true;
    }

    /// Validate and process one block/segment. Generic ordering and payload
    /// checks plus implementation-specific active-ID/capacity preflight all
    /// complete before either `frames` or scratch history is changed.
    pub fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.process_block_with_clock(events, frames, None)
    }

    /// Clock-aware transactional segment. All clock/event preflight completes
    /// before the scratch runtime or PCM is changed.
    pub fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.block_started {
            return Err(NativeInstrumentRuntimeError::BlockNotStarted);
        }
        validate_event_batch(events, frames.len(), self.max_events_per_block)?;
        self.scratch.preflight_events(events, frames.len())?;
        self.scratch.preflight_clock(clock, frames.len())?;
        self.scratch.process_block_with_clock(events, frames, clock);
        Ok(())
    }

    /// Commit the speculative state after the complete caller transaction
    /// succeeds. Calling without `begin_block` is a harmless no-op.
    pub fn commit_block(&mut self) {
        if self.block_started {
            std::mem::swap(&mut self.primary, &mut self.scratch);
            self.block_started = false;
        }
    }

    /// Abandon speculative history after a later render stage fails. The next
    /// `begin_block` refreshes scratch from the unchanged committed primary.
    pub fn rollback_block(&mut self) {
        self.block_started = false;
    }

    /// Hard-reset both twins. This API intentionally defines no chase/replay;
    /// discontinuity policy remains the caller's responsibility.
    pub fn reset(&mut self) {
        self.primary.reset();
        self.scratch.reset();
        self.block_started = false;
    }

    /// Test compatibility before copying any history. Identity, sample rate,
    /// device state and route policy are caller-owned and must be checked by a
    /// future compiler before invoking this type-level runtime comparison.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        !self.block_started
            && !previous.block_started
            && self.runtime_version == previous.runtime_version
            && self
                .primary
                .can_copy_history_from(previous.primary.as_ref())
            && self
                .scratch
                .can_copy_history_from(previous.primary.as_ref())
    }

    /// Copy the complete old committed future-sample state to both candidate
    /// twins only after both compatibility checks have passed. The source and
    /// destination remain unchanged on refusal.
    pub fn inherit_from(&mut self, previous: &Self) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_inherit_from(previous) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.primary.copy_history_from(previous.primary.as_ref());
        self.scratch.copy_history_from(previous.primary.as_ref());
        Ok(())
    }

    pub fn can_import_analytic_history(&self, history: &AnalyticInstrumentHistory<'_>) -> bool {
        !self.block_started
            && self.primary.is_quiescent()
            && self.scratch.is_quiescent()
            && history.voices.len() <= self.max_active_occurrences
            && self.primary.can_import_analytic_history(history)
            && self.scratch.can_import_analytic_history(history)
    }
    pub fn import_analytic_history(
        &mut self,
        history: &AnalyticInstrumentHistory<'_>,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_import_analytic_history(history) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.primary.import_analytic_history(history);
        self.scratch.import_analytic_history(history);
        Ok(())
    }
    pub fn can_import_analytic(&self, notes: &[NativeInstrumentAnalyticSeed]) -> bool {
        !self.block_started
            && notes.len() <= self.max_active_occurrences
            && self.primary.is_quiescent()
            && self.scratch.is_quiescent()
            && self.primary.can_import_analytic(notes)
            && self.scratch.can_import_analytic(notes)
    }
    pub fn import_analytic(
        &mut self,
        notes: &[NativeInstrumentAnalyticSeed],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_import_analytic(notes) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.primary.import_analytic(notes);
        self.scratch.import_analytic(notes);
        Ok(())
    }

    /// Preflight an explicit prepared retarget of both runtime twins. Exact
    /// `can_inherit_from` semantics remain unchanged; candidate topology and
    /// source identity are checked by the owning engine graph.
    pub fn can_retarget_from(
        &self,
        previous: &Self,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        !self.block_started
            && !previous.block_started
            && proof.allows_runtime_versions(previous.runtime_version, self.runtime_version)
            && self
                .primary
                .can_retarget_history_from(previous.primary.as_ref(), proof)
            && self
                .scratch
                .can_retarget_history_from(previous.primary.as_ref(), proof)
    }

    /// Apply a retarget only after both twins pass the complete preflight.
    pub fn retarget_from(
        &mut self,
        previous: &Self,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_retarget_from(previous, proof) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.primary
            .retarget_history_from(previous.primary.as_ref(), proof);
        self.scratch
            .retarget_history_from(previous.primary.as_ref(), proof);
        Ok(())
    }
}

pub(super) fn validate_event_batch(
    events: &[NativeInstrumentEvent],
    frame_count: usize,
    max_events: usize,
) -> Result<(), NativeInstrumentRuntimeError> {
    if events.len() > max_events {
        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
    }
    let mut previous_offset = 0;
    for (index, event) in events.iter().copied().enumerate() {
        let offset = event.frame_offset();
        if offset >= frame_count
            || (index > 0 && offset < previous_offset)
            || event.occurrence_id() == 0
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::NoteOn {
            occurrence_id,
            pitch,
            velocity,
            ..
        } = event
        {
            if pitch > 127
                || velocity == 0
                || velocity > 127
                || events[..index].iter().any(|prior| {
                    matches!(
                        prior,
                        NativeInstrumentEvent::NoteOn { occurrence_id: prior_id, .. }
                            if *prior_id == occurrence_id
                    )
                })
            {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
        }
        previous_offset = offset;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ANALYTIC_NOTE_PAYLOAD_WORDS, PreparedAnalyticNote, PreparedNativeInstrument};
    use hex_lattice_contract::DeviceContractError;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const TEST_LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 4_096,
        max_events_per_block: 8,
        max_active_occurrences: 2,
        max_tail_frames: 16,
    };

    #[derive(Clone, Copy, Default)]
    struct Voice {
        occurrence_id: u64,
        pitch: u8,
        velocity: u8,
        phase: f32,
        release_left: u8,
        active: bool,
        gated: bool,
    }

    struct FeedbackPrepared {
        compatibility_key: u8,
        release_frames: u8,
        max_tail_frames: u64,
        max_events: usize,
        max_voices: usize,
        declared_bytes: usize,
        creations: Arc<AtomicUsize>,
        alternate_twin_key: bool,
    }

    struct FeedbackRuntime {
        compatibility_key: u8,
        release_frames: u8,
        max_tail_frames: u64,
        tail_frames_remaining: u64,
        max_voices: usize,
        voices: [Voice; 2],
        feedback: f32,
    }

    impl FeedbackRuntime {
        fn apply_event(&mut self, event: NativeInstrumentEvent) {
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    velocity,
                    ..
                } => {
                    if let Some(voice) = self
                        .voices
                        .get_mut(..self.max_voices)
                        .and_then(|voices| voices.iter_mut().find(|voice| !voice.active))
                    {
                        *voice = Voice {
                            occurrence_id,
                            pitch,
                            velocity,
                            phase: 0.0,
                            release_left: self.release_frames,
                            active: true,
                            gated: true,
                        };
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(voice) = self
                        .voices
                        .iter_mut()
                        .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                    {
                        voice.gated = false;
                        voice.release_left = self.release_frames;
                        if self.release_frames == 0 {
                            *voice = Voice::default();
                            if self.voices.iter().all(|voice| !voice.active) {
                                self.tail_frames_remaining = self.max_tail_frames;
                            }
                        }
                    }
                }
            }
        }

        fn advance_release(voice: &mut Voice) {
            if voice.active && !voice.gated {
                voice.release_left = voice.release_left.saturating_sub(1);
                if voice.release_left == 0 {
                    *voice = Voice::default();
                }
            }
        }

        fn advance_many(voices: &mut [Voice; 2], frames: usize) {
            for _ in 0..frames {
                for voice in voices.iter_mut() {
                    Self::advance_release(voice);
                }
            }
        }
    }

    impl PreparedMutableInstrumentRuntime for FeedbackPrepared {
        fn runtime_version(&self) -> u32 {
            1
        }

        fn declared_twin_bytes(&self) -> usize {
            self.declared_bytes
        }

        fn max_events_per_block(&self) -> usize {
            self.max_events
        }

        fn max_active_occurrences(&self) -> usize {
            self.max_voices
        }

        fn max_release_tail_frames(&self) -> u64 {
            u64::from(self.release_frames).saturating_add(self.max_tail_frames)
        }

        fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
            let ordinal = self.creations.fetch_add(1, Ordering::Relaxed);
            Box::new(FeedbackRuntime {
                compatibility_key: if self.alternate_twin_key {
                    self.compatibility_key.wrapping_add(ordinal as u8)
                } else {
                    self.compatibility_key
                },
                release_frames: self.release_frames,
                max_tail_frames: self.max_tail_frames,
                tail_frames_remaining: 0,
                max_voices: self.max_voices,
                voices: [Voice::default(); 2],
                feedback: 0.0,
            })
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    impl MutableInstrumentRuntime for FeedbackRuntime {
        fn preflight_events(
            &self,
            events: &[NativeInstrumentEvent],
            frame_count: usize,
        ) -> Result<(), NativeInstrumentRuntimeError> {
            let mut voices = self.voices;
            let mut previous_offset = 0;
            for event in events.iter().copied() {
                let offset = event.frame_offset();
                Self::advance_many(&mut voices, offset - previous_offset);
                match event {
                    NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                        let Some(active_voices) = voices.get_mut(..self.max_voices) else {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        };
                        if active_voices
                            .iter()
                            .any(|voice| voice.active && voice.occurrence_id == occurrence_id)
                        {
                            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                        }
                        let Some(slot) = active_voices.iter_mut().find(|voice| !voice.active)
                        else {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        };
                        *slot = Voice {
                            occurrence_id,
                            active: true,
                            gated: true,
                            release_left: self.release_frames,
                            ..Voice::default()
                        };
                    }
                    NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                        let Some(voice) = voices
                            .iter_mut()
                            .find(|voice| voice.active && voice.occurrence_id == occurrence_id)
                        else {
                            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                        };
                        if !voice.gated {
                            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                        }
                        voice.gated = false;
                        voice.release_left = self.release_frames;
                        if self.release_frames == 0 {
                            *voice = Voice::default();
                        }
                    }
                }
                previous_offset = offset;
            }
            let _ = frame_count;
            Ok(())
        }

        fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
            let mut event_index = 0;
            for (frame_index, frame) in frames.iter_mut().enumerate() {
                while events
                    .get(event_index)
                    .is_some_and(|event| event.frame_offset() == frame_index)
                {
                    self.apply_event(events[event_index]);
                    event_index += 1;
                }

                let had_active_voice = self.voices.iter().any(|voice| voice.active);
                let mut excitation = 0.0_f32;
                for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                    let level = if voice.gated {
                        f32::from(voice.velocity) / 127.0
                    } else {
                        f32::from(voice.release_left) / f32::from(self.release_frames.max(1))
                    };
                    excitation += voice.phase * level;
                    voice.phase += f32::from(voice.pitch + 1) * 0.000_1;
                    Self::advance_release(voice);
                }
                let has_active_voice = self.voices.iter().any(|voice| voice.active);
                if had_active_voice && !has_active_voice {
                    self.tail_frames_remaining = self.max_tail_frames;
                }
                self.feedback = self.feedback * 0.5 + excitation * 0.25;
                if !has_active_voice && self.tail_frames_remaining == 0 {
                    self.feedback = 0.0;
                }
                frame[0] += self.feedback;
                frame[1] -= self.feedback * 0.5;
                if !has_active_voice && self.tail_frames_remaining > 0 {
                    self.tail_frames_remaining -= 1;
                    if self.tail_frames_remaining == 0 {
                        self.feedback = 0.0;
                    }
                }
            }
        }

        fn reset(&mut self) {
            self.voices = [Voice::default(); 2];
            self.feedback = 0.0;
            self.tail_frames_remaining = 0;
        }

        fn is_quiescent(&self) -> bool {
            self.voices.iter().all(|voice| !voice.active)
                && self.tail_frames_remaining == 0
                && self.feedback == 0.0
        }

        fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
            previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| {
                    previous.compatibility_key == self.compatibility_key
                        && previous.release_frames == self.release_frames
                        && previous.max_tail_frames == self.max_tail_frames
                        && previous.max_voices == self.max_voices
                })
        }

        fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>()
                && self.can_copy_history_from(previous)
            {
                self.voices = previous.voices;
                self.feedback = previous.feedback;
                self.tail_frames_remaining = previous.tail_frames_remaining;
            }
        }

        fn fixed_storage_bytes(&self) -> usize {
            std::mem::size_of::<Self>()
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn prepared(key: u8, release_frames: u8, creations: Arc<AtomicUsize>) -> FeedbackPrepared {
        FeedbackPrepared {
            compatibility_key: key,
            release_frames,
            max_tail_frames: 4,
            max_events: 4,
            max_voices: 2,
            declared_bytes: std::mem::size_of::<FeedbackRuntime>(),
            creations,
            alternate_twin_key: false,
        }
    }

    fn event_on(offset: usize, id: u64, pitch: u8, velocity: u8) -> NativeInstrumentEvent {
        NativeInstrumentEvent::NoteOn {
            frame_offset: offset,
            occurrence_id: id,
            pitch,
            velocity,
        }
    }

    struct PairRetargetProof {
        target_key: u8,
        target_value: f32,
    }

    impl PreparedInstrumentHistoryRetarget for PairRetargetProof {
        fn declared_fixed_storage_bytes(&self) -> usize {
            std::mem::size_of::<Self>()
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct PairRetargetPrepared {
        key: u8,
        reject_second_twin: bool,
        initial_value: f32,
        creations: Arc<AtomicUsize>,
    }

    struct PairRetargetRuntime {
        key: u8,
        reject_retarget: bool,
        value: f32,
    }

    impl PreparedMutableInstrumentRuntime for PairRetargetPrepared {
        fn runtime_version(&self) -> u32 {
            1
        }

        fn declared_twin_bytes(&self) -> usize {
            std::mem::size_of::<PairRetargetRuntime>()
        }

        fn max_events_per_block(&self) -> usize {
            8
        }

        fn max_active_occurrences(&self) -> usize {
            2
        }

        fn max_release_tail_frames(&self) -> u64 {
            0
        }

        fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
            let ordinal = self.creations.fetch_add(1, Ordering::Relaxed);
            Box::new(PairRetargetRuntime {
                key: self.key,
                reject_retarget: self.reject_second_twin && ordinal == 1,
                value: self.initial_value,
            })
        }

        fn prepare_history_retarget_target(
            &self,
        ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
            Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
                PairRetargetProof {
                    target_key: self.key,
                    target_value: 7.0,
                },
            )))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    impl MutableInstrumentRuntime for PairRetargetRuntime {
        fn preflight_events(
            &self,
            events: &[NativeInstrumentEvent],
            _frame_count: usize,
        ) -> Result<(), NativeInstrumentRuntimeError> {
            if events.is_empty() {
                Ok(())
            } else {
                Err(NativeInstrumentRuntimeError::InvalidEventBatch)
            }
        }

        fn process_block(&mut self, _events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]) {
            for frame in frames {
                frame[0] += self.value;
            }
        }

        fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
            previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| self.key == previous.key)
        }

        fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                self.value = previous.value;
            }
        }

        fn can_retarget_history_from(
            &self,
            previous: &dyn MutableInstrumentRuntime,
            proof: &dyn PreparedInstrumentHistoryRetarget,
        ) -> bool {
            !self.reject_retarget
                && proof
                    .as_any()
                    .downcast_ref::<PairRetargetProof>()
                    .is_some_and(|proof| {
                        proof.target_key == self.key
                            && previous
                                .as_any()
                                .downcast_ref::<Self>()
                                .is_some_and(|previous| previous.key == 1)
                    })
        }

        fn retarget_history_from(
            &mut self,
            previous: &dyn MutableInstrumentRuntime,
            proof: &dyn PreparedInstrumentHistoryRetarget,
        ) {
            let Some(proof) = proof.as_any().downcast_ref::<PairRetargetProof>() else {
                return;
            };
            if self.can_retarget_history_from(previous, proof) {
                self.value = proof.target_value;
            }
        }

        fn fixed_storage_bytes(&self) -> usize {
            std::mem::size_of::<Self>()
        }

        fn reset(&mut self) {
            self.value = 0.0;
        }

        fn is_quiescent(&self) -> bool {
            self.value == 0.0
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn retarget_preflights_both_twins_before_mutating_either() {
        let old_prepared = PairRetargetPrepared {
            key: 1,
            reject_second_twin: false,
            initial_value: 3.0,
            creations: Arc::new(AtomicUsize::new(0)),
        };
        let target_prepared = PairRetargetPrepared {
            key: 2,
            reject_second_twin: true,
            initial_value: 0.0,
            creations: Arc::new(AtomicUsize::new(0)),
        };
        let previous = NativeInstrumentRuntimePair::new(&old_prepared, TEST_LIMITS).unwrap();
        let mut target = NativeInstrumentRuntimePair::new(&target_prepared, TEST_LIMITS).unwrap();
        let InstrumentHistoryRetargetPreparation::Prepared(proof) =
            target_prepared.prepare_history_retarget_target().unwrap()
        else {
            panic!("target provides explicit retarget proof");
        };

        assert!(!target.can_retarget_from(&previous, proof.as_ref()));
        assert_eq!(
            target.retarget_from(&previous, proof.as_ref()),
            Err(NativeInstrumentRuntimeError::IncompatibleHistory)
        );
        for runtime in [&target.primary, &target.scratch] {
            assert_eq!(
                runtime
                    .as_any()
                    .downcast_ref::<PairRetargetRuntime>()
                    .unwrap()
                    .value,
                0.0,
                "failed second-twin preflight leaves both targets unchanged"
            );
        }

        // The source isn't mutated by failed candidate preflight either.
        assert_eq!(
            previous
                .primary
                .as_any()
                .downcast_ref::<PairRetargetRuntime>()
                .unwrap()
                .value,
            3.0
        );
    }

    fn event_off(offset: usize, id: u64) -> NativeInstrumentEvent {
        NativeInstrumentEvent::NoteOff {
            frame_offset: offset,
            occurrence_id: id,
        }
    }

    fn new_pair(key: u8, release_frames: u8) -> NativeInstrumentRuntimePair {
        NativeInstrumentRuntimePair::new(
            &prepared(key, release_frames, Arc::new(AtomicUsize::new(0))),
            TEST_LIMITS,
        )
        .unwrap()
    }

    fn new_pair_with_voices(
        key: u8,
        release_frames: u8,
        max_voices: usize,
    ) -> NativeInstrumentRuntimePair {
        let mut prepared = prepared(key, release_frames, Arc::new(AtomicUsize::new(0)));
        prepared.max_voices = max_voices;
        NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).unwrap()
    }

    fn render(
        runtime: &mut NativeInstrumentRuntimePair,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
    ) -> Vec<StereoFrame> {
        let mut frames = vec![[0.0, 0.0]; frame_count];
        runtime.begin_block();
        runtime.process_block(events, &mut frames).unwrap();
        runtime.commit_block();
        frames
    }

    #[test]
    fn default_capability_stays_off_and_mutable_only_does_not_fake_analytic_audio() {
        struct AnalyticOnly;
        impl PreparedNativeInstrument for AnalyticOnly {
            fn prepare_note(
                &self,
                _pitch: u8,
                _velocity: u8,
            ) -> Result<PreparedAnalyticNote, DeviceContractError> {
                Ok(PreparedAnalyticNote::new(
                    [0; ANALYTIC_NOTE_PAYLOAD_WORDS],
                    0,
                    |_, _, _, _| [0.0, 0.0],
                ))
            }
        }

        struct MutableOnly(FeedbackPrepared);
        impl PreparedNativeInstrument for MutableOnly {
            fn prepare_note(
                &self,
                _pitch: u8,
                _velocity: u8,
            ) -> Result<PreparedAnalyticNote, DeviceContractError> {
                Err(DeviceContractError::UnsupportedProcessingClass)
            }

            fn prepare_mutable_runtime(
                &self,
            ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError>
            {
                Ok(Some(Box::new(FeedbackPrepared {
                    compatibility_key: self.0.compatibility_key,
                    release_frames: self.0.release_frames,
                    max_tail_frames: self.0.max_tail_frames,
                    max_events: self.0.max_events,
                    max_voices: self.0.max_voices,
                    declared_bytes: self.0.declared_bytes,
                    creations: Arc::clone(&self.0.creations),
                    alternate_twin_key: self.0.alternate_twin_key,
                })))
            }
        }

        let analytic = AnalyticOnly;
        assert!(analytic.prepare_mutable_runtime().unwrap().is_none());

        let mutable = MutableOnly(prepared(1, 2, Arc::new(AtomicUsize::new(0))));
        assert_eq!(
            mutable.prepare_note(60, 100).unwrap_err(),
            DeviceContractError::UnsupportedProcessingClass
        );
        let factory = mutable.prepare_mutable_runtime().unwrap().unwrap();
        let pair = NativeInstrumentRuntimePair::new(factory.as_ref(), TEST_LIMITS).unwrap();
        assert_eq!(
            pair.fixed_storage_pair_bytes(),
            Some(pair.declared_pair_bytes())
        );
    }

    #[test]
    fn overlapping_same_pitch_occurrences_keep_distinct_ids_and_partition_exactly() {
        let mut whole = new_pair(1, 2);
        let whole_pcm = render(
            &mut whole,
            &[
                event_on(0, 11, 60, 83),
                event_on(0, 12, 60, 117),
                event_off(3, 11),
                event_off(5, 12),
            ],
            8,
        );

        let mut partitioned = new_pair(1, 2);
        let mut partitioned_pcm = Vec::new();
        partitioned_pcm.extend(render(
            &mut partitioned,
            &[event_on(0, 11, 60, 83), event_on(0, 12, 60, 117)],
            3,
        ));
        partitioned_pcm.extend(render(&mut partitioned, &[event_off(0, 11)], 2));
        partitioned_pcm.extend(render(&mut partitioned, &[event_off(0, 12)], 3));
        assert_eq!(whole_pcm, partitioned_pcm);
        assert!(whole_pcm.iter().any(|frame| *frame != [0.0, 0.0]));
    }

    #[test]
    fn instrument_adds_to_existing_chain_audio_without_overwriting_it() {
        let events = [event_on(0, 31, 64, 113)];
        let mut generated_only = new_pair(1, 2);
        let generated = render(&mut generated_only, &events, 5);

        let mut mixed = new_pair(1, 2);
        let mut chain_input = [[0.25, -0.5]; 5];
        mixed.begin_block();
        mixed.process_block(&events, &mut chain_input).unwrap();
        mixed.commit_block();
        let expected: [[f32; 2]; 5] =
            std::array::from_fn(|index| [0.25 + generated[index][0], -0.5 + generated[index][1]]);
        assert_eq!(chain_input, expected);
    }

    #[test]
    fn same_frame_off_then_on_order_frees_capacity_before_the_next_event() {
        let mut one_voice = prepared(1, 0, Arc::new(AtomicUsize::new(0)));
        one_voice.max_voices = 1;
        let mut runtime = NativeInstrumentRuntimePair::new(
            &one_voice,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 4_096,
                max_events_per_block: 4,
                max_active_occurrences: 1,
                max_tail_frames: 16,
            },
        )
        .unwrap();
        render(&mut runtime, &[event_on(0, 1, 60, 100)], 2);

        runtime.begin_block();
        let mut off_then_on = [[0.0, 0.0]; 2];
        runtime
            .process_block(
                &[event_off(0, 1), event_on(0, 2, 60, 100)],
                &mut off_then_on,
            )
            .unwrap();
        runtime.commit_block();

        runtime.begin_block();
        let mut on_then_off = [[7.0, -3.0]; 2];
        let unchanged = on_then_off;
        assert_eq!(
            runtime.process_block(
                &[event_on(0, 3, 60, 100), event_off(0, 2)],
                &mut on_then_off
            ),
            Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
        );
        assert_eq!(on_then_off, unchanged);
        runtime.rollback_block();
    }

    #[test]
    fn prior_block_release_advances_before_first_event_offset() {
        let mut one_voice = new_pair_with_voices(1, 4, 1);
        let mut two_voice_reference = new_pair_with_voices(1, 4, 2);
        for runtime in [&mut one_voice, &mut two_voice_reference] {
            render(runtime, &[event_on(0, 1, 60, 100)], 2);
            render(runtime, &[event_off(0, 1)], 1);
        }

        let candidate = event_on(3, 2, 64, 90);
        let actual = render(&mut one_voice, &[candidate], 4);
        let reference = render(&mut two_voice_reference, &[candidate], 4);
        assert_eq!(actual, reference);
        assert!(actual.iter().any(|frame| *frame != [0.0, 0.0]));
    }

    #[test]
    fn malformed_or_exhausted_batches_leave_pcm_and_committed_history_unchanged() {
        let mut runtime = new_pair(1, 2);
        let initial = render(&mut runtime, &[event_on(0, 1, 61, 100)], 2);

        for (events, error) in [
            (
                vec![event_on(0, 1, 61, 100)],
                NativeInstrumentRuntimeError::InvalidEventBatch,
            ),
            (
                vec![event_on(1, 2, 60, 90), event_on(0, 3, 60, 90)],
                NativeInstrumentRuntimeError::InvalidEventBatch,
            ),
            (
                vec![event_on(0, 2, 60, 90), event_on(0, 2, 62, 90)],
                NativeInstrumentRuntimeError::InvalidEventBatch,
            ),
            (
                vec![
                    event_on(0, 2, 60, 90),
                    event_on(0, 3, 62, 90),
                    event_off(1, 1),
                ],
                NativeInstrumentRuntimeError::EventCapacityExceeded,
            ),
        ] {
            runtime.begin_block();
            let mut untouched = [[4.0, -2.0]; 3];
            let before = untouched;
            assert_eq!(runtime.process_block(&events, &mut untouched), Err(error));
            assert_eq!(untouched, before);
            runtime.rollback_block();
        }

        let retry = render(&mut runtime, &[], 3);
        let mut reference = new_pair(1, 2);
        let reference_initial = render(&mut reference, &[event_on(0, 1, 61, 100)], 2);
        assert_eq!(initial, reference_initial);
        assert_eq!(retry, render(&mut reference, &[], 3));
    }

    #[test]
    fn speculative_history_rolls_back_and_committed_twins_advance_together() {
        let mut runtime = new_pair(1, 2);
        render(&mut runtime, &[event_on(0, 7, 65, 112)], 3);

        runtime.begin_block();
        let mut failed = [[0.0, 0.0]; 4];
        runtime
            .process_block(&[event_off(1, 7)], &mut failed)
            .unwrap();
        runtime.rollback_block();

        let retry = render(&mut runtime, &[], 4);
        let mut reference = new_pair(1, 2);
        render(&mut reference, &[event_on(0, 7, 65, 112)], 3);
        assert_eq!(retry, render(&mut reference, &[], 4));
    }

    #[test]
    fn analytic_import_requires_provider_opt_in_and_leaves_default_runtime_unchanged() {
        let mut target = new_pair(1, 2);
        assert!(!target.can_import_analytic(&[]));
        assert_eq!(
            target.import_analytic(&[]),
            Err(NativeInstrumentRuntimeError::IncompatibleHistory)
        );
        assert_eq!(render(&mut target, &[event_on(0, 9, 67, 120)], 4), {
            let mut fresh = new_pair(1, 2);
            render(&mut fresh, &[event_on(0, 9, 67, 120)], 4)
        });
    }

    #[test]
    fn incompatible_history_is_refused_without_copy_and_candidate_remains_reset() {
        let mut previous = new_pair(1, 2);
        render(&mut previous, &[event_on(0, 9, 67, 120)], 4);
        let mut candidate = new_pair(2, 2);
        assert!(!candidate.can_inherit_from(&previous));
        assert_eq!(
            candidate.inherit_from(&previous),
            Err(NativeInstrumentRuntimeError::IncompatibleHistory)
        );
        assert_eq!(render(&mut candidate, &[event_on(0, 9, 67, 120)], 4), {
            let mut fresh = new_pair(2, 2);
            render(&mut fresh, &[event_on(0, 9, 67, 120)], 4)
        });
    }

    #[test]
    fn different_voice_capacity_is_incompatible_history() {
        let mut previous = new_pair_with_voices(1, 2, 2);
        render(
            &mut previous,
            &[event_on(0, 40, 60, 100), event_on(0, 41, 64, 90)],
            2,
        );
        let mut candidate = new_pair_with_voices(1, 2, 1);
        assert!(!candidate.can_inherit_from(&previous));
        assert_eq!(
            candidate.inherit_from(&previous),
            Err(NativeInstrumentRuntimeError::IncompatibleHistory)
        );
        assert_eq!(render(&mut candidate, &[], 3), [[0.0, 0.0]; 3]);
    }

    #[test]
    fn caller_ceilings_and_twin_arithmetic_are_checked_before_runtime_creation() {
        let creations = Arc::new(AtomicUsize::new(0));
        let prepared_runtime = prepared(1, 1, Arc::clone(&creations));
        let too_small = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 1,
            ..TEST_LIMITS
        };
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&prepared_runtime, too_small),
            Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        ));
        assert_eq!(creations.load(Ordering::Relaxed), 0);

        let mut overflowing = prepared(1, 1, Arc::clone(&creations));
        overflowing.declared_bytes = usize::MAX;
        assert!(matches!(
            NativeInstrumentRuntimePair::new(&overflowing, TEST_LIMITS),
            Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        ));
        assert_eq!(creations.load(Ordering::Relaxed), 0);

        let pair = NativeInstrumentRuntimePair::new(&prepared_runtime, TEST_LIMITS).unwrap();
        assert_eq!(creations.load(Ordering::Relaxed), 2);
        assert_eq!(
            pair.declared_pair_bytes(),
            2 * std::mem::size_of::<FeedbackRuntime>()
        );
        assert_eq!(
            pair.fixed_storage_pair_bytes(),
            Some(pair.declared_pair_bytes())
        );
    }

    #[test]
    fn hard_reset_clears_committed_and_scratch_voices_without_chase() {
        let mut runtime = new_pair(1, 2);
        render(&mut runtime, &[event_on(0, 23, 72, 127)], 4);
        runtime.reset();
        assert_eq!(render(&mut runtime, &[], 4), [[0.0, 0.0]; 4]);
        assert!(runtime.committed_is_quiescent());
    }

    #[test]
    fn release_and_feedback_tail_become_quiescent_within_declared_bound() {
        let mut runtime = new_pair(1, 2);
        assert_eq!(runtime.max_release_tail_frames(), 6);
        assert!(runtime.committed_is_quiescent());
        runtime.begin_block();
        assert!(!runtime.committed_is_quiescent());
        runtime.rollback_block();

        render(&mut runtime, &[event_on(0, 51, 72, 127)], 2);
        assert!(!runtime.committed_is_quiescent());
        // The sample at the NoteOff offset is included in the bound.
        render(&mut runtime, &[event_off(0, 51)], 1);
        assert!(!runtime.committed_is_quiescent());

        let tail = render(&mut runtime, &[], 5);
        assert!(tail.iter().any(|frame| *frame != [0.0, 0.0]));
        assert!(runtime.committed_is_quiescent());
        assert_eq!(render(&mut runtime, &[], 2), [[0.0, 0.0]; 2]);

        let mut zero_release = new_pair(1, 0);
        assert_eq!(zero_release.max_release_tail_frames(), 4);
        render(&mut zero_release, &[event_on(0, 52, 72, 127)], 2);
        render(&mut zero_release, &[event_off(0, 52)], 1);
        assert!(
            render(&mut zero_release, &[], 3)
                .iter()
                .any(|frame| *frame != [0.0, 0.0])
        );
        assert!(zero_release.committed_is_quiescent());
    }

    #[test]
    fn caller_tail_ceiling_is_checked_before_runtime_creation() {
        let creations = Arc::new(AtomicUsize::new(0));
        let prepared = prepared(1, 2, Arc::clone(&creations));
        let too_short = NativeInstrumentRuntimeLimits {
            max_tail_frames: 5,
            ..TEST_LIMITS
        };
        assert_eq!(
            NativeInstrumentRuntimePair::new(&prepared, too_short).err(),
            Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        );
        assert_eq!(creations.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn incompatible_factory_twins_are_refused_during_control_construction() {
        let creations = Arc::new(AtomicUsize::new(0));
        let mut prepared = prepared(1, 1, Arc::clone(&creations));
        prepared.alternate_twin_key = true;
        assert_eq!(
            NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).err(),
            Some(NativeInstrumentRuntimeError::IncompatibleTwins)
        );
        assert_eq!(creations.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn event_and_occurrence_ceilings_are_checked_before_runtime_creation() {
        let creations = Arc::new(AtomicUsize::new(0));
        let prepared = prepared(1, 1, Arc::clone(&creations));
        let no_events = NativeInstrumentRuntimeLimits {
            max_events_per_block: 3,
            ..TEST_LIMITS
        };
        assert_eq!(
            NativeInstrumentRuntimePair::new(&prepared, no_events).err(),
            Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        );
        let one_occurrence = NativeInstrumentRuntimeLimits {
            max_active_occurrences: 1,
            ..TEST_LIMITS
        };
        assert_eq!(
            NativeInstrumentRuntimePair::new(&prepared, one_occurrence).err(),
            Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        );
        assert_eq!(creations.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn underreported_fixed_storage_is_refused_after_bounded_control_construction() {
        let creations = Arc::new(AtomicUsize::new(0));
        let mut prepared = prepared(1, 1, Arc::clone(&creations));
        prepared.declared_bytes -= 1;
        assert_eq!(
            NativeInstrumentRuntimePair::new(&prepared, TEST_LIMITS).err(),
            Some(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)
        );
        assert_eq!(creations.load(Ordering::Relaxed), 2);
    }
}
