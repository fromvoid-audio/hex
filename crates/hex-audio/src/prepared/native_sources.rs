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
