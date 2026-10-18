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

#[cfg(feature = "instrument-test-support")]
pub(crate) mod inline_fixture;
pub(crate) mod resource_storage;
#[cfg(test)]
mod resource_tests;
#[cfg(test)]
pub(crate) mod sampler_tests;

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
    retained_slots: &[Option<crate::shared_analytic::SharedContextLease>],
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

static NEXT_TUNING_ID: AtomicU64 = AtomicU64::new(1);
/// Exact prepared-table provenance. Zero denotes the exact legacy neutral table;
/// nonneutral IDs are reused only after all128 frequency bits compare equal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TuningIdentity(u64);

static NEXT_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SharedContextIdentity {
    tuning: TuningIdentity,
    resource_scope: u64,
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
pub(crate) type SharedContextLease = crate::owned_shared::OwnedShared<SharedContextOwner>;
pub(crate) struct SharedContextOwner {
    id: SharedContextId,
    identity: SharedContextIdentity,
    declared_bytes: usize,
    context: Option<Arc<dyn PreparedSharedAnalyticContext>>,
    resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    state_storage: Option<resource_storage::InlineStateStorage>,
    sampler_factory: Option<(
        crate::owned_shared::OwnedShared<hex_devices::sampler::PreparedSamplerRuntime>,
        u64,
    )>,
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
    fn from_prepared_on_control(
        id: SharedContextId,
        mut identity: SharedContextIdentity,
        declared_bytes: usize,
        context: Box<dyn PreparedSharedAnalyticContext>,
        resources: Option<crate::native_device_resources::DeviceResourceConfiguration>,
    ) -> Self {
        identity.resource_scope = resources
            .as_ref()
            .map_or(0, |lease| lease.source_scope_identity());
        Self {
            id,
            identity,
            declared_bytes,
            context: Some(Arc::from(context)),
            state_storage: None,
            sampler_factory: None,
            resources,
        }
    }
    fn from_inline_resources_on_control(
        id: SharedContextId,
        mut identity: SharedContextIdentity,
        resources: crate::native_device_resources::DeviceResourceConfiguration,
        max_metadata_bytes: usize,
    ) -> Result<Self, Invalid> {
        identity.resource_scope = resources.source_scope_identity();
        hex_lattice::HostDeviceResourceScope::validate_configuration_on_control(
            &resources,
            &identity.state,
            identity.sample_rate,
        )
        .map_err(|_| Invalid("inline resource owner scope mismatch"))?;
        let (state, storage) = resource_storage::prepare_state_on_control(
            &identity.state,
            &resources,
            max_metadata_bytes,
        )?;
        identity.state = state;
        Ok(Self {
            id,
            identity,
            declared_bytes: 0,
            context: None,
            resources: Some(resources),
            state_storage: Some(storage),
            sampler_factory: None,
        })
    }
    fn from_sampler_resources_on_control(
        id: SharedContextId,
        identity: SharedContextIdentity,
        resources: crate::native_device_resources::DeviceResourceConfiguration,
        tuning: &hex_lattice::TuningTable,
        max_metadata_bytes: usize,
        remaining_payload_bytes: usize,
    ) -> Result<Self, Invalid> {
        use crate::owned_shared::OwnedShared;
        use hex_devices::sampler::PreparedSamplerRuntime;
        let refused = || Invalid("sampler factory metadata preparation refused");
        if identity.device_type != hex_devices::sampler::state::TYPE_ID {
            return Err(refused());
        }
        let (body, header) =
            OwnedShared::<PreparedSamplerRuntime>::declared_parts().ok_or_else(refused)?;
        // Bound the fresh factory and decoded marker Vec BEFORE construction.
        // This component quote is not whole-graph coexistence/peak authority.
        if body > MAX_SHARED_ANALYTIC_CONTEXT_BYTES || body > remaining_payload_bytes {
            return Err(refused());
        }
        let quote = body
            .checked_add(header)
            .and_then(|n| {
                n.checked_add(hex_devices::sampler::MAX_SLICES * std::mem::size_of::<usize>())
            })
            .ok_or_else(refused)?;
        let remaining = max_metadata_bytes.checked_sub(quote).ok_or_else(refused)?;
        let mut owner = Self::from_inline_resources_on_control(id, identity, resources, remaining)?;
        let prepared = hex_devices::sampler::state::prepare_runtime_on_control(
            &owner.identity.state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: owner.identity.sample_rate,
                tuning,
                resources: owner.resources.as_ref().ok_or_else(refused)?,
            },
            crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY,
        )
        .map_err(|_| refused())?;
        if prepared.control_storage_declaration()
            != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly { bytes: body })
        {
            return Err(refused());
        }
        let handle = crate::audio_only_inputs::handle().map_err(|_| refused())?;
        owner.sampler_factory = Some((OwnedShared::new_owned(prepared), handle));
        owner.declared_bytes = body;
        Ok(owner)
    }
    pub(crate) fn id(&self) -> SharedContextId {
        self.id
    }

    pub(crate) fn device_id(&self) -> DeviceId {
        self.identity.device
    }

    pub(crate) fn declared_bytes(&self) -> usize {
        self.declared_bytes
    }

    pub(crate) fn analytic_context(&self) -> Option<&dyn PreparedSharedAnalyticContext> {
        self.context.as_deref()
    }
    pub(crate) fn require_analytic_context(
        &self,
    ) -> Result<&dyn PreparedSharedAnalyticContext, Invalid> {
        self.analytic_context()
            .ok_or(Invalid("shared analytic provider context is unavailable"))
    }
    pub(crate) fn is_inline_resource_scope(&self) -> bool {
        self.context.is_none() && self.resources.is_some()
    }

    pub(crate) fn resource_configuration(
        &self,
    ) -> Option<&crate::native_device_resources::DeviceResourceConfiguration> {
        self.resources.as_ref()
    }

    /// Clone only the actual constructor-paired concrete factory on control.
    /// The destination physical field must join the same allocation ledger.
    pub(crate) fn sampler_factory_on_control(
        &self,
    ) -> Option<crate::owned_shared::OwnedShared<hex_devices::sampler::PreparedSamplerRuntime>>
    {
        self.sampler_factory
            .as_ref()
            .map(|(factory, _)| factory.clone_on_control())
    }

    pub(crate) fn resources(&self) -> &dyn hex_lattice::DeviceResources {
        match &self.resources {
            Some(lease) => lease,
            None => &hex_lattice::EMPTY_DEVICE_RESOURCES,
        }
    }

    pub(crate) fn continuation_id(&self) -> Result<hex_lattice::AnalyticContinuationId, Invalid> {
        hex_lattice::AnalyticContinuationId::new(self.id.0)
            .map_err(|_| Invalid("shared analytic continuation identity is invalid"))
    }

    /// Evaluate only the actually applied owner. Desired intermediate revisions
    /// are never an input, and an in-flight ramp collapses to its exact boundary.
    pub(crate) fn continue_from(
        &self,
        previous: &Self,
        transition: Option<hex_lattice::AnalyticContinuation>,
        performance_frame: u64,
    ) -> Result<Option<hex_lattice::AnalyticContinuation>, Invalid> {
        if self.id == previous.id {
            return Ok(transition);
        }
        if self.identity.track != previous.identity.track
            || self.identity.resource_scope != previous.identity.resource_scope
            || self.identity.device != previous.identity.device
            || self.identity.device_type != previous.identity.device_type
            || self.identity.sample_rate != previous.identity.sample_rate
        {
            return Ok(None);
        }
        let Some(target) = self
            .analytic_context()
            .and_then(|context| context.analytic_continuation())
        else {
            return Ok(None);
        };
        let Some(source) = previous
            .analytic_context()
            .and_then(|context| context.analytic_continuation())
        else {
            return Ok(None);
        };
        if target.format() != source.format() {
            return Ok(None);
        }
        let source_id = previous.continuation_id()?;
        if self.identity == previous.identity {
            // A control preparation may recreate an identical context without
            // reuse. Rebind the fixed original start, retaining absolute clock
            // and duration; unrelated publication must not restart its ramp.
            return transition
                .map(|transition| {
                    let target_id = self.continuation_id()?;
                    let start = target
                        .prepare_start(target_id, source, source_id, transition.start())
                        .map_err(|_| {
                            Invalid("shared analytic continuation identity rebinding was refused")
                        })?;
                    hex_lattice::AnalyticContinuation::new(
                        target_id,
                        start,
                        transition.start_frame(),
                        transition.duration(),
                    )
                    .map_err(|_| Invalid("shared analytic continuation clock is invalid"))
                })
                .transpose();
        }
        let boundary = match transition {
            Some(transition) => source
                .parameters_at(source_id, &transition, performance_frame)
                .map_err(|_| Invalid("shared analytic continuation boundary was refused"))?,
            None => source.endpoint(source_id),
        };
        let target_id = self.continuation_id()?;
        let start = match target.prepare_start(target_id, source, source_id, &boundary) {
            Ok(start) => start,
            Err(hex_lattice::AnalyticContinuationError::IncompatibleSource) => return Ok(None),
            Err(_) => return Err(Invalid("shared analytic continuation source was refused")),
        };
        let duration =
            hex_lattice::AnalyticContinuation::default_duration(self.identity.sample_rate)
                .map_err(|_| Invalid("shared analytic continuation duration is invalid"))?;
        let transition =
            hex_lattice::AnalyticContinuation::new(target_id, start, performance_frame, duration)
                .map_err(|_| Invalid("shared analytic continuation clock is invalid"))?;
        // Check the provider's clock/identity law before admitting any voice.
        target
            .parameters_at(target_id, &transition, performance_frame)
            .map_err(|_| Invalid("shared analytic continuation boundary was refused"))?;
        Ok(Some(transition))
    }

    pub(crate) fn same_configuration(&self, other: &Self) -> bool {
        self.identity == other.identity
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
    tuning: hex_lattice::TuningTable,
    tuning_identity: TuningIdentity,
    owners: Vec<crate::shared_analytic::SharedContextLease>,
    ids: Vec<(SharedContextId, u16)>,
    payload_bytes: usize,
    mutable_continuations: Vec<(TrackId, DeviceId)>,
}

impl SharedAnalyticContextTable {
    /// Exhaustive audited empty-table payload check for the opt-in compiler.
    /// Vec headers belong to this outer payload; nonzero spare capacities or
    /// nested handles are unsupported rather than assumed free/complete.
    pub(crate) fn require_empty_owned_storage(&self) -> Result<(), crate::AudioStorageError> {
        let Self {
            tuning,
            tuning_identity,
            owners,
            ids,
            payload_bytes,
            mutable_continuations,
        } = self;
        let _: &[f64; 128] = tuning.frequencies();
        let _: u64 = tuning_identity.0;
        if !owners.is_empty()
            || owners.capacity() != 0
            || !ids.is_empty()
            || ids.capacity() != 0
            || *payload_bytes != 0
            || !mutable_continuations.is_empty()
            || mutable_continuations.capacity() != 0
        {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        Ok(())
    }

    pub(crate) fn empty() -> crate::owned_shared::OwnedShared<Self> {
        crate::owned_shared::OwnedShared::new_owned(Self::default())
    }

    pub(crate) fn prepare(
        project: &Project,
        registry: &NativeInstrumentRegistry<'_>,
        previous: Option<&Self>,
        resource_preparation: Option<crate::native_device_resources::DeviceResourcePreparation<'_>>,
    ) -> Result<crate::owned_shared::OwnedShared<Self>, Invalid> {
        let tuning = project.tuning().table().map_err(|e| Invalid(e.0))?;
        let tuning_identity = if tuning.is_neutral() {
            TuningIdentity::default()
        } else if let Some(old) = previous.filter(|old| old.tuning == tuning) {
            old.tuning_identity
        } else {
            TuningIdentity(
                NEXT_TUNING_ID
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                    .map_err(|_| Invalid("tuning identity exhausted"))?,
            )
        };
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| Invalid("could not reserve shared instrument context owners"))?;
        let mut payload_bytes = 0_usize;
        let mut mutable_continuations = Vec::new();
        mutable_continuations
            .try_reserve_exact(project.tracks().len())
            .map_err(|_| Invalid("could not reserve instrument continuation identities"))?;

        for track in project.tracks() {
            let Some(device) = track.instrument().filter(|device| !device.bypassed()) else {
                continue;
            };
            if device.state().type_id() == hex_devices::sampler::state::TYPE_ID {
                // This closed preparation branch is not registry/publication
                // admission. Validate the loaded codec before resource work.
                hex_devices::sampler::state::SamplerState::from_native(device.state())
                    .map_err(|_| Invalid("sampler state is not a loaded current configuration"))?;
            }
            let resources = match resource_preparation {
                Some(preparation) => preparation
                    .configuration_on_control(device.state())
                    .map_err(|_| Invalid("instrument resource scope preparation refused"))?,
                None if device.state().asset_references().is_empty() => None,
                None => return Err(Invalid("instrument resource source plan is unavailable")),
            };
            if device.state().type_id() == hex_devices::sampler::state::TYPE_ID {
                let resources =
                    resources.ok_or(Invalid("sampler resource scope is unavailable"))?;
                let identity = SharedContextIdentity {
                    tuning: tuning_identity,
                    resource_scope: resources.source_scope_identity(),
                    track: track.id(),
                    device: device.id(),
                    device_type: device.state().type_id(),
                    state: device.state().clone().without_host_gains(),
                    sample_rate: project.sample_rate(),
                };
                let previous_owner = previous.and_then(|table| table.owner_for_device(device.id()));
                let owner = if previous_owner.is_some_and(|owner| {
                    owner.compatible_identity(&identity) && owner.sampler_factory.is_some()
                }) {
                    let table = previous.ok_or(Invalid("sampler previous table is unavailable"))?;
                    let index = table
                        .index_for_device(device.id())
                        .ok_or(Invalid("sampler previous owner index is unavailable"))?;
                    table
                        .clone_owner(index)
                        .ok_or(Invalid("sampler previous owner is unavailable"))?
                } else {
                    crate::owned_shared::OwnedShared::new_owned(
                        SharedContextOwner::from_sampler_resources_on_control(
                            SharedContextId::fresh()?,
                            identity,
                            resources,
                            &tuning,
                            resource_preparation
                                .ok_or(Invalid("sampler preparation budget is unavailable"))?
                                .max_scope_metadata_bytes,
                            MAX_SHARED_CONTEXT_BYTES_PER_REVISION
                                .checked_sub(payload_bytes)
                                .ok_or(Invalid("sampler context budget overflow"))?,
                        )?,
                    )
                };
                payload_bytes = payload_bytes
                    .checked_add(owner.declared_bytes)
                    .filter(|bytes| *bytes <= MAX_SHARED_CONTEXT_BYTES_PER_REVISION)
                    .ok_or(Invalid("sampler context revision budget exceeded"))?;
                owners.push(owner);
                // Public instrument processing still refuses referenced states;
                // this merely retains the actual concrete factory/configuration
                // for the sealed host constructor's subsequent graph proof.
                continue;
            }
            let prepared = registry
                .prepare_instrument(
                    device.state(),
                    &hex_lattice::DevicePreparationContext {
                        sample_rate: project.sample_rate(),
                        tuning: &tuning,
                        resources: resources.as_ref().map_or(
                            &hex_lattice::EMPTY_DEVICE_RESOURCES
                                as &dyn hex_lattice::DeviceResources,
                            |resources| resources as &dyn hex_lattice::DeviceResources,
                        ),
                    },
                )
                .map_err(|_| Invalid("track instrument state is unsupported"))?;
            let mut mutable_continuation = false;
            if prepared.prefers_mutable_continuation()
                || previous.is_some_and(|old| {
                    old.mutable_continuations
                        .contains(&(track.id(), device.id()))
                })
            {
                if prepared
                    .prepare_mutable_continuation_runtime()
                    .map_err(|_| Invalid("mutable continuation runtime is unsupported"))?
                    .is_some()
                {
                    mutable_continuations.push((track.id(), device.id()));
                    mutable_continuation = true;
                    if resources.is_none() {
                        continue;
                    }
                }
                if !mutable_continuation && prepared.prefers_mutable_continuation() {
                    return Err(Invalid("mutable continuation capability disappeared"));
                }
            }
            let context = if mutable_continuation {
                None
            } else {
                prepared
                    .prepare_shared_analytic_context()
                    .map_err(|_| Invalid("shared analytic instrument context is unsupported"))?
            };
            if context.is_none() && resources.is_none() {
                continue;
            }
            let identity = SharedContextIdentity {
                tuning: tuning_identity,
                resource_scope: resources
                    .as_ref()
                    .map_or(0, |lease| lease.source_scope_identity()),
                track: track.id(),
                device: device.id(),
                device_type: device.state().type_id(),
                state: device.state().clone().without_host_gains(),
                sample_rate: project.sample_rate(),
            };
            if previous
                .and_then(|table| table.owner_for_device(device.id()))
                .is_some_and(|owner| {
                    owner.compatible_identity(&identity)
                        && owner.analytic_context().is_some() == context.is_some()
                })
            {
                let owner = previous
                    .and_then(|table| table.owner_for_device(device.id()))
                    .ok_or(Invalid("shared instrument context reuse index is invalid"))?;
                payload_bytes = payload_bytes
                    .checked_add(owner.declared_bytes)
                    .filter(|bytes| *bytes <= MAX_SHARED_CONTEXT_BYTES_PER_REVISION)
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

            let declared_bytes = context
                .as_ref()
                .map_or(0, |context| context.declared_context_bytes());
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
            let owner = match context {
                Some(context) => SharedContextOwner::from_prepared_on_control(
                    id,
                    identity,
                    declared_bytes,
                    context,
                    resources,
                ),
                None if identity.device_type == hex_devices::sampler::state::TYPE_ID => {
                    SharedContextOwner::from_sampler_resources_on_control(
                        id,
                        identity,
                        resources.ok_or(Invalid("sampler resource scope is unavailable"))?,
                        &tuning,
                        resource_preparation
                            .ok_or(Invalid("sampler preparation budget is unavailable"))?
                            .max_scope_metadata_bytes,
                        MAX_SHARED_CONTEXT_BYTES_PER_REVISION
                            .checked_sub(payload_bytes)
                            .ok_or(Invalid("sampler context budget exceeded"))?,
                    )?
                }
                None => SharedContextOwner::from_inline_resources_on_control(
                    id,
                    identity,
                    resources.ok_or(Invalid("inline instrument resource scope is unavailable"))?,
                    resource_preparation
                        .ok_or(Invalid("inline resource preparation budget is unavailable"))?
                        .max_scope_metadata_bytes,
                )?,
            };
            if owner.sampler_factory.is_some() {
                payload_bytes = payload_bytes
                    .checked_add(owner.declared_bytes)
                    .filter(|bytes| *bytes <= MAX_SHARED_CONTEXT_BYTES_PER_REVISION)
                    .ok_or(Invalid("sampler context budget exceeded"))?;
            }
            owners.push(crate::owned_shared::OwnedShared::new_owned(owner));
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
        Ok(crate::owned_shared::OwnedShared::new_owned(Self {
            tuning,
            tuning_identity,
            owners,
            ids,
            payload_bytes,
            mutable_continuations,
        }))
    }

    pub(crate) fn tuning(&self) -> &hex_lattice::TuningTable {
        &self.tuning
    }
    pub(crate) fn tuning_identity(&self) -> TuningIdentity {
        self.tuning_identity
    }

    pub(crate) fn requires_mutable_continuation(&self, track: TrackId, device: DeviceId) -> bool {
        self.mutable_continuations.contains(&(track, device))
    }

    pub(crate) fn owners(&self) -> impl Iterator<Item = (u16, &SharedContextOwner)> {
        self.owners.iter().enumerate().filter_map(|(index, owner)| {
            u16::try_from(index)
                .ok()
                .map(|index| (index, owner.as_ref()))
        })
    }

    pub(crate) fn index_for_track(&self, track: TrackId) -> Option<u16> {
        self.owners
            .iter()
            .position(|owner| owner.identity.track == track)
            .and_then(|index| u16::try_from(index).ok())
    }

    pub(crate) fn owner_for_device(&self, device: DeviceId) -> Option<&SharedContextOwner> {
        self.owners
            .binary_search_by_key(&device, |owner| owner.identity.device)
            .ok()
            .and_then(|index| self.owners.get(index).map(|owner| owner.as_ref()))
    }

    pub(crate) fn owner(&self, index: u16) -> Option<&SharedContextOwner> {
        self.owners
            .get(usize::from(index))
            .map(|owner| owner.as_ref())
    }

    pub(crate) fn clone_owner(
        &self,
        index: u16,
    ) -> Option<crate::shared_analytic::SharedContextLease> {
        self.owners.get(usize::from(index)).cloned()
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
            .checked_mul(std::mem::size_of::<
                crate::shared_analytic::SharedContextLease,
            >())
            .ok_or(Invalid("shared instrument context owner size overflow"))?;
        let id_index = self
            .ids
            .capacity()
            .checked_mul(std::mem::size_of::<(SharedContextId, u16)>())
            .ok_or(Invalid("shared instrument context owner size overflow"))?;
        owner_handles
            .checked_add(id_index)
            .and_then(|bytes| {
                bytes.checked_add(
                    self.mutable_continuations
                        .capacity()
                        .checked_mul(std::mem::size_of::<(TrackId, DeviceId)>())?,
                )
            })
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

    fn empty_slots() -> Vec<Option<crate::shared_analytic::SharedContextLease>> {
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
    fn empty_audio_only_storage_refuses_spare_continuation_backing() {
        let mut table = SharedAnalyticContextTable::default();
        assert!(table.require_empty_owned_storage().is_ok());
        table.mutable_continuations.try_reserve_exact(1).unwrap();
        assert!(table.mutable_continuations.is_empty());
        assert_eq!(
            table.require_empty_owned_storage(),
            Err(crate::AudioStorageError::UnknownStorage)
        );
    }

    #[test]
    fn maximum_context_table_handle_and_index_layout_is_bounded() {
        let owner_handles = MAX_SHARED_CONTEXTS_PER_REVISION
            .checked_mul(std::mem::size_of::<
                crate::shared_analytic::SharedContextLease,
            >())
            .unwrap();
        let id_index_entries = MAX_SHARED_CONTEXTS_PER_REVISION
            .checked_mul(std::mem::size_of::<(SharedContextId, u16)>())
            .unwrap();
        let table_header = std::mem::size_of::<SharedAnalyticContextTable>();
        assert_eq!(owner_handles, 4_096);
        assert_eq!(id_index_entries, 4_096);
        // One table and scalar cache identity per revision, never per clip.
        assert_eq!(table_header, 80 + 1024 + 8);
        assert_eq!(owner_handles + id_index_entries + table_header, 9_304);
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
