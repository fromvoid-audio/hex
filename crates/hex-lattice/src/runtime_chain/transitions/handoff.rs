use super::*;

impl NativeRuntimeChain {
    /// Preflight every twin before copying any state. Matching identities whose
    /// runtime compatibility changed remain freshly reset on the candidate.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        self.can_inherit_from_excluding_old_range(previous, 0..0)
    }

    /// Whether any stateful entry must defer a handoff from the same-identity
    /// entry of `previous` until that committed runtime's live transitions
    /// drain (Pulse #4178; see `NativeProcessorRuntime::transition_busy_from`).
    /// Hosts check this during pure handoff preflight, before inheritance,
    /// tail capture or reset classification, and refuse the whole handoff as
    /// transient instead of choosing another history path. It is conservative
    /// for entries a retained tail would keep. Bounded and allocation-free:
    /// one identity scan per candidate entry, like `can_inherit_from`.
    /// Settle the committed runtimes that `candidate` is
    /// `transition_busy_from`, while this (previous) chain's graph renders
    /// nothing (Pulse #4558). Pairs entries exactly as
    /// `transition_busy_from`; bounded and allocation-free. See
    /// `NativeProcessorRuntime::settle_idle_transitions`.
    pub fn settle_idle_transitions_for(&mut self, candidate: &Self) {
        for entry in candidate
            .entries
            .iter()
            .filter(|entry| entry.runtime.is_stateful())
        {
            if let Some(old) = self
                .entries
                .iter_mut()
                .find(|old| Self::same_runtime_identity(entry, old) && old.runtime.is_stateful())
                && entry.runtime.transition_busy_from(&old.runtime)
            {
                old.runtime.settle_idle_transitions();
            }
        }
    }
    pub fn transition_busy_from(&self, previous: &Self) -> bool {
        self.entries.iter().any(|entry| {
            entry.runtime.is_stateful()
                && previous
                    .entries
                    .iter()
                    .find(|old| {
                        Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                    })
                    .is_some_and(|old| entry.runtime.transition_busy_from(&old.runtime))
        })
    }

    /// Preflight history inheritance while leaving a range of old chain entries
    /// with a retained tail owner. Exclusion is resolved by old stable device
    /// identity, so candidate reordering cannot accidentally inherit a tail's
    /// history into the new signal path.
    pub fn can_inherit_from_excluding_old_range(
        &self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) -> bool {
        self.can_inherit_preflight(previous, HistoryExclusion::RetainedTail(excluded_old_range))
    }

    /// Preflight a completed foreign suffix without retaining its old endpoint.
    /// The range is only a hint: each excluded entry must be an actual foreign
    /// runtime whose committed provider observation is quiescent. Speculative
    /// silence, owned DSP and incompatible nonforeign histories remain subject
    /// to the ordinary inheritance checks. This grants no host resource return
    /// or native source/admission authority.
    pub fn can_inherit_with_completed_foreign_range(
        &self,
        previous: &Self,
        completed_old_range: std::ops::Range<usize>,
    ) -> bool {
        self.can_inherit_preflight(
            previous,
            HistoryExclusion::CompletedForeign(completed_old_range),
        )
    }

    fn can_inherit_preflight(&self, previous: &Self, excluded: HistoryExclusion) -> bool {
        if !excluded.valid_for(previous.len()) {
            return false;
        }
        self.entries.iter().all(|entry| {
            if !entry.runtime.is_stateful() {
                return true;
            }
            let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
            }) else {
                return true;
            };
            excluded.skips(index, old)
                || (entry.runtime.can_inherit_from(&old.runtime)
                    && Self::can_preserve_aligned_dry(entry, old))
        })
    }
    pub(super) fn can_preserve_aligned_dry(candidate: &RuntimeEntry, old: &RuntimeEntry) -> bool {
        match (&candidate.committed_aligned_dry, &old.committed_aligned_dry) {
            (Some(candidate), Some(old)) => candidate.can_copy_from(old) || old.is_quiet(),
            (None, Some(old)) => old.is_quiet(),
            _ => true,
        }
    }
    pub(crate) fn same_runtime_identity(a: &RuntimeEntry, b: &RuntimeEntry) -> bool {
        a.owner == b.owner
            && a.powered_off == b.powered_off
            && a.device_id == b.device_id
            && a.device_type == b.device_type
            && a.runtime_version == b.runtime_version
            && a.sample_rate == b.sample_rate
    }

    /// Check whether one exact processor can seed a pristine successor with
    /// timing/control state while its donor retains all signal history. This
    /// does not transfer custody and unsupported providers refuse.
    pub fn can_fork_timing_without_signal_history_at(&self, previous: &Self, index: usize) -> bool {
        let Some((new, old)) = self.entries.get(index).zip(previous.entries.get(index)) else {
            return false;
        };
        Self::same_runtime_identity(new, old)
            // A timing-only fork has no control-trajectory transfer contract.
            // Keep its existing fallback until an active output ramp is proved.
            && !old.committed_output_gain_ramp.active()
            && new.input_gain == old.input_gain
            && new.output_gain == old.output_gain
            && new.matrix == old.matrix
            && new.automation == old.automation
            && match (&new.configuration, &old.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b) || a.processing_eq_on_callback(b),
                _ => false,
            }
            && new
                .runtime
                .can_fork_timing_without_signal_history_from(&old.runtime)
    }

    /// Seed the checked successor without moving or copying donor signal history.
    /// Refusal leaves both runtime owners untouched; all work is bounded and
    /// provider implementations must not allocate, block or destroy storage.
    pub fn fork_timing_without_signal_history_at(
        &mut self,
        previous: &Self,
        index: usize,
    ) -> Result<(), DeviceContractError> {
        if !self.can_fork_timing_without_signal_history_at(previous, index) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let Some((new, old)) = self.entries.get_mut(index).zip(previous.entries.get(index)) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if !new
            .runtime
            .fork_timing_without_signal_history_from(&old.runtime)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        Ok(())
    }

    /// Copy compatible histories to both twins after a successful preflight.
    /// Identity also includes the stable device type, runtime version and rate;
    /// persisted state versions may change while physical history is retained.
    pub fn inherit_from(&mut self, previous: &Self) {
        self.inherit_from_excluding_old_range_unchecked(previous, 0..0);
    }

    /// Copy compatible histories except entries retained by an old tail owner.
    /// Callers must preflight the same range before entering their commit phase.
    pub fn inherit_from_excluding_old_range(
        &mut self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        if excluded_old_range.start > excluded_old_range.end
            || excluded_old_range.end > previous.entries.len()
        {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        self.inherit_from_excluding_old_range_unchecked(previous, excluded_old_range);
        Ok(())
    }

    fn inherit_from_excluding_old_range_unchecked(
        &mut self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) {
        let excluded = HistoryExclusion::RetainedTail(excluded_old_range);
        self.inherit_host_stage_ramps(previous, &excluded);
        self.inherit_bypass_ramps(previous, &excluded);
        self.inherit_modulation(previous, &excluded);
        for entry in &mut self.entries {
            if entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.reset();
            } else if let Some((index, old)) =
                previous.entries.iter().enumerate().find(|(_, old)| {
                    Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                })
                && !excluded.skips(index, old)
            {
                entry.runtime.inherit_from(&old.runtime);
            }
        }
    }

    /// Commit-phase history transfer from an exclusively borrowed retiring
    /// chain, after the same identity/history preflight as ordinary inheritance.
    /// Owned providers may exchange their preallocated banks in constant time;
    /// legacy and foreign entries retain their existing copy laws. Neither
    /// chain nor any resource is destroyed; the host still retires the donor.
    pub fn transfer_history_from(&mut self, previous: &mut Self) {
        self.transfer_history_excluding_unchecked(previous, HistoryExclusion::RetainedTail(0..0));
    }

    /// Find a single new native stage that can ramp in at the same sample
    /// origin. Every other stage must retain identity, order and compatible
    /// configuration and history. The host separately proves routing and timing.
    pub fn native_insert_transition_from(
        &self,
        previous: &Self,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> Option<NativeInsertTransition> {
        let position = self.extra_native_stage_from(
            previous,
            |added| !added.bypassed,
            |old, current| Self::entry_configuration_matches(old, current, continuation),
        )?;
        self.can_inherit_from(previous)
            .then_some(NativeInsertTransition { position })
    }

    /// Prove one smoothed zero-latency removal, including a memoryless matrix,
    /// with all surviving stages in order and history-compatible.
    pub fn native_removal_transition_from(
        &self,
        previous: &Self,
    ) -> Option<NativeRemovalTransition> {
        let position = previous.extra_native_stage_from(
            self,
            |removed| {
                !removed.removal_bypassing
                    && (!removed.bypassed
                        || removed.powered_off(removed.committed_bypass_ramp)
                        || matches!(removed.runtime, RuntimeOwnership::Memoryless))
            },
            |current, old| Self::entry_configuration_matches(old, current, None),
        )?;
        let removed = previous.entries.get(position)?;
        let identity = ProcessorInstanceIdentity::new(removed.owner, removed.device_id).ok()?;
        self.can_inherit_from(previous)
            .then_some(NativeRemovalTransition {
                position,
                identity,
                authored_bypass: removed.bypassed,
                foreign_pass: None,
            })
    }

    /// One outgoing Main stage (removed or powered off) in a single- or
    /// multi-device chain whose pass-level latency is retained until Stop.
    pub fn main_pass_removal_from(&self, previous: &Self) -> Option<NativeRemovalTransition> {
        self.pass_removal_from(previous, ProcessorChainOwner::Main)
    }

    /// One outgoing Return stage (removed or powered off) in a single- or
    /// multi-device chain whose pass-level latency is retained until Stop.
    pub fn return_power_off_from(
        &self,
        previous: &Self,
        return_id: u64,
    ) -> Option<NativeRemovalTransition> {
        self.pass_removal_from(previous, ProcessorChainOwner::Return(return_id))
    }

    /// One outgoing Track stage (removed or powered off) in a single- or
    /// multi-device chain whose pass-level latency is retained until Stop.
    pub fn track_pass_removal_from(
        &self,
        previous: &Self,
        track_id: u64,
    ) -> Option<NativeRemovalTransition> {
        self.pass_removal_from(previous, ProcessorChainOwner::Track(track_id))
    }

    fn pass_removal_from(
        &self,
        previous: &Self,
        owner: ProcessorChainOwner,
    ) -> Option<NativeRemovalTransition> {
        let max_delay = crate::MAX_MAIN_PASS_DELAY_FRAMES;
        let is_powered_off_match = |old: &RuntimeEntry, new: &RuntimeEntry| -> bool {
            new.powered_off
                && new.bypassed
                && new.owner == old.owner
                && new.device_id == old.device_id
                && new.device_type == old.device_type
                && new.sample_rate == old.sample_rate
                && new.algorithmic_latency_frames == Some(0)
                && new.input_gain == old.input_gain
                && new.output_gain == old.output_gain
                && new
                    .configuration
                    .as_deref()
                    .zip(old.configuration.as_deref())
                    .is_some_and(|(a, b)| a.processing_eq_on_callback(b))
        };

        // Match either (a) single stage removed from a chain of any length, or
        // (b) single stage switched to genuine powered-off in a chain of any length.
        let (position, entry) = if previous.entries.len().checked_sub(1) == Some(self.entries.len())
        {
            let pos = self
                .entries
                .iter()
                .zip(&previous.entries)
                .position(|(next, prev)| !Self::same_runtime_identity(next, prev))
                .unwrap_or(self.entries.len());
            let entry = previous.entries.get(pos)?;
            let remaining = previous
                .entries
                .iter()
                .take(pos)
                .chain(previous.entries.iter().skip(pos + 1));
            if !self
                .entries
                .iter()
                .zip(remaining)
                .all(|(next, prev)| Self::same_runtime_identity(next, prev))
                || !self.can_inherit_from(previous)
            {
                return None;
            }
            (pos, entry)
        } else if previous.entries.len() == self.entries.len() && !previous.entries.is_empty() {
            let mut found = None;
            for (pos, (prev, next)) in previous.entries.iter().zip(&self.entries).enumerate() {
                if !prev.bypassed && is_powered_off_match(prev, next) {
                    if found.replace((pos, prev)).is_some() {
                        return None;
                    }
                } else if !Self::same_runtime_identity(next, prev) {
                    return None;
                }
            }
            let (pos, entry) = found?;
            // Every sibling stage outside `pos` must be inheritable.
            if !self.can_inherit_from_excluding_old_range(previous, pos..pos + 1) {
                return None;
            }
            (pos, entry)
        } else {
            return None;
        };

        if entry.owner != owner {
            return None;
        }
        let (latency_frames, foreign_pass) = match &entry.runtime {
            RuntimeOwnership::Memoryless => return None,
            RuntimeOwnership::Foreign(runtime) => {
                let prepared = runtime.prepared;
                if entry.device_type != DeviceTypeId::from_static("hex.vst3")
                    || prepared.is_instrument_profile()
                {
                    return None;
                }
                let frames = NonZeroU32::new(prepared.total_latency_frames())?;
                if frames.get() > max_delay {
                    return None;
                }
                (frames.get(), Some((prepared.binding()?, frames)))
            }
            _ => {
                let frames = entry
                    .committed_aligned_dry
                    .as_ref()
                    .and_then(|dry| dry.main_pass_latency_frames())
                    .or_else(|| {
                        entry
                            .algorithmic_latency_frames
                            .filter(|&f| f > 0 && f <= max_delay)
                    })?;
                (frames, None)
            }
        };
        if entry.algorithmic_latency_frames != Some(latency_frames)
            || entry.bypassed
            || entry.removal_bypassing
            || entry.committed_bypass_ramp.is_active()
            || entry.committed_matrix_ramp.is_active()
            || entry.committed_output_gain_ramp.active()
            || entry.committed_transient_output_gain != host_output_gain_ramp::TransientGain::Follow
        {
            return None;
        }
        Some(NativeRemovalTransition {
            position,
            identity: ProcessorInstanceIdentity::new(entry.owner, entry.device_id).ok()?,
            authored_bypass: false,
            foreign_pass,
        })
    }

    /// Copy the exact settled Main delay without transferring its processor.
    pub fn settled_main_pass_delay(
        &self,
        transition: NativeRemovalTransition,
    ) -> Option<MainPassDelay> {
        if transition.identity().owner() != ProcessorChainOwner::Main
            || !self.native_removal_ready(transition)
        {
            return None;
        }
        self.entries
            .get(transition.position)?
            .committed_aligned_dry
            .as_ref()?
            .main_pass_delay()
    }

    fn extra_native_stage_from(
        &self,
        shorter: &Self,
        accepts_extra: impl Fn(&RuntimeEntry) -> bool,
        compatible: impl Fn(&RuntimeEntry, &RuntimeEntry) -> bool,
    ) -> Option<usize> {
        if shorter.len().checked_add(1) != Some(self.len()) {
            return None;
        }
        let position = shorter
            .entries
            .iter()
            .zip(&self.entries)
            .position(|(short, long)| !Self::same_runtime_identity(short, long))
            .unwrap_or(shorter.len());
        let extra = self.entries.get(position)?;
        let retained = self
            .entries
            .iter()
            .take(position)
            .chain(self.entries.iter().skip(position + 1));
        (accepts_extra(extra)
            && extra.algorithmic_latency_frames == Some(0)
            && extra.supports_smoothed_bypass
            && !matches!(extra.runtime, RuntimeOwnership::Foreign(_))
            && shorter.entries.iter().zip(retained).all(|(short, long)| {
                short.device_id != extra.device_id
                    && short.algorithmic_latency_frames == long.algorithmic_latency_frames
                    && compatible(short, long)
            }))
        .then_some(position)
    }

    /// Start a proved removal on its predecessor. Only the affected stage's
    /// wet/dry mix changes. Its DSP stays owned by the predecessor.
    pub fn begin_native_removal(&mut self, transition: NativeRemovalTransition) -> bool {
        if transition.authored_bypass {
            self.entries.get(transition.position).is_some_and(|entry| {
                transition.matches(entry) && entry.bypassed && !entry.removal_bypassing
            })
        } else {
            self.set_native_removal_bypass(transition, true)
        }
    }

    /// Restore the same stage if the pending edit is cancelled or refused.
    pub fn cancel_native_removal(&mut self, transition: NativeRemovalTransition) {
        if !transition.authored_bypass {
            self.set_native_removal_bypass(transition, false);
        }
    }

    /// Either the authored bypass or the provisional removal must reach dry.
    pub fn native_removal_ready(&self, transition: NativeRemovalTransition) -> bool {
        self.entries.get(transition.position).is_some_and(|entry| {
            transition.matches(entry)
                && entry.bypassed
                && entry.removal_bypassing != transition.authored_bypass
                && !entry.committed_bypass_ramp.is_active()
        })
    }

    fn set_native_removal_bypass(
        &mut self,
        transition: NativeRemovalTransition,
        bypassed: bool,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(transition.position) else {
            return false;
        };
        if !transition.matches(entry)
            || entry.bypassed == bypassed
            || entry.removal_bypassing == bypassed
        {
            return false;
        }
        // The foreign endpoint already mixes against its aligned dry ring.
        let ramp = if transition.foreign_pass.is_some() {
            bypass_ramp::BypassRamp::INACTIVE
        } else {
            bypass_ramp::BypassRamp::inherited(
                entry.committed_bypass_ramp,
                entry.bypassed,
                bypassed,
                entry.sample_rate,
            )
        };
        entry.bypassed = bypassed;
        entry.removal_bypassing = bypassed;
        entry.committed_bypass_ramp = ramp;
        entry.scratch_bypass_ramp = ramp;
        true
    }

    /// Commit the preflighted transition against the same two chains. The new
    /// stage ramps from dry to wet on the same signal. Only the new graph runs.
    pub fn transfer_history_with_native_insert_from(
        &mut self,
        previous: &mut Self,
        transition: NativeInsertTransition,
    ) {
        self.transfer_history_from(previous);
        if let Some(entry) = self.entries.get_mut(transition.position) {
            let ramp = bypass_ramp::BypassRamp::inherited(
                bypass_ramp::BypassRamp::INACTIVE,
                true,
                false,
                entry.sample_rate,
            );
            entry.committed_bypass_ramp = ramp;
            entry.scratch_bypass_ramp = ramp;
        }
    }

    /// Transfer only histories not retained by the donor's old output suffix.
    pub fn transfer_history_excluding_old_range(
        &mut self,
        previous: &mut Self,
        excluded: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        self.transfer_history_excluding(previous, HistoryExclusion::RetainedTail(excluded))
    }

    /// Transfer history after preflighting `can_inherit_with_completed_foreign_range`
    /// for the same range. Only quiescent foreign entries in it stay with the
    /// retiring chain. The caller supplies the separate endpoint-custody proof.
    pub fn transfer_history_with_completed_foreign_range(
        &mut self,
        previous: &mut Self,
        completed: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        self.transfer_history_excluding(previous, HistoryExclusion::CompletedForeign(completed))
    }

    fn transfer_history_excluding(
        &mut self,
        previous: &mut Self,
        excluded: HistoryExclusion,
    ) -> Result<(), DeviceContractError> {
        if !excluded.valid_for(previous.len()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        self.transfer_history_excluding_unchecked(previous, excluded);
        Ok(())
    }
    fn transfer_history_excluding_unchecked(
        &mut self,
        previous: &mut Self,
        excluded: HistoryExclusion,
    ) {
        self.inherit_host_stage_ramps(previous, &excluded);
        self.inherit_bypass_ramps(previous, &excluded);
        self.inherit_modulation(previous, &excluded);
        for entry in &mut self.entries {
            if entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.reset();
            } else if let Some((index, old)) =
                previous.entries.iter_mut().enumerate().find(|(_, old)| {
                    Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                })
                && !excluded.skips(index, old)
            {
                entry.runtime.transfer_history_from(&mut old.runtime);
            }
        }
    }

    fn inherit_modulation(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            if let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                entry.owner == old.owner
                    && entry.device_id == old.device_id
                    && entry.device_type == old.device_type
                    && entry.sample_rate == old.sample_rate
            }) && !excluded.skips(index, old)
                && let (Some(current), Some(old)) = (&mut entry.modulation, &old.modulation)
            {
                current.inherit_graph_committed(old);
            }
        }
    }

    /// Reuse one same-identity donor lookup for the independent output-trim
    /// and static-matrix trajectories. Excluded donors, bypass and automation
    /// keep the existing matrix law; the scalar proof lives with its owner.
    fn inherit_host_stage_ramps(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            host_output_gain_ramp::inherit(
                entry,
                old.filter(|&(index, old)| !excluded.skips(index, old))
                    .map(|(_, old)| old),
            );
            let ramp = match old {
                Some((index, old)) => matrix_ramp::MatrixRamp::inherited(
                    old.committed_matrix_ramp,
                    old.matrix,
                    entry.matrix,
                    entry.sample_rate,
                    !excluded.skips(index, old)
                        && !entry.bypassed
                        && !old.bypassed
                        && entry.automation.is_empty()
                        && old.automation.is_empty()
                        && !matches!(entry.runtime, RuntimeOwnership::Foreign(_)),
                ),
                None => matrix_ramp::MatrixRamp::INACTIVE,
            };
            entry.committed_matrix_ramp = ramp;
            entry.scratch_matrix_ramp = ramp;
        }
    }

    fn inherit_bypass_ramps(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            let ramp = match old {
                Some((index, old))
                    if !excluded.skips(index, old)
                        && entry.can_smooth_bypass_from(old)
                        && entry.input_gain == old.input_gain
                        && entry.output_gain == old.output_gain
                        && !matches!(entry.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(old.runtime, RuntimeOwnership::Foreign(_))
                        && entry.matrix == old.matrix
                        && entry.automation == old.automation
                        && match (&entry.configuration, &old.configuration) {
                            (Some(a), Some(b)) => {
                                std::sync::Arc::ptr_eq(a, b)
                                    || a.processing_eq_on_callback(b)
                                    || (entry.runtime.can_inherit_from(&old.runtime)
                                        && entry.runtime.can_retarget_from(&old.runtime))
                            }
                            _ => false,
                        } =>
                {
                    let ramp = bypass_ramp::BypassRamp::inherited(
                        old.committed_bypass_ramp,
                        old.bypassed,
                        entry.bypassed,
                        entry.sample_rate,
                    );
                    if entry.supports_power_off
                        && !entry.bypassed
                        && old.powered_off(old.committed_bypass_ramp)
                    {
                        bypass_ramp::BypassRamp::fresh_power_on(
                            entry.algorithmic_latency_frames.unwrap_or(0),
                            entry.sample_rate,
                        )
                    } else {
                        ramp
                    }
                }
                _ => bypass_ramp::BypassRamp::INACTIVE,
            };
            let inherited_dry = if let Some((index, old)) = old
                && !excluded.skips(index, old)
                && entry.committed_aligned_dry.is_some()
                && old.committed_aligned_dry.is_some()
                && entry.algorithmic_latency_frames == old.algorithmic_latency_frames
            {
                let committed = bypass_ramp::copy_dry(
                    &mut entry.committed_aligned_dry,
                    &old.committed_aligned_dry,
                );
                let scratch = bypass_ramp::copy_dry(
                    &mut entry.scratch_aligned_dry,
                    &old.committed_aligned_dry,
                );
                committed && scratch
            } else {
                false
            };
            if !inherited_dry {
                // A proved quiet layout change starts fresh, even if this
                // prepared candidate previously rendered another signal.
                // Reset in place; never replace or retire storage on audio.
                if let Some(history) = &mut entry.committed_aligned_dry {
                    history.reset();
                }
                if let Some(history) = &mut entry.scratch_aligned_dry {
                    history.reset();
                }
            }
            entry.committed_bypass_ramp = ramp;
            entry.scratch_bypass_ramp = ramp;
        }
    }
}
