//! Ordinary graph publication composes registered resources with the existing
//! source Root. The per-graph demand backing belongs to the ordinary graph's
//! finite metadata allowance; source workers and page loans keep their N2 owner.
use super::*;
impl PublicationControl {
    /// Configure after ordinary device-resource publication and before client
    /// exposure. The caller must include max_metadata_bytes per admitted graph
    /// in its existing ordinary metadata budget. This is source custody only;
    /// actual protected demand still gates callback installation and rendering.
    /// Every subsequent graph uses the captured-source ordinary worker entry,
    /// including source-free successors that inherit scheduled route observations.
    /// Inherited publications require the same captured project generation;
    /// document replacement uses an explicit fresh installation policy.
    pub fn configure_ordinary_streaming_on_control(
        &mut self,
        domains: crate::AudioCandidateDomainReservation,
        extents_per_domain: usize,
        max_metadata_bytes: usize,
    ) -> Result<(), &'static str> {
        if self.device_resources.is_none()
            || self.status.client_exposed.load(Acquire)
            || !self.native_custody
            || self.revision != 0
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
        {
            return Err("ordinary streams require a vacant ordinary native output");
        }
        self.candidate_root.configure_ordinary_streams(
            domains,
            extents_per_domain,
            max_metadata_bytes,
        )
    }
}
impl Publication {
    pub(crate) fn release_streams_on_control(
        &mut self,
        root: &crate::candidate_custody::CandidateRoot,
    ) {
        if let Some(streams) = self.streams.take() {
            root.release_ordinary_streams(streams);
        }
    }
    pub(super) fn stream_view(&mut self) -> Option<crate::candidate_custody::StreamActiveView<'_>> {
        Some(self.streams.as_mut()?.view(&mut self.playback))
    }
}
impl ActivePublication {
    pub(super) fn streaming_enabled(&self) -> bool {
        match self {
            Self::Legacy(p) => p.streams.is_some(),
            Self::Eligible(p) => p.streaming_enabled(),
            Self::Candidate(_) => false,
        }
    }
    pub(super) fn has_stream_action(&self) -> bool {
        match self {
            Self::Legacy(p) => p.streams.as_ref().is_some_and(|s| s.has_action()),
            Self::Eligible(p) => p.has_stream_action(),
            Self::Candidate(_) => false,
        }
    }
    pub(super) fn stream_view(&mut self) -> Option<crate::candidate_custody::StreamActiveView<'_>> {
        match self {
            Self::Legacy(p) => p.stream_view(),
            Self::Eligible(p) if p.streaming_enabled() => Some(p.stream_view()),
            _ => None,
        }
    }
}
