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
        .try_fold(0usize, |n, value| n.checked_add(value?))
        .ok_or("constructor queue DATA overflow")?;
        self.candidate_root
            .configure_publications(reservation, queue_bytes)
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
