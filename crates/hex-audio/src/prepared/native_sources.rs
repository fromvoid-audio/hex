//! Native source control entry points; actual owners stay inside CandidateRoot.
use super::*;
use crate::{NativeSourcePreparationProgress, NativeSourcePreparationReturn};
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_host::native;
use hex_plugin_scan::macos::build_identity::InstanceBuild;

impl PublicationControl {
    /// Reserve fixed control cells before exposing the native output. This is
    /// preparation custody only; variable native resources and endpoints remain
    /// unsealed and cannot grant graph/publication eligibility.
    pub fn configure_native_source_preparation(
        &mut self,
        capacity: usize,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.revision != 0
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
        {
            return Err("native preparation requires a vacant native output");
        }
        self.candidate_root
            .configure_native_source_preparation(capacity)
    }
    /// Run source preparation on the existing control worker, never UI/audio.
    /// Success parks the actual owner in Root even after refusal/close. The
    /// initial binding is derived by Root before spawn, never queue retagging.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_native_source_preparation(
        &mut self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: native::PreparationCancellation,
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
            cancellation,
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
