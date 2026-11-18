//! Control reservation and the same real publication/return rings. This owning
//! seam does not itself grant native installation or page readiness.
use super::*;
impl PublicationControl {
    /// Scalar submission observation; callback installation/ACK is separate.
    pub fn submitted_revision(&self) -> u64 {
        self.revision
    }
    /// A negative-only fault/closing observation, never admission authority.
    pub fn audio_candidate_claims_available(&self) -> bool {
        self.candidate_root.eligible_claims_open()
    }
    /// Reserve exact-demand rows per retained runtime before accepting any
    /// source domain or graph. Existing resident callers need not opt in.
    pub fn configure_audio_candidate_streaming(
        &mut self,
        extents_per_domain: usize,
    ) -> Result<(), &'static str> {
        if self.status.client_exposed.load(Acquire)
            || !self.native_custody
            || self.revision != 0
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
        {
            return Err("stream demand requires a vacant native channel");
        }
        self.candidate_root
            .configure_streaming(extents_per_domain)?;
        self.status.verified_streaming_mode.store(true, Release);
        Ok(())
    }

    /// Move the one exact page endpoint into this native channel. Queue pressure
    /// refuses before pair construction; successful late closure stays accepted
    /// in Root cleanup custody and never publishes behind terminal extraction.
    pub fn register_audio_candidate_domain_with_readiness(
        &mut self,
        domain: crate::candidate_catalog::CandidateDomain,
        reservation: hex_stream::StreamReadinessReservation,
    ) -> Result<crate::AudioCandidateDomainToken, Box<crate::AudioCandidateDomainRefusal>> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(Box::new(crate::AudioCandidateDomainRefusal {
                reason: "candidate native custody unavailable",
                domain,
            }));
        }
        let Ok(chunk) = self.publications.write_chunk_uninit(1) else {
            return Err(Box::new(crate::AudioCandidateDomainRefusal {
                reason: "candidate publication queue is full",
                domain,
            }));
        };
        self.candidate_root
            .register_domain_pages(domain, reservation, |pages| {
                chunk.fill_from_iter(std::iter::once(QueuedPublication::Domain(pages)));
            })
    }

    /// Reserve an existing N2 cell and its construction allowance before any
    /// source worker starts. Success means accepted custody even if close won;
    /// it does not mean the catalog is verified or pages can be rendered.
    pub fn begin_audio_candidate_domain_preparation(
        &mut self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
    ) -> Result<crate::AudioCandidateDomainToken, crate::AudioCandidateDomainStartError> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(crate::AudioCandidateDomainStartError::Busy(
                "candidate native custody unavailable",
            ));
        }
        self.candidate_root
            .begin_domain_preparation(config, definitions, readiness)
    }
    /// Move an already admitted granular bank into the same N2 reservation,
    /// pending constructor and readiness/publication handoff as raw sources.
    /// The combined quote covers this bank while retained here; its earlier
    /// control preparation and caller custody remain separate obligations.
    pub fn begin_audio_candidate_granular_domain_preparation(
        &mut self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        granular: hex_stream::GranularWorkerBank,
        readiness: hex_stream::StreamReadinessReservation,
    ) -> Result<crate::AudioCandidateDomainToken, crate::AudioCandidateDomainStartError> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(crate::AudioCandidateDomainStartError::Busy(
                "candidate native custody unavailable",
            ));
        }
        self.candidate_root.begin_granular_domain_preparation(
            config,
            definitions,
            granular,
            readiness,
        )
    }
    /// Test-only source-worker hold through the same reserved domain custody.
    #[cfg(feature = "stream-test-support")]
    pub fn begin_audio_candidate_domain_preparation_with_gate(
        &mut self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
        gate: hex_stream::test_support::PageGate,
    ) -> Result<crate::AudioCandidateDomainToken, crate::AudioCandidateDomainStartError> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(crate::AudioCandidateDomainStartError::Busy(
                "candidate native custody unavailable",
            ));
        }
        self.candidate_root
            .begin_domain_preparation_with_gate(config, definitions, readiness, gate)
    }
    /// Poll on the publication worker. Reserve the actual queue entry before
    /// sealing; a ready endpoint cannot escape or be dropped on queue refusal.
    pub fn poll_audio_candidate_domain_preparation(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> Result<crate::AudioCandidateDomainPreparationProgress, &'static str> {
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "candidate publication queue is full")?;
        self.candidate_root.poll_domain_preparation(token, |pages| {
            chunk.fill_from_iter(std::iter::once(QueuedPublication::Domain(pages)));
        })
    }

    /// Exact accepted, nonretiring sealed catalog association. This remains
    /// meaningful for source-free successors; it is not page readiness.
    pub fn audio_candidate_domain_is_sealed(
        &self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.candidate_root.domain_is_sealed(token)
    }
    /// Factual control-side cache eviction hint. This does not retire a domain,
    /// grant source access, or establish callback/page readiness. Reuse and
    /// retirement still validate the actual Root cell and incarnation.
    pub fn audio_candidate_domain_has_no_graph_users(
        &self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.candidate_root.domain_has_no_graph_users(token)
    }
    /// Factual delivery receipt for a nonretiring sealed domain's one exact
    /// endpoint. Ready/catalog sealing alone does not establish this receipt.
    /// This is neither page readiness nor publication authority; every render
    /// still requires exact protected extents from the actual retained loan.
    pub fn audio_candidate_domain_endpoint_is_installed(
        &self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.candidate_root.domain_endpoint_installed(token)
    }
    /// Borrowed worker-side coverage check against the actual retained sealed
    /// catalog and exact Root slot/incarnation. Neutral file facts alone cannot
    /// make an unrelated or already-retiring token cover a source.
    pub fn audio_candidate_domain_covers_source(
        &self,
        token: crate::AudioCandidateDomainToken,
        source: &hex_engine::StreamedSourceMetadata,
    ) -> bool {
        self.candidate_root.domain_covers_source(token, source)
    }

    /// Reserve the opted-in constructor Root before its first accepted owner.
    /// The existing channel must be vacant; legacy publication stays unchanged
    /// on channels that do not opt in.
    pub fn configure_audio_candidate_publication(
        &mut self,
        reservation: crate::AudioCandidatePublicationReservation,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.revision != 0
            || !self.status.preview_unused()
            || !self.resident_forwarding_virgin()
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
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
            Some(self.command_storage_bytes),
        ]
        .into_iter()
        .try_fold(native_bytes, |n, value| n.checked_add(value?))
        .ok_or("constructor queue DATA overflow")?;
        self.candidate_root
            .configure_publications(reservation, queue_bytes)?;
        // Closed constructor claims never adopt the independent owning browser
        // preview lane. The virgin predicate proves shutdown drops no PCM here.
        self.status.constructor_mode.store(true, Release);

        Ok(())
    }
    /// Reserve persistent graph construction in one existing runtime/archive
    /// cell before copying source rows or constructing a bank. The returned
    /// scalar token grants neither publication nor a no-install receipt.
    pub fn reserve_audio_candidate_preparation(
        &mut self,
        reservation: crate::AudioCandidatePreparationReservation,
        domains: Vec<crate::AudioCandidateDomainToken>,
    ) -> Result<crate::AudioCandidatePreparationToken, crate::AudioCandidatePreparationRefusal>
    {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(crate::AudioCandidatePreparationRefusal {
                reason: "candidate native custody unavailable",
                domains,
            });
        }
        self.candidate_root
            .reserve_runtime_preparation(reservation, domains)
    }
    /// Actual configured Root/control backing, independent of its DATA ceiling.
    /// Native sources require the exact accepted unsplit owner inventory; busy,
    /// pending or retiring source inspection returns None. This grants no
    /// callback, native-output or graph publication authority.
    pub fn audio_candidate_control_backing_bytes(&self) -> Option<usize> {
        self.candidate_root.actual_control_backing_bytes()
    }
    /// Reserve an empty destination for an intact preflighted resident CURRENT
    /// cell. No duplicate engine cell/source allocation is constructed.
    pub fn reserve_audio_candidate_adoption(
        &mut self,
        reservation: crate::AudioCandidatePreparationReservation,
    ) -> Result<crate::AudioCandidatePreparationToken, crate::AudioCandidatePreparationRefusal>
    {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err(crate::AudioCandidatePreparationRefusal {
                reason: "candidate native custody unavailable",
                domains: Vec::new(),
            });
        }
        self.candidate_root.reserve_runtime_adoption(reservation)
    }
    /// Move only after destination Root/context/rate and actual constructor DATA
    /// have passed. A refusal leaves the caller's original opaque Option intact.
    pub fn adopt_audio_candidate_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        context: hex_engine::AudioSourceContext,
        cell: &mut Option<hex_engine::AudioCandidatePreparation>,
    ) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("candidate native custody unavailable");
        }
        self.candidate_root
            .adopt_runtime_preparation(token, context, self.rate, cell)
    }
    /// Bind a newly ready exact domain into the already-reserved graph cell.
    /// This permits reserving graph construction BEFORE starting its source
    /// worker; no second runtime slot or source registration is introduced.
    pub fn bind_audio_candidate_preparation_domains(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        domains: Vec<crate::AudioCandidateDomainToken>,
    ) -> Result<(), crate::AudioCandidatePreparationRefusal> {
        self.candidate_root
            .bind_runtime_preparation_domains(token, domains)
    }
    /// The closure executes on the worker while dedicated construction custody
    /// remains rooted. No ordinary bank/terminal lock spans copies or compilation.
    pub fn prepare_audio_candidate_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<crate::AudioCandidatePreparationProgress, &'static str> {
        self.candidate_root.with_runtime_preparation(token, prepare)
    }
    /// Recheck both canonical counters for the limited CURRENT path before the
    /// ordinary destination seal/stage. No generation or editor relabel occurs.
    pub fn stage_current_sampler_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        expected: hex_engine::AudioSourceContext,
    ) -> Result<crate::AudioCandidateTicket, &'static str> {
        self.candidate_root
            .check_current_sampler_preparation_context(token, expected)?;
        self.stage_audio_candidate_preparation(token, expected.project_generation)
    }
    /// Queue this exact rooted, completed preparation without returning an owner
    /// to the adapter or releasing/reacquiring its reserved runtime/archive cell.
    /// Busy/refusal retains all construction custody under the same token.
    pub fn stage_audio_candidate_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        context_generation: u64,
    ) -> Result<crate::AudioCandidateTicket, &'static str> {
        self.stage_audio_candidate_preparation_inner(token, context_generation, None)
    }
    /// Publish the successor of this exact native Stop gate. Root validates the
    /// prepared owner's unchanged canonical context under its original custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn stage_native_epoch_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        fence: StoppedEditFence,
    ) -> Result<crate::AudioCandidateTicket, &'static str> {
        let context = self
            .status
            .sampler_edit
            .native_epoch_context(fence)
            .filter(|_| self.stopped_edit_fence_state(fence) == StoppedEditFenceState::Ready)
            .ok_or("native epoch fence is not ready")?;
        if self.status.applied_audio_source().is_some() {
            return Err("native epoch old runtime has not returned");
        }
        self.stage_audio_candidate_preparation_inner(
            token,
            context.project_generation,
            Some(context),
        )
    }
    fn stage_audio_candidate_preparation_inner(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        context_generation: u64,
        expected_context: Option<hex_engine::AudioSourceContext>,
    ) -> Result<crate::AudioCandidateTicket, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("candidate native custody unavailable");
        }
        if self.status.sampler_edit.native_epoch_active() && expected_context.is_none() {
            return Err("native epoch publication requires its exact ready fence");
        }
        if self.revision == u64::MAX {
            return Err("playback revision exhausted");
        }
        let reserved = if expected_context.is_some() {
            self.status
                .publication_pending
                .reserve_native_epoch_publication()
        } else {
            self.status.publication_pending.reserve_publication()
        };
        if !reserved {
            return Err("playback publication already pending");
        }
        let Ok(chunk) = self.publications.write_chunk_uninit(1) else {
            self.status.publication_pending.store(false, Release);
            return Err("candidate publication queue is full");
        };
        let revision = self.revision + 1;
        let header = (
            revision,
            context_generation,
            PublicationPolicy::InheritRuntime,
        );
        match self.candidate_root.stage_runtime_preparation(
            token,
            self.rate,
            header,
            expected_context,
            |queued| {
                chunk.fill_from_iter(std::iter::once(QueuedPublication::Eligible(queued)));
            },
        ) {
            Ok(ticket) => {
                self.revision = revision;
                self.status.submitted_revision.store(revision, Release);
                Ok(ticket)
            }
            Err(reason) => {
                self.status.publication_pending.store(false, Release);
                Err(reason)
            }
        }
    }
    pub fn cancel_audio_candidate_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
    ) -> bool {
        self.candidate_root.cancel_runtime_preparation(token)
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
        let reason = if !self.native_custody || self.status.closing.load(Acquire) {
            Some("candidate native custody unavailable")
        } else if self.status.sampler_edit.native_epoch_active() {
            Some("native epoch publication requires its exact ready fence")
        } else if owner.sample_rate() != self.rate
            || owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
        {
            Some("candidate constructor rate or phase differs")
        } else if policy != PublicationPolicy::InheritRuntime {
            Some("constructor fresh-runtime policy is not supported")
        } else if self.status.publication_pending.recording_active() {
            Some("finish recording before replacing playback")
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
        let summary = match self
            .candidate_root
            .seal_publication(&mut owner, domains.capacity())
        {
            Ok(summary) => summary,
            Err(reason) => {
                return Err(crate::AudioCandidateEligibleStageRefusal {
                    reason,
                    owner,
                    domains,
                });
            }
        };
        if !self.status.publication_pending.reserve_publication() {
            return Err(crate::AudioCandidateEligibleStageRefusal {
                reason: "playback publication already pending",
                owner,
                domains,
            });
        }
        let Ok(chunk) = self.publications.write_chunk_uninit(1) else {
            self.status.publication_pending.store(false, Release);
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
                self.status.submitted_revision.store(revision, Release);
                Ok(ticket)
            }
            Err(refused) => {
                self.status.publication_pending.store(false, Release);
                Err(refused)
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests;

impl Callback {
    pub(super) fn return_requested_domain_pages(&mut self) {
        for index in 0..self.domain_pages.len() {
            if self.held_retirement.is_some() || self.retired.is_full() {
                return;
            }
            if self.domain_pages[index]
                .as_ref()
                .is_some_and(|pages| pages.return_requested())
                && let Some(pages) = self.domain_pages[index].take()
            {
                self.return_publication(ReturnedPublication::Domain(pages));
            }
        }
    }

    pub(super) fn advance_eligible_publication(&mut self, frames: Option<usize>) {
        if frames.is_none()
            && self
                .deferred_eligible
                .as_ref()
                .is_some_and(|next| next.streaming_enabled())
        {
            return;
        }
        // A new actual candidate invalidates the old owner's discontinuity;
        // its scalar token cannot be replayed on the successor.
        self.cancel_active_stream_action();
        if let Some(ActivePublication::Eligible(active)) = self.active.as_mut() {
            active.publication_replaces_prewarm(&mut self.domain_pages);
        }
        use crate::candidate_custody::InstallDecision;
        if self.active.is_some() && self.retired.is_full() {
            return;
        }
        let Some(next) = self.deferred_eligible.as_ref() else {
            return;
        };
        let revision = next.revision();
        let context = next.diagnostic_context();
        let streaming_enabled = next.streaming_enabled();
        let incompatible_active = self
            .active
            .as_ref()
            .is_some_and(|active| !matches!(active, ActivePublication::Eligible(_)));
        let preflight_refusal = next.preflight_refusal(&self.candidate_root);
        self.trace_audio(
            AudioDiagnosticKind::PublicationAttempt,
            revision,
            context,
            frames.unwrap_or(0),
        );
        if streaming_enabled
            && !incompatible_active
            && let Some(reason) = preflight_refusal
        {
            self.reject_pending_eligible_successor(reason);
            return;
        }
        if let Some(frames) = frames
            && streaming_enabled
            && !incompatible_active
        {
            let mut reservation = self.reserve_current_stream_window(frames);
            if matches!(
                reservation,
                Err(crate::candidate_custody::QuantumFailure::Capacity)
            ) {
                if let Some(next) = self.deferred_eligible.as_mut() {
                    next.cancel_stream_readiness(&mut self.domain_pages);
                }
                reservation = self.reserve_current_stream_window(frames);
            }
            match reservation {
                Ok(Some(pins)) => {
                    let complete = pins.complete();
                    self.current_quantum_pins = Some(pins);
                    if !complete {
                        if let Some(next) = self.deferred_eligible.as_mut() {
                            next.cancel_stream_readiness(&mut self.domain_pages);
                        }
                        return;
                    }
                }
                Ok(None) => {}
                Err(crate::candidate_custody::QuantumFailure::PendingAction) => return,
                Err(crate::candidate_custody::QuantumFailure::Capacity) => {
                    if !self.retry_pending_eligible_capacity() {
                        self.reject_pending_eligible_successor(
                            PlaybackHandoffRejection::StreamedPageCapacity,
                        );
                    }
                    return;
                }
                Err(crate::candidate_custody::QuantumFailure::Invalid) => {
                    self.reject_pending_eligible_successor(
                        PlaybackHandoffRejection::StreamedPagesNotReady,
                    );
                    return;
                }
            }
        }
        let mut decision = if incompatible_active {
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
            let mut request = frames.map(|frames| crate::candidate_custody::StreamInstall {
                domains: &mut self.domain_pages,
                pending_play: self.stream_pending_play.is_some(),
                command_prefix: self.stream_prefix,
                frames,
                fenced_restart: false,
                foreign_continuation: false,
            });
            next.prepare_install(previous, &self.candidate_root, request.as_mut())
        } else {
            return;
        };
        if let Some(handoff) = self
            .deferred_eligible
            .as_mut()
            .and_then(|next| next.take_handoff_diagnostic())
        {
            if matches!(
                decision,
                InstallDecision::Refused(PlaybackHandoffRejection::StreamedPagesNotReady)
            ) {
                decision = InstallDecision::Refused(PlaybackHandoffRejection::from_error(handoff));
            }
            self.trace_audio(
                AudioDiagnosticKind::StreamPreflightFailed {
                    detail: "stream handoff demand refused",
                    handoff: Some(handoff),
                },
                revision,
                context,
                frames.unwrap_or(0),
            );
        }
        match decision {
            InstallDecision::Retry(reason) => {
                self.trace_audio(
                    AudioDiagnosticKind::PublicationWaiting { reason },
                    revision,
                    context,
                    frames.unwrap_or(0),
                );
                if self.active.is_some()
                    && let Some(next) = self.deferred_eligible.as_mut()
                {
                    next.cancel_stream_readiness(&mut self.domain_pages);
                }
            }
            InstallDecision::Refused(reason) => {
                if let Some(mut value) = self.deferred_eligible.take() {
                    value.cancel_stream_readiness(&mut self.domain_pages);
                    self.reject_publication(value.revision(), reason);
                    self.return_publication(ReturnedPublication::Eligible(
                        value.reject_before_install(),
                    ));
                    self.status.publication_pending.store(false, Release);
                    if let Some(ticket) = self.stream_pending_play.take() {
                        self.status.publish_rejected_action(
                            ticket,
                            PlaybackActionRejection::SessionNotPrepared,
                        );
                        self.stage_stream_outcome(
                            ticket,
                            action_outcomes::Outcome::Rejected(
                                PlaybackActionRejection::SessionNotPrepared,
                            ),
                        );
                        self.status
                            .stream_applied_command_prefix
                            .store(self.stream_prefix, Release);
                    }
                }
            }
            InstallDecision::Ready => {
                self.trace_audio(
                    AudioDiagnosticKind::PublicationPrepared,
                    revision,
                    context,
                    frames.unwrap_or(0),
                );
                // Every fallible operation happened while both State Boxes
                // remained in their persistent callback slots. Fixed moves only.
                if let Some(next) = self.deferred_eligible.take() {
                    let revision = next.revision();
                    let previous = self
                        .active
                        .replace(ActivePublication::Eligible(next.into_active()));
                    self.release_current_quantum_pins();
                    if let Some(ActivePublication::Eligible(mut previous)) = previous {
                        if let Some(ActivePublication::Eligible(active)) = self.active.as_mut()
                            && let Some(ticket) = active.inherit_schedule_observation(&mut previous)
                        {
                            self.status
                                .stream_invalidated_schedule
                                .store(ticket, Release);
                        }
                        previous.clear_stream_roles(&mut self.domain_pages);
                        self.return_publication(ReturnedPublication::Eligible(
                            previous.return_replaced(),
                        ));
                    }
                    self.finish_pending_stream_play();
                    self.status.stream_buffering.store(false, Release);
                    self.publish_active_routes(true);
                    self.recovery_seek_pending = None;
                    if !self.status.device_fault.load(Acquire) {
                        self.recovery_position_frozen = false;
                    }
                    self.publish_transport();
                    self.status.render_fault_record.clear();
                    self.status.render_fault.store(false, Release);
                    self.status.stream_source_changed.store(false, Release);
                    self.status.stream_read_failed.store(false, Release);
                    self.status
                        .applied_source
                        .publish(match self.active.as_ref() {
                            Some(ActivePublication::Eligible(active)) => {
                                active.applied_audio_source()
                            }
                            _ => None,
                        });
                    self.status.applied_revision.store(revision, Release);
                    self.trace_audio(
                        AudioDiagnosticKind::PublicationApplied,
                        revision,
                        context,
                        frames.unwrap_or(0),
                    );
                    self.status.publication_pending.store(false, Release);
                }
            }
        }
    }
}
