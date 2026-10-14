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
