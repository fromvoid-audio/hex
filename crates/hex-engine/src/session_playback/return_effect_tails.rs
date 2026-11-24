//! Bounded ownership and rendering for residual output from Return and Main chains.
use super::{Frame, MAX_FINITE_EFFECT_TAIL_SECONDS, SessionPlaybackError};
use crate::ReturnBus;
use hex_lattice::{NativeRuntimeChain, OutputTail, PreparedRuntimeInstance, StereoMatrix};
use hex_project::{MAX_RETURN_TRACKS, ReturnId};
use std::fmt;

pub(super) const RETURN_EFFECT_TAIL_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug)]
enum TailDestination {
    Return(ReturnId),
    Main {
        suffix_start: usize,
        input_arrival: u32,
    },
}

/// Delayed stage clocks read either the already-pushed current stamp or an
/// upcoming window before it advances the retained history.
pub(super) enum MainTailClock<'a> {
    Timeline(&'a dyn hex_lattice::NativeInstrumentClock),
    Current(&'a crate::signal_graph::SignalRuntime),
    Window(
        &'a crate::signal_graph::SignalRuntime,
        &'a [Option<crate::compensation::TransportStamp>],
    ),
}

impl<'a> MainTailClock<'a> {
    pub(super) fn current(
        runtime: Option<&'a crate::signal_graph::SignalRuntime>,
        clock: &'a dyn hex_lattice::NativeInstrumentClock,
    ) -> Self {
        runtime.map_or(Self::Timeline(clock), Self::Current)
    }
}

pub(super) struct ReturnEffectTail {
    pub(super) chain: NativeRuntimeChain,
    destination: TailDestination,
    matrix: Option<StereoMatrix>,
    gain: f32,
    muted: bool,
    pub(super) output_active: bool,
    output_scratch: bool,
}

impl fmt::Debug for ReturnEffectTail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReturnEffectTail")
            .field("destination", &self.destination)
            .field("output_active", &self.output_active)
            .finish_non_exhaustive()
    }
}

impl ReturnEffectTail {
    fn active_start(&self) -> usize {
        match self.destination {
            TailDestination::Return(_) => 0,
            TailDestination::Main { suffix_start, .. } => suffix_start,
        }
    }
    pub(super) fn return_id(&self) -> Option<ReturnId> {
        match self.destination {
            TailDestination::Return(id) => Some(id),
            TailDestination::Main { .. } => None,
        }
    }

    pub(super) fn new(mut chain: NativeRuntimeChain, bus: &ReturnBus) -> Self {
        chain.begin_output_tail();
        Self {
            chain,
            destination: TailDestination::Return(bus.id),
            matrix: bus.matrix,
            gain: bus.gain,
            muted: bus.muted,
            output_active: true,
            output_scratch: true,
        }
    }
}

pub(super) struct ReturnEffectTailBank {
    slots: Vec<Option<ReturnEffectTail>>,
    scratch: [Frame; 256],
}

impl ReturnEffectTailBank {
    pub(super) fn visit_empty_candidate_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self { slots, scratch: _ } = self;
        if slots.iter().any(Option::is_some) {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        ledger.vec(slots)
    }
    pub(super) fn new() -> Result<Self, SessionPlaybackError> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(RETURN_EFFECT_TAIL_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        if slots.capacity() > RETURN_EFFECT_TAIL_CAPACITY {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        slots.resize_with(RETURN_EFFECT_TAIL_CAPACITY, || None);
        Ok(Self {
            slots,
            scratch: [[0.0; 2]; 256],
        })
    }

    pub(super) fn slots(&self) -> &[Option<ReturnEffectTail>] {
        &self.slots
    }

    #[cfg(test)]
    pub(super) fn slots_mut(&mut self) -> &mut [Option<ReturnEffectTail>] {
        &mut self.slots
    }

    pub(super) fn begin_block(&mut self) {
        for tail in self.slots.iter_mut().flatten() {
            if tail.output_active {
                tail.chain.begin_block();
            }
            tail.output_scratch = tail.output_active;
        }
    }

    pub(super) fn rollback_block(&mut self) {
        for tail in self.slots.iter_mut().flatten() {
            tail.chain.rollback_block();
            tail.output_scratch = tail.output_active;
        }
    }

    pub(super) fn commit_block(&mut self) {
        for tail in self.slots.iter_mut().flatten() {
            if tail.output_active || tail.output_scratch {
                tail.chain.commit_block();
            }
            tail.output_active = tail.output_scratch;
        }
    }

    pub(super) fn reset(&mut self) {
        for tail in self.slots.iter_mut().flatten() {
            tail.chain.reset();
            tail.output_active = false;
            tail.output_scratch = false;
        }
    }

    pub(super) fn cancel_all_processor_auditions(&mut self) {
        for tail in self.slots.iter_mut().flatten() {
            tail.chain.cancel_all_processor_auditions();
        }
    }

    /// Render each active old bus exactly once, with zero input, into Main.
    /// The caller invokes this once after current Return processing and before
    /// the current master mix. The reusable chain owns all callback state.
    /// `output.len()` is bounded to 256 by `render_route_chunk`.
    pub(super) fn render_chunk(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        capture: &mut Option<&mut dyn super::capture::CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let (slots, scratch) = (&mut self.slots, &mut self.scratch);
        for tail in slots.iter_mut().flatten() {
            let TailDestination::Return(return_id) = tail.destination else {
                continue;
            };
            if !tail.output_scratch {
                continue;
            }
            let frames = &mut scratch[..output.len()];
            frames.fill([0.0; 2]);
            tail.chain
                .process_range_with_foreign(
                    0..tail.chain.len(),
                    frames,
                    frame_offset,
                    None,
                    None,
                    foreign,
                )
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            tail.output_scratch = !tail
                .chain
                .speculative_range_is_quiescent(0..tail.chain.len());
            if let Some(matrix) = tail.matrix {
                matrix.apply_block(frames);
            }
            if !tail.muted {
                for (destination, source) in output.iter_mut().zip(frames.iter()) {
                    destination[0] += source[0] * tail.gain;
                    destination[1] += source[1] * tail.gain;
                }
            }
            if let Some(capture) = capture.as_mut() {
                for frame in frames.iter_mut() {
                    *frame = if tail.muted {
                        [0.0; 2]
                    } else {
                        [frame[0] * tail.gain, frame[1] * tail.gain]
                    };
                }
                capture.add(
                    super::OutputCaptureId::Return(return_id),
                    frame_offset,
                    frames,
                )?;
            }
        }
        Ok(())
    }

    /// The same shared bank owns post-Main tails, but they never enter Return
    /// captures or current Main. Its callers have already bounded chunk length.
    pub(super) fn render_main_chunk(
        &mut self,
        output: &mut [Frame],
        clocks: MainTailClock<'_>,
        frame_offset: usize,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let (slots, scratch) = (&mut self.slots, &mut self.scratch);
        for tail in slots.iter_mut().flatten() {
            let TailDestination::Main {
                suffix_start,
                input_arrival,
            } = tail.destination
            else {
                continue;
            };
            if !tail.output_scratch {
                continue;
            }
            let frames = &mut scratch[..output.len()];
            frames.fill([0.0; 2]);
            let mut arrival = input_arrival;
            for entry in suffix_start..tail.chain.len() {
                let output_arrival = arrival
                    .checked_add(
                        tail.chain
                            .algorithmic_latency_frames_for_entry(entry)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?
                            .ok_or(SessionPlaybackError::DeviceRuntime)?,
                    )
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let mut process =
                    |input: &dyn hex_lattice::NativeInstrumentClock,
                     output: Option<&dyn hex_lattice::NativeInstrumentClock>| {
                        tail.chain.process_range_with_foreign(
                            entry..entry + 1,
                            frames,
                            frame_offset,
                            Some(input),
                            output,
                            foreign,
                        )
                    };
                match clocks {
                    MainTailClock::Current(runtime) => {
                        if output.len() != 1 {
                            return Err(SessionPlaybackError::DeviceRuntime);
                        }
                        let origin = |delay| {
                            crate::signal_graph::OriginClock(
                                runtime.delays.stamp_at_delay_ref(delay),
                            )
                        };
                        process(&origin(arrival), Some(&origin(output_arrival)))
                    }
                    MainTailClock::Window(runtime, stamps) => {
                        let origin = |delay| crate::compensation::OriginWindow {
                            history: &runtime.delays,
                            incoming: stamps,
                            delay,
                        };
                        process(&origin(arrival), Some(&origin(output_arrival)))
                    }
                    MainTailClock::Timeline(clock) => {
                        if arrival != 0 {
                            return Err(SessionPlaybackError::DeviceRuntime);
                        }
                        process(clock, (output_arrival == 0).then_some(clock))
                    }
                }
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                arrival = output_arrival;
            }
            tail.output_scratch = !tail
                .chain
                .speculative_range_is_quiescent(suffix_start..tail.chain.len());
            for (to, from) in output.iter_mut().zip(frames) {
                to[0] += from[0];
                to[1] += from[1];
            }
        }
        Ok(())
    }

    pub(super) fn accounted_storage_bytes(&self) -> Option<usize> {
        let Self { slots, scratch: _ } = self;
        let mut bytes = std::mem::size_of::<Self>().checked_add(
            slots
                .capacity()
                .checked_mul(std::mem::size_of::<Option<ReturnEffectTail>>())?,
        )?;
        for tail in slots.iter().flatten() {
            let ReturnEffectTail {
                chain,
                destination: _,
                matrix: _,
                gain: _,
                muted: _,
                output_active: _,
                output_scratch: _,
            } = tail;
            bytes = bytes.checked_add(chain.accounted_storage_bytes()?)?;
        }
        Some(bytes)
    }

    pub(super) fn preflight_storage_bytes() -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(
            RETURN_EFFECT_TAIL_CAPACITY
                .checked_mul(std::mem::size_of::<Option<ReturnEffectTail>>())?,
        )
    }
}

/// Publication policy for residual output made obsolete by a device edit.
/// Already retained owners and clip releases follow their own handoff proof.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DeviceEditTailPolicy {
    Preserve,
    RequireConfirmation,
    Cut,
    FadedNative(hex_lattice::ProcessorInstanceIdentity),
    FadedReturn(ReturnId),
    PoweredOffReturn(hex_lattice::ProcessorInstanceIdentity),
}

impl DeviceEditTailPolicy {
    /// A changed topology has a different ordered set of stable device IDs.
    /// State-only edits still use the ordinary tail and continuation proofs.
    pub(super) fn for_chain(
        self,
        old: &[PreparedRuntimeInstance],
        new: &[PreparedRuntimeInstance],
    ) -> Self {
        let policy = match self {
            Self::FadedNative(identity) => {
                let matches = |entry: &PreparedRuntimeInstance| {
                    entry.owner == identity.owner() && entry.device_id == identity.device_id()
                };
                if old.iter().any(matches) && !new.iter().any(matches) {
                    Self::Cut
                } else {
                    Self::RequireConfirmation
                }
            }
            Self::FadedReturn(_) | Self::PoweredOffReturn(_) => Self::RequireConfirmation,
            _ => self,
        };
        if policy != Self::Preserve
            && old
                .iter()
                .map(|device| device.device_id)
                .ne(new.iter().map(|device| device.device_id))
        {
            policy
        } else {
            Self::Preserve
        }
    }

    pub(super) fn cut_obsolete_tail(self) -> Result<bool, SessionPlaybackError> {
        match self {
            Self::Preserve => Ok(false),
            Self::RequireConfirmation
            | Self::FadedNative(_)
            | Self::FadedReturn(_)
            | Self::PoweredOffReturn(_) => Err(SessionPlaybackError::DeviceInsertNeedsConfirmation),
            Self::Cut => Ok(true),
        }
    }
}

/// A fully preflighted transfer plan. Its bounded indices and transition
/// choices move history without allocating or dropping the previous chain.
pub(super) struct ReturnEffectTailPlan {
    carry: [Option<(usize, usize)>; RETURN_EFFECT_TAIL_CAPACITY],
    capture: [Option<(usize, usize)>; MAX_RETURN_TRACKS],
    bus_history: [ReturnBusHistory; MAX_RETURN_TRACKS],
    main_capture: Option<(usize, usize, u32)>,
    // A completed suffix needs no old DSP history and no retained tail owner.
    main_quiet_suffix: Option<usize>,
    main_insert_transition: Option<hex_lattice::NativeInsertTransition>,
}

#[derive(Clone, Copy)]
enum ReturnBusHistory {
    Fresh,
    Inherit,
    SmoothInsert(hex_lattice::NativeInsertTransition),
}

impl ReturnEffectTailPlan {
    /// Only live Main suffixes need the donor's extended content-origin clock.
    /// Inactive chains stay with the returning donor rather than extending its
    /// constructor's resource custody into another zero-latency revision.
    pub(super) fn requires_main_clock(
        &self,
        old: &ReturnEffectTailBank,
    ) -> Result<bool, SessionPlaybackError> {
        if self.main_capture.is_some() {
            return Ok(true);
        }
        for (source, _) in self.carry.iter().flatten() {
            let tail = old
                .slots
                .get(*source)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if matches!(tail.destination, TailDestination::Main { .. }) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn plan_candidate_origins(
        &self,
        groups: &mut crate::audio_candidate::groups::Transfer<'_>,
        old: &ReturnEffectTailBank,
        returns: &[NativeRuntimeChain],
        main: &NativeRuntimeChain,
    ) -> Result<(), SessionPlaybackError> {
        use crate::audio_candidate::groups::Location as L;
        for (source, target) in self.carry.iter().flatten().copied() {
            let tail = old
                .slots
                .get(source)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            groups.plan(L::ReturnTail(source), L::ReturnTail(target), &tail.chain)?;
        }
        for (source, target) in self.capture.iter().flatten().copied() {
            let chain = returns
                .get(source)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            groups.plan(L::ReturnChain(source), L::ReturnTail(target), chain)?;
        }
        if let Some((target, _, _)) = self.main_capture {
            groups.plan(L::MainChain, L::ReturnTail(target), main)?;
        }
        Ok(())
    }
    /// Check only the ranges this plan will actually render. An inactive
    /// retained prefix still owns backing, but must not reserve its endpoint
    /// against the current chain. This is refusal-only, never source admission.
    pub(super) fn validate_retained_foreign_sources(
        &self,
        old: &ReturnEffectTailBank,
        returns: &[NativeRuntimeChain],
        main: &NativeRuntimeChain,
        mut validate: impl FnMut(&NativeRuntimeChain, usize) -> Result<(), SessionPlaybackError>,
    ) -> Result<(), SessionPlaybackError> {
        for (source, _) in self.carry.iter().flatten() {
            let tail = old
                .slots
                .get(*source)
                .and_then(Option::as_ref)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            validate(&tail.chain, tail.active_start())?;
        }
        for (source, _) in self.capture.iter().flatten() {
            validate(
                returns
                    .get(*source)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?,
                0,
            )?;
        }
        if let Some((_, start, _)) = self.main_capture {
            validate(main, start)?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn preflight(
        old_buses: &[ReturnBus],
        old_chains: &[NativeRuntimeChain],
        old_tails: &ReturnEffectTailBank,
        new_buses: &[ReturnBus],
        new_chains: &[NativeRuntimeChain],
        sample_rate: u32,
        processor_count: &mut usize,
        runtime_bytes: &mut usize,
        entry_count: &mut usize,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
        tail_policy: DeviceEditTailPolicy,
    ) -> Result<Self, SessionPlaybackError> {
        if old_buses.len() > MAX_RETURN_TRACKS
            || new_buses.len() > MAX_RETURN_TRACKS
            || old_chains.len() != old_buses.len()
            || new_chains.len() != new_buses.len()
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let mut plan = Self {
            carry: [None; RETURN_EFFECT_TAIL_CAPACITY],
            capture: [None; MAX_RETURN_TRACKS],
            bus_history: [ReturnBusHistory::Fresh; MAX_RETURN_TRACKS],
            main_capture: None,
            main_quiet_suffix: None,
            main_insert_transition: None,
        };
        let mut occupied = [false; RETURN_EFFECT_TAIL_CAPACITY];

        for (source_slot, tail) in old_tails.slots().iter().enumerate() {
            let Some(tail) = tail.as_ref().filter(|tail| tail.output_active) else {
                continue;
            };
            if tail.chain.committed_range_is_quiescent(0..tail.chain.len()) {
                continue;
            }
            let target_slot = occupied
                .iter()
                .position(|occupied| !occupied)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            occupied[target_slot] = true;
            plan.carry[source_slot] = Some((source_slot, target_slot));
            reserve_chain(
                &tail.chain,
                tail.active_start(),
                tail.return_id().is_none(),
                sample_rate,
                processor_count,
                runtime_bytes,
                entry_count,
            )?;
        }

        for (old_index, old_bus) in old_buses.iter().enumerate() {
            let old_chain = old_chains
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let new_index = new_buses.iter().position(|bus| bus.id == old_bus.id);
            if let DeviceEditTailPolicy::PoweredOffReturn(identity) = tail_policy
                && identity.owner() == hex_lattice::ProcessorChainOwner::Return(old_bus.id.value())
            {
                // The original now has its temporary removal bypass, so the
                // preflighted cut cannot be reclassified on that mutated chain.
                let exact = new_index.is_some_and(|index| {
                    let ([old], [off]) = (
                        old_bus.device_chain.as_ref(),
                        new_buses[index].device_chain.as_ref(),
                    ) else {
                        return false;
                    };
                    old.owner == identity.owner()
                        && off.owner == identity.owner()
                        && old.device_id == identity.device_id()
                        && off.device_id == identity.device_id()
                        && off.bypassed
                        && off
                            .definition
                            .as_ref()
                            .is_some_and(|factory| factory.is_powered_off())
                });
                if !exact {
                    return Err(SessionPlaybackError::DeviceRuntime);
                }
                // The original chain stays with the returned donor.
                continue;
            }
            if new_index.is_none() && tail_policy == DeviceEditTailPolicy::FadedReturn(old_bus.id) {
                // The old post-DSP output is already inaudible at the cut.
                // Leave the original chain with the returned donor.
                continue;
            }
            let inherits_current_bus = if let Some(new_index) = new_index {
                let new_chain = new_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let same_topology = old_chain.len() == new_chain.len();
                let same_configuration = same_topology
                    && super::handoff::configuration_matches_continuation(
                        old_chain,
                        new_chain,
                        0..old_chain.len(),
                        continuation,
                    );
                let retargetable_configuration = !same_configuration
                    && same_topology
                    && new_chain.can_retarget_parameters_from_range(old_chain, 0..old_chain.len());
                let history = if (same_configuration || retargetable_configuration)
                    && new_chain.can_inherit_from(old_chain)
                {
                    Some(ReturnBusHistory::Inherit)
                } else if tail_policy == DeviceEditTailPolicy::RequireConfirmation {
                    // The caller has proved continuous graph timing. Keep the
                    // original Return stages on one route around the insertion.
                    new_chain
                        .native_insert_transition_from(old_chain, continuation)
                        .map(ReturnBusHistory::SmoothInsert)
                } else {
                    None
                };
                if let Some(history) = history {
                    plan.bus_history[new_index] = history;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if inherits_current_bus {
                continue;
            }
            let no_outgoing_tail = old_bus.muted
                || old_bus.gain == 0.0
                || old_chain.committed_range_is_quiescent(0..old_chain.len());
            if let Some(index) = new_index {
                match tail_policy.for_chain(&old_bus.device_chain, &new_buses[index].device_chain) {
                    DeviceEditTailPolicy::Cut => {
                        // A muted Return still renders its devices. Retain
                        // compatible survivors before ruling out a detached tail.
                        let new_chain = new_chains
                            .get(index)
                            .ok_or(SessionPlaybackError::DeviceRuntime)?;
                        if new_chain.can_inherit_from(old_chain) {
                            plan.bus_history[index] = ReturnBusHistory::Inherit;
                        } else if !no_outgoing_tail {
                            return Err(SessionPlaybackError::DeviceRuntime);
                        }
                        continue;
                    }
                    policy if !no_outgoing_tail => {
                        policy.cut_obsolete_tail()?;
                    }
                    _ => {}
                }
            }
            if no_outgoing_tail {
                continue;
            }

            let target_slot = occupied
                .iter()
                .position(|occupied| !occupied)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            occupied[target_slot] = true;
            let capture = plan
                .capture
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            *capture = Some((old_index, target_slot));
            reserve_chain(
                old_chain,
                0,
                false,
                sample_rate,
                processor_count,
                runtime_bytes,
                entry_count,
            )?;
        }

        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn preflight_main(
        &mut self,
        old: &NativeRuntimeChain,
        current: &NativeRuntimeChain,
        pre_main_arrival: u32,
        sample_rate: u32,
        processor_count: &mut usize,
        runtime_bytes: &mut usize,
        entry_count: &mut usize,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
        tail_policy: DeviceEditTailPolicy,
    ) -> Result<(), SessionPlaybackError> {
        if tail_policy == DeviceEditTailPolicy::RequireConfirmation {
            self.main_insert_transition = current.native_insert_transition_from(old, continuation);
            if self.main_insert_transition.is_some() {
                return Ok(());
            }
        }
        let mut start = 0;
        let mut arrival = pre_main_arrival;
        while start < old.len().min(current.len())
            && (super::handoff::configuration_matches_continuation(
                old,
                current,
                start..start + 1,
                continuation,
            ) || current.can_retarget_parameters_from_range(old, start..start + 1))
        {
            arrival = arrival
                .checked_add(
                    old.algorithmic_latency_frames_for_entry(start)
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?
                        .ok_or(SessionPlaybackError::DeviceRuntime)?,
                )
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            start += 1;
        }
        if start == old.len() {
            return Ok(());
        }
        if !current.can_inherit_from_excluding_old_range(old, start..old.len()) {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        if old.committed_range_is_quiescent(start..old.len()) {
            // The original runtime has observed actual completion. A changed
            // endpoint may start fresh without copying that incompatible quiet
            // suffix or reserving a tail that will never produce more output.
            self.main_quiet_suffix = Some(start);
            return Ok(());
        }
        if tail_policy.cut_obsolete_tail()? {
            // Leave the suffix with the retiring graph. The ordinary Main
            // history preflight still checks each surviving processor.
            return Ok(());
        }
        let target = (0..RETURN_EFFECT_TAIL_CAPACITY)
            .find(|slot| {
                !self
                    .carry
                    .iter()
                    .flatten()
                    .any(|(_, target)| target == slot)
                    && !self
                        .capture
                        .iter()
                        .flatten()
                        .any(|(_, target)| target == slot)
            })
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        // Retain and charge the complete actual owner, including its inactive
        // prefix backing, although only the detached suffix is processed.
        reserve_chain(
            old,
            start,
            true,
            sample_rate,
            processor_count,
            runtime_bytes,
            entry_count,
        )?;
        self.main_capture = Some((target, start, arrival));
        Ok(())
    }
    pub(super) fn main_excluded(&self, old_len: usize) -> std::ops::Range<usize> {
        self.main_capture
            .map(|(_, start, _)| start)
            .or(self.main_quiet_suffix)
            .map_or(0..0, |start| start..old_len)
    }
    pub(super) fn can_inherit_main(
        &self,
        current: &NativeRuntimeChain,
        old: &NativeRuntimeChain,
    ) -> bool {
        if self.main_quiet_suffix.is_some() {
            current.can_inherit_with_completed_foreign_range(old, self.main_excluded(old.len()))
        } else {
            current.can_inherit_from_excluding_old_range(old, self.main_excluded(old.len()))
        }
    }

    pub(super) fn transfer_main_history(
        &self,
        current: &mut NativeRuntimeChain,
        old: &mut NativeRuntimeChain,
    ) {
        if let Some(transition) = self.main_insert_transition {
            current.transfer_history_with_native_insert_from(old, transition);
        } else {
            current.transfer_history_from(old);
        }
    }
    pub(super) fn validate_main(
        &self,
        bank: &ReturnEffectTailBank,
        old: &NativeRuntimeChain,
    ) -> Result<(), SessionPlaybackError> {
        if let Some((target, start, _)) = self.main_capture
            && (start >= old.len() || !bank.slots.get(target).is_some_and(Option::is_none))
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        Ok(())
    }
    pub(super) fn capture_main(
        &self,
        bank: &mut ReturnEffectTailBank,
        current: &mut NativeRuntimeChain,
        old: &mut NativeRuntimeChain,
    ) -> Result<(), SessionPlaybackError> {
        self.validate_main(bank, old)?;
        if let Some((target, start, arrival)) = self.main_capture {
            current
                .transfer_history_excluding_old_range(old, start..old.len())
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            old.begin_output_tail();
            let chain = std::mem::replace(old, NativeRuntimeChain::empty());
            bank.slots[target] = Some(ReturnEffectTail {
                chain,
                destination: TailDestination::Main {
                    suffix_start: start,
                    input_arrival: arrival,
                },
                matrix: None,
                gain: 1.0,
                muted: false,
                output_active: true,
                output_scratch: true,
            });
        }
        Ok(())
    }

    pub(super) fn retained_storage_bytes(
        &self,
        old: &ReturnEffectTailBank,
        returns: &[NativeRuntimeChain],
        main: &NativeRuntimeChain,
    ) -> Option<usize> {
        let mut bytes = 0_usize;
        for (source, _) in self.carry.iter().flatten() {
            bytes = bytes.checked_add(
                old.slots
                    .get(*source)?
                    .as_ref()?
                    .chain
                    .accounted_storage_bytes()?,
            )?;
        }
        for (source, _) in self.capture.iter().flatten() {
            bytes = bytes.checked_add(returns.get(*source)?.accounted_storage_bytes()?)?;
        }
        if self.main_capture.is_some() {
            bytes = bytes.checked_add(main.accounted_storage_bytes()?)?;
        }
        Some(bytes)
    }
    pub(super) fn captures_bus(&self) -> bool {
        self.capture.iter().any(Option::is_some)
    }
    pub(super) fn transfer_bus_history(
        &self,
        index: usize,
        current: &mut NativeRuntimeChain,
        old: &mut NativeRuntimeChain,
    ) {
        match self.bus_history.get(index) {
            Some(ReturnBusHistory::Inherit) => current.transfer_history_from(old),
            Some(ReturnBusHistory::SmoothInsert(transition)) => {
                current.transfer_history_with_native_insert_from(old, *transition);
            }
            Some(ReturnBusHistory::Fresh) | None => {}
        }
    }

    /// Validate every source and destination together before the owner commit
    /// starts. This keeps an invariant error from partially moving the bank.
    pub(super) fn validate_commit(
        &self,
        candidate: &ReturnEffectTailBank,
        previous: &ReturnEffectTailBank,
        previous_chains: &[NativeRuntimeChain],
        previous_buses: &[ReturnBus],
    ) -> Result<(), SessionPlaybackError> {
        for (source, target) in self.carry.iter().flatten().copied() {
            let (Some(Some(_)), Some(None)) =
                (previous.slots.get(source), candidate.slots.get(target))
            else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
        }
        for (source, target) in self.capture.iter().flatten().copied() {
            if previous_buses.get(source).is_none()
                || previous_chains.get(source).is_none()
                || !candidate.slots.get(target).is_some_and(Option::is_none)
            {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
        }
        Ok(())
    }

    pub(super) fn apply_carry(
        &self,
        candidate: &mut ReturnEffectTailBank,
        previous: &mut ReturnEffectTailBank,
    ) -> Result<(), SessionPlaybackError> {
        for (source, target) in self.carry.iter().flatten().copied() {
            let (Some(Some(_)), Some(None)) =
                (previous.slots.get(source), candidate.slots.get(target))
            else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
        }
        for (source, target) in self.carry.iter().flatten().copied() {
            let (Some(candidate_slot), Some(previous_slot)) = (
                candidate.slots.get_mut(target),
                previous.slots.get_mut(source),
            ) else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            if candidate_slot.is_some() || previous_slot.is_none() {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            std::mem::swap(candidate_slot, previous_slot);
        }
        Ok(())
    }

    pub(super) fn capture_buses(
        &self,
        candidate: &mut ReturnEffectTailBank,
        previous_chains: &mut [NativeRuntimeChain],
        previous_buses: &[ReturnBus],
    ) -> Result<(), SessionPlaybackError> {
        for (source, target) in self.capture.iter().flatten().copied() {
            if previous_buses.get(source).is_none()
                || previous_chains.get(source).is_none()
                || !candidate.slots.get(target).is_some_and(Option::is_none)
            {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
        }
        for (old_index, target) in self.capture.iter().enumerate() {
            let Some((planned_index, target_slot)) = *target else {
                continue;
            };
            if planned_index != old_index {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            let bus = previous_buses
                .get(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let chain = previous_chains
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let slot = candidate
                .slots
                .get_mut(target_slot)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if slot.is_some() {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            let owned = std::mem::replace(chain, NativeRuntimeChain::empty());
            *slot = Some(ReturnEffectTail::new(owned, bus));
        }
        Ok(())
    }
}

fn reserve_chain(
    chain: &NativeRuntimeChain,
    active_start: usize,
    allow_unbounded_foreign: bool,
    sample_rate: u32,
    processor_count: &mut usize,
    runtime_bytes: &mut usize,
    entry_count: &mut usize,
) -> Result<(), SessionPlaybackError> {
    let active = active_start..chain.len();
    if !allow_unbounded_foreign && chain.range_has_unbounded_foreign_tail(active.clone()) {
        return Err(SessionPlaybackError::DeviceRuntime);
    }
    match chain
        .zero_input_drain_for_range(active)
        // The active range is validated by this caller; `None` therefore means
        // its serial finite declarations overflowed their aggregate bound.
        .ok_or(SessionPlaybackError::ReleaseBankFull)?
    {
        OutputTail::Finite { max_tail_frames } => {
            let horizon = u64::from(sample_rate)
                .checked_mul(MAX_FINITE_EFFECT_TAIL_SECONDS)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            if max_tail_frames > horizon {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
        }
        // The fixed tail bank retains this owner until Stop/seek or a proven
        // drain. A full bank refuses a later handoff without discarding it.
        OutputTail::Unbounded => {}
    }
    *processor_count = processor_count
        .checked_add(chain.stateful_count())
        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
    *runtime_bytes = runtime_bytes
        .checked_add(chain.declared_pair_bytes())
        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
    *entry_count = entry_count
        .checked_add(chain.len())
        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
    if *processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
        || *runtime_bytes > 2 * hex_lattice::MAX_PROCESSOR_BANK_RUNTIME_BYTES
        || *entry_count > super::MAX_RETAINED_SUFFIX_ENTRIES
    {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    Ok(())
}
