//! Control-only owning pair. Not a publisher, readiness receipt or runtime policy.
use hex_engine::storage_report::PlaybackClaims;
use hex_stream::VerifiedOwnedStreamRevision;

/// Claims and verifier retain the same actual engine root. No Clone, mutable
/// Playback access, implicit native publication or callback disposal is exposed.
pub struct PublicationClaims {
    claims: PlaybackClaims,
    verified: VerifiedOwnedStreamRevision,
}
pub struct PublicationClaimsMismatch {
    pub claims: PlaybackClaims,
    pub verified: VerifiedOwnedStreamRevision,
}
impl PublicationClaims {
    /// Control-only mismatch recovery returns both owners unchanged. The error
    /// Box uses existing control OOM/abort semantics, not callback allocation.
    pub fn new(
        claims: PlaybackClaims,
        verified: VerifiedOwnedStreamRevision,
    ) -> Result<Self, Box<PublicationClaimsMismatch>> {
        if !verified.matches_claims(&claims) {
            return Err(Box::new(PublicationClaimsMismatch { claims, verified }));
        }
        Ok(Self { claims, verified })
    }
    /// Solely control disposal/recovery; this is not a publication acceptance.
    pub fn into_parts_on_control(self) -> (PlaybackClaims, VerifiedOwnedStreamRevision) {
        (self.claims, self.verified)
    }
}

/// Archived graph/catalog owners. The sealed runtime owner may move separately;
/// this half remains in bounded control/lifecycle custody. No native receipt.
pub struct PublicationAnchors {
    pub(crate) claims: hex_engine::storage_report::AnchoredPlaybackClaims,
    pub(crate) verified: VerifiedOwnedStreamRevision,
}
impl PublicationClaims {
    pub fn split_on_control(
        self,
    ) -> Result<
        (
            hex_engine::storage_report::ClaimedPlaybackOwner,
            PublicationAnchors,
        ),
        Box<Self>,
    > {
        let Self { claims, verified } = self;
        match claims.split_on_control() {
            Ok((owner, claims)) => Ok((owner, PublicationAnchors { claims, verified })),
            Err(claims) => Err(Box::new(Self {
                claims: *claims,
                verified,
            })),
        }
    }
}
impl PublicationAnchors {
    pub fn matches_tracked_owner(
        &self,
        owner: &hex_engine::storage_report::OwnerTrackedPlayback,
    ) -> bool {
        owner.matches_anchors(&self.claims) && self.verified.matches_tracked_owner(owner)
    }

    pub fn matches_owner(&self, owner: &hex_engine::storage_report::ClaimedPlaybackOwner) -> bool {
        self.claims.matches_owner(owner) && self.verified.matches_claimed_owner(owner)
    }
}

#[path = "publication_claims/slots.rs"]
pub(crate) mod slots;

pub use slots::{
    ClaimAssociation, ClaimReceipt, ClaimReclamation, ClaimReturnedOwnerRefusal, ClaimSlotError,
    ClaimStageRefusal, ClaimUnionCapacityReport, Fact as ClaimFactKind,
};

/// Owning post-disposal refusal: original anchors and physical reservation retained.
pub struct CatalogAnchorCommitRefusal {
    pub reason: ClaimSlotError,
    pub anchors: PublicationAnchors,
    pub reservation: hex_stream::CatalogCustodyReservation,
}
pub struct CatalogCoverageAnchorCommitRefusal {
    pub reason: ClaimSlotError,
    pub anchors: PublicationAnchors,
    pub reservation: hex_stream::CatalogCoverageReservation,
}
impl PublicationAnchors {
    /// Actual native accepted/final-transfer + owner-group closure authority is
    /// not integrated yet. Physical stream coverage alone cannot bypass that
    /// gate or complete Unknown aggregate ownership. Return BOTH original inputs.
    pub fn commit_catalog_coverage_on_control(
        self,
        reservation: hex_stream::CatalogCoverageReservation,
        _reaper: &mut hex_stream::MediaDomainReaper,
    ) -> Result<
        (
            hex_engine::storage_report::AnchoredPlaybackClaims,
            hex_engine::OwnedPreparedRevision,
            hex_stream::CatalogCoveragePending,
        ),
        Box<CatalogCoverageAnchorCommitRefusal>,
    > {
        Err(Box::new(CatalogCoverageAnchorCommitRefusal {
            reason: ClaimSlotError::Incomplete,
            anchors: self,
            reservation,
        }))
    }
    /// No authoritative disposal/final-transfer proof exists yet. Never reconstruct
    /// PlaybackClaims or a disposed runtime, and never mint a completion assertion.
    pub fn commit_catalog_custody_on_control(
        self,
        reservation: hex_stream::CatalogCustodyReservation,
        _reaper: &mut hex_stream::MediaDomainReaper,
    ) -> Result<
        (
            hex_engine::storage_report::AnchoredPlaybackClaims,
            hex_engine::OwnedPreparedRevision,
            hex_stream::CatalogCustodyAwaiter,
        ),
        Box<CatalogAnchorCommitRefusal>,
    > {
        Err(Box::new(CatalogAnchorCommitRefusal {
            reason: ClaimSlotError::Incomplete,
            anchors: self,
            reservation,
        }))
    }
}
