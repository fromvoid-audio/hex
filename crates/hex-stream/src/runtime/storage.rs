//! Concrete upstream composition. No public trusted-provider mint, owner clone,
//! lease manufacture, handoff protocol or lifecycle/admission authority.
use super::*;
use hex_engine::{AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpaqueStreamCost {
    AllocatorMetadata,
    SharedControlBlocks,
    CatalogChannel,
    WorkerThread,
    OperatingSystem,
    StackThreadLocals,
}
impl OpaqueStreamCost {
    const ALL: [Self; 6] = [
        Self::AllocatorMetadata,
        Self::SharedControlBlocks,
        Self::CatalogChannel,
        Self::WorkerThread,
        Self::OperatingSystem,
        Self::StackThreadLocals,
    ];
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamHandleLocation {
    ControlOwner,
    PagesOwner,
    ControlPair,
    PagesPair,
    OwnerStop,
    WorkerStop,
    CatalogResult,
    RequestProducer,
    RequestConsumer,
    ReadyProducer,
    ReadyConsumer,
    ReturnProducer,
    ReturnConsumer,
    FailureProducer,
    FailureConsumer,
    KernelPlan(usize),
    SourceManifest(usize),
    GranularRequestProducer,
    GranularRequestConsumer,
    GranularResultProducer,
    GranularResultConsumer,
    GranularOwnerStop,
    GranularWorkerStop,
    GranularOwnerEpoch,
    GranularWorkerEpoch,
    GranularManifest(usize),
    GranularKernel(usize),
    ScalarSource(usize),
    ScalarProjection(usize),
    ScalarCursorSource,
    ScalarCursorProjection,
    CallerLeaseOwner(usize),
    CallerLeaseCatalog(usize),
    CallerLeasePair(usize),
}
impl StreamHandleLocation {
    const FIXED: [Self; 15] = [
        Self::ControlOwner,
        Self::PagesOwner,
        Self::ControlPair,
        Self::PagesPair,
        Self::OwnerStop,
        Self::WorkerStop,
        Self::CatalogResult,
        Self::RequestProducer,
        Self::RequestConsumer,
        Self::ReadyProducer,
        Self::ReadyConsumer,
        Self::ReturnProducer,
        Self::ReturnConsumer,
        Self::FailureProducer,
        Self::FailureConsumer,
    ];
    const GRANULAR: [Self; 8] = [
        Self::GranularRequestProducer,
        Self::GranularRequestConsumer,
        Self::GranularResultProducer,
        Self::GranularResultConsumer,
        Self::GranularOwnerStop,
        Self::GranularWorkerStop,
        Self::GranularOwnerEpoch,
        Self::GranularWorkerEpoch,
    ];
    fn queue(self) -> bool {
        matches!(
            self,
            Self::RequestProducer
                | Self::RequestConsumer
                | Self::ReadyProducer
                | Self::ReadyConsumer
                | Self::ReturnProducer
                | Self::ReturnConsumer
                | Self::FailureProducer
                | Self::FailureConsumer
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamOwningHandleObligation {
    pub domain: usize,
    pub location: StreamHandleLocation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpaqueStreamHandleGroup {
    CatalogChannel,
    WorkerThread,
    GranularWorkerThread,
    UnregisteredCallerAliases,
    UnobservedConstructor,
}
#[derive(Clone, Copy, Debug)]
pub struct StreamSourceDomainObligation<'a> {
    pub metadata: &'a hex_engine::StreamedSourceMetadata,
    pub owning_handles: usize,
    pub metadata_coverage_domains: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct AudioStreamStorageReservation {
    pub domains: usize,
    pub caller_leases: usize,
    pub max_listed_data_bytes: usize,
    pub max_listed_construction_peak_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStreamStorageError {
    Engine(AudioStorageError),
    Capacity,
    Overflow,
    PairMismatch,
    UnregisteredLease,
    MissingSourceCoverage,
    UnknownData,
    MissingObligation,
    ReservationExceeded,
    UnresolvedObligations,
}
impl From<AudioStorageError> for AudioStreamStorageError {
    fn from(e: AudioStorageError) -> Self {
        Self::Engine(e)
    }
}
struct Domain<'a> {
    control: &'a StreamControl,
    pages: &'a StreamCallbackPages,
    opaque: u8,
}
#[cfg(test)]
pub(super) fn pending_pair_data(
    control: &StreamControl,
    pages: &StreamCallbackPages,
) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
    Domain {
        control,
        pages,
        opaque: 0,
    }
    .footprint()
}
pub(super) fn closed_constructor_pair_data(
    control: &StreamControl,
    pages: &StreamCallbackPages,
) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
    pair_data_with_handles(control, pages, 2)
}
pub(super) fn closed_prebound_pair_data(
    control: &StreamControl,
    pages: &StreamCallbackPages,
    disposal: &super::PreparedStreamDisposal,
) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
    if !disposal.matches_prepared_pair(control, pages) {
        return Err(AudioStreamStorageError::PairMismatch);
    }
    pair_data_with_handles(control, pages, 3)
}
fn pair_data_with_handles(
    control: &StreamControl,
    pages: &StreamCallbackPages,
    pair_handles: usize,
) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
    if !Arc::ptr_eq(&control.owner, &pages._owner) || !Arc::ptr_eq(&control.pair_id, &pages.pair_id)
    {
        return Err(AudioStreamStorageError::PairMismatch);
    }
    // Only the factory endpoints and (when supplied above) the exact prebound
    // disposal may hold the pair. No arbitrary extra lease/Weak is adopted;
    // after consumption no public owner remains available to clone.
    if Arc::strong_count(&control.owner) != 2
        || Arc::strong_count(&control.pair_id) != pair_handles
        || Arc::weak_count(&control.owner) != 0
        || Arc::weak_count(&control.pair_id) != 0
        || Arc::weak_count(&control.owner.stop) != 0
        || Arc::strong_count(&control.owner.stop) != 2
    {
        return Err(AudioStreamStorageError::UnregisteredLease);
    }
    {
        let state = lock_unpoisoned(&control.owner.catalog_state);
        let catalog = state
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .ok_or(AudioStreamStorageError::UnknownData)?;
        if Arc::strong_count(catalog) != 1 || Arc::weak_count(catalog) != 0 {
            return Err(AudioStreamStorageError::UnregisteredLease);
        }
    }
    Domain {
        control,
        pages,
        opaque: 0,
    }
    .footprint()
}
impl Domain<'_> {
    fn matches(&self, control: &StreamControl, pages: &StreamCallbackPages) -> bool {
        Arc::ptr_eq(&self.control.owner, &control.owner)
            && Arc::ptr_eq(&self.control.pair_id, &pages.pair_id)
    }
    fn matches_lease(&self, lease: &CatalogLease) -> bool {
        Arc::ptr_eq(&self.control.owner, &lease.owner)
            && Arc::ptr_eq(&self.control.pair_id, &lease.pair_id)
    }
    fn footprint(&self) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
        let StreamControl { pair_id: _, owner } = self.control;
        let StreamCallbackPages {
            pair_id: _,
            endpoint,
            max_ready_pages_per_pump: _,
            _owner: _,
        } = self.pages;
        let CatalogOwner {
            transport_data,
            config: _,
            stop: _,
            worker_wake: _,
            worker,
            catalog_state,
            #[cfg(test)]
                worker_finished: _,
        } = owner.as_ref();
        if Arc::weak_count(&self.control.owner) != 0 || Arc::weak_count(&self.control.pair_id) != 0
        {
            return Err(AudioStreamStorageError::UnknownData);
        }
        // A constructor capacity reservation, never a saved worker address or
        // live-location/retirement receipt. Completion racing this observation
        // can release storage, but cannot grow this fixed constructor domain.
        let worker = lock_unpoisoned(worker);
        if owner.stop.load(Ordering::Acquire) || worker.as_ref().is_none_or(JoinHandle::is_finished)
        {
            return Err(AudioStreamStorageError::UnknownData);
        }
        let state = lock_unpoisoned(catalog_state);
        let CatalogState {
            receiver: _,
            result,
        } = &*state;
        let catalog = result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .ok_or(AudioStreamStorageError::UnknownData)?;
        let VerifiedCatalog {
            footprint: _,
            sources: _,
            scalar_sources: _,
            scalar_shared_data: _,
            data,
        } = catalog.as_ref();
        if Arc::weak_count(catalog) != 0 || Arc::weak_count(&self.control.owner.stop) != 0 {
            return Err(AudioStreamStorageError::UnknownData);
        }
        // Both guards are local: callers receive scalar copies only. In
        // particular they may immediately query the catalog without reentry.
        Ok(crate::StreamDataFootprint {
            endpoint: endpoint.data_footprint(),
            transport: *transport_data,
            worker: *data,
        })
    }
    fn covers(&self, source: &hex_engine::StreamedSourceMetadata) -> bool {
        lock_unpoisoned(&self.control.owner.catalog_state)
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .is_some_and(|catalog| {
                catalog
                    .sources
                    .binary_search_by_key(&source.key(), hex_engine::StreamedSourceMetadata::key)
                    .ok()
                    .and_then(|i| catalog.sources.get(i))
                    .is_some_and(|s| s == source)
            })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioStreamStorageSummary {
    pub roots: AudioRootStorageSummary,
    pub distinct_domains: usize,
    pub listed_data_bytes: Option<usize>,
    pub listed_construction_peak_bytes: Option<usize>,
    pub metadata_bytes: usize,
    pub opaque_cost_obligations: usize,
    /// Explicit actual selected Arc fields: two owners/two pair handles,
    /// owner+worker stop handles, catalog-result owner, eight queue halves,
    /// each worker reader manifest/plan handle, each granular queue/atomic/source
    /// handle and three fields per selected lease.
    /// Opaque std and unregistered external handles remain separate groups.
    pub unresolved_known_stream_handles: usize,
    pub unresolved_opaque_handle_groups: usize,
    pub unresolved_constructor_handle_groups: usize,
    pub unresolved_unregistered_caller_handle_groups: usize,
    pub unresolved_source_domain_bindings: usize,
    pub unresolved_domain_retirements: usize,
}
impl AudioStreamStorageSummary {
    /// Only the listed DATA-capacity contract. This says nothing about opaque
    /// costs, owning handles, actual source leases or lifetime closure.
    pub fn require_listed_data(self) -> Result<Self, AudioStreamStorageError> {
        self.roots
            .require_known_storage()
            .map_err(|_| AudioStreamStorageError::UnknownData)?;
        if self.listed_data_bytes.is_none() || self.listed_construction_peak_bytes.is_none() {
            return Err(AudioStreamStorageError::UnknownData);
        }
        Ok(self)
    }
    pub fn require_complete(self) -> Result<(), AudioStreamStorageError> {
        self.require_listed_data()?;
        Err(AudioStreamStorageError::UnresolvedObligations)
    }
}
/// Borrows all selected actual owners. Reporting may take the existing catalog
/// and join-handle mutexes on CONTROL only. It never wakes, joins, stops, clones,
/// transfers, publishes or changes an endpoint. No callback use is permitted.
pub struct AudioStreamStorageLedger<'a> {
    roots: &'a AudioRootStorageLedger<'a>,
    domains: Vec<Domain<'a>>,
    leases: Vec<&'a CatalogLease>,
    reservation: AudioStreamStorageReservation,
    valid: bool,
}
impl<'a> AudioStreamStorageLedger<'a> {
    pub fn try_new_on_control(
        roots: &'a AudioRootStorageLedger<'a>,
        reservation: AudioStreamStorageReservation,
    ) -> Result<Self, AudioStreamStorageError> {
        roots.summary()?;
        reservation
            .domains
            .checked_mul(std::mem::size_of::<Domain<'_>>())
            .ok_or(AudioStreamStorageError::Overflow)?;
        reservation
            .caller_leases
            .checked_mul(std::mem::size_of::<&CatalogLease>())
            .ok_or(AudioStreamStorageError::Overflow)?;
        let mut domains = Vec::new();
        let mut leases = Vec::new();
        domains
            .try_reserve_exact(reservation.domains)
            .map_err(|_| AudioStreamStorageError::Capacity)?;
        leases
            .try_reserve_exact(reservation.caller_leases)
            .map_err(|_| AudioStreamStorageError::Capacity)?;
        Ok(Self {
            roots,
            domains,
            leases,
            reservation,
            valid: true,
        })
    }
    /// Generic page providers cannot certify actual domain allocation custody.
    /// ```compile_fail
    /// fn provider<'a>(ledger:&mut hex_stream::AudioStreamStorageLedger<'a>,
    ///     pages:&'a hex_stream::StreamCallbackPages, provider:&'a dyn hex_engine::MediaPages) {
    ///     ledger.borrow_domain_on_control(provider,pages);
    /// }
    /// ```
    pub fn borrow_domain_on_control(
        &mut self,
        control: &'a StreamControl,
        pages: &'a StreamCallbackPages,
    ) -> Result<(), AudioStreamStorageError> {
        if !self.valid {
            return Err(AudioStreamStorageError::UnknownData);
        }
        self.valid = false;
        if !Arc::ptr_eq(&control.owner, &pages._owner)
            || !Arc::ptr_eq(&control.pair_id, &pages.pair_id)
        {
            return Err(AudioStreamStorageError::PairMismatch);
        }
        if self.domains.iter().any(|d| d.matches(control, pages)) {
            self.valid = true;
            return Ok(());
        }
        if self.domains.len() == self.reservation.domains {
            return Err(AudioStreamStorageError::Capacity);
        }
        let mut domain = Domain {
            control,
            pages,
            opaque: 0,
        };
        domain.opaque |= OpaqueStreamCost::AllocatorMetadata.bit();
        domain.opaque |= OpaqueStreamCost::SharedControlBlocks.bit();
        domain.opaque |= OpaqueStreamCost::CatalogChannel.bit();
        domain.opaque |= OpaqueStreamCost::WorkerThread.bit();
        domain.opaque |= OpaqueStreamCost::OperatingSystem.bit();
        domain.opaque |= OpaqueStreamCost::StackThreadLocals.bit();
        self.domains.push(domain);
        self.valid = true;
        Ok(())
    }
    pub fn borrow_caller_lease_on_control(
        &mut self,
        lease: &'a CatalogLease,
    ) -> Result<(), AudioStreamStorageError> {
        if !self.valid {
            return Err(AudioStreamStorageError::UnknownData);
        }
        self.valid = false;
        let domain = self
            .domains
            .iter()
            .find(|d| d.matches_lease(lease))
            .ok_or(AudioStreamStorageError::UnregisteredLease)?;
        let state = lock_unpoisoned(&domain.control.owner.catalog_state);
        if !state
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .is_some_and(|catalog| Arc::ptr_eq(catalog, &lease.catalog))
        {
            return Err(AudioStreamStorageError::UnregisteredLease);
        }
        if self.leases.iter().any(|old| std::ptr::eq(*old, lease)) {
            return Err(AudioStreamStorageError::UnregisteredLease);
        }
        if self.leases.len() == self.reservation.caller_leases {
            return Err(AudioStreamStorageError::Capacity);
        }
        self.leases.push(lease);
        self.valid = true;
        Ok(())
    }
    /// Each selected physical constructor field remains a separate obligation.
    /// Worker records are constructor observations, never lifetime receipts.
    pub fn owning_handle_obligations(
        &self,
    ) -> impl Iterator<Item = Result<StreamOwningHandleObligation, AudioStreamStorageError>> + '_
    {
        self.domains
            .iter()
            .enumerate()
            .flat_map(|(index, domain)| {
                let data = domain.footprint();
                let fixed = StreamHandleLocation::FIXED
                    .into_iter()
                    .map(move |location| {
                        let data = data?;
                        if location.queue() && !data.transport.queue_constructor_supported {
                            return Err(AudioStreamStorageError::UnknownData);
                        }
                        Ok(StreamOwningHandleObligation {
                            domain: index,
                            location,
                        })
                    });
                let kernel_handles = data.map_or(0, |d| {
                    if d.worker.constructor_complete {
                        d.worker.kernel_handles
                    } else {
                        0
                    }
                });
                fixed
                    .chain((0..kernel_handles).map(move |i| {
                        Ok(StreamOwningHandleObligation {
                            domain: index,
                            location: StreamHandleLocation::KernelPlan(i),
                        })
                    }))
                    .chain(
                        (0..data.map_or(0, |d| d.worker.manifest_handles)).map(move |i| {
                            Ok(StreamOwningHandleObligation {
                                domain: index,
                                location: StreamHandleLocation::SourceManifest(i),
                            })
                        }),
                    )
                    .chain(
                        data.ok()
                            .and_then(|d| d.worker.granular)
                            .into_iter()
                            .flat_map(move |granular| {
                                StreamHandleLocation::GRANULAR
                                    .into_iter()
                                    .chain(
                                        (0..granular.worker.bank.manifest_handles)
                                            .map(StreamHandleLocation::GranularManifest),
                                    )
                                    .chain(
                                        (0..granular.worker.bank.kernel_handles)
                                            .map(StreamHandleLocation::GranularKernel),
                                    )
                                    .map(move |location| {
                                        Ok(StreamOwningHandleObligation {
                                            domain: index,
                                            location,
                                        })
                                    })
                            }),
                    )
                    .chain(
                        (0..data.map_or(0, |d| d.worker.scalar_source_handles))
                            .flat_map(move |i| {
                                [
                                    StreamHandleLocation::ScalarSource(i),
                                    StreamHandleLocation::ScalarProjection(i),
                                ]
                            })
                            .map(move |location| {
                                Ok(StreamOwningHandleObligation {
                                    domain: index,
                                    location,
                                })
                            }),
                    )
                    .chain(
                        (data.map_or(0, |d| d.worker.scalar_cursor_handle_bound) != 0)
                            .then_some([
                                StreamHandleLocation::ScalarCursorSource,
                                StreamHandleLocation::ScalarCursorProjection,
                            ])
                            .into_iter()
                            .flatten()
                            .map(move |location| {
                                Ok(StreamOwningHandleObligation {
                                    domain: index,
                                    location,
                                })
                            }),
                    )
            })
            .chain(
                self.leases
                    .iter()
                    .enumerate()
                    .flat_map(|(lease_index, lease)| {
                        let domain = self.domains.iter().position(|d| d.matches_lease(lease));
                        [
                            StreamHandleLocation::CallerLeaseOwner(lease_index),
                            StreamHandleLocation::CallerLeaseCatalog(lease_index),
                            StreamHandleLocation::CallerLeasePair(lease_index),
                        ]
                        .into_iter()
                        .map(move |location| {
                            domain
                                .map(|domain| StreamOwningHandleObligation { domain, location })
                                .ok_or(AudioStreamStorageError::UnregisteredLease)
                        })
                    }),
            )
    }
    pub fn opaque_handle_obligations(
        &self,
    ) -> impl Iterator<Item = (usize, OpaqueStreamHandleGroup)> + '_ {
        self.domains.iter().enumerate().flat_map(|(index, domain)| {
            let unknown = !domain.footprint().is_ok_and(|d| {
                d.transport.queue_constructor_supported && d.worker.constructor_complete
            });
            [
                Some(OpaqueStreamHandleGroup::CatalogChannel),
                Some(OpaqueStreamHandleGroup::WorkerThread),
                domain
                    .footprint()
                    .is_ok_and(|d| d.worker.granular.is_some())
                    .then_some(OpaqueStreamHandleGroup::GranularWorkerThread),
                Some(OpaqueStreamHandleGroup::UnregisteredCallerAliases),
                unknown.then_some(OpaqueStreamHandleGroup::UnobservedConstructor),
            ]
            .into_iter()
            .flatten()
            .map(move |kind| (index, kind))
        })
    }
    /// Complete metadata coverage is a fact, not owner identity or a lease. An
    /// obligation remains for EVERY handle, even with several matching domains.
    pub fn source_domain_obligations(
        &self,
    ) -> impl Iterator<Item = StreamSourceDomainObligation<'a>> + '_ {
        self.roots
            .source_domain_obligations()
            .map(|row| StreamSourceDomainObligation {
                metadata: row.metadata,
                owning_handles: row.owning_handles,
                metadata_coverage_domains: self
                    .domains
                    .iter()
                    .filter(|d| d.covers(row.metadata))
                    .count(),
            })
    }
    pub fn opaque_cost_obligations(&self) -> impl Iterator<Item = (usize, OpaqueStreamCost)> + '_ {
        self.domains.iter().enumerate().flat_map(|(index, d)| {
            OpaqueStreamCost::ALL
                .into_iter()
                .filter(move |cost| d.opaque & cost.bit() != 0)
                .map(move |cost| (index, cost))
        })
    }
    pub fn domain_data(
        &self,
        index: usize,
    ) -> Result<crate::StreamDataFootprint, AudioStreamStorageError> {
        self.domains
            .get(index)
            .ok_or(AudioStreamStorageError::Capacity)?
            .footprint()
    }
    /// Actual retained query metadata remains observable after refusal, without
    /// making a partial graph/domain DATA total usable.
    pub fn metadata_capacity_bytes(&self) -> Option<usize> {
        self.domains
            .capacity()
            .checked_mul(std::mem::size_of::<Domain<'_>>())?
            .checked_add(
                self.leases
                    .capacity()
                    .checked_mul(std::mem::size_of::<&CatalogLease>())?,
            )?
            .checked_add(std::mem::size_of::<Self>())
    }
    pub fn summary(&self) -> Result<AudioStreamStorageSummary, AudioStreamStorageError> {
        if !self.valid {
            return Err(AudioStreamStorageError::UnknownData);
        }
        let roots = self.roots.summary()?;
        let metadata_bytes = self
            .metadata_capacity_bytes()
            .ok_or(AudioStreamStorageError::Overflow)?;
        let mut listed = Some(
            roots
                .retained_bytes
                .checked_add(metadata_bytes)
                .ok_or(AudioStreamStorageError::Overflow)?,
        );
        let mut peak = Some(
            roots
                .construction_peak_bytes
                .checked_add(metadata_bytes)
                .ok_or(AudioStreamStorageError::Overflow)?,
        );
        let mut handles = 0usize;
        let expected = OpaqueStreamCost::ALL
            .into_iter()
            .fold(0, |mask, cost| mask | cost.bit());
        for domain in &self.domains {
            if domain.opaque != expected {
                return Err(AudioStreamStorageError::MissingObligation);
            }
            match domain.footprint() {
                Ok(data) => {
                    listed = listed
                        .zip(data.listed_data_bytes())
                        .map(|(a, b)| a.checked_add(b).ok_or(AudioStreamStorageError::Overflow))
                        .transpose()?;
                    peak = peak
                        .zip(data.listed_construction_peak())
                        .map(|(a, b)| a.checked_add(b).ok_or(AudioStreamStorageError::Overflow))
                        .transpose()?;
                    handles = handles
                        .checked_add(if data.transport.queue_constructor_supported {
                            15
                        } else {
                            7
                        })
                        .and_then(|n| {
                            n.checked_add(if data.worker.constructor_complete {
                                data.worker
                                    .kernel_handles
                                    .checked_add(data.worker.manifest_handles)
                                    .and_then(|n| {
                                        n.checked_add(
                                            data.worker.granular.map_or(Some(0), |g| {
                                                g.worker.shared_handle_bound()
                                            })?,
                                        )
                                    })
                                    .and_then(|n| {
                                        n.checked_add(
                                            data.worker.scalar_source_handles.checked_mul(2)?,
                                        )
                                    })
                                    .and_then(|n| {
                                        n.checked_add(data.worker.scalar_cursor_handle_bound)
                                    })?
                            } else {
                                0
                            })
                        })
                        .ok_or(AudioStreamStorageError::Overflow)?;
                }
                Err(AudioStreamStorageError::UnknownData) => {
                    listed = None;
                    peak = None;
                }
                Err(e) => return Err(e),
            }
        }
        for source in self.roots.streamed_sources() {
            if !self.domains.iter().any(|d| d.covers(source)) {
                return Err(AudioStreamStorageError::MissingSourceCoverage);
            }
        }
        handles = handles
            .checked_add(
                self.leases
                    .len()
                    .checked_mul(3)
                    .ok_or(AudioStreamStorageError::Overflow)?,
            )
            .ok_or(AudioStreamStorageError::Overflow)?;
        if listed.is_some_and(|bytes| bytes > self.reservation.max_listed_data_bytes)
            || peak.is_some_and(|bytes| bytes > self.reservation.max_listed_construction_peak_bytes)
        {
            return Err(AudioStreamStorageError::ReservationExceeded);
        }
        Ok(AudioStreamStorageSummary {
            roots,
            distinct_domains: self.domains.len(),
            listed_data_bytes: listed,
            listed_construction_peak_bytes: peak,
            metadata_bytes,
            opaque_cost_obligations: self.opaque_cost_obligations().count(),
            unresolved_known_stream_handles: handles,
            unresolved_constructor_handle_groups: self
                .opaque_handle_obligations()
                .filter(|(_, kind)| *kind == OpaqueStreamHandleGroup::UnobservedConstructor)
                .count(),
            unresolved_opaque_handle_groups: self
                .opaque_handle_obligations()
                .filter(|(_, group)| {
                    matches!(
                        group,
                        OpaqueStreamHandleGroup::CatalogChannel
                            | OpaqueStreamHandleGroup::WorkerThread
                            | OpaqueStreamHandleGroup::GranularWorkerThread
                    )
                })
                .count(),
            unresolved_unregistered_caller_handle_groups: self.domains.len(),
            unresolved_source_domain_bindings: self.roots.streamed_handle_obligations(),
            unresolved_domain_retirements: self.domains.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{
        AudioStorageLedgerReservation, AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource,
        StreamSourceKey, StreamedSourceMetadata,
    };
    use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
    use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Project};
    fn root_reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 64,
            root_rows: 8,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        }
    }
    fn stream_reservation() -> AudioStreamStorageReservation {
        AudioStreamStorageReservation {
            domains: 8,
            caller_leases: 8,
            max_listed_data_bytes: usize::MAX,
            max_listed_construction_peak_bytes: usize::MAX,
        }
    }
    fn config() -> StreamConfig {
        let mut c = StreamConfig::with_pool_pages(4);
        c.source_scratch_frames = 4096;
        c
    }
    fn source() -> (crate::worker::tests::Fixture, SourceDefinition) {
        let fixture = crate::worker::tests::Fixture::pcm16();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(&fixture.0, limits).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "storage.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(73, asset),
            48_000,
            48_000,
            4096,
            4096,
            IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        let mut path = std::path::PathBuf::with_capacity(733);
        path.push(&fixture.0);
        (fixture, SourceDefinition::new(path, metadata, limits))
    }
    fn runtime(definition: SourceDefinition) -> (StreamControl, StreamCallbackPages) {
        let mut definitions = Vec::with_capacity(17);
        definitions.push(definition);
        let mut runtime = StreamRuntime::new(config(), definitions).unwrap();
        runtime.wait_until_ready(Duration::from_secs(5)).unwrap();
        runtime.split().ok().unwrap()
    }
    fn bank(metadata: StreamedSourceMetadata) -> OwnedAudioInputs {
        let asset = metadata.key().asset();
        OwnedAudioInputs::try_new(
            vec![(
                asset,
                OwnedAudioSource::try_streamed(metadata).ok().unwrap(),
            )],
            AudioStorageReservation {
                allocation_items: 32,
                handle_obligations: 64,
                max_prepared_data_bytes: usize::MAX,
                max_construction_data_bytes: usize::MAX,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap()
    }
    #[test]
    fn actual_domain_data_and_opaque_obligations_are_distinct_from_complete_admission() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let (_fixture, definition) = source();
        let expected_path = definition.path.capacity();
        let (control, pages) = runtime(definition);
        let root = AudioRootStorageLedger::try_new_on_control(root_reservation()).unwrap();
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&root, stream_reservation()).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        let data = report.domain_data(0).unwrap();
        assert_eq!(data.worker.definition_paths, expected_path);
        assert!(data.worker.definition_paths > data.worker.reader_paths);
        assert_eq!(data.worker.construction_definition_slots.capacity, 17);
        assert_eq!(data.worker.source_slots.capacity, 1);
        assert_eq!(
            data.transport.page_objects,
            4 * std::mem::size_of::<PageBuffer>()
        );
        assert_eq!(
            data.transport.pcm_boxes,
            4 * hex_engine::STREAM_PAGE_FRAMES * std::mem::size_of::<hex_engine::Frame>()
        );
        let expected_queues = config().request_capacity
            * std::mem::size_of::<crate::endpoint::PageRequest>()
            + (config().ready_capacity + config().return_capacity)
                * std::mem::size_of::<Box<PageBuffer>>()
            + config().failure_capacity * std::mem::size_of::<StreamFailure>();
        assert_eq!(data.transport.queue_slot_data_declared, expected_queues);
        let s = report.summary().unwrap().require_listed_data().unwrap();
        assert_eq!(s.distinct_domains, 1);
        assert_eq!(s.opaque_cost_obligations, 6);
        assert_eq!(s.unresolved_known_stream_handles, 17);
        let handles = report
            .owning_handle_obligations()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(handles.len(), s.unresolved_known_stream_handles);
        // One raw reader retains a manifest handle beside its conversion
        // plan; this report selects the original pair without caller leases.
        assert_eq!(handles.len(), StreamHandleLocation::FIXED.len() + 2);
        for location in StreamHandleLocation::FIXED {
            assert!(
                handles
                    .iter()
                    .any(|row| row.domain == 0 && row.location == location)
            );
        }
        assert_eq!(report.opaque_handle_obligations().count(), 3);
        assert_eq!(s.unresolved_opaque_handle_groups, 2);
        assert_eq!(
            s.require_complete(),
            Err(AudioStreamStorageError::UnresolvedObligations)
        );
        assert_eq!(
            s.listed_data_bytes.unwrap(),
            root.summary().unwrap().retained_bytes
                + s.metadata_bytes
                + data.listed_data_bytes().unwrap()
        );
        assert!(s.listed_construction_peak_bytes.unwrap() > s.listed_data_bytes.unwrap());
        #[cfg(feature = "allocation-probe")]
        {
            let observed = allocation_counter::measure(|| {
                for _ in 0..8 {
                    std::hint::black_box(report.summary().unwrap());
                    std::hint::black_box(report.owning_handle_obligations().count());
                    std::hint::black_box(report.source_domain_obligations().count());
                }
            });
            assert_eq!(observed.count_total, 0);
            assert_eq!(observed.bytes_total, 0);
            assert_eq!(observed.count_current, 0);
            assert_eq!(observed.bytes_current, 0);
        }
        report.domains[0].opaque &= !OpaqueStreamCost::CatalogChannel.bit();
        assert_eq!(
            report.summary(),
            Err(AudioStreamStorageError::MissingObligation)
        );
        drop(report);
        drop(root);
        assert!(control.shutdown(pages).unwrap().accounting_complete);
    }
    #[test]
    fn equal_metadata_distinct_domains_and_allocations_never_collapse_into_identity() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let (_fixture, definition) = source();
        let mut a = bank(definition.metadata.clone());
        let b = bank(definition.metadata.clone());
        let token = a.source_token(definition.metadata.key().asset()).unwrap();
        a.alias_on_control(&token).unwrap();
        a.alias_on_control(&token).unwrap();
        let (control, pages) = runtime(definition.clone());
        let (other, other_pages) = runtime(definition);
        let lease = control.catalog_lease().unwrap();
        let alias = lease.clone();
        let count = Arc::strong_count(&control.owner);
        let mut roots = AudioRootStorageLedger::try_new_on_control(root_reservation()).unwrap();
        roots.borrow_caller_inputs_on_control(&a).unwrap();
        roots.borrow_caller_inputs_on_control(&b).unwrap();
        assert_eq!(roots.streamed_sources().count(), 2);
        assert_eq!(roots.streamed_handle_obligations(), 4);
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&roots, stream_reservation()).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        report
            .borrow_domain_on_control(&other, &other_pages)
            .unwrap();
        report.borrow_caller_lease_on_control(&lease).unwrap();
        report.borrow_caller_lease_on_control(&alias).unwrap();
        let s = report.summary().unwrap().require_listed_data().unwrap();
        assert_eq!(s.distinct_domains, 2);
        assert_eq!(s.roots.unresolved_handles, 4);
        assert_eq!(s.unresolved_source_domain_bindings, 4);
        assert_eq!(s.opaque_cost_obligations, 12);
        assert_eq!(s.unresolved_known_stream_handles, 17 * 2 + 6);
        assert_eq!(
            report
                .owning_handle_obligations()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .len(),
            s.unresolved_known_stream_handles
        );
        let bindings = report.source_domain_obligations().collect::<Vec<_>>();
        assert_eq!(bindings.len(), 2);
        assert!(
            bindings
                .iter()
                .all(|row| row.metadata_coverage_domains == 2)
        );
        assert_eq!(
            bindings.iter().map(|row| row.owning_handles).sum::<usize>(),
            4
        );
        let d0 = report.domain_data(0).unwrap().listed_data_bytes().unwrap();
        let d1 = report.domain_data(1).unwrap().listed_data_bytes().unwrap();
        assert_eq!(
            s.listed_data_bytes,
            Some(s.roots.retained_bytes + s.metadata_bytes + d0 + d1)
        );
        assert_eq!(Arc::strong_count(&control.owner), count);
        drop(report);
        drop(roots);
        drop(alias);
        drop(lease);
        assert!(control.shutdown(pages).unwrap().accounting_complete);
        assert!(other.shutdown(other_pages).unwrap().accounting_complete);
    }
    #[test]
    fn unsampled_worker_stays_unknown_and_shared_kernel_data_is_charged_once() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let (_fixture, mut first) = source();
        let metadata = &first.metadata;
        first.metadata = StreamedSourceMetadata::new(
            metadata.key(),
            48_000,
            44_100,
            4096,
            hex_project::converted_frame_count(4096, 48_000, 44_100).unwrap(),
            hex_project::SINC_CONVERSION_V1,
            metadata.fingerprint(),
        )
        .unwrap();
        let mut second = first.clone();
        second.key = StreamSourceKey::new(74, first.key.asset());
        second.metadata = StreamedSourceMetadata::new(
            second.key,
            48_000,
            44_100,
            4096,
            first.metadata.playback_frames(),
            hex_project::SINC_CONVERSION_V1,
            first.metadata.fingerprint(),
        )
        .unwrap();
        let runtime = StreamRuntime::new(config(), vec![first, second]).unwrap();
        let (mut control, pages) = runtime.split().ok().unwrap();
        let roots = AudioRootStorageLedger::try_new_on_control(root_reservation()).unwrap();
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&roots, stream_reservation()).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        // No readiness poll has collected a constructor receipt, even if the
        // worker has already sent it. Never guess from an unobserved worker.
        let unknown = report.summary().unwrap();
        assert_eq!(unknown.listed_data_bytes, None);
        assert_eq!(unknown.unresolved_constructor_handle_groups, 1);
        assert_eq!(
            unknown.require_listed_data(),
            Err(AudioStreamStorageError::UnknownData)
        );
        drop(report);
        control.wait_until_ready(Duration::from_secs(5)).unwrap();
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&roots, stream_reservation()).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        let data = report.domain_data(0).unwrap();
        let kernel = hex_media::ResampleKernel::new(48_000, 44_100).unwrap();
        let layout = kernel.control_data_layout();
        assert!(layout.weight_capacity > 0);
        assert_eq!(data.worker.kernel_objects, layout.owner_payload_bytes);
        assert_eq!(
            data.worker.kernel_weights,
            layout.weight_capacity * layout.weight_element_bytes
        );
        assert_eq!(data.worker.kernel_handles, 2);
        assert_eq!(data.worker.source_slots.capacity, 2);
        assert_eq!(
            report
                .summary()
                .unwrap()
                .require_listed_data()
                .unwrap()
                .unresolved_known_stream_handles,
            19
        );
        // An ordinary read query after reporting must not retain a catalog
        // guard; that would deadlock this control-side query.
        assert!(control.catalog_footprint().is_some());
        // Test-only stop injection models the terminal worker observation;
        // production shutdown cannot consume these borrowed halves.
        control.owner.stop.store(true, Ordering::Release);
        control.owner.worker_wake.unpark();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !lock_unpoisoned(&control.owner.worker)
            .as_ref()
            .unwrap()
            .is_finished()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let ended = report.summary().unwrap();
        assert_eq!(ended.listed_data_bytes, None);
        assert_eq!(ended.unresolved_constructor_handle_groups, 1);
        assert_eq!(
            ended.require_complete(),
            Err(AudioStreamStorageError::UnknownData)
        );
        drop(report);
        assert!(control.shutdown(pages).unwrap().accounting_complete);
    }
    #[test]
    fn mismatched_pairs_missing_catalog_coverage_and_pressure_refuse_without_owner_effects() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let (_fixture, definition) = source();
        let b = bank(definition.metadata.clone());
        let metadata = &definition.metadata;
        let mut wrong_fingerprint = metadata.fingerprint();
        wrong_fingerprint[0] ^= 1;
        let wrong = bank(
            StreamedSourceMetadata::new(
                metadata.key(),
                metadata.source_rate(),
                metadata.project_rate(),
                metadata.source_frames(),
                metadata.playback_frames(),
                metadata.conversion_version(),
                wrong_fingerprint,
            )
            .unwrap(),
        );
        let (control, pages) = runtime(definition);
        let (_other_fixture, other_definition) = source();
        let (other, other_pages) = runtime(other_definition);
        let mut wrong_roots =
            AudioRootStorageLedger::try_new_on_control(root_reservation()).unwrap();
        wrong_roots.borrow_caller_inputs_on_control(&wrong).unwrap();
        let mut wrong_report =
            AudioStreamStorageLedger::try_new_on_control(&wrong_roots, stream_reservation())
                .unwrap();
        wrong_report
            .borrow_domain_on_control(&control, &pages)
            .unwrap();
        assert_eq!(
            wrong_report.summary(),
            Err(AudioStreamStorageError::MissingSourceCoverage)
        );
        drop(wrong_report);
        let mut roots = AudioRootStorageLedger::try_new_on_control(root_reservation()).unwrap();
        roots.borrow_caller_inputs_on_control(&b).unwrap();
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&roots, stream_reservation()).unwrap();
        assert_eq!(
            report.borrow_domain_on_control(&control, &other_pages),
            Err(AudioStreamStorageError::PairMismatch)
        );
        assert_eq!(report.summary(), Err(AudioStreamStorageError::UnknownData));
        drop(report);
        let mut report =
            AudioStreamStorageLedger::try_new_on_control(&roots, stream_reservation()).unwrap();
        assert_eq!(
            report.summary(),
            Err(AudioStreamStorageError::MissingSourceCoverage)
        );
        report.borrow_domain_on_control(&control, &pages).unwrap();
        assert!(report.summary().unwrap().require_listed_data().is_ok());
        drop(report);
        let mut r = stream_reservation();
        r.max_listed_data_bytes = 0;
        let mut report = AudioStreamStorageLedger::try_new_on_control(&roots, r).unwrap();
        report.borrow_domain_on_control(&control, &pages).unwrap();
        assert_eq!(
            report.summary(),
            Err(AudioStreamStorageError::ReservationExceeded)
        );
        drop(report);
        drop(roots);
        assert!(control.shutdown(pages).unwrap().accounting_complete);
        assert!(other.shutdown(other_pages).unwrap().accounting_complete);
    }
}
