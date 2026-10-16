//! Actual catalog/pair custody for a candidate that has not been installed.
//! Full metadata coverage chooses a real retained lease, never an owner identity.
use hex_stream::{CatalogLease, PreparedStreamDisposal, StreamCallbackPages, StreamControl};

pub type CandidateDomain = (StreamControl, StreamCallbackPages);
pub(crate) struct CatalogCustody {
    pairs: Vec<CandidateDomain>,
    leases: Vec<CatalogLease>,
    lease_drop_pending: bool,
    disposals: Vec<PreparedStreamDisposal>,
    current: Option<PreparedStreamDisposal>,
    current_pair: Option<CandidateDomain>,
    count: usize,
    failed: bool,
}
impl CatalogCustody {
    pub(crate) fn prepare(
        owner: &hex_engine::AudioCandidateOwner,
        pairs: Vec<CandidateDomain>,
    ) -> Result<Self, Vec<CandidateDomain>> {
        // Reserve before moving any input. Pair/lease matching is checked by
        // the stream owner; no raw catalog or public constructor is adopted.
        if pairs.len() > crate::prepared::RETIRED {
            return Err(pairs);
        }
        let mut disposals = Vec::new();
        if disposals.try_reserve_exact(pairs.len()).is_err() {
            return Err(pairs);
        }
        let mut leases = Vec::new();
        if leases.try_reserve_exact(pairs.len()).is_err() {
            return Err(pairs);
        }
        for (control, pages) in &pairs {
            let Ok(lease) = control.catalog_lease() else {
                return Err(pairs);
            };
            if !pages.matches_catalog(&lease) {
                return Err(pairs);
            }
            let Ok(disposal) = PreparedStreamDisposal::try_prepare_on_control(control, pages)
            else {
                return Err(pairs);
            };
            disposals.push(disposal);
            leases.push(lease);
        }
        if owner
            .streamed_sources()
            .any(|source| !leases.iter().any(|lease| lease.covers_source(source)))
        {
            return Err(pairs);
        }
        let count = pairs.len();
        Ok(Self {
            pairs,
            leases,
            lease_drop_pending: false,
            disposals,
            current: None,
            current_pair: None,
            count,
            failed: false,
        })
    }
    pub(crate) fn into_pairs(self) -> Vec<CandidateDomain> {
        self.pairs
    }
    pub(crate) fn domain_count(&self) -> usize {
        self.count
    }
    pub(crate) fn cleanup(&mut self) -> Result<bool, ()> {
        if self.failed || self.lease_drop_pending {
            return Err(());
        }
        self.lease_drop_pending = true;
        self.leases.clear(); // only after actual normal runtime/root destruction
        self.lease_drop_pending = false;
        if self.current.is_none() {
            if self.pairs.is_empty() {
                return Ok(true);
            }
            self.current = self.disposals.pop();
            self.current_pair = self.pairs.pop();
            let Some(current) = self.current.as_mut() else {
                self.failed = true;
                return Err(());
            };
            if current.begin_on_control(&mut self.current_pair).is_err() {
                self.failed = true;
                return Err(());
            }
        }
        let Some(current) = self.current.as_mut() else {
            self.failed = true;
            return Err(());
        };
        match current.advance_on_control() {
            Ok(Some(report)) if report.worker_joined && report.accounting_complete => {
                // This capsule owns no domain resources after normal physical
                // destruction. Metadata drop is outside the ordinary bank lock.
                drop(self.current.take());
                Ok(self.pairs.is_empty())
            }
            Ok(None) => Ok(false),
            _ => {
                self.failed = true;
                Err(())
            }
        }
    }
    pub(crate) fn metadata_bytes(&self) -> Option<usize> {
        let mut bytes = self
            .pairs
            .capacity()
            .checked_mul(std::mem::size_of::<CandidateDomain>())?
            .checked_add(
                self.leases
                    .capacity()
                    .checked_mul(std::mem::size_of::<CatalogLease>())?,
            )?
            .checked_add(
                self.disposals
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedStreamDisposal>())?,
            )?;
        for disposal in self.disposals.iter().chain(self.current.iter()) {
            bytes = bytes.checked_add(disposal.metadata_bytes()?)?;
        }
        Some(bytes)
    }
}
