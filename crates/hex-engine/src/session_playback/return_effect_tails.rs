//! Bounded ownership and rendering for residual output from replaced Returns.
use super::{Frame, MAX_FINITE_EFFECT_TAIL_SECONDS, SessionPlaybackError};
use crate::ReturnBus;
use hex_lattice::{NativeRuntimeChain, OutputTail, StereoMatrix};
use hex_project::{MAX_RETURN_TRACKS, ReturnId};
use std::fmt;

pub(super) const RETURN_EFFECT_TAIL_CAPACITY: usize = 64;

pub(super) struct ReturnEffectTail {
    pub(super) chain: NativeRuntimeChain,
    pub(super) return_id: ReturnId,
    matrix: Option<StereoMatrix>,
    gain: f32,
    muted: bool,
    pub(super) output_active: bool,
    output_scratch: bool,
}

impl fmt::Debug for ReturnEffectTail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReturnEffectTail")
            .field("return_id", &self.return_id)
            .field("output_active", &self.output_active)
            .finish_non_exhaustive()
    }
}

impl ReturnEffectTail {
    pub(super) fn new(chain: NativeRuntimeChain, bus: &ReturnBus) -> Self {
        Self {
            chain,
            return_id: bus.id,
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

    /// Render each active old bus exactly once, with zero input, into Main.
    /// The caller invokes this once after current Return processing and before
    /// the current master mix. The reusable chain owns all callback state.
    /// `output.len()` is bounded to 256 by `render_route_chunk`.
    pub(super) fn render_chunk(&mut self, output: &mut [Frame]) {
        let (slots, scratch) = (&mut self.slots, &mut self.scratch);
        for tail in slots.iter_mut().flatten() {
            if !tail.output_scratch {
                continue;
            }
            let frames = &mut scratch[..output.len()];
            frames.fill([0.0; 2]);
            tail.chain.process_block(frames);
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
        }
    }

    pub(super) fn accounted_storage_bytes(&self) -> Option<usize> {
        let mut bytes = std::mem::size_of::<Self>().checked_add(
            self.slots
                .capacity()
                .checked_mul(std::mem::size_of::<Option<ReturnEffectTail>>())?,
        )?;
        for tail in self.slots.iter().flatten() {
            bytes = bytes.checked_add(tail.chain.accounted_storage_bytes()?)?;
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
            let unchanged = if let Some(new_index) = new_index {
                let new_chain = new_chains
                    .get(new_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let same_configuration = old_chain.len() == new_chain.len()
                    && old_chain.configuration_matches_range(new_chain, 0..old_chain.len());
                if same_configuration && new_chain.can_inherit_from(old_chain) {
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
            if unchanged
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
        .output_tail_for_range(0..chain.len())
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
        .checked_add(chain.declared_runtime_bytes())
        .and_then(|bytes| bytes.checked_add(chain.declared_runtime_bytes()))
        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
    *entry_count = entry_count
        .checked_add(chain.len())
        .ok_or(SessionPlaybackError::ReleaseBankFull)?;
    if *processor_count > hex_lattice::MAX_STATEFUL_PROCESSORS
        || *runtime_bytes
            > 2 * hex_lattice::MAX_STATEFUL_PROCESSORS * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES
        || *entry_count > super::MAX_RETAINED_SUFFIX_ENTRIES
    {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    Ok(())
}
