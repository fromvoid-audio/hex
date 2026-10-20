//! Affine worker access, without exposing the owning endpoint payload.
use super::*;

/// Exclusive publication operations. Swapping loans never swaps endpoint owners.
/// Preparation closures receive nonextracting typed operations, not the child owner.
///
/// The concrete endpoint cannot be obtained through the loan.
/// ```compile_fail
/// use hex_audio::prepared::{PublicationControl, WorkerPublication};
/// fn extract<'a>(loan: &'a mut WorkerPublication<'_>) -> &'a mut PublicationControl { loan }
/// ```
/// Owning telemetry handles cannot escape.
/// ```compile_fail
/// use hex_audio::prepared::{Status, WorkerPublication};
/// fn extract(loan: &WorkerPublication<'_>) -> std::sync::Arc<Status> {
///     std::sync::Arc::clone(loan.status())
/// }
/// ```
/// Loans cannot be fabricated or duplicated.
/// ```compile_fail
/// use hex_audio::prepared::{PublicationControl, WorkerPublication};
/// fn fabricate(control: &mut PublicationControl) {
///     let _ = WorkerPublication { control };
/// }
/// ```
/// ```compile_fail
/// use hex_audio::prepared::WorkerPublication;
/// fn duplicate(loan: WorkerPublication<'_>) { let _ = loan.clone(); }
/// ```
/// The endpoint cannot close while its loan is still in use.
/// ```compile_fail
/// use hex_audio::prepared::WorkerControl;
/// fn close(worker: &mut WorkerControl) {
///     let loan = worker.control().unwrap();
///     worker.close();
///     let _ = loan.status();
/// }
/// ```
/// There is no generic callback lending the concrete endpoint.
/// ```compile_fail
/// use hex_audio::prepared::{PublicationControl, WorkerPublication};
/// fn extract(loan: &mut WorkerPublication<'_>) {
///     loan.with_control(|_: &mut PublicationControl| {});
/// }
/// ```
pub struct WorkerPublication<'a> {
    control: &'a mut PublicationControl,
}
impl<'a> WorkerPublication<'a> {
    pub(super) fn new(control: &'a mut PublicationControl) -> Self {
        Self { control }
    }
    pub fn status(&self) -> &Status {
        self.control.status()
    }
}
impl WorkerPublication<'_> {
    /// Closed pending Editor+paired-source construction. Model commit and a
    /// NEW current worker/backend validation still precede any publication.
    #[expect(clippy::too_many_arguments, reason = "finite actual pending operands")]
    pub fn construct_pending_original_sampler_with_host_on_control<
        'owner,
        P: hex_engine::sampler_host_fields::OriginalSamplerPendingSession<'owner>,
    >(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        worker: &hex_engine::sampler_host_fields::SamplerWorkerFields<'_>,
        current_source: &hex_engine::sampler_host_fields::CapturedSourceFields<'_>,
        files: &dyn hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        families: &dyn hex_engine::sampler_host_fields::SamplerFamilyInventoryFields,
        session: &mut P,
        previous: Option<&hex_engine::AudioCompiledRevision>,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<bool, hex_engine::AudioStorageError> {
        if !self.control.native_custody || self.control.status.closing.load(Acquire) {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        self.control
            .candidate_root
            .construct_pending_original_sampler_in_host(
                token,
                worker,
                current_source,
                files,
                families,
                session,
                previous,
                reservation,
            )
    }
    /// Closed construction in the SAME token-selected persistent parent.
    /// This returns neither owners nor native publication authority. Caller
    /// exclusions precede Audio's six single-try guards and end before DSP/IO.
    #[expect(clippy::too_many_arguments, reason = "finite mandatory host operands")]
    pub fn construct_original_sampler_with_host_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        worker: &hex_engine::sampler_host_fields::SamplerWorkerFields<'_>,
        source: &hex_engine::sampler_host_fields::CapturedSourceFields<'_>,
        files: &dyn hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        families: &dyn hex_engine::sampler_host_fields::SamplerFamilyInventoryFields,
        resident: &dyn hex_engine::sampler_host_fields::OriginalSamplerResidentFields,
        producer: &hex_engine::native_device_resources::OriginalResourceBankPlan<'_>,
        previous: Option<&hex_engine::AudioCompiledRevision>,
        original: &hex_engine::AudioRootStorageLedger<'_>,
        writers: &mut hex_engine::audio_transform::OriginalSamplerTransaction<'_>,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        if !self.control.native_custody || self.control.status.closing.load(Acquire) {
            return Err(hex_engine::AudioStorageError::SourceMismatch);
        }
        self.control
            .candidate_root
            .construct_original_sampler_in_host(
                token,
                worker,
                source,
                files,
                families,
                resident,
                producer,
                previous,
                original,
                writers,
                reservation,
            )
    }

    pub(crate) fn enable_native_custody(&mut self) -> Result<(), String> {
        self.control.enable_native_custody()
    }
    #[cfg(test)]
    pub(crate) fn native_candidate_root(&self) -> crate::candidate_custody::CandidateRoot {
        self.control.native_candidate_root()
    }

    pub fn output_identity(&self) -> Option<u64> {
        self.control.output_identity()
    }

    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        self.control.capture_fresh_runtime_fence()
    }

    pub fn publish(&mut self, playback: Playback) -> Result<u64, PublishError> {
        self.control.publish(playback)
    }

    pub fn publish_with_policy(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
    ) -> Result<u64, PublishError> {
        self.control.publish_with_policy(playback, policy)
    }

    pub fn publish_with_policy_and_context_generation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
    ) -> Result<u64, PublishError> {
        self.control.publish_with_policy_and_context_generation(
            playback,
            policy,
            control_context_generation,
        )
    }

    pub fn cancel_fresh_runtime_publication(
        &self,
        revision: u64,
    ) -> FreshRuntimeCancellationResult {
        self.control.cancel_fresh_runtime_publication(revision)
    }

    pub fn reclaim(&mut self) -> usize {
        self.control.reclaim()
    }

    pub fn configure_audio_candidate_domains(
        &mut self,
        reservation: crate::AudioCandidateDomainReservation,
    ) -> Result<(), &'static str> {
        self.control.configure_audio_candidate_domains(reservation)
    }

    pub fn register_audio_candidate_domain(
        &mut self,
        domain: crate::CandidateDomain,
    ) -> Result<crate::AudioCandidateDomainToken, Box<crate::AudioCandidateDomainRefusal>> {
        self.control.register_audio_candidate_domain(domain)
    }

    pub fn retire_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.control.retire_audio_candidate_domain(token)
    }

    pub fn recover_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> Option<hex_stream::PagePoolReport> {
        self.control.recover_audio_candidate_domain(token)
    }

    pub fn audio_candidate_domain_usage(&self) -> Option<crate::AudioCandidateDomainUsage> {
        self.control.audio_candidate_domain_usage()
    }

    pub fn stage_audio_candidate(
        &mut self,
        owner: hex_engine::AudioCandidateOwner,
        domains: Vec<crate::CandidateDomain>,
    ) -> Result<crate::AudioCandidateTicket, crate::AudioCandidateStageRefusal> {
        self.control.stage_audio_candidate(owner, domains)
    }

    pub fn recover_audio_candidate(
        &mut self,
        ticket: &crate::AudioCandidateTicket,
    ) -> Option<crate::AudioCandidateRecovery> {
        self.control.recover_audio_candidate(ticket)
    }

    pub fn audio_candidate_custody(&self) -> crate::AudioCandidateCustodyReport {
        self.control.audio_candidate_custody()
    }

    pub fn request_shutdown(&mut self) {
        self.control.request_shutdown()
    }

    pub fn submitted_revision(&self) -> u64 {
        self.control.submitted_revision()
    }

    pub fn audio_candidate_claims_available(&self) -> bool {
        self.control.audio_candidate_claims_available()
    }

    pub fn configure_audio_candidate_streaming(
        &mut self,
        extents_per_domain: usize,
    ) -> Result<(), &'static str> {
        self.control
            .configure_audio_candidate_streaming(extents_per_domain)
    }

    pub fn register_audio_candidate_domain_with_readiness(
        &mut self,
        domain: crate::candidate_catalog::CandidateDomain,
        reservation: hex_stream::StreamReadinessReservation,
    ) -> Result<crate::AudioCandidateDomainToken, Box<crate::AudioCandidateDomainRefusal>> {
        self.control
            .register_audio_candidate_domain_with_readiness(domain, reservation)
    }

    pub fn begin_audio_candidate_domain_preparation(
        &mut self,
        config: hex_stream::StreamConfig,
        definitions: Vec<hex_stream::SourceDefinition>,
        readiness: hex_stream::StreamReadinessReservation,
    ) -> Result<crate::AudioCandidateDomainToken, crate::AudioCandidateDomainStartError> {
        self.control
            .begin_audio_candidate_domain_preparation(config, definitions, readiness)
    }

    pub fn poll_audio_candidate_domain_preparation(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> Result<crate::AudioCandidateDomainPreparationProgress, &'static str> {
        self.control.poll_audio_candidate_domain_preparation(token)
    }

    pub fn audio_candidate_domain_is_sealed(
        &self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.control.audio_candidate_domain_is_sealed(token)
    }

    pub fn audio_candidate_domain_covers_source(
        &self,
        token: crate::AudioCandidateDomainToken,
        source: &hex_engine::StreamedSourceMetadata,
    ) -> bool {
        self.control
            .audio_candidate_domain_covers_source(token, source)
    }

    pub fn configure_audio_candidate_publication(
        &mut self,
        reservation: crate::AudioCandidatePublicationReservation,
    ) -> Result<(), &'static str> {
        self.control
            .configure_audio_candidate_publication(reservation)
    }

    pub fn reserve_audio_candidate_preparation(
        &mut self,
        reservation: crate::AudioCandidatePreparationReservation,
        domains: Vec<crate::AudioCandidateDomainToken>,
    ) -> Result<crate::AudioCandidatePreparationToken, crate::AudioCandidatePreparationRefusal>
    {
        self.control
            .reserve_audio_candidate_preparation(reservation, domains)
    }

    pub fn bind_audio_candidate_preparation_domains(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        domains: Vec<crate::AudioCandidateDomainToken>,
    ) -> Result<(), crate::AudioCandidatePreparationRefusal> {
        self.control
            .bind_audio_candidate_preparation_domains(token, domains)
    }

    /// Bind only this actual pre-close Main preparation. The returned affine
    /// continuation cannot perform later construction or audible publication.
    pub fn begin_captured_main_file_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        expected: hex_engine::AudioCandidateIdentity,
        frames: u64,
    ) -> Result<crate::CapturedMainFile, crate::CapturedMainFileError> {
        self.control.begin_captured_main_file_on_control(token, expected, frames)
    }
    /// Operate on the actual rooted child without exposing its owning value.
    /// The host retains reservation, cancellation, refusal and cleanup custody.
    /// A callback cannot retain the preparation loan beyond the guarded call.
    /// ```compile_fail
    /// use hex_audio::{AudioCandidatePreparationToken, prepared::WorkerPublication};
    /// use hex_engine::AudioCandidatePreparationOperations;
    /// fn escape<'a>(worker: &'a mut WorkerPublication<'_>, token: AudioCandidatePreparationToken) -> Option<AudioCandidatePreparationOperations<'a>> {
    ///     let mut escaped = None;
    ///     let _ = worker.prepare_audio_candidate_on_control(token, |operations| { escaped = Some(operations); Ok(()) });
    ///     escaped
    /// }
    /// ```
    pub fn prepare_audio_candidate_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        prepare: impl FnOnce(
            hex_engine::AudioCandidatePreparationOperations<'_>,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<crate::AudioCandidatePreparationProgress, &'static str> {
        self.control
            .prepare_audio_candidate_on_control(token, |cell| prepare(cell.operations_on_control()))
    }

    pub fn stage_audio_candidate_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        context_generation: u64,
    ) -> Result<crate::AudioCandidateTicket, &'static str> {
        self.control
            .stage_audio_candidate_preparation(token, context_generation)
    }

    pub fn cancel_audio_candidate_preparation(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
    ) -> bool {
        self.control.cancel_audio_candidate_preparation(token)
    }

    pub fn cancel_audio_candidate(
        &self,
        ticket: &crate::AudioCandidateTicket,
    ) -> crate::AudioCandidateCancellationResult {
        self.control.cancel_audio_candidate(ticket)
    }

    pub fn stage_eligible_audio_candidate(
        &mut self,
        owner: hex_engine::AudioCandidateOwner,
        domains: Vec<crate::AudioCandidateDomainToken>,
        policy: PublicationPolicy,
        context_generation: u64,
    ) -> Result<crate::AudioCandidateTicket, crate::AudioCandidateEligibleStageRefusal> {
        self.control
            .stage_eligible_audio_candidate(owner, domains, policy, context_generation)
    }

    pub fn poll_resident_publication(&mut self) {
        self.control.poll_resident_publication()
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests;
