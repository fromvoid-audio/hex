//! Control-constructed pairing of verified catalog and borrowed page storage.
use crate::{CatalogLease, StreamCallbackPages};
use std::fmt;

/// Actual catalog/page pairing. Construction, transfer and final reclamation
/// belong on control; the page view may be borrowed by rendering. This does not
/// prove installed-page readiness or authorize native publication. No `Clone`.
pub struct VerifiedPageEndpoint {
    pages: StreamCallbackPages,
    catalog: CatalogLease,
}

/// Failed consumption retains both original owners without endpoint mutation.
pub struct PageEndpointMismatch {
    pages: Box<StreamCallbackPages>,
    catalog: CatalogLease,
}

impl VerifiedPageEndpoint {
    pub fn new(
        pages: StreamCallbackPages,
        catalog: CatalogLease,
    ) -> Result<Self, PageEndpointMismatch> {
        if !catalog.matches_pages(&pages) {
            // Control-side error storage, matching shutdown's owning errors.
            return Err(PageEndpointMismatch {
                pages: Box::new(pages),
                catalog,
            });
        }
        Ok(Self { pages, catalog })
    }

    /// Borrow the existing endpoint; no owner is created or destroyed.
    pub fn pages(&mut self) -> &mut StreamCallbackPages {
        &mut self.pages
    }

    pub fn catalog(&self) -> &CatalogLease {
        &self.catalog
    }

    /// Borrow the pairing's capacity report without locks, allocation or clones.
    /// Shared components are charged once per catalog; this is not admission.
    pub fn allocation_footprint(&self) -> crate::StreamAllocationFootprint {
        self.pages.complete_footprint(&self.catalog)
    }

    /// Recover the original owners on control. Release this catalog lease and
    /// any other graph leases before retrying `StreamControl::shutdown(pages)`.
    /// The existing shutdown API retains mismatched/Busy pairs transactionally.
    pub fn into_parts(self) -> (StreamCallbackPages, CatalogLease) {
        (self.pages, self.catalog)
    }
}

impl PageEndpointMismatch {
    pub fn into_parts(self) -> (StreamCallbackPages, CatalogLease) {
        (*self.pages, self.catalog)
    }
}

impl fmt::Debug for PageEndpointMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageEndpointMismatch")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for PageEndpointMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("verified catalog and page endpoint do not match")
    }
}

impl std::error::Error for PageEndpointMismatch {}
