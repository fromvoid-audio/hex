use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_lattice::{
    DeviceMeter, DevicePreparationContext, NativeProcessorRuntime, NativeRuntimeChain,
    PreparedNativeProcessor, PreparedProcessorFactory, PreparedRuntimeInstance,
    PreparedScopedRuntimeInstance, ProcessorChainOwner, StereoFrame, StereoMatrix, TuningTable,
    conformance::resource_pcm::{BorrowedPcmEffect, prepare_effect},
};
use std::sync::Arc;

struct PreparedPcm(BorrowedPcmEffect);
impl PreparedNativeProcessor for PreparedPcm {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<PcmRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(PcmRuntime(self.0.clone()))
    }
}
struct PcmRuntime(BorrowedPcmEffect);
impl NativeProcessorRuntime for PcmRuntime {
    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        self.0.process_block(frames, resources);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn is_quiescent(&self) -> bool {
        self.0.is_quiescent()
    }
    // Deliberately permissive: source-scope protection must come from the host,
    // rather than being accidentally supplied by this synthetic provider.
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        self.0
            .copy_history_from(&previous.as_any().downcast_ref::<Self>().unwrap().0);
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
struct PreparedOwnedPcm(BorrowedPcmEffect);
struct OwnedPcm {
    committed: BorrowedPcmEffect,
    speculative: BorrowedPcmEffect,
}
impl hex_lattice::PreparedOwnedProcessorRuntime for PreparedOwnedPcm {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<OwnedPcm>()
    }
    fn create_runtime(
        &self,
    ) -> Result<
        Box<dyn hex_lattice::OwnedProcessorRuntime>,
        hex_lattice_contract::DeviceContractError,
    > {
        Ok(Box::new(OwnedPcm {
            committed: self.0.clone(),
            speculative: self.0.clone(),
        }))
    }
    fn output_tail(&self) -> hex_lattice::OutputTail {
        hex_lattice::OutputTail::Finite { max_tail_frames: 4 }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}
impl hex_lattice::OwnedProcessorRuntime for OwnedPcm {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }
    fn begin_block(&mut self) {
        self.speculative = self.committed.clone();
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        self.speculative.process_block(frames, resources);
    }
    fn commit_block(&mut self) {
        self.committed = self.speculative.clone();
    }
    fn rollback_block(&mut self) {
        self.speculative = self.committed.clone();
    }
    fn reset(&mut self) {
        self.committed.reset();
        self.speculative.reset();
    }
    fn committed_is_quiescent(&self) -> bool {
        self.committed.is_quiescent()
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.speculative.is_quiescent()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
pub(super) fn owner() -> Option<(DeviceResourceBank, Vec<AssetId>)> {
    let ids = tests::ids();
    let bank = tests::bank(&ids)?;
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &tests::specs(&ids),
        tests::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    Some((
        DeviceResourceBank::try_new_on_control(
            bank,
            catalog,
            65_536,
            65_536,
            &AtomicBool::new(false),
        )
        .ok()
        .unwrap(),
        ids,
    ))
}
pub(super) fn scoped(
    owner: &DeviceResourceBank,
    asset: AssetId,
    bypassed: bool,
) -> PreparedScopedRuntimeInstance<DeviceResourceConfiguration> {
    let state = tests::state(asset);
    let resources = owner
        .prepare_configuration_on_control(&state, 4096)
        .unwrap();
    let processor = prepare_effect(
        &state,
        &DevicePreparationContext {
            sample_rate: 44_100,
            tuning: &TuningTable::default(),
            resources: &resources,
        },
        0,
    )
    .unwrap();
    PreparedScopedRuntimeInstance {
        resources,
        instance: PreparedRuntimeInstance {
            meter: Arc::new(DeviceMeter::default()),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 44_100,
            configuration: Some(Arc::new(state)),
            definition: Some(PreparedProcessorFactory::Legacy(Arc::new(PreparedPcm(
                processor,
            )))),
        },
    }
}
fn process(
    chain: &mut NativeRuntimeChain<DeviceResourceConfiguration>,
    count: usize,
) -> Vec<StereoFrame> {
    let mut frames = vec![[1.0; 2]; count];
    chain.process_block(&mut frames);
    frames
}
#[test]
fn actual_effect_entry_retains_source_and_rollback_partition_bypass_history() {
    let Some((owner, ids)) = owner() else { return };
    let mut chain = NativeRuntimeChain::new_prepared(vec![scoped(&owner, ids[0], false)]).unwrap();
    let mut bypassed =
        NativeRuntimeChain::new_prepared(vec![scoped(&owner, ids[0], true)]).unwrap();
    drop(owner); // Actual chain owners alone retain the original PCM.
    chain.begin_block();
    assert_eq!(process(&mut chain, 2), tests::PCM[..2]);
    chain.rollback_block();
    chain.begin_block();
    assert_eq!(process(&mut chain, 1), tests::PCM[..1]);
    assert_eq!(process(&mut chain, 3), tests::PCM[1..]);
    chain.commit_block();
    chain.begin_block();
    assert_eq!(process(&mut chain, 1), [[0.0; 2]]);
    chain.rollback_block();
    bypassed.begin_block();
    assert_eq!(process(&mut bypassed, 2), [[1.0; 2]; 2]);
    bypassed.commit_block();
    // Bypass still advanced history, proven by inheritance into an active entry
    // prepared from the exact retained configuration scope.
    let resources = bypassed
        .storage_view()
        .entries()
        .next()
        .unwrap()
        .host_resources()
        .try_clone_on_control(4096)
        .unwrap();
    let state = tests::state(ids[0]);
    let processor = prepare_effect(
        &state,
        &DevicePreparationContext {
            sample_rate: 44_100,
            tuning: &TuningTable::default(),
            resources: &resources,
        },
        0,
    )
    .unwrap();
    let mut active = NativeRuntimeChain::new_prepared(vec![PreparedScopedRuntimeInstance {
        resources,
        instance: PreparedRuntimeInstance {
            meter: Arc::new(DeviceMeter::default()),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: state.type_id(),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 44_100,
            configuration: Some(Arc::new(state)),
            definition: Some(PreparedProcessorFactory::Legacy(Arc::new(PreparedPcm(
                processor,
            )))),
        },
    }])
    .unwrap();
    assert!(active.can_inherit_from(&bypassed));
    active.inherit_from(&bypassed);
    active.begin_block();
    assert_eq!(process(&mut active, 2), tests::PCM[2..]);
    active.rollback_block();
}
#[test]
fn effect_host_refuses_cross_scope_history_and_preserves_old_tail() {
    let Some((owner, ids)) = owner() else { return };
    let mut old = NativeRuntimeChain::new_prepared(vec![scoped(&owner, ids[0], false)]).unwrap();
    let mut candidate =
        NativeRuntimeChain::new_prepared(vec![scoped(&owner, ids[1], false)]).unwrap();
    let Some((independent_owner, independent_ids)) = self::owner() else {
        panic!("known native layout")
    };
    assert_eq!(independent_ids[0], ids[0]);
    let independent = NativeRuntimeChain::new_prepared(vec![scoped(
        &independent_owner,
        independent_ids[0],
        false,
    )])
    .unwrap();
    // Exactly equal authored state and provider keys, different actual custody.
    assert!(!independent.configuration_matches_range(&old, 0..1));
    assert!(!independent.can_retarget_parameters_from_range(&old, 0..1));
    assert!(!independent.can_inherit_from(&old));

    old.begin_block();
    assert_eq!(process(&mut old, 1), tests::PCM[..1]);
    old.commit_block();
    assert!(!candidate.can_inherit_from(&old));
    assert!(!candidate.configuration_matches_range(&old, 0..1));
    assert!(!candidate.can_retarget_parameters_from_range(&old, 0..1));
    assert!(candidate.can_inherit_from_excluding_old_range(&old, 0..1));
    candidate.inherit_from(&old); // Defensive guard also protects unchecked API.
    candidate.begin_block();
    assert_eq!(process(&mut candidate, 2), [[0.5; 2]; 2]);
    candidate.rollback_block();
    // Ownership moves through the same whole-chain tail transfer used by hosts.
    let mut tail = std::mem::replace(&mut old, NativeRuntimeChain::empty());
    drop(owner);
    tail.begin_block();
    assert_eq!(process(&mut tail, 2), tests::PCM[1..3]);
    tail.commit_block();
    assert!(old.is_empty());
}
#[test]
fn immutable_configuration_inventory_requires_every_actual_alias_and_exact_bytes() {
    let Some((owner, ids)) = owner() else { return };
    let config = owner
        .prepare_configuration_on_control(&tests::state(ids[0]), 4096)
        .unwrap();
    let bytes = config.declared_metadata_bytes().unwrap();
    assert!(
        owner
            .prepare_configuration_on_control(&tests::state(ids[0]), bytes)
            .is_ok()
    );
    assert!(matches!(
        owner.prepare_configuration_on_control(&tests::state(ids[0]), bytes - 1),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    let alias_bytes = std::mem::size_of::<DeviceResourceConfiguration>();
    assert!(matches!(
        config.try_clone_on_control(alias_bytes - 1),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    let alias = config.try_clone_on_control(alias_bytes).unwrap();
    let reservation = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 32,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&owner, AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_device_resource_configuration_on_control(&config, AudioStorageRole::Current)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .borrow_device_resource_configuration_on_control(&alias, AudioStorageRole::Predecessor)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_bytes - before.data_bytes, alias_bytes);
    assert_eq!(after.data_allocations, before.data_allocations); // only alias header, no new allocation
    assert_eq!(
        config.get(0).unwrap().pcm.frame(0),
        alias.get(0).unwrap().pcm.frame(0)
    );
    drop(ledger);
    drop(config);
    drop(owner);
    assert_eq!(alias.get(0).unwrap().pcm.frame(3), Some(tests::PCM[3]));
}

#[test]
fn owned_transaction_effect_uses_entry_scope_through_rollback_and_dry_bypass() {
    let Some((owner, ids)) = owner() else { return };
    for bypassed in [false, true] {
        let mut prepared = scoped(&owner, ids[0], bypassed);
        let processor = prepare_effect(
            prepared.instance.configuration.as_ref().unwrap(),
            &DevicePreparationContext {
                sample_rate: 44_100,
                tuning: &TuningTable::default(),
                resources: &prepared.resources,
            },
            0,
        )
        .unwrap();
        prepared.instance.definition = Some(PreparedProcessorFactory::Owned(Arc::new(
            PreparedOwnedPcm(processor),
        )));
        let mut chain = NativeRuntimeChain::new_prepared(vec![prepared]).unwrap();
        chain.begin_block();
        assert_eq!(
            process(&mut chain, 2),
            if bypassed {
                vec![[1.0; 2]; 2]
            } else {
                tests::PCM[..2].to_vec()
            }
        );
        chain.rollback_block();
        chain.begin_block();
        assert_eq!(
            process(&mut chain, 4),
            if bypassed {
                vec![[1.0; 2]; 4]
            } else {
                tests::PCM.to_vec()
            }
        );
        chain.commit_block();
        assert!(chain.committed_range_is_quiescent(0..1)); // bypass advanced the same real cursor
    }
}

#[test]
fn actual_effect_scopes_close_current_candidate_tail_and_history_union_at_exact_limit() {
    let Some((owner, ids)) = owner() else { return };
    let current = scoped(&owner, ids[0], false);
    let current_anchor = current.resources.try_clone_on_control(4096).unwrap();
    let next = scoped(&owner, ids[1], false);
    let history = next.resources.try_clone_on_control(4096).unwrap();
    let pending_reclaim = next.resources.try_clone_on_control(4096).unwrap();
    let mut current = NativeRuntimeChain::new_prepared(vec![current]).unwrap();
    let candidate = NativeRuntimeChain::new_prepared(vec![next]).unwrap();
    let tail = std::mem::replace(&mut current, NativeRuntimeChain::empty());
    // Every visited reference is to the ACTUAL retained field. No synthetic
    // row/alias count substitutes an owner. These runtimes are on control.
    let collect = |limits: AudioStorageLedgerReservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
        ledger.borrow_device_resource_bank_on_control(&owner, AudioStorageRole::Caller)?;
        ledger.borrow_device_resource_configuration_on_control(
            &current_anchor,
            AudioStorageRole::Current,
        )?;
        ledger
            .borrow_device_resource_configuration_on_control(&history, AudioStorageRole::Archive)?;
        ledger.borrow_device_resource_configuration_on_control(
            &pending_reclaim,
            AudioStorageRole::PendingReclaim,
        )?;
        for (chain, role) in [
            (&candidate, AudioStorageRole::Current),
            (&tail, AudioStorageRole::Predecessor),
        ] {
            for entry in chain.storage_view().entries() {
                ledger.borrow_device_resource_configuration_on_control(
                    entry.host_resources(),
                    role,
                )?;
            }
        }
        ledger.validate_closed_shared_handles()?;
        ledger.summary()
    };
    let mut limits = AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let summary = collect(limits).unwrap();
    summary.require_known_storage().unwrap();
    // Retirement is deliberately still unqualified by a borrowed ledger.
    assert!(summary.require_complete().is_err());
    limits.max_retained_bytes = summary.retained_bytes;
    limits.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(limits).unwrap(), summary);
    limits.max_retained_bytes -= 1;
    assert!(collect(limits).is_err());
    limits.max_retained_bytes += 1;
    limits.max_construction_peak_bytes -= 1;
    assert!(collect(limits).is_err());
    // Saturate actual handle rows without mutating or releasing any live role.
    limits.max_construction_peak_bytes += 1;
    limits.handle_rows = 1;
    assert!(collect(limits).is_err());
    assert_eq!(
        tail.storage_view()
            .entries()
            .next()
            .unwrap()
            .host_resources()
            .get(0)
            .unwrap()
            .pcm
            .frame(0),
        Some(tests::PCM[0])
    );
    assert_eq!(
        candidate
            .storage_view()
            .entries()
            .next()
            .unwrap()
            .host_resources()
            .get(0)
            .unwrap()
            .pcm
            .frame(0),
        Some([0.5; 2])
    );
}

struct MustNotConstruct;
impl PreparedNativeProcessor for MustNotConstruct {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        1
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        panic!("invalid resource configuration reached runtime factory")
    }
}
#[test]
fn closed_scope_checks_actual_slots_and_rate_before_runtime_construction() {
    let Some((owner, ids)) = owner() else { return };
    for invalid in 0..3 {
        let mut prepared = scoped(&owner, ids[0], false);
        prepared.instance.definition =
            Some(PreparedProcessorFactory::Legacy(Arc::new(MustNotConstruct)));
        match invalid {
            0 => prepared.instance.configuration = Some(Arc::new(tests::state(ids[1]))),
            1 => prepared.instance.configuration = None,
            _ => prepared.instance.sample_rate = 0,
        }
        let expected = if invalid == 2 {
            hex_lattice_contract::DeviceContractError::InvalidParameterValue
        } else {
            hex_lattice_contract::DeviceContractError::AssetResourcesUnavailable
        };
        assert!(
            matches!(NativeRuntimeChain::new_prepared(vec![prepared]), Err(error) if error == expected)
        );
    }
    let resources = owner
        .prepare_configuration_on_control(&tests::state(ids[0]), 4096)
        .unwrap();
    // Original rate remains independent of project/output rate.
    hex_lattice::HostDeviceResourceScope::validate_configuration_on_control(
        &resources,
        &tests::state(ids[0]),
        48_000,
    )
    .unwrap();
    assert_eq!(resources.get(0).unwrap().sample_rate, 44_100);
}

#[test]
fn refused_configuration_anchor_returns_the_identical_lease_for_exact_budget_retry() {
    let Some((owner, ids)) = owner() else { return };
    let state = tests::state(ids[0]);
    let lease = owner.prepare_lease_on_control(&state, 4096).unwrap();
    let before = lease.get(0).unwrap();
    let identity = before.identity;
    let pointer = match before.pcm {
        DeviceResourcePcm::Mono(samples) => samples.as_ptr() as usize,
        DeviceResourcePcm::Stereo(frames) => frames.as_ptr() as usize,
    };
    let (_, overhead) =
        crate::owned_shared::OwnedShared::<PreparedDeviceResourceLease>::declared_parts().unwrap();
    let exact = lease.declared_metadata_bytes().unwrap()
        + overhead
        + std::mem::size_of::<DeviceResourceConfiguration>();
    let refused = lease
        .try_into_configuration_on_control(exact - 1)
        .err()
        .unwrap();
    assert_eq!(refused.reason, ResourcePreparationError::MetadataCapacity);
    let view = refused.lease.get(0).unwrap();
    assert_eq!(view.identity, identity);
    let returned_pointer = match view.pcm {
        DeviceResourcePcm::Mono(samples) => samples.as_ptr() as usize,
        DeviceResourcePcm::Stereo(frames) => frames.as_ptr() as usize,
    };
    assert_eq!(pointer, returned_pointer);
    let config = refused
        .lease
        .try_into_configuration_on_control(exact)
        .ok()
        .unwrap();
    assert_eq!(config.declared_metadata_bytes().unwrap(), exact);
    drop(owner);
    assert_eq!(config.get(0).unwrap().pcm.frame(0), Some([0.25, -0.75]));
}
