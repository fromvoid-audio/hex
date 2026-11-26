use super::*;

/// Paired mutable runtime with committed primary history and speculative
/// scratch history. Construction, compatibility decisions and final retirement
/// belong on the control side; processing only borrows caller-owned event/PCM
/// slices and performs no allocation or final drop.
pub struct NativeInstrumentRuntimePair {
    pub(super) active_occurrence_ids: bool,
    pub(super) occurrence_gain: bool,
    pub(super) note_pressure: bool,
    pub(super) parameter_automation: bool,
    pub(super) steals_releasing_voices: bool,
    pub(super) runtime_version: u32,
    pub(super) declared_pair_bytes: usize,
    pub(super) max_events_per_block: usize,
    pub(super) max_active_occurrences: usize,
    pub(super) max_release_tail_frames: u64,
    pub(super) primary: Box<dyn MutableInstrumentRuntime>,
    pub(super) scratch: Box<dyn MutableInstrumentRuntime>,
    pub(super) block_started: bool,
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
        let occurrence_gain = prepared.supports_occurrence_gain();
        let active_occurrence_ids = prepared.supports_active_occurrence_ids();
        let note_pressure = prepared.supports_note_pressure();
        let steals_releasing_voices = prepared.steals_releasing_voices();
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
            || max_active_occurrences
                .checked_add(max_events_per_block)
                .is_none()
        {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }

        let descriptors = prepared.parameter_automation_descriptors();
        automation::validate_grants(descriptors)?;
        let mut primary = prepared.create_runtime();
        let mut scratch = prepared.create_runtime();
        let parameter_automation = !descriptors.is_empty();
        if !automation::matches_grants(primary.as_mut(), prepared, descriptors)
            || !automation::matches_grants(scratch.as_mut(), prepared, descriptors)
        {
            return Err(NativeInstrumentRuntimeError::IncompatibleTwins);
        }
        automation::validate_target_views(primary.as_ref(), scratch.as_ref(), descriptors)?;
        if !primary.can_copy_history_from(scratch.as_ref())
            || !scratch.can_copy_history_from(primary.as_ref())
            || primary.supports_active_occurrence_ids() != active_occurrence_ids
            || scratch.supports_active_occurrence_ids() != active_occurrence_ids
            || primary.supports_occurrence_gain() != occurrence_gain
            || scratch.supports_occurrence_gain() != occurrence_gain
            || primary.supports_note_pressure() != note_pressure
            || scratch.supports_note_pressure() != note_pressure
        {
            return Err(NativeInstrumentRuntimeError::IncompatibleTwins);
        }
        if primary.fixed_storage_bytes() > declared_twin_bytes
            || scratch.fixed_storage_bytes() > declared_twin_bytes
        {
            return Err(NativeInstrumentRuntimeError::RuntimeBudgetExceeded);
        }

        Ok(Self {
            active_occurrence_ids,
            occurrence_gain,
            note_pressure,
            parameter_automation,
            steals_releasing_voices,
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

    pub const fn supports_active_occurrence_ids(&self) -> bool {
        self.active_occurrence_ids
    }
    pub const fn supports_note_pressure(&self) -> bool {
        self.note_pressure
    }

    /// Observe scratch while a speculative block is open, otherwise committed
    /// primary (also after rollback). This never opens/closes a block or changes
    /// history/PCM. The provider receives at most `max_active_occurrences` slots.
    /// Success returns a validated strictly ascending nonzero unique prefix;
    /// callers must ignore every output entry on error, including partial writes.
    /// Empty live state succeeds with zero entries even in an empty caller buffer.
    pub fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        if !self.active_occurrence_ids {
            return Err(NativeInstrumentRuntimeError::UnsupportedActiveOccurrenceIds);
        }
        let capacity = output.len().min(self.max_active_occurrences);
        let runtime = if self.block_started {
            &self.scratch
        } else {
            &self.primary
        };
        let count = runtime.active_occurrence_ids(&mut output[..capacity])?;
        if count > capacity {
            return Err(NativeInstrumentRuntimeError::InvalidActiveOccurrenceIds);
        }
        let ids = &output[..count];
        if ids.contains(&0) || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(NativeInstrumentRuntimeError::InvalidActiveOccurrenceIds);
        }
        Ok(count)
    }

    pub const fn supports_occurrence_gain(&self) -> bool {
        self.occurrence_gain
    }

    /// The factory's `steals_releasing_voices` declaration, fixed at admission.
    pub const fn steals_releasing_voices(&self) -> bool {
        self.steals_releasing_voices
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
    /// An open speculative block is never eligible for a committed checkpoint.
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
    /// Actual concrete Box payload sizes, without exposing either runtime.
    /// Matching a type does not prove absence of nested allocations; an owning
    /// host must separately audit its explicitly supported concrete class.
    pub fn inline_storage_for_type<T: Any>(&self) -> Option<(usize, usize)> {
        if !self.primary.as_any().is::<T>() || !self.scratch.as_any().is::<T>() {
            return None;
        }
        let primary = std::mem::size_of_val(self.primary.as_ref());
        let scratch = std::mem::size_of_val(self.scratch.as_ref());
        (primary == std::mem::size_of::<T>()
            && scratch == std::mem::size_of::<T>()
            && self.primary.fixed_storage_bytes() == primary
            && self.scratch.fixed_storage_bytes() == scratch)
            .then_some((primary, scratch))
    }

    /// Begin a speculative block by restoring scratch from committed primary.
    pub fn begin_block(&mut self) {
        self.scratch.copy_history_from(self.primary.as_ref());
        self.block_started = true;
    }

    /// Check generic event bounds and both twins' immutable event-domain,
    /// active-ID and capacity validation against their current histories.
    /// This does not inspect a clock, open a block, process PCM or change either
    /// history. Success is not renderability: ordinary processing still requires
    /// its complete event and clock preflight against current scratch state.
    pub fn preflight_event_batch(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        validate_event_batch(
            events,
            frame_count,
            self.max_events_per_block,
            self.note_pressure,
        )?;
        self.primary
            .preflight_events(events, frame_count, _resources)?;
        self.scratch
            .preflight_events(events, frame_count, _resources)
    }

    /// Validate and process one block/segment. Generic ordering and payload
    /// checks plus implementation-specific active-ID/capacity preflight all
    /// complete before either `frames` or scratch history is changed.
    pub fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.process_block_with_clock(events, frames, None, _resources)
    }

    /// Clock-aware transactional segment. All clock/event preflight completes
    /// before the scratch runtime or PCM is changed.
    pub fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.block_started {
            return Err(NativeInstrumentRuntimeError::BlockNotStarted);
        }
        validate_event_batch(
            events,
            frames.len(),
            self.max_events_per_block,
            self.note_pressure,
        )?;
        self.scratch
            .preflight_events(events, frames.len(), _resources)?;
        self.scratch
            .preflight_clock(clock, frames.len(), _resources)?;
        self.scratch
            .process_block_with_clock(events, frames, clock, _resources);
        Ok(())
    }

    /// Transactional per-occurrence gain segment. The original ramp span and
    /// slice offset preserve arbitrary block partitions. Work is bounded by the
    /// declared active occurrences plus block events (including new onsets).
    /// The primary remains the block's committed baseline; all segment preflight
    /// uses current scratch history, as with ordinary clock/event processing.
    pub fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[crate::NativeInstrumentOccurrenceGain],
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if gains.is_empty() {
            return self.process_block_with_clock(events, frames, clock, resources);
        }
        if !self.block_started {
            return Err(NativeInstrumentRuntimeError::BlockNotStarted);
        }
        if !self.occurrence_gain {
            return Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain);
        }
        let maximum = self
            .max_active_occurrences
            .checked_add(self.max_events_per_block)
            .ok_or(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)?;
        crate::instrument_gain::validate_occurrence_gains(gains, frames.len(), maximum)?;
        validate_event_batch(
            events,
            frames.len(),
            self.max_events_per_block,
            self.note_pressure,
        )?;
        self.scratch
            .preflight_events(events, frames.len(), resources)?;
        self.scratch
            .preflight_clock(clock, frames.len(), resources)?;
        self.scratch.preflight_occurrence_gains(events, gains)?;
        self.scratch
            .process_block_with_clock_and_occurrence_gains(events, frames, clock, gains, resources);
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
            && self.note_pressure == previous.note_pressure
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
            && self.note_pressure == previous.note_pressure
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

pub(crate) fn validate_event_batch(
    events: &[NativeInstrumentEvent],
    frame_count: usize,
    max_events: usize,
    note_pressure: bool,
) -> Result<(), NativeInstrumentRuntimeError> {
    if events.len() > max_events {
        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
    }
    let mut previous_offset = 0;
    for (index, event) in events.iter().copied().enumerate() {
        let offset = event.frame_offset();
        if offset >= frame_count
            || (index > 0 && offset < previous_offset)
            || event.occurrence_id() == Some(0)
            || matches!(event, NativeInstrumentEvent::Expression { expression, .. } if !expression.is_valid())
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::NotePressure { value, .. } = event {
            if value > 127 {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            if !note_pressure {
                return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
            }
        }
        if let NativeInstrumentEvent::NotePitchBend { cents, .. } = event
            && !(-4_800..=4_800).contains(&cents)
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::NoteTimbre { value, .. } = event
            && value > 127
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::ControlChange { cc, value, .. } = event
            && (cc > 127 || value > 127)
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::PitchBend { value, .. } = event
            && !(-8192..=8191).contains(&value)
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        if let NativeInstrumentEvent::NoteOn {
            occurrence_id,
            pitch,
            velocity,
            ..
        } = event
            && (pitch > 127
                || velocity == 0
                || velocity > 127
                || events[..index].iter().any(|prior| {
                    matches!(
                        prior,
                        NativeInstrumentEvent::NoteOn { occurrence_id: prior_id, .. }
                            if *prior_id == occurrence_id
                    )
                }))
        {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        previous_offset = offset;
    }
    Ok(())
}
