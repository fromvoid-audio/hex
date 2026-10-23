//! Source-bound effect preparation and borrowed graph processing on the existing
//! copied transport. Native ABI/child custody stays in hex-plugin-host; joining
//! worker custody stays in plugin_worker. These constructor pairs are resources,
//! not Root/publication admission. The output owner must retain the matching
//! workers through actual callback terminal return and final control disposal.
//! No canonical edits, defaults, scheduler or native DSP runs here on callback.
use crate::plugin_worker::{NativeWorker, NativeWorkerStartFailure};
use hex_lattice::{
    ForeignProcessorNode, ForeignProcessors, PreparedForeignProcessor, ProcessorInstanceIdentity,
};
use hex_lattice_contract::{
    DeviceContractError, ForeignProcessorBinding as Binding, NativeDeviceState,
};
use hex_plugin_host::{
    Error,
    native::Prepared,
    transport::{Callback, Fault, MAX_FRAMES},
};

mod definition;
pub use definition::NativeEffectDefinition;
mod callbacks;
pub use callbacks::NativeEffectCallbacks;

struct Configuration {
    identity: ProcessorInstanceIdentity,
    binding: Binding,
    sample_rate: u32,
    state: NativeDeviceState,
    prepared: PreparedForeignProcessor,
}

/// One actually prepared native source and its exact constructor-paired queues.
/// Borrow this owner during graph preparation; split only when transferring the
/// worker and callback to the output's respective existing custody owners.
pub struct NativeEffect {
    worker: NativeWorker,
    callback: NativeEffectCallback,
}
impl NativeEffect {
    pub fn prepare_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
    ) -> Result<Self, Error> {
        Self::prepare_retained_on_control(identity, instance)
            .map_err(NativeWorkerStartFailure::finish_on_control)
    }
    /// Preserve the actual source/queue owners on every preparation refusal.
    /// Root must retain the failure until observed native disposal and final
    /// control destruction; successful construction is not graph admission.
    #[allow(clippy::result_large_err)] // Root reserves both the larger success owner and actual failure packet before startup.
    pub fn prepare_retained_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
    ) -> Result<Self, NativeWorkerStartFailure> {
        let (configuration, dry) = match Self::configuration_on_control(identity, &instance) {
            Ok(parts) => parts,
            Err(cause) => {
                return Err(NativeWorkerStartFailure::retain_source_on_control(
                    cause, instance,
                ));
            }
        };
        let (worker, callback) = NativeWorker::start_retained_on_control(instance)?;
        Ok(Self {
            worker,
            callback: NativeEffectCallback {
                configuration,
                transport: callback,
                input: [[0.0; 2]; MAX_FRAMES],
                dry_output: [[0.0; 2]; MAX_FRAMES],
                dry,
                dry_cursor: 0,
            },
        })
    }
    fn configuration_on_control(
        identity: ProcessorInstanceIdentity,
        instance: &Prepared,
    ) -> Result<(Configuration, Vec<[f32; 2]>), Error> {
        let config = instance.configuration();
        if config.binding.instance != identity.device_id()
            || !config.sample_rate.is_finite()
            || config.sample_rate <= 0.0
            || config.sample_rate.fract() != 0.0
            || config.sample_rate > f64::from(u32::MAX)
        {
            return Err(Error::Protocol(
                "native effect identity or sample rate differs",
            ));
        }
        // A restored canonical envelope remains original; normalized startup
        // capture is not a replacement edit to the user's saved configuration.
        let state = config
            .restore
            .as_ref()
            .unwrap_or(&instance.ready().initial_state)
            .clone();
        state
            .require_asset_free_processing()
            .map_err(|_| Error::Protocol("native effect configuration requires assets"))?;
        hex_plugin_state::available(&state, Some(config.identity)).map_err(Error::State)?;
        let prepared = PreparedForeignProcessor::new(
            config.quantum,
            instance.ready().algorithmic_latency_frames,
            u32::from(config.quantum) * 2,
        )
        .and_then(|prepared| prepared.with_binding(config.binding))
        .map_err(|_| Error::Protocol("native effect latency overflow"))?;
        if prepared.total_latency_frames() > hex_engine::compensation::MAX_COMPENSATION_FRAMES {
            return Err(Error::Protocol("native effect latency exceeds graph limit"));
        }
        let configuration = Configuration {
            identity,
            binding: config.binding,
            sample_rate: config.sample_rate as u32,
            state,
            prepared,
        };
        // Reserve before starting the joining worker so allocation refusal
        // leaves the original prepared source under this control owner.
        let dry_frames = prepared.total_latency_frames() as usize;
        let mut dry = Vec::new();
        dry.try_reserve_exact(dry_frames)
            .map_err(|_| Error::Protocol("native effect dry history allocation refused"))?;
        dry.resize(dry_frames, [0.0; 2]);
        Ok((configuration, dry))
    }
    pub(crate) fn retire_on_control(
        &mut self,
    ) -> Result<crate::plugin_worker::NativeWorkerRetirement, Error> {
        self.worker.retire_on_control()
    }
    pub fn identity(&self) -> ProcessorInstanceIdentity {
        self.callback.configuration.identity
    }
    pub fn configuration(&self) -> &NativeDeviceState {
        &self.callback.configuration.state
    }
    pub fn binding(&self) -> Binding {
        self.callback.configuration.binding
    }
    pub fn prepared(&self) -> PreparedForeignProcessor {
        self.callback.configuration.prepared
    }
    /// Unique owner transfer, never a source/Root/publication ACK. The caller
    /// retains worker control custody until its actual callback carrier returns.
    pub fn into_parts(self) -> (NativeWorker, NativeEffectCallback) {
        (self.worker, self.callback)
    }
}

/// One source's callback endpoint and aligned dry history. All storage is
/// allocated on control. Move/return this through the output's real carrier;
/// never finally destroy it or the matching worker on the callback lane.
pub struct NativeEffectCallback {
    configuration: Configuration,
    transport: Callback,
    input: [[f32; 2]; MAX_FRAMES],
    dry_output: [[f32; 2]; MAX_FRAMES],
    dry: Vec<[f32; 2]>,
    dry_cursor: usize,
}
impl NativeEffectCallback {
    pub fn binding(&self) -> Binding {
        self.configuration.binding
    }
    pub fn identity(&self) -> ProcessorInstanceIdentity {
        self.configuration.identity
    }
    pub fn matches_worker(&self, worker: &NativeWorker) -> bool {
        worker.owns_callback(&self.transport)
    }
    /// Bound one actual complete host callback before the engine stages any
    /// segments. A chunk/subsegment length is not the physical callback extent.
    pub fn begin_host_callback(&mut self, frames: usize) -> Result<(), Fault> {
        self.transport.begin_physical_callback(frames)
    }
    /// Only after complete final host-format acceptance. Engine success alone
    /// must not dispatch copied input; refusal revokes instead of retry/replay.
    pub fn finish_host_callback(&mut self, accepted: bool) -> Result<(), Fault> {
        self.transport.finish(accepted)
    }
    fn matches(&self, node: ForeignProcessorNode<'_>) -> bool {
        let config = &self.configuration;
        node.owner == config.identity.owner()
            && node.device_id == config.identity.device_id()
            && node.device_type == config.state.type_id()
            && node.sample_rate == config.sample_rate
            && node.prepared == config.prepared
            && node.configuration.processing_eq(&config.state)
    }
}
impl ForeignProcessors for NativeEffectCallback {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if !self.matches(node)
            || frames.len() > MAX_FRAMES
            || !node.input_gain.is_finite()
            || node.input_gain < 0.0
        {
            self.reject_callback();
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        self.input[..frames.len()].copy_from_slice(frames);
        // Keep the untrimmed source in dry history; wet input trim must not
        // contaminate samples heard after a later aligned bypass transition.
        for (index, frame) in frames.iter().enumerate() {
            let dry = self.dry[self.dry_cursor];
            self.dry[self.dry_cursor] = *frame;
            self.dry_cursor += 1;
            if self.dry_cursor == self.dry.len() {
                self.dry_cursor = 0;
            }
            if node.bypassed {
                self.dry_output[index] = dry;
            }
        }
        if node.input_gain != 1.0 {
            for frame in &mut self.input[..frames.len()] {
                frame[0] *= node.input_gain;
                frame[1] *= node.input_gain;
            }
        }
        self.transport
            .stage_segment(offset, &self.input[..frames.len()], &[], frames)
            .map_err(|_| DeviceContractError::ForeignProcessingFailed)?;
        // Both paths advance. Refusal irreversibly revokes this epoch, so dry
        // and foreign history are never rolled back or replayed.
        if node.bypassed {
            frames.copy_from_slice(&self.dry_output[..frames.len()]);
        }
        Ok(())
    }
    fn reject_callback(&mut self) {
        self.transport.reject(Fault::Revoked);
    }
}

#[cfg(test)]
mod tests;
