//! Control-prepared ownership for shared immutable analytic instrument contexts.
use crate::Invalid;
use hex_lattice::{
    MAX_SHARED_ANALYTIC_CONTEXT_BYTES, NativeInstrumentRegistry, PreparedNativeInstrument,
    PreparedSharedAnalyticContext,
};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{DeviceId, Project, TrackId};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(crate) const MAX_SHARED_CONTEXTS_PER_REVISION: usize = hex_project::MAX_TRACKS;
pub(crate) const MAX_SHARED_CONTEXT_BYTES_PER_REVISION: usize = 2 * 1024 * 1024;
pub(crate) const MAX_RETAINED_SHARED_CONTEXTS: usize = 16_384;
pub(crate) const MAX_RETAINED_SHARED_CONTEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextLeaseSource {
    Revision(u16),
    Retained(u16),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ContextLeasePlan {
    pub(crate) id: SharedContextId,
    pub(crate) source: ContextLeaseSource,
    pub(crate) target: u16,
    pub(crate) bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextLeasePlanError {
    ContextTooLarge,
    ByteBudgetExceeded,
    SlotBudgetExceeded,
    ArithmeticOverflow,
}

/// Deduplicate old-context requests and assign candidate lease slots without
/// mutating either playback owner. The candidate's preallocated slot table is
/// only read; ownership moves happen after every graph handoff preflight.
pub(crate) fn preflight_context_lease_plan(
    plan: &mut Vec<ContextLeasePlan>,
    retained_slots: &[Option<Arc<SharedContextOwner>>],
) -> Result<(), ContextLeasePlanError> {
    plan.sort_unstable_by_key(|request| {
        let retained_first = matches!(request.source, ContextLeaseSource::Retained(_));
        (request.id, !retained_first)
    });
    plan.dedup_by_key(|request| request.id);

    let mut retained_bytes = 0_usize;
    let mut target_cursor = 0_usize;
    for request in plan.iter_mut() {
        if request.bytes > MAX_SHARED_ANALYTIC_CONTEXT_BYTES {
            return Err(ContextLeasePlanError::ContextTooLarge);
        }
        retained_bytes = retained_bytes
            .checked_add(request.bytes)
            .ok_or(ContextLeasePlanError::ArithmeticOverflow)?;
        if retained_bytes > MAX_RETAINED_SHARED_CONTEXT_BYTES {
            return Err(ContextLeasePlanError::ByteBudgetExceeded);
        }
        while retained_slots
            .get(target_cursor)
            .is_some_and(Option::is_some)
        {
            target_cursor = target_cursor
                .checked_add(1)
                .ok_or(ContextLeasePlanError::ArithmeticOverflow)?;
        }
        let target =
            u16::try_from(target_cursor).map_err(|_| ContextLeasePlanError::SlotBudgetExceeded)?;
        if target_cursor >= MAX_RETAINED_SHARED_CONTEXTS || target_cursor >= retained_slots.len() {
            return Err(ContextLeasePlanError::SlotBudgetExceeded);
        }
        request.target = target;
        target_cursor = target_cursor
            .checked_add(1)
            .ok_or(ContextLeasePlanError::ArithmeticOverflow)?;
    }
    Ok(())
}

static NEXT_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SharedContextIdentity {
    track: TrackId,
    device: DeviceId,
    device_type: DeviceTypeId,
    state: NativeDeviceState,
    sample_rate: u32,
}

/// A process-local identity assigned on the control side. It is neither a
/// provider pointer nor persisted project data and remains stable when a
/// compatible immutable context is reused across revisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SharedContextId(u64);

impl SharedContextId {
    fn fresh() -> Result<Self, Invalid> {
        NEXT_CONTEXT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map(Self)
            .map_err(|_| Invalid("shared instrument context identity exhausted"))
    }
}

/// One unique immutable configuration owner. Its declared byte count covers
/// all context-owned storage; the table owner and Arc handle are accounted
/// separately from the 2 MiB payload budget.
pub(crate) struct SharedContextOwner {
    id: SharedContextId,
    identity: SharedContextIdentity,
    declared_bytes: usize,
    context: Arc<dyn PreparedSharedAnalyticContext>,
}

impl std::fmt::Debug for SharedContextOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedContextOwner")
            .field("id", &self.id)
            .field("device", &self.identity.device)
            .field("declared_bytes", &self.declared_bytes)
            .finish_non_exhaustive()
    }
}

impl SharedContextOwner {
    pub(crate) fn id(&self) -> SharedContextId {
        self.id
    }

    pub(crate) fn declared_bytes(&self) -> usize {
        self.declared_bytes
    }

    pub(crate) fn context(&self) -> &dyn PreparedSharedAnalyticContext {
        self.context.as_ref()
    }

    fn compatible_identity(&self, identity: &SharedContextIdentity) -> bool {
        self.identity == *identity
    }
}

/// Sorted-by-device table shared by Arrangement and Session banks in one
/// prepared revision. Note entries store a checked u16 index into this table;
/// callback sampling is O(1) and does not clone an Arc.
#[derive(Debug, Default)]
pub(crate) struct SharedAnalyticContextTable {
    owners: Vec<Arc<SharedContextOwner>>,
    ids: Vec<(SharedContextId, u16)>,
    payload_bytes: usize,
}

impl SharedAnalyticContextTable {
    pub(crate) fn empty() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn prepare(
        project: &Project,
        registry: &NativeInstrumentRegistry<'_>,
        previous: Option<&Self>,
    ) -> Result<Arc<Self>, Invalid> {
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| Invalid("could not reserve shared instrument context owners"))?;
        let mut payload_bytes = 0_usize;

        for track in project.tracks() {
            let Some(device) = track.instrument().filter(|device| !device.bypassed()) else {
                continue;
            };
            let prepared = registry
                .prepare_instrument(device.state(), project.sample_rate())
                .map_err(|_| Invalid("track instrument state is unsupported"))?;
            let Some(context) = prepared
                .prepare_shared_analytic_context()
                .map_err(|_| Invalid("shared analytic instrument context is unsupported"))?
            else {
                continue;
            };
            let identity = SharedContextIdentity {
                track: track.id(),
                device: device.id(),
                device_type: device.state().type_id(),
                state: device.state().clone().without_output_gain(),
                sample_rate: project.sample_rate(),
            };
            if previous
                .and_then(|table| table.owner_for_device(device.id()))
                .is_some_and(|owner| owner.compatible_identity(&identity))
            {
                let owner = previous
                    .and_then(|table| table.owner_for_device(device.id()))
                    .ok_or(Invalid("shared instrument context reuse index is invalid"))?;
                payload_bytes = payload_bytes
                    .checked_add(owner.declared_bytes)
                    .ok_or(Invalid("shared instrument context budget overflow"))?;
                let previous_index = previous
                    .and_then(|table| table.index_for_device(device.id()))
                    .ok_or(Invalid("shared instrument context reuse index is invalid"))?;
                owners.push(
                    previous
                        .and_then(|table| table.clone_owner(previous_index))
                        .ok_or(Invalid("shared instrument context owner is unavailable"))?,
                );
                continue;
            }

            let declared_bytes = context.declared_context_bytes();
            if declared_bytes > MAX_SHARED_ANALYTIC_CONTEXT_BYTES {
                return Err(Invalid("shared analytic context exceeds its byte limit"));
            }
            payload_bytes = payload_bytes
                .checked_add(declared_bytes)
                .ok_or(Invalid("shared instrument context budget overflow"))?;
            if payload_bytes > MAX_SHARED_CONTEXT_BYTES_PER_REVISION {
                return Err(Invalid(
                    "shared instrument context revision budget exceeded",
                ));
            }
            let id = SharedContextId::fresh()?;
            owners.push(Arc::new(SharedContextOwner {
                id,
                identity,
                declared_bytes,
                context: Arc::from(context),
            }));
        }
        if owners.len() > MAX_SHARED_CONTEXTS_PER_REVISION {
            return Err(Invalid("shared instrument context count limit reached"));
        }
        if payload_bytes > MAX_SHARED_CONTEXT_BYTES_PER_REVISION {
            return Err(Invalid(
                "shared instrument context revision budget exceeded",
            ));
        }
        owners.sort_unstable_by_key(|owner| owner.identity.device);
        let mut ids = Vec::new();
        ids.try_reserve_exact(owners.len())
            .map_err(|_| Invalid("could not reserve shared instrument context index"))?;
        for (index, owner) in owners.iter().enumerate() {
            ids.push((
                owner.id,
                u16::try_from(index)
                    .map_err(|_| Invalid("shared instrument context index overflow"))?,
            ));
        }
        ids.sort_unstable_by_key(|(id, _)| *id);
        Ok(Arc::new(Self {
            owners,
            ids,
            payload_bytes,
        }))
    }

    pub(crate) fn owner_for_device(&self, device: DeviceId) -> Option<&SharedContextOwner> {
        self.owners
            .binary_search_by_key(&device, |owner| owner.identity.device)
            .ok()
            .and_then(|index| self.owners.get(index).map(Arc::as_ref))
    }

    pub(crate) fn owner(&self, index: u16) -> Option<&SharedContextOwner> {
        self.owners.get(usize::from(index)).map(Arc::as_ref)
    }

    pub(crate) fn clone_owner(&self, index: u16) -> Option<Arc<SharedContextOwner>> {
        self.owners.get(usize::from(index)).map(Arc::clone)
    }

    pub(crate) fn index_for_device(&self, device: DeviceId) -> Option<u16> {
        self.owners
            .binary_search_by_key(&device, |owner| owner.identity.device)
            .ok()
            .and_then(|index| u16::try_from(index).ok())
    }

    pub(crate) fn index_for_id(&self, id: SharedContextId) -> Option<u16> {
        self.ids
            .binary_search_by_key(&id, |(owner_id, _)| *owner_id)
            .ok()
            .and_then(|index| self.ids.get(index).map(|(_, owner_index)| *owner_index))
    }

    pub(crate) fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    #[cfg(test)]
    pub(crate) fn storage_bytes(&self) -> Result<usize, Invalid> {
        let owner_handles = self
            .owners
            .capacity()
            .checked_mul(std::mem::size_of::<Arc<SharedContextOwner>>())
            .ok_or(Invalid("shared instrument context owner size overflow"))?;
        let id_index = self
            .ids
            .capacity()
            .checked_mul(std::mem::size_of::<(SharedContextId, u16)>())
            .ok_or(Invalid("shared instrument context owner size overflow"))?;
        owner_handles
            .checked_add(id_index)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or(Invalid("shared instrument context owner size overflow"))
    }

    #[cfg(test)]
    pub(crate) fn context_count(&self) -> usize {
        self.owners.len()
    }
}

#[allow(dead_code)]
fn _prepared_config_is_send_sync<T: PreparedNativeInstrument>() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests(count: usize, bytes: usize) -> Vec<ContextLeasePlan> {
        (0..count)
            .map(|index| ContextLeasePlan {
                id: SharedContextId(u64::try_from(index).unwrap() + 1),
                source: ContextLeaseSource::Revision(u16::try_from(index).unwrap()),
                target: u16::MAX,
                bytes,
            })
            .collect()
    }

    fn empty_slots() -> Vec<Option<Arc<SharedContextOwner>>> {
        (0..MAX_RETAINED_SHARED_CONTEXTS).map(|_| None).collect()
    }

    #[test]
    fn retained_context_plan_accepts_exact_byte_and_slot_caps() {
        let mut byte_exact = requests(
            MAX_RETAINED_SHARED_CONTEXT_BYTES / MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
            MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
        );
        let slots = empty_slots();
        preflight_context_lease_plan(&mut byte_exact, &slots).unwrap();
        assert_eq!(
            byte_exact.last().unwrap().target as usize + 1,
            byte_exact.len()
        );
        assert_eq!(
            byte_exact.len() * MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
            MAX_RETAINED_SHARED_CONTEXT_BYTES
        );

        let mut slot_exact = requests(MAX_RETAINED_SHARED_CONTEXTS, 0);
        preflight_context_lease_plan(&mut slot_exact, &slots).unwrap();
        assert_eq!(slot_exact[0].target, 0);
        assert_eq!(
            slot_exact.last().unwrap().target as usize + 1,
            MAX_RETAINED_SHARED_CONTEXTS
        );
    }

    #[test]
    fn retained_context_plan_refusal_is_atomic_and_retryable() {
        let slots = empty_slots();
        let mut over_bytes = requests(
            MAX_RETAINED_SHARED_CONTEXT_BYTES / MAX_SHARED_ANALYTIC_CONTEXT_BYTES + 1,
            MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
        );
        assert_eq!(
            preflight_context_lease_plan(&mut over_bytes, &slots),
            Err(ContextLeasePlanError::ByteBudgetExceeded)
        );
        assert!(slots.iter().all(Option::is_none));
        over_bytes.pop();
        preflight_context_lease_plan(&mut over_bytes, &slots).unwrap();
        assert_eq!(over_bytes.len(), 512);

        let mut over_slots = requests(MAX_RETAINED_SHARED_CONTEXTS + 1, 0);
        assert_eq!(
            preflight_context_lease_plan(&mut over_slots, &slots),
            Err(ContextLeasePlanError::SlotBudgetExceeded)
        );
        assert!(slots.iter().all(Option::is_none));
        over_slots.pop();
        preflight_context_lease_plan(&mut over_slots, &slots).unwrap();
        assert_eq!(over_slots.len(), MAX_RETAINED_SHARED_CONTEXTS);

        let mut malformed = requests(2, 1);
        malformed[0].bytes = usize::MAX;
        assert_eq!(
            preflight_context_lease_plan(&mut malformed, &slots),
            Err(ContextLeasePlanError::ContextTooLarge)
        );
        assert!(slots.iter().all(Option::is_none));
    }

    #[test]
    fn retained_context_plan_refuses_a_short_destination_table() {
        let mut plan = requests(1, 16);
        assert_eq!(
            preflight_context_lease_plan(&mut plan, &[]),
            Err(ContextLeasePlanError::SlotBudgetExceeded)
        );
        preflight_context_lease_plan(&mut plan, &[None]).unwrap();
        assert_eq!(plan[0].target, 0);
    }

    #[test]
    fn retained_context_plan_deduplicates_by_process_identity() {
        let mut plan = requests(2, 1);
        plan[1].id = plan[0].id;
        plan[1].source = ContextLeaseSource::Retained(7);
        let slots = empty_slots();
        preflight_context_lease_plan(&mut plan, &slots).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].source, ContextLeaseSource::Retained(7));
        assert_eq!(plan[0].target, 0);
    }

    #[test]
    fn maximum_context_table_handle_and_index_layout_is_bounded() {
        let owner_handles = MAX_SHARED_CONTEXTS_PER_REVISION
            .checked_mul(std::mem::size_of::<Arc<SharedContextOwner>>())
            .unwrap();
        let id_index_entries = MAX_SHARED_CONTEXTS_PER_REVISION
            .checked_mul(std::mem::size_of::<(SharedContextId, u16)>())
            .unwrap();
        let table_header = std::mem::size_of::<SharedAnalyticContextTable>();
        assert_eq!(owner_handles, 2_048);
        assert_eq!(id_index_entries, 4_096);
        assert_eq!(table_header, 56);
        assert_eq!(owner_handles + id_index_entries + table_header, 6_200);
    }

    #[test]
    fn fresh_playback_lease_slots_recycle_across_sequential_revisions() {
        for revision in 0..300_usize {
            let slots = empty_slots();
            let mut plan = requests(1, 16);
            plan[0].id = SharedContextId(u64::try_from(revision).unwrap() + 1);
            preflight_context_lease_plan(&mut plan, &slots).unwrap();
            assert_eq!(plan[0].target, 0);
        }
    }
}
