//! Transactional, bounded input split for two exact processor owners.
//!
//! This is a borrowed signal operation, not resource custody, graph admission,
//! routing or an output-tail owner. The caller retains both chains and must
//! begin, commit or roll back their block transactions together with this ramp.
//! The caller must exclude live/transient automation of the target and retain
//! the exact old and new owners throughout the transition, and retain the old
//! owner until its actual signal history has drained. Identity checks do not
//! replace that custody contract or protect against a dropped owner's address
//! being reused.
use crate::{
    DeviceContractError, NativeInstrumentClock, NativeRuntimeChain, OutputTail, RefuseForeign,
    StereoFrame,
};

/// Fixed-sample bypass transition. Committed and speculative positions stay
/// independent so a later host block refusal can restore the exact trajectory.
pub struct TailSplit {
    index: usize,
    old_bypassed: bool,
    owner: crate::ProcessorChainOwner,
    device_id: u64,
    device_type: crate::DeviceTypeId,
    sample_rate: u32,
    runtime_version: u32,
    old_runtime: usize,
    new_runtime: usize,
    released: bool,
    total: u32,
    committed: u32,
    scratch: u32,
    open: bool,
}

impl NativeRuntimeChain {
    /// Identify a same-processor bypass edit that carries an output tail.
    pub fn tail_bypass_change_at(&self, old: &Self, index: usize) -> bool {
        self.entries
            .get(index)
            .zip(old.entries.get(index))
            .is_some_and(|(next, previous)| {
                Self::same_runtime_identity(next, previous)
                    && next.bypassed != previous.bypassed
                    && next.output_tail != (OutputTail::Finite { max_tail_frames: 0 })
                    && previous.output_tail != (OutputTail::Finite { max_tail_frames: 0 })
            })
    }

    /// Check split eligibility without mutating either runtime.
    pub fn can_prepare_tail_split_at(&self, old: &Self, index: usize) -> bool {
        let Some((next, previous)) = self.entries.get(index).zip(old.entries.get(index)) else {
            return false;
        };
        self.tail_bypass_change_at(old, index)
            && next.algorithmic_latency_frames == Some(0)
            && previous.algorithmic_latency_frames == Some(0)
            && next.automation.is_empty()
            && previous.automation.is_empty()
            && !previous.committed_bypass_ramp.is_active()
            && !previous.committed_matrix_ramp.is_active()
            && previous.committed_host_gains == [previous.input_gain, previous.output_gain]
            && next.runtime.can_split_tail_input_from(&previous.runtime)
            && self.can_fork_timing_without_signal_history_at(old, index)
    }

    /// Prepare a transition by seeding an exact pristine successor's timing,
    /// without moving or copying the donor's signal history. This returns no
    /// resource lease and does not begin either chain's block transaction.
    pub fn prepare_tail_split_at(
        &mut self,
        old: &Self,
        index: usize,
    ) -> Result<TailSplit, DeviceContractError> {
        let Some((next, previous)) = self.entries.get(index).zip(old.entries.get(index)) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if !self.can_prepare_tail_split_at(old, index) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let old_bypassed = previous.bypassed;
        let owner = previous.owner;
        let device_id = previous.device_id;
        let device_type = previous.device_type;
        let sample_rate = previous.sample_rate;
        let runtime_version = previous.runtime_version;
        let old_runtime = previous
            .runtime
            .owned_identity()
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        let new_runtime = next
            .runtime
            .owned_identity()
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        let total = sample_rate.div_ceil(200).max(1);
        self.fork_timing_without_signal_history_at(old, index)?;
        Ok(TailSplit {
            index,
            old_bypassed,
            owner,
            device_id,
            device_type,
            sample_rate,
            runtime_version,
            old_runtime,
            new_runtime,
            released: false,
            total,
            committed: 0,
            scratch: 0,
            open: false,
        })
    }
}

impl TailSplit {
    /// Begin alongside both chain transactions. A nested begin refuses.
    pub fn begin_block(&mut self) -> Result<(), DeviceContractError> {
        if self.open || self.released {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        self.scratch = self.committed;
        self.open = true;
        Ok(())
    }

    /// Render one target entry into separately owned old/new buffers. Both
    /// output buffers and input must have equal lengths no larger than 256.
    /// The supplied clock's offset zero must correspond to this input slice;
    /// unchanged prefixes and downstream routing remain the caller's owners.
    /// On error the caller must roll back both chains and this split together.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        old: &mut NativeRuntimeChain,
        next: &mut NativeRuntimeChain,
        input: &[StereoFrame],
        old_output: &mut [StereoFrame],
        new_output: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if !self.open
            || input.len() > 256
            || input.len() != old_output.len()
            || input.len() != new_output.len()
            || frame_offset.checked_add(input.len()).is_none()
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let Some((previous, successor)) = old
            .entries
            .get_mut(self.index)
            .zip(next.entries.get_mut(self.index))
        else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if !NativeRuntimeChain::same_runtime_identity(previous, successor)
            || previous.owner != self.owner
            || previous.device_id != self.device_id
            || previous.device_type != self.device_type
            || previous.sample_rate != self.sample_rate
            || previous.runtime_version != self.runtime_version
            || previous.runtime.owned_identity() != Some(self.old_runtime)
            || successor.runtime.owned_identity() != Some(self.new_runtime)
            || previous.bypassed != self.old_bypassed
            || successor.bypassed == self.old_bypassed
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let mut old_dry = [[0.0; 2]; 256];
        let mut gains = [0.0_f32; 256];
        for (index, sample) in input.iter().enumerate() {
            let gain = self.scratch as f32 / self.total as f32;
            self.scratch = self.scratch.saturating_add(1).min(self.total);
            gains[index] = gain;
            old_dry[index] = sample.map(|value| value * (1.0 - gain));
            old_output[index] = old_dry[index];
            new_output[index] = sample.map(|value| value * gain);
        }
        // The old history must render wet even when it came from a bypassed
        // owner; its audible dry-to-wet interpolation is applied afterward.
        previous.bypassed = false;
        let old_result = NativeRuntimeChain::process_entry_with_foreign_inner(
            previous,
            old_output,
            frame_offset,
            clock,
            clock,
            &mut RefuseForeign,
            None,
            None,
        );
        previous.bypassed = self.old_bypassed;
        old_result?;
        NativeRuntimeChain::process_entry_with_foreign_inner(
            successor,
            new_output,
            frame_offset,
            clock,
            clock,
            &mut RefuseForeign,
            None,
            None,
        )?;
        if self.old_bypassed {
            for (index, sample) in old_output.iter_mut().enumerate() {
                for channel in 0..2 {
                    sample[channel] = old_dry[index][channel]
                        + (sample[channel] - old_dry[index][channel]) * gains[index];
                }
            }
        }
        Ok(())
    }

    /// Commit after both chains and every later render stage succeeded.
    pub fn commit_block(&mut self) -> Result<(), DeviceContractError> {
        if !self.open {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        self.committed = self.scratch;
        self.open = false;
        Ok(())
    }
    /// Undo this block alongside both chain rollbacks.
    pub fn rollback_block(&mut self) {
        self.scratch = self.committed;
        self.open = false;
    }
    /// A completed ramp still requires the old owner's tail to drain.
    pub fn is_complete(&self) -> bool {
        self.committed >= self.total
    }

    /// Once the transition commits, allow the original runtime to continue as
    /// a wet residual owner, including when it was originally bypassed. This
    /// changes only the retained runtime, never saved device authoring state.
    /// The caller separately starts and accounts for residual-output custody.
    pub fn finish_retained_tail(
        &mut self,
        old: &mut NativeRuntimeChain,
    ) -> Result<(), DeviceContractError> {
        if self.open || self.released || !self.is_complete() {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let Some(entry) = old.entries.get_mut(self.index) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if entry.owner != self.owner
            || entry.device_id != self.device_id
            || entry.device_type != self.device_type
            || entry.sample_rate != self.sample_rate
            || entry.runtime_version != self.runtime_version
            || entry.runtime.owned_identity() != Some(self.old_runtime)
            || entry.bypassed != self.old_bypassed
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        entry.bypassed = false;
        self.released = true;
        Ok(())
    }
}
