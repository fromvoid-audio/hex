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

/// Sealed engine root/catalog pairing for the opt-in owned metadata path.
/// This is control verification only, not page readiness or native admission.
/// No raw graph owner, implicit Clone, or callback-final reclamation is exposed.
pub struct VerifiedOwnedStreamRevision {
    revision: hex_engine::OwnedPreparedRevision,
    catalog: CatalogLease,
}
pub struct OwnedStreamRevisionMismatch {
    revision: hex_engine::OwnedPreparedRevision,
    catalog: CatalogLease,
    source: StreamSourceKey,
}
impl VerifiedOwnedStreamRevision {
    pub fn new(
        revision: hex_engine::OwnedPreparedRevision,
        catalog: CatalogLease,
    ) -> Result<Self, OwnedStreamRevisionMismatch> {
        let missing = revision
            .metadata()
            .streamed_sources()
            .find(|source| !catalog.covers_source(source))
            .map(|source| source.key());
        if let Some(source) = missing {
            return Err(OwnedStreamRevisionMismatch {
                revision,
                catalog,
                source,
            });
        }
        Ok(Self { revision, catalog })
    }
    pub(crate) fn catalog_on_control(&self) -> &CatalogLease {
        &self.catalog
    }
    pub fn metadata(&self) -> hex_engine::OwnedPreparedRevisionView<'_> {
        self.revision.metadata()
    }
    pub fn matches_playback(&self, playback: &hex_engine::playback::Playback) -> bool {
        playback.same_prepared_owner(&self.revision)
    }
    pub fn matches_claims(&self, claims: &hex_engine::storage_report::PlaybackClaims) -> bool {
        claims.same_prepared_owner(&self.revision)
    }
    pub fn matches_tracked_owner(
        &self,
        owner: &hex_engine::storage_report::OwnerTrackedPlayback,
    ) -> bool {
        owner.same_prepared_owner(&self.revision)
    }
    pub fn matches_claimed_owner(
        &self,
        owner: &hex_engine::storage_report::ClaimedPlaybackOwner,
    ) -> bool {
        owner.same_prepared_owner(&self.revision)
    }
    pub fn same_catalog(&self, other: &Self) -> bool {
        self.catalog.same_catalog(&other.catalog)
    }
    pub fn into_parts(self) -> (hex_engine::OwnedPreparedRevision, CatalogLease) {
        (self.revision, self.catalog)
    }
}
impl OwnedStreamRevisionMismatch {
    pub fn source(&self) -> StreamSourceKey {
        self.source
    }
    pub fn into_parts(self) -> (hex_engine::OwnedPreparedRevision, CatalogLease) {
        (self.revision, self.catalog)
    }
}
impl fmt::Debug for VerifiedOwnedStreamRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedOwnedStreamRevision")
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for OwnedStreamRevisionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedStreamRevisionMismatch")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
