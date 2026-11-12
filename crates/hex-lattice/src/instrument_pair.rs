//! Optional single-owner mutable instrument pairs. Existing independent twins
//! retain their factory and accounting contract in `instrument_runtime`.
use crate::{
    InstrumentHistoryRetargetPreparation, NativeInstrumentClock, NativeInstrumentEvent,
    NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits, PreparedInstrumentHistoryRetarget,
    StereoFrame,
};
use hex_lattice_contract::DeviceContractError;
use std::any::Any;

/// Control-only factory for ONE allocation owning its law and BOTH histories.
/// The declaration includes every unique owned sampling byte, buffer, handle
/// and transaction field once. No borrowed per-edit owner may evade this charge.
/// Only existing process-wide immutable assets may be borrowed. The runtime
/// must remain valid after factory destruction and retire on control, never in
/// processing. This is an alternative capability, not a per-twin adapter.
pub trait PreparedOwnedInstrumentRuntime: Send + Sync {
    fn runtime_version(&self) -> u32;
    fn declared_pair_bytes(&self) -> usize;
    fn max_events_per_block(&self) -> usize;
    fn max_active_occurrences(&self) -> usize;
    fn max_release_tail_frames(&self) -> u64;
    fn create_pair(&self) -> Box<dyn OwnedInstrumentRuntime>;
    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Unchanged)
    }
}

/// Exclusive pair ownership prevents independently writable shared laws. The
/// implementation stores immutable prepared target plus applied source boundary
/// and two independent histories. Source staging may change ONLY an inactive
/// candidate after read-only whole-host preflight; rendering never writes the
/// law. Begin checkpoints committed history; process touches scratch only;
/// commit selects scratch; abort preserves committed future PCM bit-for-bit.
/// All methods after construction are bounded, allocation-free and infallible
/// after their preflight. No method drops an owned sampling resource. Complete
/// source/target/rate/layout compatibility is required, not merely an ID.
/// Host ownership/generation and musical clock checks remain additional gates.
pub trait OwnedInstrumentRuntime: Any + Send {
    fn fixed_storage_pair_bytes(&self) -> usize;
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
    ) -> Result<(), NativeInstrumentRuntimeError>;
    /// Read-only clock preflight, using the same borrowed host lattice clock as
    /// independent twins. Clock-dependent providers must reject missing/invalid
    /// spans here; no callback clock allocation or alternative generation law.
    fn preflight_clock(
        &self,
        _clock: Option<&dyn NativeInstrumentClock>,
        _frame_count: usize,
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
    fn begin_block(&mut self);
    fn process_block(&mut self, events: &[NativeInstrumentEvent], frames: &mut [StereoFrame]);
    fn commit_block(&mut self);
    fn rollback_block(&mut self);
    fn reset(&mut self);
    fn committed_is_quiescent(&self) -> bool;
    fn speculative_is_quiescent(&self) -> bool;
    fn can_inherit_from(&self, previous: &dyn OwnedInstrumentRuntime) -> bool;
    /// Copy law/source boundary and committed future-sample history into both
    /// candidate histories; prepared target must remain exactly compatible.
    fn inherit_from(&mut self, previous: &dyn OwnedInstrumentRuntime);
    fn can_retarget_from(
        &self,
        _previous: &dyn OwnedInstrumentRuntime,
        _proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        false
    }
    /// Stage current committed source into this inactive candidate; preserve
    /// oscillator phases/feedback/gates/ages and note/device clocks. A failed
    /// later host commit leaves the old pair untouched; retry stages anew from
    /// the current source. Detached tails retain their original complete owner.
    fn retarget_from(
        &mut self,
        _previous: &dyn OwnedInstrumentRuntime,
        _proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
    }
    fn as_any(&self) -> &dyn Any;
}

/// Host transaction guard for the alternative paired capability. Metadata here
/// is the same host-side admission/transaction class as the existing twin guard;
/// implementation payload (including its own state) is charged in full below.
pub struct NativeOwnedInstrumentRuntimePair {
    runtime: Box<dyn OwnedInstrumentRuntime>,
    version: u32,
    bytes: usize,
    events: usize,
    voices: usize,
    tail_frames: u64,
    open: bool,
}
impl NativeOwnedInstrumentRuntimePair {
    pub fn new(
        prepared: &dyn PreparedOwnedInstrumentRuntime,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        let version = prepared.runtime_version();
        let bytes = prepared.declared_pair_bytes();
        let events = prepared.max_events_per_block();
        let voices = prepared.max_active_occurrences();
        let tail_frames = prepared.max_release_tail_frames();
        if version == 0
            || bytes == 0
            || bytes > limits.max_pair_bytes
            || events == 0
            || events > limits.max_events_per_block
            || voices == 0
            || voices > limits.max_active_occurrences
            || tail_frames > limits.max_tail_frames
        {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }
        let runtime = prepared.create_pair();
        if runtime.fixed_storage_pair_bytes() > bytes {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }
        Ok(Self {
            runtime,
            version,
            bytes,
            events,
            voices,
            tail_frames,
            open: false,
        })
    }
    pub fn declared_pair_bytes(&self) -> usize {
        self.bytes
    }
    pub fn fixed_storage_pair_bytes(&self) -> usize {
        self.runtime.fixed_storage_pair_bytes()
    }
    pub fn max_active_occurrences(&self) -> usize {
        self.voices
    }
    pub fn max_release_tail_frames(&self) -> u64 {
        self.tail_frames
    }
    pub fn committed_state_available(&self) -> bool {
        !self.open
    }
    pub fn committed_is_quiescent(&self) -> bool {
        !self.open && self.runtime.committed_is_quiescent()
    }
    pub fn speculative_is_quiescent(&self) -> bool {
        self.open && self.runtime.speculative_is_quiescent()
    }
    pub fn begin_block(&mut self) {
        self.runtime.begin_block();
        self.open = true;
    }
    pub fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.process_block_with_clock(events, frames, None)
    }
    pub fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.open {
            return Err(NativeInstrumentRuntimeError::BlockNotStarted);
        }
        crate::instrument_runtime::validate_event_batch(events, frames.len(), self.events, false)?;
        self.runtime.preflight_events(events, frames.len())?;
        self.runtime.preflight_clock(clock, frames.len())?;
        self.runtime.process_block_with_clock(events, frames, clock);
        Ok(())
    }
    pub fn commit_block(&mut self) {
        if self.open {
            self.runtime.commit_block();
            self.open = false;
        }
    }
    pub fn rollback_block(&mut self) {
        if self.open {
            self.runtime.rollback_block();
            self.open = false;
        }
    }
    pub fn reset(&mut self) {
        self.runtime.reset();
        self.open = false;
    }
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        !self.open
            && !previous.open
            && self.version == previous.version
            && self.runtime.can_inherit_from(previous.runtime.as_ref())
    }
    pub fn inherit_from(&mut self, previous: &Self) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_inherit_from(previous) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.runtime.inherit_from(previous.runtime.as_ref());
        Ok(())
    }
    pub fn can_retarget_from(
        &self,
        previous: &Self,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        !self.open
            && !previous.open
            && self.version == previous.version
            && self
                .runtime
                .can_retarget_from(previous.runtime.as_ref(), proof)
    }
    pub fn retarget_from(
        &mut self,
        previous: &Self,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.can_retarget_from(previous, proof) {
            return Err(NativeInstrumentRuntimeError::IncompatibleHistory);
        }
        self.runtime.retarget_from(previous.runtime.as_ref(), proof);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
