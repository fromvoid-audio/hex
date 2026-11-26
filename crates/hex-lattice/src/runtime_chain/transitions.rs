//! Live insert/removal/bypass transitions, history transfer, and quiescence queries.
use super::*;

/// One staged native insertion at a proved position in a pair of chains.
/// Constructed by `NativeRuntimeChain::native_insert_transition_from`.
#[derive(Clone, Copy)]
pub struct NativeInsertTransition {
    position: usize,
}

/// The exact stage on a retiring predecessor. An authored bypass retains its
/// existing path to dry and needs no provisional mix.
#[derive(Clone, Copy)]
pub struct NativeRemovalTransition {
    position: usize,
    identity: ProcessorInstanceIdentity,
    authored_bypass: bool,
    foreign_pass: Option<(hex_lattice_contract::ForeignProcessorBinding, NonZeroU32)>,
}

impl NativeRemovalTransition {
    pub fn identity(self) -> ProcessorInstanceIdentity {
        self.identity
    }

    /// The exact endpoint and frame count of this Main stage's aligned history.
    pub fn foreign_main_pass(self) -> Option<(hex_lattice_contract::ForeignProcessorBinding, u32)> {
        let (binding, frames) = self.foreign_pass?;
        (self.identity.owner() == ProcessorChainOwner::Main).then_some((binding, frames.get()))
    }

    /// The exact original binding for a hosted Return going Off or being removed.
    pub fn foreign_return_pass(self) -> Option<hex_lattice_contract::ForeignProcessorBinding> {
        let (binding, _) = self.foreign_pass?;
        matches!(self.identity.owner(), ProcessorChainOwner::Return(_)).then_some(binding)
    }

    /// The exact frame latency of a hosted Return stage going Off or being removed.
    pub fn foreign_return_pass_frames(self) -> Option<u32> {
        let (_, frames) = self.foreign_pass?;
        matches!(self.identity.owner(), ProcessorChainOwner::Return(_)).then_some(frames.get())
    }

    fn matches(self, entry: &RuntimeEntry) -> bool {
        entry.owner == self.identity.owner() && entry.device_id == self.identity.device_id()
            && self.foreign_pass.is_none_or(|(binding, frames)| {
                matches!(&entry.runtime,
                    RuntimeOwnership::Foreign(runtime) if runtime.prepared.binding() == Some(binding)
                        && runtime.prepared.total_latency_frames() == frames.get())
            })
    }
}

/// The retiring chain can keep a whole tail range, or leave only completed
/// foreign processors behind while other entries transfer normally.
enum HistoryExclusion {
    RetainedTail(std::ops::Range<usize>),
    CompletedForeign(std::ops::Range<usize>),
}

impl HistoryExclusion {
    fn range(&self) -> &std::ops::Range<usize> {
        match self {
            Self::RetainedTail(range) | Self::CompletedForeign(range) => range,
        }
    }

    fn valid_for(&self, len: usize) -> bool {
        let range = self.range();
        range.start <= range.end && range.end <= len
    }

    fn skips(&self, index: usize, old: &RuntimeEntry) -> bool {
        self.range().contains(&index)
            && (matches!(self, Self::RetainedTail(_))
                || (matches!(old.runtime, RuntimeOwnership::Foreign(_))
                    && old.runtime.committed_is_quiescent()
                    && old.committed_is_quiescent()))
    }
}

impl NativeRuntimeChain {
    /// Whether two complete chains perform the same static processing without
    /// signal, automation or unfinished host-ramp history. This observes only
    /// committed state; callers retain transport/source and ownership proofs.
    pub fn static_processing_matches(&self, other: &Self) -> bool {
        self.source_rejoin_processing_matches(other, |runtime| {
            matches!(runtime, RuntimeOwnership::Memoryless)
        })
    }

    /// Whether the whole post-instrument chain can copy committed history
    /// from a detached source. It must process the same stream without a clock
    /// or auxiliary signal, emit nothing autonomously, and explicitly admit
    /// this use. No ownership moves during preflight.
    pub fn detached_source_history_matches(&self, previous: &Self) -> bool {
        self.source_rejoin_processing_matches(previous, |runtime| match runtime {
            RuntimeOwnership::Memoryless => true,
            RuntimeOwnership::Legacy { primary, .. } => primary.supports_detached_source_rejoin(),
            RuntimeOwnership::Owned(runtime) => runtime.supports_detached_source_rejoin(),
            RuntimeOwnership::Foreign(_) => false,
        }) && self.can_inherit_from(previous)
    }

    fn source_rejoin_processing_matches(
        &self,
        other: &Self,
        supports: impl Fn(&RuntimeOwnership) -> bool,
    ) -> bool {
        let settled = |entry: &RuntimeEntry| {
            supports(&entry.runtime)
                && entry.algorithmic_latency_frames == Some(0)
                && entry.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                && entry.automation.is_empty()
                && entry.modulation.is_none()
                && !entry.committed_matrix_ramp.is_active()
                && !entry.committed_bypass_ramp.is_active()
                && !entry.committed_output_gain_ramp.active()
                && entry.committed_transient_output_gain
                    == host_output_gain_ramp::TransientGain::Follow
                && entry.committed_host_gains == [entry.input_gain, entry.output_gain]
        };
        self.len() == other.len()
            && self.configuration_matches_range(other, 0..self.len())
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| {
                    left.bypassed == right.bypassed
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && settled(left)
                        && settled(right)
                })
    }

    /// Whether a same-position range has exact immutable processor
    /// configuration identity, in addition to its prepared processing
    /// semantics. Missing synthetic identities compare conservatively false.
    /// Native processors explicitly supporting a compatible smoothed bypass
    /// may differ in bypass state; their history stays with the live chain. The
    /// same rule applies to memoryless matrix entries. Foreign sources retain
    /// their separate aligned-bypass identity contract. Different foreign
    /// program revisions can match only through exact retained program-owner
    /// correspondence, with unchanged native/host envelope metadata. The host
    /// still validates the current frozen quantum and actual source custody.
    /// Revision handoff can call this on the audio callback; it can inspect up to
    /// `range.len() * MAX_NATIVE_DEVICE_STATE_BYTES` payload bytes in the
    /// worst case. Larger VST3 state must share its immutable payload allocation.
    pub fn configuration_matches_range(&self, other: &Self, range: std::ops::Range<usize>) -> bool {
        self.configuration_matches_range_inner(other, range, None)
    }

    /// Compare a single host-authenticated foreign continuation alongside the
    /// ordinary immutable graph rules. This is comparison only: it neither
    /// validates the host's witness nor transfers source or tail custody.
    pub fn configuration_matches_range_with_foreign_continuation(
        &self,
        other: &Self,
        range: std::ops::Range<usize>,
        continuation: &ForeignConfigurationContinuation<'_>,
    ) -> bool {
        self.configuration_matches_range_inner(other, range, Some(continuation))
    }

    fn configuration_matches_range_inner(
        &self,
        other: &Self,
        range: std::ops::Range<usize>,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> bool {
        if range.start > range.end
            || range.end > self.entries.len()
            || range.end > other.entries.len()
        {
            return false;
        }
        self.entries[range.clone()]
            .iter()
            .zip(&other.entries[range])
            .all(|(left, right)| Self::entry_configuration_matches(left, right, continuation))
    }

    pub(crate) fn entry_configuration_matches(
        left: &RuntimeEntry,
        right: &RuntimeEntry,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> bool {
        left.owner == right.owner
        && left.device_id == right.device_id
        && left.device_type == right.device_type
        && left.runtime_version == right.runtime_version
        && left.sample_rate == right.sample_rate
        && left.runtime.foreign_configuration_matches(&right.runtime)
        // Foreign bypass is the host's aligned wet/dry selection:
        // the same source still processes, at the same latency.
        // This compares graph views only. The host must separately
        // retain/admit the actual endpoint and its dry history.
        && (left.bypassed == right.bypassed
            || (left.can_smooth_bypass_from(right)
                && left.input_gain == right.input_gain
                && left.output_gain == right.output_gain
                && !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                && !matches!(right.runtime, RuntimeOwnership::Foreign(_)))
            || matches!((&left.runtime, &right.runtime),
                (RuntimeOwnership::Foreign(left), RuntimeOwnership::Foreign(right))
                    if left.prepared == right.prepared))
        && (left.automation == right.automation
            || match (&left.runtime, &right.runtime) {
                (RuntimeOwnership::Foreign(a), RuntimeOwnership::Foreign(b))
                    if a.prepared == b.prepared => a.prepared.binding().is_some_and(|binding|
                        left.automation.shares_foreign_program_history_with(&right.automation, binding)),
                _ => false,
            })
        && left.matrix == right.matrix
        && match (&left.configuration, &right.configuration) {
            (Some(previous_state), Some(candidate_state)) => {
                std::sync::Arc::ptr_eq(previous_state, candidate_state)
                    || previous_state.processing_eq_on_callback(candidate_state)
                    || continuation.is_some_and(|proof| {
                        proof.matches(left, right, previous_state, candidate_state)
                    })
            }
            _ => false,
        }
    }

    /// Preflight a retiring-donor transfer for a source effect recipe. The
    /// native provider, trims and topology must still match. A live edit may
    /// replace a static matrix with a provider that explicitly proves it has
    /// no signal history to carry, or retarget a matching unautomated,
    /// memoryless source matrix through the existing host ramp. A stateful
    /// provider may also admit a changed host matrix if both candidate twins
    /// explicitly prove that their own processing configuration is unchanged
    /// and the ordinary history and retarget preflights succeed. Compatible
    /// automation changes keep the existing DSP history under the same
    /// configuration and tail bound.
    /// A live continuation may append one new, memoryless identity matrix
    /// after an exactly matching nonempty prefix. The new stage has neutral
    /// trims, no automation/modulation, latency or tail, and a distinct source
    /// device identity. Strict continuation compares the entire declaration.
    pub fn can_transfer_source_recipe_history_from(
        &self,
        previous: &Self,
        allow_control_edit: bool,
    ) -> bool {
        self.can_transfer_source_history_from(previous, allow_control_edit, false)
    }

    /// Exact source recipe continuation with an authored power edit on one
    /// or more opted-in native stages. The caller proves live playback and
    /// the same receiving track, in addition to its normal generator proof.
    pub fn can_transfer_source_power_edit_from(&self, previous: &Self) -> bool {
        self.can_transfer_source_history_from(previous, false, true)
    }

    fn can_transfer_source_history_from(
        &self,
        previous: &Self,
        allow_control_edit: bool,
        allow_power_edit: bool,
    ) -> bool {
        // An identity-only final stage adds no signal or control history. Keep
        // the old prefix exact and transfer it to the candidate before use.
        // The candidate's new identity must not alias any donor entry.
        let neutral_append = allow_control_edit
            && !previous.is_empty()
            && previous.len().checked_add(1) == Some(self.len())
            && self.entries.last().is_some_and(|appended| {
                matches!(appended.owner, ProcessorChainOwner::Source(_))
                    && previous.entries.iter().all(|old| {
                        old.owner == appended.owner
                            && old.sample_rate == appended.sample_rate
                            && old.device_id != appended.device_id
                    })
                    && matches!(appended.runtime, RuntimeOwnership::Memoryless)
                    && appended.configuration.is_some()
                    && !appended.bypassed
                    && appended.input_gain == 1.0
                    && appended.output_gain == 1.0
                    && appended.matrix == StereoMatrix::identity()
                    && appended.algorithmic_latency_frames == Some(0)
                    && appended.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                    && appended.automation.is_empty()
                    && appended.modulation.is_none()
                    && appended.committed_transient_output_gain
                        == host_output_gain_ramp::TransientGain::Follow
                    && appended.scratch_transient_output_gain
                        == host_output_gain_ramp::TransientGain::Follow
                    && appended.committed_host_gains == [1.0, 1.0]
                    && appended.scratch_host_gains == [1.0, 1.0]
                    && !appended.scratch_matrix_ramp.is_active()
                    && !appended.scratch_output_gain_ramp.active()
                    && !appended.scratch_bypass_ramp.is_active()
                    && !appended.committed_matrix_ramp.is_active()
                    && !appended.committed_output_gain_ramp.active()
                    && !appended.committed_bypass_ramp.is_active()
            });
        if neutral_append {
            return self.entries.iter().zip(&previous.entries).enumerate().all(
                |(index, (left, right))| {
                    !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(right.runtime, RuntimeOwnership::Foreign(_))
                        && left.bypassed == right.bypassed
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && left.algorithmic_latency_frames == right.algorithmic_latency_frames
                        && left.output_tail == right.output_tail
                        && self.configuration_matches_range(previous, index..index + 1)
                        && left.runtime.same_foreign_modulation(&right.runtime)
                        && match (&left.modulation, &right.modulation) {
                            (None, None) => true,
                            (Some(left), Some(right)) => left.same_declaration(right),
                            _ => false,
                        }
                },
            ) && !self.transition_busy_from(previous)
                && self.can_inherit_from(previous);
        }
        self.len() == previous.len()
            && self.entries.iter().zip(&previous.entries).enumerate().all(
                |(index, (left, right))| {
                    let power_edit = allow_power_edit
                        && matches!(left.owner, ProcessorChainOwner::Source(_))
                        && left.supports_power_off
                        && right.supports_power_off;
                    !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(right.runtime, RuntimeOwnership::Foreign(_))
                        && (left.bypassed == right.bypassed || power_edit)
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && left.algorithmic_latency_frames == right.algorithmic_latency_frames
                        && ((left.output_tail == right.output_tail
                            && self.configuration_matches_range(previous, index..index + 1))
                            || allow_control_edit
                                && left.matches_memoryless_modulation_handoff(right)
                            // A static, zero-latency source matrix has no DSP
                            // history to retarget, even when different authored
                            // settings resolve to the same matrix. The existing identity preflight
                            // and inherited matrix ramp preserve its live signal.
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && matches!(left.runtime, RuntimeOwnership::Memoryless)
                                && matches!(right.runtime, RuntimeOwnership::Memoryless)
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.algorithmic_latency_frames == Some(0)
                                && right.algorithmic_latency_frames == Some(0)
                                && left.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                                && right.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.output_tail == right.output_tail
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && left.runtime.can_retarget_source_host_matrix_from(&right.runtime)
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && !left.bypassed
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.output_tail == right.output_tail
                                && left.matrix == right.matrix
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && left.runtime.can_retarget_source_parameters_from(&right.runtime)
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && left.output_tail == right.output_tail
                                && left.matrix == right.matrix
                                && left
                                    .configuration
                                    .as_ref()
                                    .zip(right.configuration.as_ref())
                                    .is_some_and(|(new, old)| {
                                        std::sync::Arc::ptr_eq(new, old)
                                            || new.processing_eq_on_callback(old)
                                    })
                                && self
                                    .can_retarget_parameters_from_range(previous, index..index + 1))
                        && (allow_control_edit
                            || left.runtime.same_foreign_modulation(&right.runtime))
                        && (allow_control_edit
                            || match (&left.modulation, &right.modulation) {
                                (None, None) => true,
                                (Some(left), Some(right)) => left.same_declaration(right),
                                _ => false,
                            })
                },
            )
            && !self.transition_busy_from(previous)
            && self.can_inherit_from(previous)
    }

    /// Whether a same-position processor range can keep history across an
    /// authored parameter change. Entry identity and processing topology must
    /// remain exact; a changed configuration is accepted only when both
    /// candidate twins explicitly support retargeting from the old committed
    /// runtime, or when a compatible unbypassed, unautomated entry changes
    /// its static host matrix. The host ramps that matrix; a stateful entry
    /// must also explicitly admit history and parameter retargeting. A simultaneous bypass change also requires explicit
    /// smoothed bypass support, transferable history and unchanged host
    /// gains, matrix and automation. Empty and malformed ranges are refused.
    pub fn can_retarget_parameters_from_range(
        &self,
        previous: &Self,
        range: std::ops::Range<usize>,
    ) -> bool {
        if range.start >= range.end
            || range.end > self.entries.len()
            || range.end > previous.entries.len()
        {
            return false;
        }
        self.entries[range.clone()]
            .iter()
            .zip(&previous.entries[range])
            .all(|(candidate, old)| {
                // Static host coefficients ramp independently of processor
                // history. A stateful provider still has to admit the retarget.
                let stateful_matrix_retarget = matches!(
                    &candidate.runtime,
                    RuntimeOwnership::Legacy { primary, scratch }
                        if primary.supports_manual_host_matrix_retarget()
                            && scratch.supports_manual_host_matrix_retarget()
                ) || matches!(
                    &candidate.runtime,
                    RuntimeOwnership::Owned(runtime)
                        if runtime.supports_manual_host_matrix_retarget()
                );
                let stateful_matrix_retarget = stateful_matrix_retarget
                    && candidate.runtime.can_inherit_from(&old.runtime)
                    && candidate.runtime.can_retarget_from(&old.runtime);
                let static_matrix_retarget = !candidate.bypassed
                    && candidate.automation.is_empty()
                    && old.automation.is_empty()
                    && ((!candidate.runtime.is_stateful() && !old.runtime.is_stateful())
                        || stateful_matrix_retarget);
                let bypass_retarget = candidate.bypassed != old.bypassed
                    && candidate.configuration.is_some()
                    && old.configuration.is_some()
                    && candidate.can_smooth_bypass_from(old)
                    && candidate.input_gain == old.input_gain
                    && candidate.output_gain == old.output_gain
                    && candidate.matrix == old.matrix
                    && candidate.automation == old.automation
                    && candidate.runtime.can_inherit_from(&old.runtime)
                    && candidate.runtime.can_retarget_from(&old.runtime);
                let stable_processing_identity = Self::can_preserve_aligned_dry(candidate, old)
                    && candidate.owner == old.owner
                    && candidate.device_id == old.device_id
                    && candidate.device_type == old.device_type
                    && candidate.runtime_version == old.runtime_version
                    && candidate.sample_rate == old.sample_rate
                    && candidate
                        .runtime
                        .foreign_configuration_matches(&old.runtime)
                    && (candidate.bypassed == old.bypassed || bypass_retarget)
                    && (candidate.matrix == old.matrix
                        || static_matrix_retarget
                        || ((!candidate.automation.is_empty() || !old.automation.is_empty())
                            && candidate.runtime.can_retarget_from(&old.runtime)));
                if !stable_processing_identity {
                    return false;
                }
                let configuration_matches = match (&candidate.configuration, &old.configuration) {
                    (Some(candidate), Some(old)) => {
                        std::sync::Arc::ptr_eq(candidate, old)
                            || candidate.processing_eq_on_callback(old)
                    }
                    _ => false,
                };
                if configuration_matches || static_matrix_retarget {
                    return true;
                }
                candidate.runtime.can_retarget_from(&old.runtime)
            })
    }

    /// Combine residual output-tail declarations for a serial range, excluding
    /// fixed algorithmic latency. Reclamation uses `zero_input_drain_for_range`.
    /// A range
    /// with any unbounded processor is unbounded; finite declarations add with
    /// checked arithmetic. Invalid ranges return `None`.
    pub fn output_tail_for_range(&self, range: std::ops::Range<usize>) -> Option<OutputTail> {
        if range.start > range.end || range.end > self.entries.len() {
            return None;
        }
        let entries = &self.entries[range];
        if entries
            .iter()
            .any(|entry| entry.output_tail == OutputTail::Unbounded)
        {
            return Some(OutputTail::Unbounded);
        }
        let finite_frames = entries.iter().try_fold(0_u64, |total, entry| {
            let OutputTail::Finite { max_tail_frames } = entry.output_tail else {
                return None;
            };
            total.checked_add(max_tail_frames)
        })?;
        Some(OutputTail::Finite {
            max_tail_frames: finite_frames,
        })
    }

    /// A foreign endpoint without a finite source/host drain bound cannot be
    /// admitted into a finite retained-tail transition. Invalid ranges refuse
    /// conservatively. Built-in unbounded runtimes retain their existing policy.
    pub fn range_has_unbounded_foreign_tail(&self, range: std::ops::Range<usize>) -> bool {
        self.entries.get(range).is_none_or(|entries| {
            entries.iter().any(|entry| {
                matches!(&entry.runtime, RuntimeOwnership::Foreign(value)
                if value.prepared.output_tail() == OutputTail::Unbounded)
            })
        })
    }

    /// Total zero-input drain bound for a serial range: fixed intrinsic latency
    /// plus residual output duration for every entry. Bypassed processors still
    /// advance private state, so this uses intrinsic rather than audible latency.
    /// Unknown latency or residual duration is unbounded; invalid ranges and
    /// checked sum overflow return `None`. Reclamation must also test quiescence.
    pub fn zero_input_drain_for_range(&self, range: std::ops::Range<usize>) -> Option<OutputTail> {
        let entries = self.entries.get(range)?;
        if entries.iter().any(|entry| {
            entry.algorithmic_latency_frames.is_none() || entry.output_tail == OutputTail::Unbounded
        }) {
            return Some(OutputTail::Unbounded);
        }
        let frames = entries.iter().try_fold(0_u64, |total, entry| {
            let OutputTail::Finite { max_tail_frames } = entry.output_tail else {
                return None;
            };
            total
                .checked_add(u64::from(entry.algorithmic_latency_frames?))?
                .checked_add(max_tail_frames)
        })?;
        Some(OutputTail::Finite {
            max_tail_frames: frames,
        })
    }

    /// True only when every committed processor in a valid range has no
    /// future output under zero input. Invalid ranges conservatively return
    /// false. Stateless matrix entries are immediately quiescent.
    pub fn committed_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        self.entries
            .get(range)
            .is_some_and(|entries| entries.iter().all(RuntimeEntry::committed_is_quiescent))
    }

    /// Render silent input while a bypass mix still needs to reach its target,
    /// even when there is no remaining processor signal history.
    pub fn needs_zero_input_render(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.scratch_bypass_ramp.is_active() || !entry.committed_is_quiescent())
    }

    /// Speculative counterpart used only after processing scratch state in the
    /// current uncommitted render block.
    pub fn speculative_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        self.entries.get(range).is_some_and(|entries| {
            entries.iter().all(|entry| {
                (entry.powered_off(entry.scratch_bypass_ramp)
                    || entry.runtime.speculative_is_quiescent())
                    && entry
                        .scratch_aligned_dry
                        .as_ref()
                        .is_none_or(bypass_ramp::AlignedDry::is_quiet)
            })
        })
    }
}

mod handoff;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_transitions_match_and_stay_quiescent() {
        let mut current = NativeRuntimeChain::empty();
        let mut previous = NativeRuntimeChain::empty();
        assert!(current.static_processing_matches(&previous));
        assert!(current.can_inherit_from(&previous));
        assert!(!current.transition_busy_from(&previous));
        assert!(!current.needs_zero_input_render());
        current.transfer_history_from(&mut previous);
    }
}
