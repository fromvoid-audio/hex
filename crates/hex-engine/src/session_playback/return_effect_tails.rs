//! Bounded ownership and rendering for residual output from replaced Returns.
use super::{Frame, MAX_FINITE_EFFECT_TAIL_SECONDS, SessionPlaybackError};
use crate::ReturnBus;
use hex_lattice::{NativeRuntimeChain, OutputTail, StereoMatrix};
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
    pub(super) fn return_id(&self) -> Option<ReturnId> {
        match self.destination {
            TailDestination::Return(id) => Some(id),
            TailDestination::Main { .. } => None,
        }
    }

    pub(super) fn new(chain: NativeRuntimeChain, bus: &ReturnBus) -> Self {
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
                .process_block(frames)
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
        compensation: Option<&crate::signal_graph::SignalRuntime>,
        clock: &dyn hex_lattice::NativeInstrumentClock,
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
                if let Some(compensation) = compensation {
                    if frames.len() != 1 {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    let origin = crate::signal_graph::OriginClock(
                        compensation.delays.stamp_at_delay(arrival),
                    );
                    tail.chain
                        .process_range_with_clock(entry..entry + 1, frames, Some(&origin))
                } else {
                    if arrival != 0 {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    tail.chain
                        .process_range_with_clock(entry..entry + 1, frames, Some(clock))
                }
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                arrival = arrival
                    .checked_add(
                        tail.chain
                            .algorithmic_latency_frames_for_entry(entry)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?
                            .ok_or(SessionPlaybackError::DeviceRuntime)?,
                    )
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
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

/// A fully preflighted transfer plan. It contains indices only; applying it
/// moves runtime owners without allocating or dropping the previous chain.
pub(super) struct ReturnEffectTailPlan {
    carry: [Option<(usize, usize)>; RETURN_EFFECT_TAIL_CAPACITY],
    capture: [Option<(usize, usize)>; MAX_RETURN_TRACKS],
    inherit_bus: [bool; MAX_RETURN_TRACKS],
    main_capture: Option<(usize, usize, u32)>,
}

impl ReturnEffectTailPlan {
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
            inherit_bus: [false; MAX_RETURN_TRACKS],
            main_capture: None,
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
            let inherits_current_bus = if let Some(new_index) = new_index {
                let new_chain = new_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let same_topology = old_chain.len() == new_chain.len();
                let same_configuration = same_topology
                    && old_chain.configuration_matches_range(new_chain, 0..old_chain.len());
                let retargetable_configuration = !same_configuration
                    && same_topology
                    && new_chain.can_retarget_parameters_from_range(old_chain, 0..old_chain.len());
                if (same_configuration || retargetable_configuration)
                    && new_chain.can_inherit_from(old_chain)
                {
                    if let Some(should_inherit) = plan.inherit_bus.get_mut(new_index) {
                        *should_inherit = true;
                    } else {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if inherits_current_bus
                || old_bus.muted
                || old_bus.gain == 0.0
                || old_chain.committed_range_is_quiescent(0..old_chain.len())
            {
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
    ) -> Result<(), SessionPlaybackError> {
        let mut start = 0;
        let mut arrival = pre_main_arrival;
        while start < old.len().min(current.len())
            && (old.configuration_matches_range(current, start..start + 1)
                || current.can_retarget_parameters_from_range(old, start..start + 1))
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
        if start == old.len() || old.committed_range_is_quiescent(start..old.len()) {
            return Ok(());
        }
        if !current.can_inherit_from_excluding_old_range(old, start..old.len()) {
            return Err(SessionPlaybackError::DeviceRuntime);
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
            .map_or(0..0, |(_, start, _)| start..old_len)
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
                .inherit_from_excluding_old_range(old, start..old.len())
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
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
    pub(super) fn inherit_bus(&self, index: usize) -> bool {
        self.inherit_bus.get(index).copied().unwrap_or(false)
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
    sample_rate: u32,
    processor_count: &mut usize,
    runtime_bytes: &mut usize,
    entry_count: &mut usize,
) -> Result<(), SessionPlaybackError> {
    match chain
        .zero_input_drain_for_range(0..chain.len())
        // The full range is validated by this caller; `None` therefore means
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
