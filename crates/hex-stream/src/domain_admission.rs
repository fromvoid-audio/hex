//! Control-owned admission into preallocated reusable escrow slots.
use super::*;

/// Preallocated reusable slots and return capacity. This caller reservation is
/// a simultaneous-domain refusal policy, not a total-memory/project/track cap.
/// Legacy `domain_escrow` remains fixed-input and retains its capacity rules.
pub fn reusable_domain_escrow(
    capacity: usize,
) -> Result<
    (
        Box<CallbackDomainBatch>,
        TerminalBatchSender,
        MediaDomainReaper,
    ),
    Box<DomainSetupFailure>,
> {
    build(Vec::new(), capacity, capacity, true, &mut || Ok(()))
}

/// Moved with the graph in its sole publication envelope. No Clone. A failed
/// callback install returns this payload unchanged; retain/retry off control.
pub struct DomainAdmission {
    index: usize,
    id: DomainId,
    endpoint: Box<PageReadiness>,
    seal: Arc<()>,
}
impl DomainAdmission {
    pub fn id(&self) -> DomainId {
        self.id
    }
    pub fn catalog(&self) -> &CatalogLease {
        self.endpoint.catalog()
    }
}

/// Control-only synchronous refusal retains every original input owner.
pub struct DomainAdmissionFailure {
    pub reason: DomainRefusal,
    pub input: MediaDomainInput,
}

impl CallbackDomainBatch {
    pub fn install_domain(
        &mut self,
        admission: DomainAdmission,
    ) -> Result<DomainId, DomainAdmission> {
        let valid = Arc::ptr_eq(&self.seal, &admission.seal)
            && self
                .slots
                .get(admission.index)
                .is_some_and(|slot| slot.vacant && slot.endpoint.is_none());
        if !valid {
            return Err(admission);
        }
        let slot = &mut self.slots[admission.index];
        slot.id = admission.id;
        slot.endpoint = Some(admission.endpoint);
        slot.vacant = false;
        // The admission seal is an additional reference created on control; it
        // must travel back for control reclamation, never be dropped here.
        self.slots[admission.index].admission_seal = Some(admission.seal);
        Ok(admission.id)
    }
}

impl MediaDomainReaper {
    /// Control-only lease creation for a serialized publisher. Retired or
    /// failed slots cannot manufacture a new graph lease.
    pub fn catalog_lease(&self, id: DomainId) -> Option<CatalogLease> {
        self.slots
            .iter()
            .find(|slot| {
                !slot.vacant
                    && slot.id == id
                    && slot.endpoint.is_none()
                    && slot.report.is_none()
                    && slot.terminal_error.is_none()
            })?
            .control
            .as_ref()?
            .catalog_lease()
            .ok()
    }
    /// Control recovery for an accepted envelope that never installed on audio.
    /// Keeps its original pool/pair available for reported shutdown, not Drop.
    pub fn recover_uninstalled(
        &mut self,
        admission: DomainAdmission,
    ) -> Result<(), DomainAdmission> {
        if !Arc::ptr_eq(&self.seal, &admission.seal)
            || !self.slots.get(admission.index).is_some_and(|slot| {
                !slot.vacant
                    && slot.id == admission.id
                    && slot.endpoint.is_none()
                    && slot.report.is_none()
                    && slot
                        .control
                        .as_ref()
                        .is_some_and(|control| control.matches_catalog(admission.catalog()))
            })
        {
            return Err(admission);
        }
        self.slots[admission.index].endpoint = Some(admission.endpoint);
        Ok(())
    }

    /// Borrow actual occupied identities into caller-owned storage. Control only.
    pub fn visit_domains(&self, mut visit: impl FnMut(usize, DomainId)) {
        for (index, slot) in self.slots.iter().enumerate() {
            if !slot.vacant {
                visit(index, slot.id);
            }
        }
    }

    pub fn matches_domain_catalog(&self, id: DomainId, catalog: &CatalogLease) -> bool {
        self.slots.iter().any(|slot| {
            !slot.vacant
                && slot.id == id
                && slot.endpoint.is_none()
                && slot.report.is_none()
                && slot.terminal_error.is_none()
                && slot
                    .control
                    .as_ref()
                    .is_some_and(|control| control.matches_catalog(catalog))
        })
    }

    /// Exact physical-reference preflight; no graph-count or Weak inference.
    /// A queued/active/retired graph lease prevents retirement. With no external
    /// references, only serialized control can subsequently create a lease.
    pub fn domain_has_only_escrow_references(&self, id: DomainId) -> bool {
        self.slots.iter().any(|slot| {
            !slot.vacant
                && slot.id == id
                && slot.endpoint.is_none()
                && slot.report.is_none()
                && slot.terminal_error.is_none()
                && slot
                    .retained_leases
                    .len()
                    .checked_add(3)
                    .is_some_and(|known| {
                        slot.control
                            .as_ref()
                            .is_some_and(|control| control.owner_references() == known)
                    })
        })
    }

    /// Borrowed admission preflight for a serialized control publisher.
    pub fn can_reserve_domain(&self, input: &MediaDomainInput) -> Result<(), DomainRefusal> {
        self.reservation_for(input).map(|_| ())
    }
    fn reservation_for(
        &self,
        input: &MediaDomainInput,
    ) -> Result<(usize, Option<usize>, Option<usize>), DomainRefusal> {
        let refusal = if !self.reusable || self.terminal_batch.is_some() {
            Some(DomainRefusal::TerminalAlreadyReturned)
        } else if !input.control.matches_catalog(input.endpoint.catalog()) {
            Some(DomainRefusal::WrongPair)
        } else if input
            .retained_leases
            .iter()
            .any(|lease| !input.endpoint.catalog().same_catalog(lease))
        {
            Some(DomainRefusal::WrongLease)
        } else if self
            .slots
            .iter()
            .any(|slot| !slot.vacant && slot.id == input.id)
        {
            Some(DomainRefusal::DuplicateIdentity)
        } else if self
            .slots
            .iter()
            .filter_map(|slot| slot.control.as_ref())
            .any(|control| control.matches_catalog(input.endpoint.catalog()))
        {
            Some(DomainRefusal::DuplicateCatalog)
        } else {
            None
        };
        if let Some(reason) = refusal {
            return Err(reason);
        }
        let Some(index) = self.slots.iter().position(|slot| slot.vacant) else {
            return Err(DomainRefusal::DomainCapacity);
        };
        let leases =
            AllocationCapacity::of::<CatalogLease>(input.retained_leases.capacity()).bytes();
        let bytes = input.endpoint.retained_data_bytes().and_then(|bytes| {
            bytes
                .checked_add(leases?)?
                .checked_add(std::mem::size_of::<PageReadiness>())
        });
        let total = self
            .footprint
            .domain_data_bytes
            .and_then(|total| total.checked_add(bytes?));
        if total.is_none() {
            return Err(DomainRefusal::InvalidCapacity);
        }
        Ok((index, bytes, total))
    }

    /// Synchronous publication refusal only: recover original owners before
    /// callback installation. Accepted envelopes require normal retirement.
    pub fn rollback_domain(
        &mut self,
        admission: DomainAdmission,
    ) -> Result<MediaDomainInput, DomainAdmission> {
        if !Arc::ptr_eq(&self.seal, &admission.seal)
            || !self.slots.get(admission.index).is_some_and(|slot| {
                !slot.vacant
                    && slot.id == admission.id
                    && slot.endpoint.is_none()
                    && slot.control.is_some()
                    && slot.report.is_none()
            })
        {
            return Err(admission);
        }
        let slot = &mut self.slots[admission.index];
        let input = MediaDomainInput {
            id: slot.id,
            control: slot.control.take().expect("validated control"),
            endpoint: *admission.endpoint,
            retained_leases: std::mem::take(&mut slot.retained_leases),
        };
        self.footprint.domain_data_bytes = self
            .footprint
            .domain_data_bytes
            .and_then(|bytes| bytes.checked_sub(slot.data_bytes));
        slot.data_bytes = 0;
        slot.vacant = true;
        Ok(input)
    }

    /// Only an accounting-complete outcome frees a reusable reservation.
    /// Caller must retain this actual report before admitting a successor.
    pub fn take_reclaimed_report(&mut self, id: DomainId) -> Option<ReportedShutdown> {
        if !self.reusable {
            return None;
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| !slot.vacant && slot.id == id)?;
        let report = slot
            .report
            .filter(|report| report.successful_report().is_some())?;
        let total = self
            .footprint
            .domain_data_bytes?
            .checked_sub(slot.data_bytes)?;
        slot.retained_leases = Vec::new();
        slot.report = None;
        slot.data_bytes = 0;
        slot.vacant = true;
        self.footprint.domain_data_bytes = Some(total);
        self.update_phase();
        Some(report)
    }

    /// Validate first; only the endpoint moves into the bounded envelope. Worker
    /// control and physical retained leases stay in this durable control root.
    pub fn reserve_domain(
        &mut self,
        input: MediaDomainInput,
    ) -> Result<DomainAdmission, Box<DomainAdmissionFailure>> {
        let (index, bytes, total) = match self.reservation_for(&input) {
            Ok(reservation) => reservation,
            Err(reason) => return Err(Box::new(DomainAdmissionFailure { reason, input })),
        };
        let MediaDomainInput {
            id,
            control,
            endpoint,
            retained_leases,
        } = input;
        self.slots[index] = ControlSlot {
            vacant: false,
            data_bytes: bytes.expect("checked domain bytes"),
            id,
            control: Some(control),
            retained_leases,
            endpoint: None,
            report: None,
            terminal_error: None,
        };
        self.footprint.domain_data_bytes = total;
        Ok(DomainAdmission {
            index,
            id,
            endpoint: Box::new(endpoint),
            seal: Arc::clone(&self.seal),
        })
    }
}
