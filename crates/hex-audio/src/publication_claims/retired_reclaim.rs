//! Retired installed predecessors only; no admission or final-active shortcut.
use super::*;
use hex_engine::storage_report::{
    AnchoredPlaybackClaims, OwnerGroupArchive, OwnerGroupsDisposed, OwnerTransferManifest,
};

/// Private receipt custody, independent of public report extraction. Inline N=2
/// storage is a domain reservation, not a dependent-claim or memory limit.
pub(crate) struct CleanupSuccessor {
    pub(crate) domain: hex_stream::DomainId,
    pub(crate) receipt: Option<hex_stream::CatalogCustodyReceipt>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DependencyPhase {
    Registered,
    Covered,
    ReclaimPending,
    Failed,
}
pub(super) struct Dependency {
    stamp: ClaimStamp,
    runtime: ClaimIdentity,
    domain: hex_stream::DomainId,
    cleanup_cell: usize,
    binding: hex_stream::CatalogCoverageBinding,
    pending: Option<hex_stream::CatalogCoveragePending>,
    failed_ack: Option<hex_stream::CatalogCoverageAck>,
    covered: Option<hex_stream::CatalogCoverageCovered>,
    phase: DependencyPhase,
}
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Destruction {
    #[default]
    Retained,
    Pending,
    Complete,
    Failed,
}
/// Separate persistent fields: no aggregate owning temporary is unwound. A
/// failed take remains irreversible; other owners and the first payload stay.
#[derive(Default)]
pub(super) struct ReclaimCapsule {
    claims: Option<AnchoredPlaybackClaims>,
    prepared: Option<hex_engine::OwnedPreparedRevision>,
    archive: Option<OwnerGroupArchive>,
    proof: Option<OwnerGroupsDisposed>,
    manifest: Option<OwnerTransferManifest>,
    stream: Option<crate::prepared::StreamBinding>,
    cancellation: Option<std::sync::Arc<crate::prepared::FreshRuntimeCancellation>>,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    destruction: Destruction,
    #[cfg(test)]
    probe: Option<Box<DisposalProbe>>,
}
impl ReclaimCapsule {
    fn dispose(&mut self) -> Result<(), ClaimSlotError> {
        match self.destruction {
            Destruction::Complete => return Ok(()),
            Destruction::Pending | Destruction::Failed => return Err(ClaimSlotError::Incomplete),
            Destruction::Retained => {}
        }
        if self.first_panic.is_some() {
            return Err(ClaimSlotError::Incomplete);
        }
        self.destruction = Destruction::Pending;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            drop(self.probe.take());
            drop(self.stream.take());
            drop(self.cancellation.take());
            drop(self.manifest.take());
            drop(self.proof.take());
            drop(self.archive.take());
            drop(self.claims.take());
            drop(self.prepared.take());
        }));
        match result {
            Ok(()) => {
                self.destruction = Destruction::Complete;
                Ok(())
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.destruction = Destruction::Failed;
                Err(ClaimSlotError::Incomplete)
            }
        }
    }
}
fn preflight_retired(
    s: &Slot,
    archive: &OwnerGroupArchive,
    a: &ClaimAssociation,
) -> Result<(), ClaimSlotError> {
    let identity = s.identity.ok_or(ClaimSlotError::PairMismatch)?;
    let claims = s
        .anchors
        .as_ref()
        .map(|a| &a.claims)
        .or(s.reclaim.claims.as_ref())
        .ok_or(ClaimSlotError::Incomplete)?;
    if s.phase != Phase::Accepted
        || s.disposal != Disposal::Complete
        || !s.facts.custody_returned
        || (s.facts.quiescence_required && !s.facts.quiescent)
        || s.facts.incomplete
        || s.returned.is_some()
        || s.reclaim.destruction != Destruction::Retained
    {
        return Err(ClaimSlotError::RetirementPending);
    }
    // Never erase Unknown or upgrade a fixture merely because its real Box died.
    if claims.is_incomplete_fixture()
        || claims.report().graph_data_bytes.is_none()
        || claims.report().prepared_container_bytes.is_none()
        || archive.report().incomplete_source
    {
        return Err(ClaimSlotError::Incomplete);
    }
    if claims.identity() != identity
        || s.tracked_disposed
            .as_ref()
            .is_none_or(|p| p.identity() != identity || p.is_incomplete_fixture())
        || s.installation
            .as_ref()
            .is_none_or(|p| !p.matches_runtime(a, identity))
        || s.final_transfer.as_ref().is_none_or(|(fact, manifest)| {
            manifest.predecessor() != identity
                || manifest.is_incomplete_fixture()
                || !fact.matches(a, manifest)
        })
        || !archive.original_runtime_was_disposed()
        || archive.remaining_handle_count() != 0
    {
        return Err(ClaimSlotError::RetirementPending);
    }
    let paired = s
        .anchors
        .as_ref()
        .map(|a| a.verified.matches_owner_archive(archive))
        .or_else(|| {
            s.reclaim
                .prepared
                .as_ref()
                .map(|root| archive.same_prepared_owner(root))
        })
        .unwrap_or(false);
    if !paired {
        return Err(ClaimSlotError::PairMismatch);
    }
    if s.reclaim.archive.is_some()
        || s.reclaim.proof.is_some()
        || s.reclaim.manifest.is_some()
        || s.reclaim.stream.is_some()
        || s.reclaim.cancellation.is_some()
    {
        return Err(ClaimSlotError::Busy);
    }
    Ok(())
}
impl ClaimUnionSlots {
    /// Borrowed control transaction, BEFORE lease/path moves or cleanup. Both
    /// caller-reserved row and inline output custody must already be available.
    fn register_retired_dependency(
        &mut self,
        a: &ClaimAssociation,
        reaper: &mut hex_stream::MediaDomainReaper,
        successors: &mut [Option<CleanupSuccessor>; crate::prepared::RETIRED],
    ) -> Result<(), ClaimSlotError> {
        let s = self.slot(a)?;
        let archive = self.archives[a.slot]
            .as_ref()
            .ok_or(ClaimSlotError::Incomplete)?;
        preflight_retired(s, archive, a)?;
        if self.dependencies[a.slot].is_some() || s.catalog_reservation.is_some() {
            return Err(ClaimSlotError::Busy);
        }
        let domain = s.coverage_domain.ok_or(ClaimSlotError::Incomplete)?;
        let identity = s.identity.ok_or(ClaimSlotError::PairMismatch)?;
        if s.coverage_reservation.is_none() {
            return Err(ClaimSlotError::Incomplete);
        }
        let cell = successors
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.domain == domain))
            .or_else(|| successors.iter().position(Option::is_none))
            .ok_or(ClaimSlotError::Capacity)?;
        if successors[cell]
            .as_ref()
            .is_some_and(|s| s.receipt.is_some())
        {
            return Err(ClaimSlotError::Busy);
        }
        // All outputs are reserved. Commit's API-reported refusal restores the
        // exact original verifier, claims and reservation; nothing is discarded.
        let s = &mut self.slots[a.slot];
        let Some((reservation, binding)) = s.coverage_reservation.take() else {
            return Err(ClaimSlotError::Incomplete);
        };
        let Some(anchors) = s.anchors.take() else {
            s.coverage_reservation = Some((reservation, binding));
            return Err(ClaimSlotError::Incomplete);
        };
        let created = successors[cell].is_none();
        if created {
            successors[cell] = Some(CleanupSuccessor {
                domain,
                receipt: None,
            });
        }
        // Durable exact dependency exists BEFORE the coverage source moves.
        // Exclusive reaper/root custody excludes cleanup/reuse through commit.
        let dependency = self.dependencies[a.slot].insert(Dependency {
            stamp: a.stamp(),
            runtime: identity,
            domain,
            cleanup_cell: cell,
            binding,
            pending: None,
            failed_ack: None,
            covered: None,
            phase: DependencyPhase::Registered,
        });
        match reaper.commit_verified_catalog_coverage(reservation, anchors.verified) {
            Ok((prepared, pending)) => {
                s.reclaim.claims = Some(anchors.claims);
                s.reclaim.prepared = Some(prepared);
                dependency.pending = Some(pending);
                Ok(())
            }
            Err(refusal) => {
                let hex_stream::VerifiedCoverageRefusal {
                    reservation,
                    verified,
                    ..
                } = *refusal;
                s.anchors = Some(PublicationAnchors {
                    claims: anchors.claims,
                    verified,
                });
                if let Some(dependency) = self.dependencies[a.slot].take() {
                    s.coverage_reservation = Some((reservation, dependency.binding));
                }
                if created {
                    successors[cell] = None;
                }
                Err(ClaimSlotError::Incomplete)
            }
        }
    }
    pub(crate) fn has_cleanup_dependents(&self, domain: hex_stream::DomainId) -> bool {
        self.dependencies
            .iter()
            .flatten()
            .any(|d| d.domain == domain)
    }
    /// Called only inside existing exclusive extracted cleanup custody, outside
    /// ordinary root lock. No callback accesses these rows or invokes providers.
    pub(crate) fn advance_retired_claims_on_control(
        &mut self,
        reaper: &mut hex_stream::MediaDomainReaper,
        successors: &mut [Option<CleanupSuccessor>; crate::prepared::RETIRED],
    ) {
        for index in 0..self.slots.len() {
            let a = ClaimAssociation {
                root: self.root,
                slot: index,
                incarnation: self.slots[index].incarnation,
            };
            if self.dependencies[index].is_none() {
                let _ = self.register_retired_dependency(&a, reaper, successors);
            }
            let Some(d) = self.dependencies[index].as_mut() else {
                continue;
            };
            if d.phase == DependencyPhase::Failed || d.failed_ack.is_some() {
                continue;
            }
            if d.phase == DependencyPhase::Registered {
                let _ = reaper.advance_catalog_coverage_on_control(d.domain);
                let Some(pending) = d.pending.take() else {
                    d.phase = DependencyPhase::Failed;
                    continue;
                };
                match reaper.catalog_coverage_ack(pending) {
                    Ok(ack) => match reaper.consume_catalog_coverage_ack(ack) {
                        Ok(covered) => {
                            d.covered = Some(covered);
                            d.phase = DependencyPhase::Covered;
                        }
                        Err((_, ack)) => {
                            // Preserve actual authority on refusal; no decrement
                            // or reuse. A private persistent carrier is required.
                            d.phase = DependencyPhase::Failed;
                            d.failed_ack = Some(ack);
                        }
                    },
                    Err((_, pending)) => {
                        d.pending = Some(pending);
                        continue;
                    }
                }
            }
            if d.phase == DependencyPhase::Covered {
                if d.stamp != a.stamp()
                    || self.slots[index].identity != Some(d.runtime)
                    || self.archives[index].as_ref().is_none_or(|archive| {
                        preflight_retired(&self.slots[index], archive, &a).is_err()
                    })
                {
                    continue;
                }
                let Some(covered) = d.covered.as_ref() else {
                    continue;
                };
                // Current authority remains borrowed THROUGH the fixed owner
                // moves. No destructor/report consumption occurs in this scope.
                let mut authorized = false;
                {
                    if let Ok(permit) =
                        reaper.validate_current_catalog_coverage(&d.binding, covered)
                        && permit.matches_binding(&d.binding)
                        && self.archives[index].as_ref().is_some_and(|archive| {
                            preflight_retired(&self.slots[index], archive, &a).is_ok()
                        })
                    {
                        stage_capsule(&mut self.slots[index], &mut self.archives[index]);
                        authorized = true;
                    }
                }
                if !authorized {
                    if let Some(receipt) = successors[d.cleanup_cell]
                        .as_ref()
                        .and_then(|s| s.receipt.as_ref())
                    {
                        if let Ok(permit) =
                            reaper.validate_current_catalog_cleanup(&d.binding, receipt)
                            && permit.matches_binding(&d.binding)
                            && self.archives[index].as_ref().is_some_and(|archive| {
                                preflight_retired(&self.slots[index], archive, &a).is_ok()
                            })
                        {
                            stage_capsule(&mut self.slots[index], &mut self.archives[index]);
                            authorized = true;
                        }
                    }
                }
                if !authorized {
                    continue;
                }
                d.phase = DependencyPhase::ReclaimPending;
            }
            if d.phase == DependencyPhase::ReclaimPending {
                // Reaper permit and ordinary root lock are both absent here.
                if self.slots[index].reclaim.dispose().is_err() {
                    self.slots[index].facts.incomplete = true;
                    d.phase = DependencyPhase::Failed;
                    continue;
                }
                // Only actual normal destruction clears this exact dependency.
                // Ancestors' charge/role facts are never decremented by ACK.
                let mut vacant = Slot::vacant();
                vacant.incarnation = self.slots[index].incarnation;
                self.slots[index] = vacant;
                self.dependencies[index] = None;
            }
        }
    }
}
fn stage_capsule(s: &mut Slot, archive: &mut Option<OwnerGroupArchive>) {
    // Every predicate/destination was preflighted while authority is borrowed.
    s.reclaim.archive = archive.take();
    s.reclaim.proof = s.tracked_disposed.take();
    s.reclaim.manifest = s.final_transfer.take().map(|(_, manifest)| manifest);
    s.reclaim.stream = s.returned_stream.take();
    s.reclaim.cancellation = s.returned_cancellation.take();
}

#[cfg(test)]
struct DisposalProbe(Box<dyn FnMut() + Send>);
#[cfg(test)]
impl Drop for DisposalProbe {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    fn root() -> hex_engine::OwnedPreparedRevision {
        hex_engine::PreparedRevision::prepare(
            &hex_project::Project::new(48_000).unwrap(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared()
    }
    #[test]
    fn real_capsule_destructor_retains_other_owners_on_panic_and_never_retries() {
        for panic in [false, true] {
            let revision = root();
            let (owner, claims) = hex_engine::storage_report::incomplete_owned_pair_for_test(
                revision.clone_on_control(),
            )
            .ok()
            .unwrap();
            drop(owner); // fixture owner never admitted; no role/proof asserted
            let count = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&count);
            let threads = Arc::new(Mutex::new(Vec::new()));
            let observed_threads = Arc::clone(&threads);
            let mut capsule = ReclaimCapsule {
                claims: Some(claims),
                prepared: Some(revision),
                probe: Some(Box::new(DisposalProbe(Box::new(move || {
                    observed.fetch_add(1, Ordering::Relaxed);
                    observed_threads
                        .lock()
                        .unwrap()
                        .push(std::thread::current().id());
                    assert!(!panic, "actual capsule probe destructor failure");
                })))),
                ..ReclaimCapsule::default()
            };
            assert_eq!(capsule.dispose().is_err(), panic);
            assert_eq!(count.load(Ordering::Relaxed), 1);
            assert_eq!(*threads.lock().unwrap(), [std::thread::current().id()]);
            if panic {
                assert!(capsule.first_panic.is_some());
                assert!(capsule.claims.as_ref().unwrap().is_incomplete_fixture());
                assert!(capsule.prepared.is_some());
                assert!(matches!(capsule.dispose(), Err(ClaimSlotError::Incomplete)));
            } else {
                assert!(capsule.claims.is_none() && capsule.prepared.is_none());
                assert!(capsule.dispose().is_ok());
            }
            assert_eq!(count.load(Ordering::Relaxed), 1);
        }
    }
    #[test]
    fn never_installed_and_incomplete_owners_refuse_without_dependency_or_owner_moves() {
        let root = root();
        let (mut owner, claims) =
            hex_engine::storage_report::incomplete_owned_pair_for_test(root.clone_on_control())
                .ok()
                .unwrap();
        owner.prepare_owner_groups_for_test().unwrap();
        let reservation = ClaimReservation {
            items: 32_768,
            anchors: 16,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        };
        let (owner, archive, _) = owner
            .try_acquire_owner_groups_on_control(&claims, reservation)
            .ok()
            .unwrap();
        let mut runtime =
            hex_stream::StreamRuntime::new(hex_stream::StreamConfig::with_pool_pages(4), vec![])
                .unwrap();
        runtime
            .wait_until_ready(std::time::Duration::from_secs(2))
            .unwrap();
        let (control, pages) = runtime.split().ok().unwrap();
        let verified =
            hex_stream::VerifiedOwnedStreamRevision::new(root, control.catalog_lease().unwrap())
                .unwrap();
        let mut slots = ClaimUnionSlots::new(reservation).unwrap();
        let a = slots.install_tracked_incomplete_for_test(
            &owner,
            archive,
            PublicationAnchors { claims, verified },
        );
        let (_, _, mut reaper) = hex_stream::reusable_domain_escrow(2).unwrap();
        let mut successors = std::array::from_fn(|_| None);
        assert!(
            slots
                .register_retired_dependency(&a, &mut reaper, &mut successors)
                .is_err()
        );
        assert!(slots.dependencies.iter().all(Option::is_none));
        assert!(successors.iter().all(Option::is_none));
        assert!(
            slots
                .slot(&a)
                .unwrap()
                .anchors
                .as_ref()
                .unwrap()
                .matches_tracked_owner(&owner)
        );
        assert!(slots.slot(&a).unwrap().reclaim.claims.is_none());
        let refusal = control.shutdown(pages).err().unwrap();
        assert!(matches!(refusal, hex_stream::ShutdownError::Busy(_)));
        let (control, pages) = refusal.into_parts().unwrap();
        drop(slots);
        drop(owner);
        let report = control.shutdown(pages).unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        // Actual fixture cleanup only; none of these assertions supplies install,
        // transfer, catalog cleanup or complete admission authority.
    }
}
