//! Control-owned complete graph/catalog binding; no native admission policy.
use crate::CatalogLease;
use hex_engine::{PreparedRevision, StreamSourceKey};
use std::{fmt, sync::Arc};

/// One immutable graph paired with the actual verified catalog that covers all
/// its Arrangement and Session streamed bindings. Unused catalog sources are
/// allowed. Construction, owner transfer and reclamation belong on control.
///
/// This wrapper neither owns callback pages nor proves page readiness. It must
/// not be constructed, cloned or finally dropped on a callback; final catalog
/// release can stop/wake/join its worker. Native publication remains refused.
/// No `Clone` implementation implicitly creates another graph/catalog owner.
pub struct VerifiedStreamRevision {
    revision: Arc<PreparedRevision>,
    catalog: CatalogLease,
}

/// A rejected binding retains both inputs unchanged for control-side recovery.
pub struct StreamRevisionMismatch {
    revision: Arc<PreparedRevision>,
    catalog: CatalogLease,
    source: StreamSourceKey,
}

impl fmt::Debug for VerifiedStreamRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedStreamRevision")
            .field(
                "streamed_bindings",
                &self.revision.streamed_sources().count(),
            )
            .finish_non_exhaustive()
    }
}

impl VerifiedStreamRevision {
    /// Validate complete metadata membership, including fingerprint, generation,
    /// rates, frame counts and conversion version. No allocation, cloning,
    /// worker mutation or owner drop occurs while checking the borrowed graph.
    pub fn new(
        revision: Arc<PreparedRevision>,
        catalog: CatalogLease,
    ) -> Result<Self, StreamRevisionMismatch> {
        let missing = revision
            .streamed_sources()
            .find(|source| !catalog.covers_source(source))
            .map(|source| source.key());
        if let Some(source) = missing {
            return Err(StreamRevisionMismatch {
                revision,
                catalog,
                source,
            });
        }
        Ok(Self { revision, catalog })
    }

    /// Borrow the exact immutable graph; this creates no new Arc owner.
    pub fn revision(&self) -> &Arc<PreparedRevision> {
        &self.revision
    }

    /// Borrow its verified catalog without creating a lease.
    pub fn catalog(&self) -> &CatalogLease {
        &self.catalog
    }

    /// Actual owner identity, not equality of generation/source keys.
    pub fn same_catalog(&self, other: &Self) -> bool {
        self.catalog.same_catalog(&other.catalog)
    }

    /// Recover both owners on control; native endpoint admission is separate.
    pub fn into_parts(self) -> (Arc<PreparedRevision>, CatalogLease) {
        (self.revision, self.catalog)
    }
}

impl StreamRevisionMismatch {
    pub fn source(&self) -> StreamSourceKey {
        self.source
    }

    pub fn into_parts(self) -> (Arc<PreparedRevision>, CatalogLease) {
        (self.revision, self.catalog)
    }
}

impl fmt::Debug for StreamRevisionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamRevisionMismatch")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for StreamRevisionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("prepared graph source is absent from the verified catalog")
    }
}

impl std::error::Error for StreamRevisionMismatch {}
