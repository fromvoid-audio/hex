use super::*;

/// Paired mutable MIDI processor runtime with committed primary history and
/// speculative scratch history. Construction and final retirement belong on the
/// control side; callback processing borrows caller-owned event slices and
/// performs zero allocations, locks, I/O, or final drops.
pub struct NativeMidiProcessorRuntimePair {
    runtime_version: u32,
    declared_pair_bytes: usize,
    max_input_events_per_block: usize,
    max_output_events_per_block: usize,
    max_active_occurrences: usize,
    max_work_units_per_block: usize,
    primary: Box<dyn MutableMidiProcessorRuntime>,
    scratch: Box<dyn MutableMidiProcessorRuntime>,
    block_started: bool,
}

impl NativeMidiProcessorRuntimePair {
    /// Validate declarations against caller limits before admitting either twin.
    pub fn new(
        prepared: &dyn PreparedNativeMidiProcessor,
        limits: NativeMidiProcessorLimits,
    ) -> Result<Self, NativeMidiProcessorError> {
        let runtime_version = prepared.runtime_version();
        let declared_twin_bytes = prepared.declared_twin_bytes();
        let max_input_events_per_block = prepared.max_input_events_per_block();
        let max_output_events_per_block = prepared.max_output_events_per_block();
        let max_active_occurrences = prepared.max_active_occurrences();
        let max_work_units_per_block = prepared.max_work_units_per_block();
        let declared_pair_bytes = declared_twin_bytes
            .checked_mul(2)
            .ok_or(NativeMidiProcessorError::RuntimeBudgetExceeded)?;

        if runtime_version == 0
            || declared_twin_bytes == 0
            || declared_pair_bytes > limits.max_pair_bytes
            || max_input_events_per_block == 0
            || max_input_events_per_block > limits.max_input_events_per_block
            || max_output_events_per_block == 0
            || max_output_events_per_block > limits.max_output_events_per_block
            || max_active_occurrences == 0
            || max_active_occurrences > limits.max_active_occurrences
            || max_work_units_per_block == 0
            || max_work_units_per_block > limits.max_work_units_per_block
        {
            return Err(NativeMidiProcessorError::RuntimeBudgetExceeded);
        }

        let primary = prepared.create_runtime();
        let scratch = prepared.create_runtime();
        if !primary.can_copy_history_from(scratch.as_ref())
            || !scratch.can_copy_history_from(primary.as_ref())
            || !primary.is_quiescent()
            || !scratch.is_quiescent()
        {
            return Err(NativeMidiProcessorError::IncompatibleTwins);
        }
        if primary.fixed_storage_bytes() > declared_twin_bytes
            || scratch.fixed_storage_bytes() > declared_twin_bytes
        {
            return Err(NativeMidiProcessorError::RuntimeBudgetExceeded);
        }

        Ok(Self {
            runtime_version,
            declared_pair_bytes,
            max_input_events_per_block,
            max_output_events_per_block,
            max_active_occurrences,
            max_work_units_per_block,
            primary,
            scratch,
            block_started: false,
        })
    }

    pub const fn runtime_version(&self) -> u32 {
        self.runtime_version
    }

    pub const fn declared_pair_bytes(&self) -> usize {
        self.declared_pair_bytes
    }

    pub const fn max_input_events_per_block(&self) -> usize {
        self.max_input_events_per_block
    }

    pub const fn max_output_events_per_block(&self) -> usize {
        self.max_output_events_per_block
    }

    pub const fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    pub const fn max_work_units_per_block(&self) -> usize {
        self.max_work_units_per_block
    }

    pub fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        self.primary
            .fixed_storage_bytes()
            .checked_add(self.scratch.fixed_storage_bytes())
    }

    pub fn committed_is_quiescent(&self) -> bool {
        !self.block_started && self.primary.is_quiescent()
    }

    pub fn speculative_is_quiescent(&self) -> bool {
        self.block_started && self.scratch.is_quiescent()
    }

    pub fn committed_active_occurrences(&self) -> usize {
        self.primary.active_occurrence_count()
    }

    pub fn speculative_active_occurrences(&self) -> usize {
        if self.block_started {
            self.scratch.active_occurrence_count()
        } else {
            self.primary.active_occurrence_count()
        }
    }

    /// Begin a speculative block by copying committed primary state to scratch.
    pub fn begin_block(&mut self) {
        self.scratch.copy_history_from(self.primary.as_ref());
        self.block_started = true;
    }

    /// Validate and transform one bounded event batch into caller-owned
    /// preallocated output storage. Preflight completes before either scratch
    /// history or `output` changes.
    pub fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output: &mut [NativeInstrumentEvent],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        if !self.block_started {
            return Err(NativeMidiProcessorError::BlockNotStarted);
        }
        validate_midi_event_batch(
            input,
            frame_count,
            self.max_input_events_per_block,
            NativeMidiProcessorError::InputEventCapacityExceeded,
            NativeMidiProcessorError::InvalidInputEventBatch,
        )?;
        let effective_output_cap = output.len().min(self.max_output_events_per_block);
        let predicted =
            self.scratch
                .preflight_block(input, frame_count, effective_output_cap, clock)?;
        if predicted > effective_output_cap {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }

        let mut sink =
            MidiProcessorOutput::new(output, effective_output_cap, self.max_work_units_per_block);
        self.scratch
            .process_block(input, frame_count, &mut sink, clock);
        let written = sink.len();
        let work_units = sink.work_units();

        if written != predicted
            || work_units > self.max_work_units_per_block
            || self.scratch.active_occurrence_count() > self.max_active_occurrences
            || validate_midi_event_batch(
                &output[..written],
                frame_count,
                effective_output_cap,
                NativeMidiProcessorError::OutputEventCapacityExceeded,
                NativeMidiProcessorError::InvalidOutputEventBatch,
            )
            .is_err()
        {
            self.scratch.copy_history_from(self.primary.as_ref());
            return Err(NativeMidiProcessorError::InvalidOutputEventBatch);
        }

        Ok(written)
    }

    /// Emit deterministic paired `NoteOff` events for every active occurrence
    /// held in speculative scratch state at `frame_offset`.
    pub fn flush_active_note_offs(
        &mut self,
        frame_offset: usize,
        frame_count: usize,
        output: &mut [NativeInstrumentEvent],
    ) -> Result<usize, NativeMidiProcessorError> {
        if !self.block_started {
            return Err(NativeMidiProcessorError::BlockNotStarted);
        }
        if frame_count == 0 || frame_offset >= frame_count {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        let effective_output_cap = output.len().min(self.max_output_events_per_block);
        let predicted = self.scratch.preflight_flush_active_note_offs(
            frame_offset,
            frame_count,
            effective_output_cap,
        )?;
        if predicted > effective_output_cap {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }

        let mut sink =
            MidiProcessorOutput::new(output, effective_output_cap, self.max_work_units_per_block);
        self.scratch.flush_active_note_offs(frame_offset, &mut sink);
        let written = sink.len();

        if written != predicted
            || sink.work_units() > self.max_work_units_per_block
            || !self.scratch.is_quiescent()
            || validate_midi_event_batch(
                &output[..written],
                frame_count,
                effective_output_cap,
                NativeMidiProcessorError::OutputEventCapacityExceeded,
                NativeMidiProcessorError::InvalidOutputEventBatch,
            )
            .is_err()
        {
            self.scratch.copy_history_from(self.primary.as_ref());
            return Err(NativeMidiProcessorError::InvalidOutputEventBatch);
        }

        Ok(written)
    }

    /// Commit speculative scratch state as the new primary state without allocation.
    pub fn commit_block(&mut self) {
        if self.block_started {
            std::mem::swap(&mut self.primary, &mut self.scratch);
            self.block_started = false;
        }
    }

    /// Discard speculative scratch changes and keep committed primary intact.
    pub fn rollback_block(&mut self) {
        if self.block_started {
            self.scratch.copy_history_from(self.primary.as_ref());
            self.block_started = false;
        }
    }

    /// Clear both committed and scratch histories (Stop / hard reset).
    pub fn reset(&mut self) {
        self.primary.reset();
        self.scratch.reset();
        self.block_started = false;
    }

    /// Read-only compatibility preflight for transferring committed history
    /// across compatible graph revisions.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        !self.block_started
            && !previous.block_started
            && self.runtime_version == previous.runtime_version
            && self.max_active_occurrences >= previous.primary.active_occurrence_count()
            && self
                .primary
                .can_copy_history_from(previous.primary.as_ref())
            && self
                .scratch
                .can_copy_history_from(previous.primary.as_ref())
    }

    /// Copy committed state from `previous` into both twins after preflight.
    pub fn inherit_from(&mut self, previous: &Self) -> Result<(), NativeMidiProcessorError> {
        if !self.can_inherit_from(previous) {
            return Err(NativeMidiProcessorError::IncompatibleHistory);
        }
        self.primary.copy_history_from(previous.primary.as_ref());
        self.scratch.copy_history_from(previous.primary.as_ref());
        Ok(())
    }
}
