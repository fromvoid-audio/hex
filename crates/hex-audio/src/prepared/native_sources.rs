//! Native source control entry points; actual owners stay inside CandidateRoot.
use super::*;
use crate::{
    NativeSourceDataReservation, NativeSourcePreparationProgress, NativeSourcePreparationReturn,
};
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_host::native;
use hex_plugin_scan::macos::build_identity::InstanceBuild;

impl PublicationControl {
    /// Exclude transport/publication while the callback acknowledges the exact
    /// current native runtime after the admitted command prefix. This witness
    /// grants no source ownership and does not acknowledge graph/endpoint return.
    pub fn request_native_epoch_fence(
        &self,
        expected: AppliedAudioSource,
    ) -> Result<StoppedEditFence, &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.status.publication_pending.recording_active()
            || self.status.applied_audio_source() != Some(expected)
        {
            return Err("native epoch output is not current or publication is pending");
        }
        if !self.status.publication_pending.reserve_native_epoch() {
            return Err("native epoch publication or recording is pending");
        }
        match self.status.sampler_edit.begin_source(
            expected,
            self.status.admitted_ticket.load(Acquire),
            false,
            false,
            true,
        ) {
            Ok(token) => Ok(token),
            Err(reason) => {
                self.status.publication_pending.release_native_epoch();
                Err(reason)
            }
        }
    }

    /// Queue return of exactly this stopped runtime and its original native
    /// endpoints through the existing carriers. Keep the stopped fence held
    /// through renewal/successor installation. Enqueue success is not return:
    /// source renewal still requires control ingestion and all graph refs gone.
    pub fn return_stopped_native_epoch_runtime(
        &mut self,
        ticket: &crate::AudioCandidateTicket,
        fence: StoppedEditFence,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self
                .status
                .sampler_edit
                .native_epoch_context(fence)
                .is_none()
            || self.stopped_edit_fence_state(fence) != StoppedEditFenceState::Ready
        {
            return Err("native epoch requires an observed stopped source fence");
        }
        let target = ticket
            .native_epoch_target(self.candidate_root.root_id())
            .ok_or("native epoch runtime belongs to another output")?;
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "native epoch publication is busy")?;
        chunk.fill_from_iter(std::iter::once(QueuedPublication::NativeEpoch {
            target,
            fence,
        }));
        Ok(())
    }
    /// Renew custody of this exact loaded source after its original callback
    /// endpoint and every old graph reference returned. The new binding is a
    /// pending operation: query facts, complete with actual presentation, then
    /// construct/loan fresh paired endpoints before proposing a successor graph.
    pub fn quiesce_native_source_preparation(
        &mut self,
        previous: Binding,
        graph: u64,
    ) -> Result<Binding, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native epoch output is closing");
        }
        self.candidate_root
            .quiesce_native_source_preparation(previous, graph)
    }
    /// Reserve fixed cells and per-cell variable DATA allowances before exposing
    /// the output. The whole pool's allowances remain charged, including vacant
    /// cells, so other admissions cannot spend that capacity. This is not actual
    /// producer/resource sealing and cannot grant graph/publication eligibility.
    pub fn configure_native_source_preparation(
        &mut self,
        capacity: usize,
        limits: NativeSourceDataReservation,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.revision != 0
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
        {
            return Err("native preparation requires a vacant native output");
        }
        self.candidate_root
            .configure_native_source_preparation(capacity, limits)
    }
    /// Run source preparation on the existing control worker, never UI/audio.
    /// Success parks the actual owner in Root even after refusal/close. The
    /// initial binding and cancellation pair are created by Root before spawn.
    /// Callers cancel by exact binding; no descriptor-owning token escapes.
    #[allow(clippy::too_many_arguments)] // Complete typed native request; no separate caller cancellation owner.
    pub fn begin_native_source_preparation(
        &mut self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || preparation.sample_rate != f64::from(self.rate)
            || self.requested_physical_callback_frames != Some(preparation.quantum)
        {
            return Err("native preparation output profile differs or is closing");
        }
        self.candidate_root.begin_native_source_preparation(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            trust,
        )
    }
    /// Supply actual graph/device presentation contributions, or None when
    /// unavailable. The exact Pending becomes a Root-held Prepared or retained
    /// refusal. Ok acknowledges custody only; inspect progress/error separately.
    /// Run on the existing control worker. This does not publish a graph.
    pub fn complete_native_source_preparation(
        &mut self,
        binding: Binding,
        presentation: Option<hex_plugin_host::protocol::PresentationLatency>,
    ) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        self.candidate_root
            .complete_native_source_preparation(binding, presentation)
    }
    /// Construct the actual copied-transport worker/callback pair inside Root.
    /// Ok acknowledges retained custody, including constructor refusal. Neither
    /// the pair nor its facts grant graph/publication admission yet.
    pub fn start_native_effect(&mut self, binding: Binding) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        self.candidate_root.start_native_effect(binding)
    }
    /// Run pure graph compilation with definitions borrowed from these exact
    /// Root-owned effect pairs. This does not transfer endpoints or authorize
    /// publication. The visitor runs on the existing control worker.
    pub fn with_native_effect_definition<R>(
        &self,
        bindings: &[Binding],
        prepare: impl FnOnce(&crate::plugin_effects::NativeEffectDefinition<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native effect output is closing".into());
        }
        self.candidate_root
            .with_native_effect_definition(bindings, prepare)
    }
    /// Borrow selected actual Root source definitions while preparing inside
    /// this same Root's reserved cell. One Work guard covers both sides; no
    /// nested acquisition, source extraction or independent lifecycle exists.
    pub fn prepare_audio_candidate_with_native_sources_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        bindings: &[Binding],
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<crate::AudioCandidatePreparationProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native effect output is closing");
        }
        self.candidate_root
            .with_native_runtime_preparation(token, bindings, prepare)
    }
    /// Copy the original restored envelope, or actual initial capture for a
    /// fresh source. Read-only state is never Root/graph admission authority.
    pub fn native_source_prepared_state(&self, binding: Binding) -> Option<NativeDeviceState> {
        self.candidate_root.native_source_prepared_state(binding)
    }
    pub fn native_source_preparation_progress(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationProgress> {
        self.candidate_root
            .native_source_preparation_progress(binding)
    }
    /// Record retirement of this exact source. Original graph references defer
    /// endpoint return and worker disposal; a true result needs no retry.
    /// False records no request: inspect progress and retry a still-live source
    /// when the control executor is busy. Unknown or terminal bindings refuse.
    pub fn retire_native_source_preparation(&mut self, binding: Binding) -> bool {
        self.candidate_root
            .retire_native_source_preparation(binding)
    }
    /// Observe the original construction/cleanup failure without taking its
    /// owner or treating an unresolved child as a recoverable return.
    pub fn native_source_preparation_error(&self, binding: Binding) -> Option<String> {
        self.candidate_root.native_source_preparation_error(binding)
    }
    pub fn recover_native_source_preparation(
        &mut self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationReturn> {
        self.candidate_root
            .recover_native_source_preparation(binding)
    }
}

impl PublicationControl {
    /// Queue the unique actual endpoint; the existing Root keeps its worker.
    /// Queue acceptance grants no graph, canonical edit or disposal authority.
    /// A full queue refuses before extracting the original constructor pair.
    pub fn publish_native_source_endpoint(&mut self, binding: Binding) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "native endpoint publication is busy")?;
        let endpoint = self.candidate_root.loan_native_endpoint(binding)?;
        chunk.fill_from_iter(std::iter::once(QueuedPublication::Native(endpoint)));
        Ok(())
    }
}
impl Callback {
    pub(super) fn return_native_epoch_runtime(
        &mut self,
        target: crate::candidate_custody::NativeEpochTarget,
        fence: StoppedEditFence,
    ) {
        let Some(ActivePublication::Eligible(active)) = self.active.as_ref() else {
            return;
        };
        let Some(source) = active.applied_audio_source() else {
            return;
        };
        if self
            .status
            .sampler_edit
            .native_epoch_context(fence)
            .is_none()
            || !active.matches_native_epoch_target(&target)
            || !self.status.sampler_edit.matches_source(fence, source)
            || active
                .runtime()
                .and_then(|runtime| runtime.status())
                .is_none_or(|s| s.playing)
            || !self.native_sources.has_endpoints()
        {
            return;
        }
        // begin_block reserved return capacity before popping this scalar
        // request. Only fixed moves occur; all original destruction is control.
        self.cancel_active_stream_action();
        if let Some(ActivePublication::Eligible(mut active)) = self.active.take() {
            active.clear_stream_roles(&mut self.domain_pages);
            self.native_sources.request_epoch_return();
            self.return_publication(ReturnedPublication::Eligible(
                active.return_stopped_native_epoch(),
            ));
            self.status.applied_source.publish(None);
            self.publish_active_routes(true);
            self.publish_transport();
        }
    }
    pub(super) fn return_requested_native_sources(&mut self) {
        for index in 0..hex_lattice::MAX_STATEFUL_PROCESSORS {
            if self.held_retirement.is_some() || self.retired.is_full() {
                return;
            }
            let Some(endpoint) = self.native_sources.take_requested(index) else {
                continue;
            };
            self.return_publication(ReturnedPublication::Native(endpoint));
        }
    }
    pub(crate) fn has_native_source_endpoints(&self) -> bool {
        self.native_sources.has_endpoints()
    }
    pub(crate) fn extract_native_source_endpoints_on_control(
        &mut self,
        destination: &mut [Option<crate::candidate_custody::NativeEndpointEnvelope>;
                 hex_lattice::MAX_STATEFUL_PROCESSORS],
    ) -> bool {
        self.native_sources
            .extract(self.candidate_root.root_id(), destination)
    }
}

#[cfg(test)]
mod tests;
