//! The pinned Audio fixed-ring constructor, not a general queue registry. The
//! audited fresh factory owns two non-Clone endpoint fields; its actual core
//! borrow/layout and Element type describe the frozen allocations. Element
//! owning descendants are never certified absent by this physical record.
use super::*;
use crate::{SharedMetadata, sampler_host_fields::BodyRef};
use std::{
    alloc::Layout,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const DISPOSED: u8 = 2;
const FAILED: u8 = 3;

/// Public INTERNAL audited producer schema. Only the owning Audio module may
/// implement it under the repository guard. This is not hostile-client privacy,
/// arbitrary byte/identity input or permission to qualify a legacy endpoint.
pub trait NativeFixedRingFields {
    type Element;
    fn producer_core(&self) -> BodyRef<'_>;
    fn consumer_core(&self) -> BodyRef<'_>;
    fn capacity(&self) -> usize;
    fn pinned_constructor_supported(&self) -> bool;
}
struct State {
    core_bytes: usize,
    core_body_bytes: usize,
    backing_bytes: usize,
    endpoint_lineages: [u64; 2],
    lease_lineages: [u64; 2],
    phases: [AtomicU8; 2],
}
pub struct NativeFixedRingRegistration {
    state: SharedMetadata<State>,
}
pub struct NativeFixedRingLease {
    state: SharedMetadata<State>,
    endpoint: usize,
}

fn sum(a: usize, b: usize) -> Result<usize, AudioStorageError> {
    a.checked_add(b).ok_or(AudioStorageError::Overflow)
}
fn core_bytes(layout: Layout) -> Result<usize, AudioStorageError> {
    if !crate::pcm_arc::native_layout_known() {
        return Err(AudioStorageError::UnknownStorage);
    }
    Ok(Layout::array::<AtomicUsize>(2)
        .map_err(|_| AudioStorageError::Overflow)?
        .extend(layout)
        .map_err(|_| AudioStorageError::Overflow)?
        .0
        .pad_to_align()
        .size())
}
impl NativeFixedRingRegistration {
    pub fn metadata_requested_bytes_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<State>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
        sum(body, header)
    }
    /// Actual Audio factory instantiates Core=rtrb::RingBuffer<Element>. This
    /// static preconstruction quote is not an origin or combined-host approval.
    pub fn requested_bytes_on_control<Core, Element>(
        capacity: usize,
    ) -> Result<usize, AudioStorageError> {
        let backing =
            Layout::array::<Element>(capacity).map_err(|_| AudioStorageError::Overflow)?;
        sum(
            sum(core_bytes(Layout::new::<Core>())?, backing.size())?,
            Self::metadata_requested_bytes_on_control()?,
        )
    }
    pub fn construction_scratch_bytes_on_control<Core>() -> Result<usize, AudioStorageError> {
        sum(
            sum(size_of::<Core>(), size_of::<State>())?,
            size_of::<(Self, [NativeFixedRingLease; 2])>(),
        )
    }
    /// Actual fresh paired endpoints only, before escape. Reference equality is
    /// consistency of the constructor pair, NOT an allocation origin proof.
    pub fn pair_actual_fresh_ring_on_control<F: NativeFixedRingFields>(
        fields: &F,
    ) -> Result<(Self, [NativeFixedRingLease; 2]), AudioStorageError> {
        if !fields.pinned_constructor_supported() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let producer = fields.producer_core();
        if !producer.same(fields.consumer_core()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        let core_layout = producer.layout()?;
        let core_bytes = core_bytes(core_layout)?;
        let backing_bytes = Layout::array::<F::Element>(fields.capacity())
            .map_err(|_| AudioStorageError::Overflow)?
            .size();
        sum(
            sum(core_bytes, backing_bytes)?,
            Self::metadata_requested_bytes_on_control()?,
        )?;
        let endpoint_lineages = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let lease_lineages = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let state = SharedMetadata::new_on_control(State {
            core_bytes,
            core_body_bytes: core_layout.size(),
            backing_bytes,
            endpoint_lineages,
            lease_lineages,
            phases: std::array::from_fn(|_| AtomicU8::new(LIVE)),
        });
        let leases = std::array::from_fn(|endpoint| NativeFixedRingLease {
            state: state.clone(),
            endpoint,
        });
        Ok((Self { state }, leases))
    }
    /// Constructor correlation of one actual affine endpoint field. Neither
    /// layout/count equality nor this match certifies element descendants.
    pub fn matches_live_endpoint_on_control(
        &self,
        lease: &NativeFixedRingLease,
        endpoint: usize,
    ) -> bool {
        endpoint < 2
            && lease.endpoint == endpoint
            && self.state.shares_owner_with(&lease.state)
            && self.state.phases[endpoint].load(Ordering::Acquire) == LIVE
    }
    /// Increment over the old queue-body/backing-only budget. This private
    /// constructor record supplies actual pinned core padding/header plus its
    /// own metadata allocation, not origin/closure/admission authority.
    pub fn legacy_budget_increment_on_control(&self) -> Result<usize, AudioStorageError> {
        if self
            .state
            .phases
            .iter()
            .any(|phase| !matches!(phase.load(Ordering::Acquire), LIVE | DISPOSED))
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        sum(
            self.state
                .core_bytes
                .checked_sub(self.state.core_body_bytes)
                .ok_or(AudioStorageError::UnknownStorage)?,
            Self::metadata_requested_bytes_on_control()?,
        )
    }
    pub fn matches_actual_leases_on_control(&self, leases: [&NativeFixedRingLease; 2]) -> bool {
        leases.into_iter().enumerate().all(|(index, lease)| {
            lease.endpoint == index && self.state.shares_owner_with(&lease.state)
        })
    }
    pub fn is_normally_disposed_on_control(&self) -> bool {
        self.state
            .phases
            .iter()
            .all(|p| p.load(Ordering::Acquire) == DISPOSED)
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.mutate(|ledger| self.visit_fields(ledger))
    }
    pub(crate) fn visit_fields<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let phases = self
            .state
            .phases
            .each_ref()
            .map(|p| p.load(Ordering::Acquire));
        if phases.iter().any(|p| !matches!(*p, LIVE | DISPOSED)) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let paired = self.state.paired_owner_on_control();
        if !ledger.skip_observed_shared_field(&self.state) {
            ledger.observe_shared(paired.arc(), paired.parts(), Owner::exclusive(&self.state))?;
        }
        let mut physical = Owner::exclusive(self.state.as_ref());
        physical.domain = OwnerDomain::RetainedNative;
        if phases.contains(&LIVE) {
            ledger.bytes(
                physical,
                Location("native-fixed-ring-core", 0),
                self.state.core_bytes,
            )?;
            ledger.bytes(
                physical,
                Location("native-fixed-ring-elements", 0),
                self.state.backing_bytes,
            )?;
        }
        let metadata = Owner::shared(paired.arc());
        for (index, phase) in phases.into_iter().enumerate() {
            if phase == LIVE {
                ledger.handle_at(
                    physical,
                    Owner::exclusive(&self.state.endpoint_lineages[index]),
                    Location("native-fixed-ring-endpoint", index),
                    self.state.endpoint_lineages[index],
                )?;
                if let Some(arithmetic) = &mut ledger.arithmetic {
                    arithmetic.existing(metadata)?;
                }
                ledger.handle_at(
                    metadata,
                    Owner::exclusive(&self.state.lease_lineages[index]),
                    Location("native-fixed-ring-lease", index),
                    self.state.lease_lineages[index],
                )?;
            }
        }
        Ok(())
    }
}
impl NativeFixedRingLease {
    pub fn begin_actual_endpoint_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.state.phases[self.endpoint];
        if phase.load(Ordering::Acquire) != LIVE {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        phase.store(DROP_PENDING, Ordering::Release);
        Ok(())
    }
    pub fn complete_actual_endpoint_drop_on_control(&mut self) -> Result<(), AudioStorageError> {
        let phase = &self.state.phases[self.endpoint];
        if phase.load(Ordering::Acquire) != DROP_PENDING {
            phase.store(FAILED, Ordering::Release);
            return Err(AudioStorageError::UnknownStorage);
        }
        phase.store(DISPOSED, Ordering::Release);
        Ok(())
    }
}
impl Drop for NativeFixedRingLease {
    fn drop(&mut self) {
        if self.state.phases[self.endpoint].load(Ordering::Acquire) != DISPOSED {
            self.state.phases[self.endpoint].store(FAILED, Ordering::Release);
        }
    }
}
