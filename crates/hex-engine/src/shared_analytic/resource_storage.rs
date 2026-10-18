//! Fresh current-state construction and selected inline-owner metadata only.
//! No historical format reader, raw Arc upgrade or publication permission.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageRole,
    audio_storage::{Location, Owner},
    owned_shared::OwnedShared,
};
use hex_lattice_contract::DeviceAssetReference;

pub(super) struct InlineStateStorage {
    payload: (usize, usize),
    references: Option<(usize, usize)>,
    handles: [u64; 2],
}
pub(super) fn prepare_state_on_control(
    source: &NativeDeviceState,
    resources: &crate::native_device_resources::DeviceResourceConfiguration,
    max_metadata_bytes: usize,
) -> Result<(NativeDeviceState, InlineStateStorage), Invalid> {
    let refused = || Invalid("inline resource owner metadata preparation refused");
    let payload =
        OwnedShared::<[u8]>::declared_slice_parts(source.payload().len()).ok_or_else(refused)?;
    let references = if source.asset_references().is_empty() {
        None
    } else {
        Some(
            OwnedShared::<[DeviceAssetReference]>::declared_slice_parts(
                source.asset_references().len(),
            )
            .ok_or_else(refused)?,
        )
    };
    let (owner_bytes, overhead) =
        OwnedShared::<SharedContextOwner>::declared_parts().ok_or_else(refused)?;
    // Include transient Vec -> Arc backing coexistence before either fresh
    // allocation. The old canonical source is borrowed, never adopted/upgraded.
    let quote = owner_bytes
        .checked_add(overhead)
        .and_then(|n| n.checked_add(std::mem::size_of::<SharedContextLease>()))
        .and_then(|n| n.checked_add(payload.0.checked_mul(2)?))
        .and_then(|n| n.checked_add(payload.1))
        .and_then(|n| {
            n.checked_add(references.map_or(Some(0), |(bytes, header)| {
                bytes.checked_mul(2)?.checked_add(header)
            })?)
        })
        .and_then(|n| {
            n.checked_add(
                resources
                    .declared_metadata_bytes()
                    .ok()?
                    .checked_sub(std::mem::size_of_val(resources))?,
            )
        })
        .ok_or_else(refused)?;
    if quote > max_metadata_bytes {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(source.payload().len())
        .map_err(|_| refused())?;
    if bytes.capacity() != source.payload().len() {
        return Err(refused());
    }
    bytes.extend_from_slice(source.payload());
    let mut refs = Vec::new();
    refs.try_reserve_exact(source.asset_references().len())
        .map_err(|_| refused())?;
    if refs.capacity() != source.asset_references().len() {
        return Err(refused());
    }
    refs.extend_from_slice(source.asset_references());
    let state = NativeDeviceState::new(source.type_id(), source.state_version(), bytes)
        .and_then(|state| state.with_asset_references(refs))
        .and_then(|state| state.with_input_gain_millidb(source.input_gain_millidb()))
        .and_then(|state| state.with_output_gain_millidb(source.output_gain_millidb()))
        .map_err(|_| refused())?;
    let handles = [
        crate::audio_only_inputs::handle().map_err(|_| refused())?,
        crate::audio_only_inputs::handle().map_err(|_| refused())?,
    ];
    Ok((
        state,
        InlineStateStorage {
            payload,
            references,
            handles,
        },
    ))
}
fn visit_state<'a>(
    state: &'a NativeDeviceState,
    storage: &InlineStateStorage,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if state.payload_storage_observation().0 != storage.payload.0
        || state
            .asset_reference_storage_observation()
            .map(|value| value.0)
            != storage
                .references
                .map(|parts| parts.0 / std::mem::size_of::<DeviceAssetReference>())
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    ledger.bytes(
        Owner::device_payload(state),
        Location("inline-state-payload", 0),
        storage
            .payload
            .0
            .checked_add(storage.payload.1)
            .ok_or(AudioStorageError::Overflow)?,
    )?;
    if !ledger.skip_observed_canonical_state_field(state, 0) {
        ledger.handle_at(
            Owner::device_payload(state),
            Owner::exclusive(state),
            Location("inline-state-field", 0),
            storage.handles[0],
        )?;
    }
    if let Some((bytes, overhead)) = storage.references {
        ledger.bytes(
            Owner::device_asset_references(state),
            Location("inline-state-references", 0),
            bytes
                .checked_add(overhead)
                .ok_or(AudioStorageError::Overflow)?,
        )?;
        if !ledger.skip_observed_canonical_state_field(state, 1) {
            ledger.handle_at(
                Owner::device_asset_references(state),
                Owner::exclusive(state),
                Location("inline-state-field", 1),
                storage.handles[1],
            )?;
        }
    }
    Ok(())
}
pub(crate) fn visit_owner<'a>(
    owner: &'a SharedContextLease,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    ledger.root(owner, role)?;
    visit_owner_contents(owner, ledger)
}
// Enclosing graph visitors charge the actual complete owner Vec capacity;
// reuse this contents visitor without charging its embedded handle twice.
pub(crate) fn visit_owner_contents<'a>(
    owner: &'a SharedContextLease,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let SharedContextOwner {
        id: _,
        identity,
        declared_bytes,
        context,
        resources,
        state_storage,
        sampler_factory,
    } = owner.as_ref();
    let SharedContextIdentity {
        tuning: _,
        resource_scope: _,
        track: _,
        device: _,
        device_type: _,
        state,
        sample_rate: _,
    } = identity;
    if context.is_some() {
        return Err(AudioStorageError::UnknownStorage);
    }
    match sampler_factory {
        Some((factory, _)) if identity.device_type == hex_devices::sampler::state::TYPE_ID => {
            if factory.control_storage_declaration()
                != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                    bytes: *declared_bytes,
                })
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        None if *declared_bytes == 0
            && identity.device_type != hex_devices::sampler::state::TYPE_ID => {}
        _ => return Err(AudioStorageError::UnknownStorage),
    }
    let empty_sampler = sampler_factory.is_some()
        && resources.is_none()
        && state_storage.is_none()
        && matches!(
            hex_devices::sampler::state::DeviceStateRead::from_native(state),
            Ok(hex_devices::sampler::state::DeviceStateRead::Empty)
        );
    if !empty_sampler && (state_storage.is_none() || resources.is_none()) {
        return Err(AudioStorageError::UnknownStorage);
    }
    let visited = ledger.skip_observed_shared_contents(owner.arc());
    ledger.observe_shared(owner.arc(), owner.parts(), Owner::exclusive(owner))?;
    if !visited {
        if empty_sampler {
            // Actual cloned canonical empty-state field, not a fresh receipt,
            // fabricated byte claim, or invented empty resource configuration.
            ledger.borrow_canonical_native_state_on_control(state)?;
        } else {
            visit_state(
                state,
                state_storage
                    .as_ref()
                    .ok_or(AudioStorageError::UnknownStorage)?,
                ledger,
            )?;
            resources
                .as_ref()
                .ok_or(AudioStorageError::UnknownStorage)?
                .visit_storage(ledger)?;
        }
        if let Some((factory, handle)) = sampler_factory {
            ledger.shared(
                factory.arc(),
                factory.parts(),
                Owner::exclusive(factory),
                *handle,
                None,
            )?;
        }
    }
    Ok(())
}
pub(crate) fn visit_revision<'a>(
    revision: &'a crate::PreparedRevision,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    let arrangement = revision.arrangement().shared_contexts();
    let session = revision.session().shared_contexts();
    for owner in &arrangement.owners {
        visit_owner(owner, ledger, role)?;
    }
    if !Arc::ptr_eq(arrangement, session) {
        for owner in &session.owners {
            visit_owner(owner, ledger, role)?;
        }
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected actual inline owner fields only. This does not declare the
    /// revision/table allocations, provider contexts or whole graph known, and
    /// it provides no active-runtime or retirement/publication authority.
    pub fn borrow_revision_inline_resource_owners_on_control(
        &mut self,
        revision: &'a crate::PreparedRevision,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_revision(revision, ledger, role))
    }
}

impl<'a> AudioRootStorageLedger<'a> {
    /// Selected current and carried inline-owner fields while the actual
    /// Session kernel is held on control. This borrow cannot authorize access
    /// to a concurrently active runtime or prove whole-host retirement.
    pub fn borrow_session_inline_resource_owners_on_control(
        &mut self,
        session: &'a crate::SessionPlayback,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| session.visit_inline_resource_owners_on_control(ledger, role))
    }
}

pub(crate) fn visit_table_contents<'a>(
    table: &'a OwnedShared<SharedAnalyticContextTable>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let visited = ledger.skip_observed_shared_contents(table.arc());
    ledger.observe_shared(table.arc(), table.parts(), Owner::exclusive(table))?;
    if visited {
        return Ok(());
    }
    let SharedAnalyticContextTable {
        tuning,
        tuning_identity,
        owners,
        ids,
        payload_bytes,
        mutable_continuations,
    } = table.as_ref();
    let _: &[f64; 128] = tuning.frequencies();
    let _: u64 = tuning_identity.0;
    ledger.vec(owners)?;
    ledger.vec(ids)?;
    ledger.vec(mutable_continuations)?;
    for owner in owners {
        visit_owner_contents(owner, ledger)?;
    }
    let actual = owners
        .iter()
        .try_fold(0usize, |sum, owner| sum.checked_add(owner.declared_bytes))
        .ok_or(AudioStorageError::Overflow)?;
    if actual != *payload_bytes {
        return Err(AudioStorageError::SourceMismatch);
    }
    Ok(())
}
pub(crate) fn visit_table<'a>(
    table: &'a OwnedShared<SharedAnalyticContextTable>,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    // One cached template can be held by multiple real revisions. Its one
    // physical table field is counted once, while distinct table aliases still
    // require distinct headers/handles even when their allocation is shared.
    if ledger.skip_observed_root(table) {
        return Ok(());
    }
    ledger.root(table, role)?;
    visit_table_contents(table, ledger)
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual Arrangement and Session context table headers, backing capacities
    /// and nested resource-owner metadata. Includes both physical table handles
    /// when their allocation is shared. Opaque provider contexts and enclosing
    /// graph/runtime storage remain unsupported;
    /// this grants no publication or retirement authority. Do not combine with
    /// overlapping inline-owner-only inventory of the same physical fields.
    pub fn borrow_revision_context_tables_on_control(
        &mut self,
        revision: &'a crate::PreparedRevision,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            visit_table(revision.arrangement().shared_contexts(), ledger, role)?;
            visit_table(revision.session().shared_contexts(), ledger, role)?;
            for track in revision.arrangement().tracks() {
                if let Some(instrument) = &track.instrument {
                    visit_table(&instrument.shared_contexts, ledger, role)?;
                }
            }
            revision
                .session()
                .visit_midi_context_tables_on_control(ledger, role)
        })
    }
}

#[cfg(test)]
mod arithmetic_tests;
