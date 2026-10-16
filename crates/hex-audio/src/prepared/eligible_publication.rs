//! Control reservation and the same real publication/return rings. This owning
//! seam does not itself grant native installation or page readiness.
use super::*;
impl Control {
    /// Reserve the opted-in constructor Root before its first accepted owner.
    /// The existing channel must be vacant; legacy publication stays unchanged
    /// on channels that do not opt in.
    pub fn configure_audio_candidate_publication(
        &mut self,
        reservation: crate::AudioCandidatePublicationReservation,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.revision != 0
            || self.closing.load(Acquire)
            || self.publication_pending.load(Acquire)
            || self.held_control_return.is_some()
            || self.latest_fresh_runtime_cancellation.is_some()
        {
            return Err("constructor publication requires a vacant native channel");
        }
        let native_bytes = self
            .candidate_root
            .native_boundary_bytes()
            .ok_or("constructor native carriers lack exact retained terminal custody")?;
        let returned = self
            .retired
            .as_ref()
            .ok_or("native return endpoint missing")?;
        // Pinned rtrb allocates exactly capacity elements and a shared RingBuffer
        // payload. Its opaque Arc control blocks remain bounded RESOURCE groups.
        let queue_bytes = [
            self.publications
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<QueuedPublication>())
                .and_then(|n| n.checked_add(std::mem::size_of_val(self.publications.buffer()))),
            returned
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<ReturnedPublication>())
                .and_then(|n| n.checked_add(std::mem::size_of_val(returned.buffer()))),
            self.commands
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<QueuedCommand>())
                .and_then(|n| n.checked_add(std::mem::size_of_val(self.commands.buffer()))),
        ]
        .into_iter()
        .try_fold(native_bytes, |n, value| n.checked_add(value?))
        .ok_or("constructor queue DATA overflow")?;
        self.candidate_root
            .configure_publications(reservation, queue_bytes)
    }
    /// Cancel this exact retained slot/incarnation before its callback apply
    /// decision. No owning handle or per-call allocation escapes the Root.
    pub fn cancel_audio_candidate(
        &self,
        ticket: &crate::AudioCandidateTicket,
    ) -> crate::AudioCandidateCancellationResult {
        self.candidate_root.cancel_eligible(ticket)
    }
    /// Queue acceptance is distinct from installation and its applied ACK.
    /// Every synchronous refusal returns the original owner and domain Vec;
    /// after acceptance the Root retains anchors through physical disposal and
    /// original-bank report consumption, including abandoned Control custody.
    pub fn stage_eligible_audio_candidate(
        &mut self,
        mut owner: hex_engine::AudioCandidateOwner,
        domains: Vec<crate::AudioCandidateDomainToken>,
        policy: PublicationPolicy,
        context_generation: u64,
    ) -> Result<crate::AudioCandidateTicket, crate::AudioCandidateEligibleStageRefusal> {
        let reason = if !self.native_custody || self.closing.load(Acquire) {
            Some("candidate native custody unavailable")
        } else if owner.sample_rate() != self.rate
            || owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
        {
            Some("candidate constructor rate or phase differs")
        } else if policy != PublicationPolicy::InheritRuntime {
            Some("constructor fresh-runtime policy is not supported")
        } else if self.gain_recording.target().is_some() {
            Some("finish gain recording before replacing playback")
        } else if self.revision == u64::MAX {
            Some("playback revision exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(crate::AudioCandidateEligibleStageRefusal {
                reason,
                owner,
                domains,
            });
        }
        let summary = match self.candidate_root.seal_publication(&mut owner) {
            Ok(summary) => summary,
            Err(reason) => {
                return Err(crate::AudioCandidateEligibleStageRefusal {
                    reason,
                    owner,
                    domains,
                });
            }
        };
        if self
            .publication_pending
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
            return Err(crate::AudioCandidateEligibleStageRefusal {
                reason: "playback publication already pending",
                owner,
                domains,
            });
        }
        let Ok(chunk) = self.publications.write_chunk_uninit(1) else {
            self.publication_pending.store(false, Release);
            return Err(crate::AudioCandidateEligibleStageRefusal {
                reason: "candidate publication queue is full",
                owner,
                domains,
            });
        };
        let revision = self.revision + 1;
        // The actual ring slot is already reserved; fill is nonfallible. Root
        // holds the same terminal barrier through split and this final move.
        match self.candidate_root.stage_eligible(
            owner,
            domains,
            (revision, context_generation, policy),
            summary,
            |queued| {
                chunk.fill_from_iter(std::iter::once(QueuedPublication::Eligible(queued)));
            },
        ) {
            Ok(ticket) => {
                self.revision = revision;
                Ok(ticket)
            }
            Err(refused) => {
                self.publication_pending.store(false, Release);
                Err(refused)
            }
        }
    }
}

#[cfg(test)]
mod tests;

impl Callback {
    pub(super) fn advance_eligible_publication(&mut self) {
        use crate::candidate_custody::InstallDecision;
        if self.active.is_some() && self.retired.is_full() {
            return;
        }
        let decision = if self
            .active
            .as_ref()
            .is_some_and(|active| !matches!(active, ActivePublication::Eligible(_)))
        {
            InstallDecision::Refused(PlaybackHandoffRejection::ConstructorClaimsUnavailable)
        } else if let Some(next) = self.deferred_eligible.as_mut() {
            #[cfg(all(test, feature = "allocation-probe"))]
            if let Some(bytes) = self.test_eligible_transfer_allowance.take() {
                let _ = next.test_set_transfer_allowance(bytes);
            }
            let previous = match self.active.as_mut() {
                Some(ActivePublication::Eligible(active)) => Some(active),
                _ => None,
            };
            next.prepare_install(previous, &self.candidate_root)
        } else {
            return;
        };
        match decision {
            InstallDecision::Retry => {}
            InstallDecision::Refused(reason) => {
                if let Some(value) = self.deferred_eligible.take() {
                    self.status
                        .publish_rejected_revision(value.revision(), reason);
                    self.return_publication(ReturnedPublication::Eligible(
                        value.reject_before_install(),
                    ));
                    self.publication_pending.store(false, Release);
                }
            }
            InstallDecision::Ready => {
                // Every fallible operation happened while both State Boxes
                // remained in their persistent callback slots. Fixed moves only.
                if let Some(next) = self.deferred_eligible.take() {
                    let revision = next.revision();
                    let previous = self
                        .active
                        .replace(ActivePublication::Eligible(next.into_active()));
                    if let Some(ActivePublication::Eligible(previous)) = previous {
                        self.return_publication(ReturnedPublication::Eligible(
                            previous.return_replaced(),
                        ));
                    }
                    self.publish_active_routes(true);
                    self.publish_transport();
                    self.status.render_fault_record.clear();
                    self.status.render_fault.store(false, Release);
                    self.status.applied_revision.store(revision, Release);
                    self.publication_pending.store(false, Release);
                }
            }
        }
    }
}
