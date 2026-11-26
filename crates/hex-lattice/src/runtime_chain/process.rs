//! Real-time audio block, tapped, foreign, and host-gain processing and transaction lifecycle.
use super::*;

mod entry;

impl NativeRuntimeChain {
    /// Enter a residual-output owner after complete admission preflight. Owned
    /// sustain/freeze providers can release their held input-independent cycle;
    /// history and all storage stay with this chain until control retirement.
    pub fn begin_output_tail(&mut self) {
        for entry in &mut self.entries {
            entry.automation_frozen = true;
            entry.runtime.begin_output_tail();
        }
    }

    /// Start a transactional block by copying committed histories into twins.
    pub fn begin_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.begin();
            }
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.scratch_output_gain_ramp = entry.committed_output_gain_ramp;
            entry.scratch_transient_output_gain = entry.committed_transient_output_gain;
            entry.scratch_matrix_ramp = entry.committed_matrix_ramp;
            entry.scratch_bypass_ramp = entry.committed_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            if !entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.begin();
            }
        }
    }

    /// Process the summed track signal through all registered stateful entries.
    pub fn process_block(&mut self, frames: &mut [StereoFrame]) -> Result<(), DeviceContractError> {
        Self::process_entries(&mut self.entries, frames, None, None)
    }

    /// Process the full chain while observing each entry's output in order.
    /// The observer borrows the current audio block and must not retain it.
    /// This is intended for bounded, allocation-free diagnostics such as an
    /// analyzer tap; musical behavior remains unchanged.
    pub fn process_block_observed(
        &mut self,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        Self::process_entries_observed(
            &mut self.entries,
            frames,
            frame_offset,
            None,
            None,
            observer,
        )
    }

    /// Process a control-prevalidated effect segment without ending the
    /// surrounding transactional block. The range indexes the prepared effect
    /// order; callers must begin once and commit only after the full render block.
    pub fn process_range(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
    ) -> Result<(), DeviceContractError> {
        self.process_range_with_clock(range, frames, None, None)
    }

    /// Process a segment with explicit content origins for both host stages.
    /// `clock` describes entering content: provider controls and host input gain
    /// use it. `output_clock` describes emitted content after each processor's
    /// own algorithmic latency and drives host output gain. Each unavailable
    /// origin remains independently None; neither stage borrows the other's clock.
    /// The clocks must describe every entry in the range. Different arrivals
    /// require single-entry calls; zero-latency ranges explicitly reuse a clock.
    /// This method never shifts clocks or grants foreign automation support.
    pub fn process_range_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries(entries, frames, clock, output_clock)?;
        Ok(())
    }

    /// Process a prepared segment and observe each matching post-node signal.
    /// The callback receives the stable device ID and a borrowed view of the
    /// rendered segment. It must do bounded, nonblocking work and cannot retain
    /// the samples after returning.
    pub fn process_range_observed(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        self.process_range_observed_with_clock(range, frames, frame_offset, None, None, observer)
    }

    /// Clock-aware segment processing with the same borrowed post-node tap.
    /// Stage origins follow `process_range_with_clock`; output observation does
    /// not change which content origin drives provider/input or output controls.
    pub fn process_range_observed_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries_observed(
            entries,
            frames,
            frame_offset,
            clock,
            output_clock,
            observer,
        )?;
        Ok(())
    }

    /// Observe synchronous input/output borrows around each complete entry.
    /// Samples must not be retained, allocated from, or used for blocking work.
    pub fn process_block_tapped(
        &mut self,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        Self::process_entries_tapped(
            &mut self.entries,
            frames,
            frame_offset,
            None,
            None,
            observer,
        )
    }

    /// Process a prepared range with input/output taps and no content clock.
    pub fn process_range_tapped(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_clock(range, frames, frame_offset, None, None, observer)
    }

    /// Clock-aware paired taps. Input is before host trim, Output after the
    /// entire entry. Both phases have the same identity, offset and frame count.
    /// The independent stage-origin contract is `process_range_with_clock`.
    pub fn process_range_tapped_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries_tapped(entries, frames, frame_offset, clock, output_clock, observer)?;
        Ok(())
    }

    /// Process a segment without observation through the same borrowed path.
    /// Stage origins follow `process_range_with_clock`; foreign DSP continues
    /// to receive only its entering-content clock and existing host trim law.
    pub fn process_range_with_foreign(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_context(
            range,
            frames,
            StageProcessContext::new(frame_offset, clock, output_clock).with_foreign(foreign),
            &mut |_, _, _, _, _, _| {},
        )
    }

    /// Validate an inline request against the exact prepared current target.
    /// A missing or frozen instance returns false without mutating the chain.
    /// Host trims cannot be applied while bypassed. Provider controls may
    /// advance under warm bypass; a powered-off entry cannot apply input.
    pub fn validate_transient_input(
        &self,
        input: NativeTransientInput,
    ) -> Result<bool, DeviceContractError> {
        let mut matching = self.entries.iter().filter(|entry| {
            !entry.automation_frozen
                && entry.owner == input.identity.owner()
                && entry.device_id == input.identity.device_id()
        });
        let Some(entry) = matching.next() else {
            return Ok(false);
        };
        if matching.next().is_some()
            || entry.automation.input_target() != Some(input.operation.target())
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if entry.powered_off(entry.committed_bypass_ramp)
            || (entry.bypassed && input.operation.target().is_host())
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if input.operation.target() == hex_lattice_contract::DeviceAutomationTarget::HostOutputGain
            && matches!(entry.runtime, RuntimeOwnership::Foreign(_))
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if let Some(value) = input.operation.value() {
            entry.automation.validate_input(value)?;
        }
        if let hex_lattice_contract::DeviceAutomationTarget::Parameter(id) =
            input.operation.target()
            && (!entry.matrix.is_identity() || !entry.runtime.supports_parameter_automation(id))
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Ok(true)
    }

    /// Drop an armed output override and its trajectory at a lifecycle fence.
    /// Call between render transactions. Missing input during rendering does
    /// not cross this fence; a delivered Clear can settle without its request.
    pub fn invalidate_transient_output_gain(&mut self, identity: ProcessorInstanceIdentity) {
        for entry in self.entries.iter_mut().filter(|entry| {
            !entry.automation_frozen
                && entry.owner == identity.owner()
                && entry.device_id == identity.device_id()
        }) {
            host_output_gain_ramp::invalidate_transient(entry);
        }
    }

    /// Stage a graph segment through borrowed clocks, optional foreign endpoints,
    /// transient input, and materialized external automation with per-node taps.
    /// Successful processing still grants no dispatch: final host-format
    /// acceptance remains exclusively with the audio owner. On error all
    /// foreign work for this callback is revoked; owned DSP can then roll back.
    pub fn process_range_tapped_with_context(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_tapped(range, frames, None, context, observer)
    }

    /// Run one exact effect stage with its admitted key in the ordinary
    /// begin/commit/rollback transaction. Main and key are borrowed only for
    /// this call. A stereo key requires a stateful native or foreign identity-matrix
    /// stage. The caller rolls back the complete block on any stage refusal.
    pub fn process_auxiliary_stage_with_context(
        &mut self,
        stage: usize,
        block: StereoAuxiliaryBlock<'_>,
        context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let end = stage
            .checked_add(1)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        let (main, key) = block.into_parts();
        self.process_tapped(stage..end, main, Some(key), context, observer)
    }

    fn process_tapped(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let StageProcessContext {
            frame_offset,
            clock,
            output_clock,
            foreign,
            mut transient,
            external,
        } = context;
        let mut refuse_foreign = RefuseForeign;
        let foreign: &mut dyn ForeignProcessors = foreign.unwrap_or(&mut refuse_foreign);
        let requested = transient.as_ref().map(|context| context.input);
        let mut applied = None;
        let result =
            (|| {
                let entries = self
                    .entries
                    .get_mut(range)
                    .ok_or(DeviceContractError::InvalidDescriptor)?;
                if let Some(requested) = requested
                    && entries
                        .iter()
                        .filter(|entry| {
                            entry.owner == requested.identity.owner()
                                && entry.device_id == requested.identity.device_id()
                        })
                        .take(2)
                        .count()
                        > 1
                {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                if let Some(set) = external {
                    for (index, entry) in entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| !entry.automation_frozen)
                    {
                        let identity = ProcessorInstanceIdentity {
                            owner: entry.owner,
                            device_id: entry.device_id,
                        };
                        if let Some(window) = set.window(identity)
                            && (window.identity != identity
                                || entries[..index].iter().any(|previous| {
                                    !previous.automation_frozen
                                        && previous.owner == entry.owner
                                        && previous.device_id == entry.device_id
                                }))
                        {
                            return Err(DeviceContractError::InvalidDescriptor);
                        }
                    }
                }
                for entry in entries {
                    let input = requested
                        .filter(|requested| {
                            !entry.automation_frozen
                                && entry.owner == requested.identity.owner()
                                && entry.device_id == requested.identity.device_id()
                        })
                        .map(|requested| requested.operation);
                    if let Some(operation) = input {
                        if entry.automation.input_target() != Some(operation.target()) {
                            return Err(DeviceContractError::InvalidDescriptor);
                        }
                        if let Some(value) = operation.value() {
                            entry.automation.validate_input(value)?;
                        }
                        if let hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter) =
                            operation.target()
                            && (!entry.matrix.is_identity()
                                || !entry.runtime.supports_parameter_automation(parameter))
                        {
                            return Err(DeviceContractError::UnsupportedProcessingClass);
                        }
                    }

                    if !frames.is_empty()
                        && !entry.bypassed
                        && let Some(operation) = input
                    {
                        host_output_gain_ramp::stage_transient(entry, operation)?;
                    }
                    observer(
                        entry.owner,
                        entry.device_id,
                        entry.device_type,
                        NodeSignalPhase::Input,
                        frame_offset,
                        frames,
                    );
                    let identity = ProcessorInstanceIdentity {
                        owner: entry.owner,
                        device_id: entry.device_id,
                    };
                    let window = if entry.automation_frozen {
                        None
                    } else {
                        external.and_then(|set| set.window(identity))
                    };
                    if window.is_some_and(|window| window.identity != identity) {
                        return Err(DeviceContractError::InvalidDescriptor);
                    }
                    let started_powered_off =
                        input.is_some() && entry.powered_off(entry.scratch_bypass_ramp);
                    Self::process_entry_with_foreign(
                        entry,
                        frames,
                        EntryProcessContext {
                            key,
                            frame_offset,
                            clock,
                            output_clock,
                            foreign,
                            input: input.and_then(NativeTransientOperation::value).filter(
                                |value| !matches!(value, NativeTransientValue::HostOutputGain(_)),
                            ),
                            external: window,
                        },
                    )?;
                    if !frames.is_empty()
                        && !started_powered_off
                        && let Some(operation) = input
                        && (!entry.bypassed
                            || matches!(
                                operation.target(),
                                hex_lattice_contract::DeviceAutomationTarget::Parameter(_)
                            ))
                    {
                        applied = Some(NativeTransientApplication {
                            identity: ProcessorInstanceIdentity {
                                owner: entry.owner,
                                device_id: entry.device_id,
                            },
                            operation,
                            offset: frame_offset,
                        });
                    }

                    observer(
                        entry.owner,
                        entry.device_id,
                        entry.device_type,
                        NodeSignalPhase::Output,
                        frame_offset,
                        frames,
                    );
                }
                Ok(())
            })();
        if result.is_err() {
            foreign.reject_callback();
            if let Some(context) = &mut transient {
                context.applied = None;
            }
        } else if let Some(context) = &mut transient
            && context.applied.is_none()
        {
            context.applied = applied;
        }
        result
    }

    /// Commit histories after all render chunks and source reads succeeded.
    pub fn commit_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                // Commit follows successful rendering of every provider; a
                // refused render aborts the enclosing transaction instead.
                let _ = modulation.commit();
            }
            entry.committed_host_gains = entry.scratch_host_gains;
            entry.committed_output_gain_ramp = entry.scratch_output_gain_ramp;
            entry.committed_transient_output_gain = entry.scratch_transient_output_gain;
            entry.committed_matrix_ramp = entry.scratch_matrix_ramp;
            let was_powered_off = entry.powered_off(entry.committed_bypass_ramp);
            entry.committed_bypass_ramp = entry.scratch_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.committed_aligned_dry, &entry.scratch_aligned_dry);
            if !was_powered_off {
                entry.runtime.commit();
                if entry.powered_off(entry.committed_bypass_ramp) {
                    entry.runtime.reset();
                }
                let reading = entry.runtime.take_dynamics_meter();
                if let Some(reading) = reading.filter(|_| entry.metering && !entry.bypassed) {
                    entry.meter.observe_dynamics(reading);
                }
            }
        }
    }
    /// Restore every speculative effect after a later host render stage refuses.
    pub fn rollback_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.rollback();
            }
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.scratch_output_gain_ramp = entry.committed_output_gain_ramp;
            entry.scratch_transient_output_gain = entry.committed_transient_output_gain;
            entry.scratch_matrix_ramp = entry.committed_matrix_ramp;
            entry.scratch_bypass_ramp = entry.committed_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            if !entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.rollback();
            }
        }
    }

    /// Reset both committed and speculative state at a hard discontinuity.
    pub fn reset(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.reset_all();
            }
            entry.committed_host_gains = [entry.input_gain, entry.output_gain];
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.committed_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
            entry.committed_transient_output_gain = Default::default();
            entry.scratch_transient_output_gain = Default::default();
            entry.committed_matrix_ramp = matrix_ramp::MatrixRamp::INACTIVE;
            entry.scratch_matrix_ramp = matrix_ramp::MatrixRamp::INACTIVE;
            entry.committed_bypass_ramp = bypass_ramp::BypassRamp::INACTIVE;
            entry.scratch_bypass_ramp = bypass_ramp::BypassRamp::INACTIVE;
            if let Some(history) = &mut entry.committed_aligned_dry {
                history.reset();
            }
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            entry.runtime.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_block_processing_passes_audio_unchanged() {
        let mut chain = NativeRuntimeChain::empty();
        let mut block = [[0.25, -0.5]; 4];
        chain.begin_block();
        chain
            .process_block(&mut block)
            .expect("process empty chain");
        chain.commit_block();
        assert_eq!(block, [[0.25, -0.5]; 4]);
        chain.rollback_block();
        chain.reset();
    }
}
