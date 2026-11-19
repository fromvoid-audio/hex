//! Source-bound effect preparation and borrowed graph processing on the existing
//! copied transport. Native ABI/child custody stays in hex-plugin-host; joining
//! worker custody stays in plugin_worker. These constructor pairs are resources,
//! not Root/publication admission. The output owner must retain the matching
//! workers through actual callback terminal return and final control disposal.
//! No canonical edits, defaults, scheduler or native DSP runs here on callback.
use crate::plugin_worker::{NativeWorker, NativeWorkerStartFailure};
use hex_lattice::{
    ForeignModulationSamples, ForeignNote, ForeignProcessorNode, ForeignProcessors,
    PreparedForeignProcessor, ProcessorInstanceIdentity,
};
use hex_lattice_contract::{
    DeviceContractError, ForeignProcessorBinding as Binding, MAX_FOREIGN_AUTOMATION_TARGETS,
    NativeDeviceState,
};
use hex_plugin_host::{
    Error,
    native::Prepared,
    transport::{
        AcceptedManualParameter, AuthoredSegment, Callback, EffectSegmentOrigins, Fault,
        InputEvents, MAX_FRAMES, ManualParameterOrigin, MidiEvent, ProcessTiming, SegmentTiming,
    },
};

mod automation;
pub use automation::AuthoredSelection;
mod bypass;
mod definition;
mod tail;
mod transport;
pub use definition::{NativeEffectDefinition, UnavailableEffect};
use transport::EffectTransport;
mod hosting;
mod offline_preparation;
pub use offline_preparation::OfflineEffectPreparation;
pub(crate) mod manual_base_capture;
pub use hex_lattice_contract::ForeignProcessorBinding as EffectBinding;
pub use hex_plugin_host::native::StartupTimings as EffectStartupTimings;
pub use hex_plugin_host::protocol::{
    Presentation as EffectPresentationReceipt, PresentationPlan as EffectPresentationPlan,
};
pub use hosting::{EffectPresentation, EffectSelection, Hosting, PluginProfile};
pub(crate) use manual_base_capture::BaseCaptureSource;
pub use manual_base_capture::ManualBaseCapture;
mod canonical_bases;
pub use canonical_bases::{CanonicalBasesCapture, CapturedCanonicalBases};
mod control;
mod parameter_extension;
mod state_approval;
pub(crate) use control::{NativeEffectControl, NativeEffectReference};
mod source_metadata;
pub use source_metadata::EffectSourceMetadata;
mod callbacks;
pub use callbacks::NativeEffectCallbacks;
pub(crate) use callbacks::{NativeEffectCycle, NativeEffectEntry};

#[derive(Clone)]
struct Configuration {
    identity: ProcessorInstanceIdentity,
    binding: Binding,
    sample_rate: u32,
    state: NativeDeviceState,
    prepared: PreparedForeignProcessor,
    authored: automation::SourceCapabilities,
    dense_raw_id: Option<u32>,
    program_revision: std::num::NonZeroU64,
    state_verifier: Option<hex_lattice_contract::Vst3StateVerifier>,
}

impl Configuration {
    fn matches_processing_state(&self, state: &NativeDeviceState) -> bool {
        self.state_verifier
            .is_some_and(|verifier| verifier.approves_processing(state))
            || state.processing_eq(&self.state)
    }

    fn reserve_program_revision_on_control(
        &mut self,
    ) -> Result<std::num::NonZeroU64, &'static str> {
        let next = self
            .program_revision
            .get()
            .checked_add(1)
            .and_then(std::num::NonZeroU64::new)
            .ok_or("native automation program revisions exhausted")?;
        self.program_revision = next;
        Ok(next)
    }
}

/// One actually prepared native source and its exact constructor-paired queues.
/// Borrow this owner during graph preparation; split only when transferring the
/// worker and callback to the output's respective existing custody owners.
pub struct NativeEffect {
    worker: NativeWorker,
    callback: NativeEffectCallback,
    source_metadata: EffectSourceMetadata,
    state_authority: hex_lattice_contract::Vst3StateAuthority,
}

/// Original-source facts retained by Root or private offline preparation while
/// the same SDK lease renews.
/// No copied metadata or external caller can construct this authority packet.
pub(crate) struct NativeEffectRenewal {
    binding: Binding,
    identity: ProcessorInstanceIdentity,
    sample_rate: u32,
    prepared: PreparedForeignProcessor,
    authored: automation::SourceCapabilities,
    dense_raw_id: Option<u32>,
    state_authority: hex_lattice_contract::Vst3StateAuthority,
}
/// Listed constructor backing of the original unsplit effect. Inline effect
/// storage belongs to the host cell. The configuration state aliases the actual
/// worker source's restored/captured payload, so it adds one handle, no payload
/// or shared group. Copied source paths/digests add listed control DATA and no
/// shared handles; temporary query copies belong to their caller. Diagnostics
/// and construction/exchange peaks remain separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEffectStorage {
    pub worker: crate::plugin_worker::NativeWorkerStorage,
    pub dry_data_bytes: usize,
    pub source_metadata_data_bytes: usize,
    pub state_alias_handles: usize,
}
impl NativeEffectStorage {
    pub fn data_bytes(&self) -> Option<usize> {
        self.worker
            .data_bytes
            .checked_add(self.dry_data_bytes)?
            .checked_add(self.source_metadata_data_bytes)
    }
    pub fn shared_handle_bound(&self) -> Option<usize> {
        self.worker
            .shared_handle_bound
            .checked_add(self.state_alias_handles)
    }
}
impl NativeEffect {
    /// Root calls this only after all graph references and the unique endpoint
    /// loan have returned. Old copied queues/dry history are destroyed on this
    /// control lane; failures retain the exact native/transport custody packet.
    #[allow(clippy::result_large_err)]
    pub(crate) fn into_quiescent_on_control(
        self,
    ) -> Result<(hex_plugin_host::native::Quiescent, NativeEffectRenewal), NativeWorkerStartFailure>
    {
        let Self {
            worker,
            callback,
            source_metadata,
            state_authority,
        } = self;
        let NativeEffectCallback {
            transport,
            configuration,
            dry,
            ..
        } = callback;
        let renewal = NativeEffectRenewal {
            binding: configuration.binding,
            identity: configuration.identity,
            sample_rate: configuration.sample_rate,
            prepared: configuration.prepared,
            authored: configuration.authored,
            dense_raw_id: configuration.dense_raw_id,
            state_authority,
        };
        drop((configuration.state, dry, source_metadata));
        transport
            .into_quiescent_on_control(worker)
            .map(|owner| (owner, renewal))
    }
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
        Self::prepare_retained_with_data_limit_on_control(identity, instance, usize::MAX)
    }
    /// Root's known steady DATA check, before dry storage, queues, owner Boxes
    /// or the joining thread are constructed. Includes the native constructor's
    /// original diagnostic allowance; preparation/exchange peaks and opaque
    /// resource reservations remain separate and cannot alone seal
    /// a native publication. Refusal retains the original instance for disposal.
    #[allow(clippy::result_large_err)]
    pub(crate) fn prepare_retained_with_data_limit_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
        max_data_bytes: usize,
    ) -> Result<Self, NativeWorkerStartFailure> {
        Self::prepare_retained_with_authored_selection_on_control(
            identity,
            instance,
            max_data_bytes,
            None,
        )
    }

    fn preconstruction_data_bytes_on_control(
        instance: &Prepared,
        prepared: PreparedForeignProcessor,
        reserve_dense: bool,
    ) -> Option<usize> {
        let source =
            instance.storage_with_authored_capacity_on_control(MAX_FOREIGN_AUTOMATION_TARGETS)?;
        let worker = if reserve_dense {
            NativeWorker::preconstruction_dense_effect_for_source_on_control(source)?
        } else {
            NativeWorker::preconstruction_data_bytes_for_source_on_control(source)?
        };
        worker
            .checked_add(
                (prepared.total_latency_frames() as usize)
                    .checked_mul(std::mem::size_of::<[f32; 2]>())?,
            )?
            .checked_add(EffectSourceMetadata::required_data_bytes(
                instance.configuration(),
            )?)
    }

    /// Query and arm selected raw parameters on the original source before it
    /// moves to the worker. IDs must be sorted and unique. This control-only
    /// preparation grants no graph admission, and refusal preserves custody.
    #[allow(clippy::result_large_err)]
    pub fn prepare_retained_with_authored_selection_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
        max_data_bytes: usize,
        authored: Option<(hex_plugin_state::ClassId, &[u32])>,
    ) -> Result<Self, NativeWorkerStartFailure> {
        Self::prepare_with_renewal_on_control(
            identity,
            instance,
            max_data_bytes,
            authored,
            None,
            None,
        )
    }

    #[allow(clippy::result_large_err)] // Refusal retains the actual source.
    pub(crate) fn prepare_with_renewal_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
        max_data_bytes: usize,
        authored: Option<(hex_plugin_state::ClassId, &[u32])>,
        renewal: Option<NativeEffectRenewal>,
        requested_dense: Option<u32>,
    ) -> Result<Self, NativeWorkerStartFailure> {
        let previous_dense = renewal.as_ref().and_then(|old| old.dense_raw_id);
        if previous_dense.is_some_and(|previous| requested_dense.is_some_and(|id| id != previous)) {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native renewal dense parameter differs"),
                instance,
            ));
        }
        let dense_raw_id = requested_dense.or(previous_dense);
        Self::prepare_with_transport_on_control(
            identity,
            instance,
            max_data_bytes,
            authored,
            renewal,
            dense_raw_id,
        )
    }

    /// Reserve one exact continuous raw parameter on the original stereo
    /// effect. The selected raw ID must be included in the source metadata
    /// request. The host still owns graph admission and callback acceptance.
    #[allow(clippy::result_large_err)]
    pub fn prepare_retained_with_dense_parameter_on_control(
        identity: ProcessorInstanceIdentity,
        instance: Prepared,
        max_data_bytes: usize,
        authored: (hex_plugin_state::ClassId, &[u32]),
        dense_raw_id: u32,
    ) -> Result<Self, NativeWorkerStartFailure> {
        Self::prepare_with_transport_on_control(
            identity,
            instance,
            max_data_bytes,
            Some(authored),
            None,
            Some(dense_raw_id),
        )
    }

    #[allow(clippy::result_large_err)] // The actual refused source stays in control custody.
    fn prepare_with_transport_on_control(
        identity: ProcessorInstanceIdentity,
        mut instance: Prepared,
        max_data_bytes: usize,
        authored: Option<(hex_plugin_state::ClassId, &[u32])>,
        renewal: Option<NativeEffectRenewal>,
        dense_raw_id: Option<u32>,
    ) -> Result<Self, NativeWorkerStartFailure> {
        let mut configuration = match Self::configuration_on_control(identity, &instance) {
            Ok(configuration) => configuration,
            Err(cause) => {
                return Err(NativeWorkerStartFailure::retain_source_on_control(
                    cause, instance,
                ));
            }
        };
        if renewal.as_ref().is_some_and(|old| {
            old.identity != identity
                || old.sample_rate != configuration.sample_rate
                || old.binding.root != configuration.binding.root
                || old.binding.instance != configuration.binding.instance
                || old.binding.graph > configuration.binding.graph
                || old.binding.epoch >= configuration.binding.epoch
        }) {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native renewal source differs"),
                instance,
            ));
        }
        if dense_raw_id.is_some_and(|id| {
            configuration.prepared.is_instrument_profile()
                || authored.is_none_or(|(_, selected)| selected.binary_search(&id).is_err())
        }) {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native dense parameter selection differs"),
                instance,
            ));
        }
        let (retained, authority) = match renewal {
            Some(old) => {
                // A changed processing profile must be qualified again before
                // old isolated states can reuse this worker.
                let authority = (old.prepared.with_binding(configuration.binding).ok()
                    == Some(configuration.prepared))
                .then_some(old.state_authority);
                (Some(old.authored), authority)
            }
            None => (None, None),
        };
        let state_authority = match authority
            .map(Ok)
            .unwrap_or_else(hex_lattice_contract::Vst3StateAuthority::new_on_control)
        {
            Ok(authority) => authority,
            Err(_) => {
                return Err(NativeWorkerStartFailure::retain_source_on_control(
                    Error::Protocol("native state approval authorities exhausted"),
                    instance,
                ));
            }
        };
        configuration.state =
            match state_authority.approve_processing_on_control(configuration.state.clone()) {
                Ok(state) => state,
                Err(_) => {
                    return Err(NativeWorkerStartFailure::retain_source_on_control(
                        Error::Protocol("native original processing approval refused"),
                        instance,
                    ));
                }
            };
        configuration.state_verifier = Some(state_authority.verifier());
        // Preserve an already known lease refusal before considering the
        // budget. Worker transfer validates again against a later close/exit.
        if let Err(cause) = instance.validate_worker_transfer_on_control() {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                cause, instance,
            ));
        }
        let dry_frames = configuration.prepared.total_latency_frames() as usize;
        // Reserve the actual retirement diagnostics before starting the worker;
        // the unused allowance is not part of its resident storage inventory.
        let known_data = Self::preconstruction_data_bytes_on_control(
            &instance,
            configuration.prepared,
            dense_raw_id.is_some(),
        )
        .and_then(|n| n.checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES));
        if known_data.is_none_or(|bytes| bytes > max_data_bytes) {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native effect data reservation exceeded or unavailable"),
                instance,
            ));
        }
        let mut authored_bases = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
        if let Some((class, ids)) = authored {
            let (capabilities, bases) = match automation::SourceCapabilities::arm(
                &mut instance,
                class,
                ids,
                retained.as_ref(),
            ) {
                Ok(observed) => observed,
                Err(cause) => {
                    return Err(NativeWorkerStartFailure::retain_source_on_control(
                        cause, instance,
                    ));
                }
            };
            configuration.authored = capabilities;
            authored_bases = bases;
        }
        if dense_raw_id.is_some_and(|id| {
            !configuration
                .authored
                .parameters()
                .iter()
                .any(|parameter| parameter.id == id && parameter.steps == 0)
        }) {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native dense parameter must be continuous"),
                instance,
            ));
        }
        configuration.dense_raw_id = dense_raw_id;
        if let Err(cause) =
            instance.reserve_authored_parameter_capacity_on_control(MAX_FOREIGN_AUTOMATION_TARGETS)
        {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                cause, instance,
            ));
        }
        if authored.is_none() {
            configuration.authored =
                automation::SourceCapabilities::reserved(instance.configuration().identity.class);
        }
        let mut source_metadata =
            match EffectSourceMetadata::from_configuration(instance.configuration()) {
                Ok(source) => source,
                Err(cause) => {
                    return Err(NativeWorkerStartFailure::retain_source_on_control(
                        cause, instance,
                    ));
                }
            };
        source_metadata.authored_bases = authored_bases;
        source_metadata.dense_parameter = dense_raw_id;
        let mut dry = Vec::new();
        if dry.try_reserve_exact(dry_frames).is_err() || dry.capacity() != dry_frames {
            return Err(NativeWorkerStartFailure::retain_source_on_control(
                Error::Protocol("native effect dry history allocation refused"),
                instance,
            ));
        }
        dry.resize(dry_frames, [0.0; 2]);
        let tail = tail::TailDrain::new(
            instance.ready().tail_frames,
            configuration.prepared.total_latency_frames(),
        );
        let (worker, transport) = if dense_raw_id.is_some() {
            let (worker, callback) =
                NativeWorker::start_retained_dense_effect_on_control(instance)?;
            (worker, EffectTransport::Dense(callback))
        } else {
            let (worker, callback) = NativeWorker::start_retained_on_control(instance)?;
            (worker, EffectTransport::Ordinary(callback))
        };
        Ok(Self {
            worker,
            source_metadata,
            state_authority,
            callback: NativeEffectCallback {
                configuration,
                transport,
                input: [[0.0; 2]; MAX_FRAMES],
                dry_output: [[0.0; 2]; MAX_FRAMES],
                dry,
                dry_cursor: 0,
                concealed_output: false,
                tail,
                bypass: bypass::BypassMix::default(),
            },
        })
    }
    fn configuration_on_control(
        identity: ProcessorInstanceIdentity,
        instance: &Prepared,
    ) -> Result<Configuration, Error> {
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
        .map(|prepared| {
            let prepared = if config.input_channels == 0 {
                prepared.with_instrument_profile()
            } else {
                prepared
            };
            prepared.with_output_tail(if instance.ready().tail_frames == u32::MAX {
                hex_lattice::OutputTail::Unbounded
            } else {
                hex_lattice::OutputTail::Finite {
                    // Acceptance is observed on a later physical callback;
                    // a visited endpoint must also finish its complete extent.
                    // Two quanta bound that observation/coverage rounding. This
                    // does not add algorithmic latency or delay audible samples.
                    // SDK u32 + two validated u16 quanta fits this u64 bound.
                    max_tail_frames: u64::from(instance.ready().tail_frames)
                        + u64::from(config.quantum) * 2,
                }
            })
        })
        .and_then(|prepared| prepared.with_binding(config.binding))
        .map_err(|_| Error::Protocol("native effect latency overflow"))?;
        if prepared.total_latency_frames() > hex_engine::compensation::MAX_COMPENSATION_FRAMES {
            return Err(Error::Protocol("native effect latency exceeds graph limit"));
        }
        Ok(Configuration {
            identity,
            binding: config.binding,
            sample_rate: config.sample_rate as u32,
            state,
            prepared,
            authored: automation::SourceCapabilities::default(),
            dense_raw_id: None,
            program_revision: std::num::NonZeroU64::new(config.binding.epoch)
                .ok_or(Error::Protocol("native automation source epoch is invalid"))?,
            state_verifier: None,
        })
    }
    pub(crate) fn reserve_program_revision_on_control(
        &mut self,
    ) -> Result<std::num::NonZeroU64, &'static str> {
        self.callback
            .configuration
            .reserve_program_revision_on_control()
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
    pub fn finite_tail_frames(&self) -> Option<u64> {
        match self.prepared().output_tail() {
            hex_lattice::OutputTail::Finite { max_tail_frames } => Some(max_tail_frames),
            hex_lattice::OutputTail::Unbounded => None,
        }
    }
    /// Borrow the actual unsplit pair's copied-channel backing. This partial
    /// inventory excludes dry history, state, worker/native owners and all
    /// construction temporaries; it cannot close a Root resource witness.
    pub fn transport_storage_on_control(
        &self,
    ) -> Option<hex_plugin_host::transport::ConstructorStorage> {
        self.callback.transport.storage_on_control(&self.worker)
    }
    /// Immutable source backing from this exact unsplit constructor pair.
    /// Other worker/dry/error resources and preparation peaks remain separate.
    pub fn retained_source_storage_on_control(
        &self,
    ) -> Option<hex_plugin_host::native::PreparedStorage> {
        self.callback
            .transport
            .source_storage_on_control(&self.worker)
    }
    /// Inspect the original worker/source/queues plus actual aligned dry Vec
    /// capacity. Equal state is not used to deduplicate payloads: the alias is
    /// established by this private constructor's direct clone of that source.
    /// This still cannot seal publication without diagnostics/peak reservations
    /// and the existing callback loan/return and physical disposal contracts.
    pub fn constructor_storage_on_control(&self) -> Option<NativeEffectStorage> {
        Some(NativeEffectStorage {
            worker: self
                .callback
                .transport
                .worker_storage_on_control(&self.worker)?,
            dry_data_bytes: self
                .callback
                .dry
                .capacity()
                .checked_mul(std::mem::size_of::<[f32; 2]>())?,
            source_metadata_data_bytes: self.source_metadata.data_bytes()?,
            state_alias_handles: 1,
        })
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
    transport: EffectTransport,
    input: [[f32; 2]; MAX_FRAMES],
    dry_output: [[f32; 2]; MAX_FRAMES],
    dry: Vec<[f32; 2]>,
    dry_cursor: usize,
    concealed_output: bool,
    tail: tail::TailDrain,
    bypass: bypass::BypassMix,
}
impl NativeEffectCallback {
    pub fn binding(&self) -> Binding {
        self.configuration.binding
    }
    pub fn identity(&self) -> ProcessorInstanceIdentity {
        self.configuration.identity
    }
    pub fn matches_worker(&self, worker: &NativeWorker) -> bool {
        self.transport.matches_worker(worker)
    }
    /// Reserve an observation only on its constructor-paired endpoint. This
    /// is not graph, state, transport or publication authorization.
    pub(crate) fn reserve_observed_parameter_input(
        &self,
        source: &hex_plugin_host::transport::ParameterInputSource,
    ) -> Option<hex_plugin_host::transport::ParameterInputFence<'_>> {
        self.transport.reserve_observed_parameter_input(source)
    }
    /// Bound one actual complete host callback before the engine stages any
    /// segments. A chunk/subsegment length is not the physical callback extent.
    pub fn begin_host_callback(&mut self, frames: usize) -> Result<(), Fault> {
        self.transport.begin_physical_callback(frames)?;
        self.concealed_output = false;
        self.tail.begin();
        Ok(())
    }
    /// Only after complete final host-format acceptance. Engine success alone
    /// must not dispatch copied input; refusal revokes instead of retry/replay.
    pub fn finish_host_callback(&mut self, accepted: bool) -> Result<(), Fault> {
        self.transport.finish(accepted)?;
        self.tail.commit();
        Ok(())
    }
    /// Consume the exact operation and input clock from this accepted endpoint
    /// callback. This is still before the mix's recording/delivery decision.
    pub(crate) fn take_accepted_editor_quantum(
        &mut self,
    ) -> Option<hex_plugin_host::transport::AcceptedEditorQuantum> {
        self.transport.take_accepted_editor_quantum()
    }

    pub fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        self.transport.take_accepted_manual_parameter()
    }
    pub(crate) fn end_staging_at_rendered_prefix(&mut self, frame: usize) {
        self.transport.end_staging_at_rendered_prefix(frame);
    }
    /// This original live epoch has accepted enough consecutive zero input to
    /// drain its source-reported tail and fixed processor/bridge latency. New
    /// staged input invalidates the view; speculative zeros cannot establish it.
    /// Infinite reports and revoked epochs return false. This is neither graph
    /// admission nor authority to drop/return an endpoint on the callback lane.
    pub fn reported_tail_is_drained(&self) -> bool {
        // Silence at an instrument's audio input says nothing about held MIDI
        // notes. Only the graph's future note owner can prove its release.
        !self.configuration.prepared.is_instrument_profile()
            && !self.transport.is_revoked()
            && self.tail.drained()
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn transport_fault_for_test(&self) -> Option<Fault> {
        self.transport.fault()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn pending_requests_for_test(&self) -> usize {
        self.transport.pending_requests_for_test()
    }
    /// Contained placeholder for a revoked endpoint (#3977): advance the dry
    /// history exactly as bypass does and return the input delayed by the
    /// endpoint's unchanged declared latency, so the graph stays aligned. No
    /// transport work is staged. An oversized segment, which the graph never
    /// produces, is silenced rather than written past fixed storage.
    pub(crate) fn pass_through_revoked(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frames: &mut [[f32; 2]],
    ) {
        if frames.len() > MAX_FRAMES || !node.output_gain.is_finite() || node.output_gain < 0.0 {
            frames.fill([0.0; 2]);
            return;
        }
        if self.configuration.prepared.is_instrument_profile() {
            frames.fill([0.0; 2]);
        }
        self.advance_dry(frames);
        frames.copy_from_slice(&self.dry_output[..frames.len()]);
        if !node.bypassed {
            for frame in frames {
                frame[0] *= node.output_gain;
                frame[1] *= node.output_gain;
            }
        }
    }
    /// Push `frames` into the dry history and record the aligned dry output.
    fn advance_dry(&mut self, frames: &[[f32; 2]]) {
        for (index, frame) in frames.iter().enumerate() {
            self.dry_output[index] = self.dry[self.dry_cursor];
            self.dry[self.dry_cursor] = *frame;
            self.dry_cursor += 1;
            if self.dry_cursor == self.dry.len() {
                self.dry_cursor = 0;
            }
        }
    }
    /// A fake endpoint on a real copied transport, for containment tests that
    /// need no SDK instance. Latency is the transport's 2Q bridge only.
    #[cfg(test)]
    pub(crate) fn for_test(
        identity: ProcessorInstanceIdentity,
        binding: Binding,
        state: NativeDeviceState,
        transport: Callback,
    ) -> Self {
        let quantum = transport.quantum_frames() as u16;
        let prepared = PreparedForeignProcessor::new(quantum, 0, u32::from(quantum) * 2)
            .and_then(|prepared| prepared.with_binding(binding))
            .expect("test latency fits");
        let latency = prepared.total_latency_frames();
        Self {
            configuration: Configuration {
                identity,
                binding,
                sample_rate: 48_000,
                state,
                prepared,
                authored: automation::SourceCapabilities::default(),
                dense_raw_id: None,
                program_revision: std::num::NonZeroU64::new(binding.epoch).unwrap(),
                state_verifier: None,
            },
            transport: EffectTransport::Ordinary(transport),
            input: [[0.0; 2]; MAX_FRAMES],
            dry_output: [[0.0; 2]; MAX_FRAMES],
            dry: vec![[0.0; 2]; latency as usize],
            dry_cursor: 0,
            concealed_output: false,
            tail: tail::TailDrain::new(0, latency),
            bypass: bypass::BypassMix::default(),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_instrument_profile_for_test(mut self) -> Self {
        self.configuration.prepared = self.configuration.prepared.with_instrument_profile();
        self
    }
    fn matches(&self, node: ForeignProcessorNode<'_>) -> bool {
        let config = &self.configuration;
        node.owner == config.identity.owner()
            && node.device_id == config.identity.device_id()
            && node.device_type == config.state.type_id()
            && node.sample_rate == config.sample_rate
            && node.prepared == config.prepared
            && config.matches_processing_state(node.configuration)
    }
}
impl ForeignProcessors for NativeEffectCallback {
    fn returns_post_trim(&self, _node: ForeignProcessorNode<'_>) -> bool {
        true
    }
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.process_segment_with_parameters(node, offset, frames, &[], clock)
    }
    fn process_segment_with_parameters(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, parameters, &[], clock)
    }
    fn process_segment_with_notes(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.stage_segment(node, offset, frames, parameters, notes, clock, None)
    }

    fn process_segment_with_modulation(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        lane: ForeignModulationSamples<'_>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.stage_segment(node, offset, frames, &[], &[], clock, Some(lane))
    }

    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        // Scalar compensated rendering only finishes staging on the last
        // segment. Avoid comparing processor facts for an incomplete callback.
        self.transport.has_complete_staged_callback()
            && self.reported_tail_is_drained()
            && self.matches(node)
    }

    fn reject_callback(&mut self) {
        self.transport.reject(Fault::Revoked);
    }
}

impl NativeEffectCallback {
    #[allow(clippy::too_many_arguments)] // Separate borrowed views share one physical acceptance.
    pub(super) fn stage_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        lane: Option<ForeignModulationSamples<'_>>,
    ) -> Result<(), DeviceContractError> {
        if !self.matches(node)
            || lane.is_some_and(|lane| Some(lane.id) != self.configuration.dense_raw_id)
            || node.automation.is_some_and(|program| {
                program.binding != self.configuration.binding
                    || !self.configuration.authored.has_metadata_container()
                    || !parameters.is_empty()
            })
            || (!notes.is_empty() && !self.configuration.prepared.is_instrument_profile())
            || frames.len() > MAX_FRAMES
            || !node.input_gain.is_finite()
            || node.input_gain < 0.0
            || !node.output_gain.is_finite()
            || node.output_gain < 0.0
        {
            self.reject_callback();
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        let mut events;
        let events: &[MidiEvent] = if notes.is_empty() {
            &[]
        } else {
            events = InputEvents::default();
            for note in notes {
                let status = if note.on { 0x90 } else { 0x80 };
                if note.channel >= 16
                    || events
                        .push(MidiEvent {
                            frame: note.frame,
                            bytes: [status | note.channel, note.pitch, note.velocity],
                            len: 3,
                            note_id: note.note_id,
                        })
                        .is_err()
                {
                    self.reject_callback();
                    frames.fill([0.0; 2]);
                    return Err(DeviceContractError::ForeignProcessingFailed);
                }
            }
            events.as_slice()
        };
        if self.configuration.prepared.is_instrument_profile() {
            frames.fill([0.0; 2]);
        }
        self.input[..frames.len()].copy_from_slice(frames);
        // Keep the untrimmed source in dry history; wet input trim must not
        // contaminate samples heard after a later aligned bypass transition.
        // The aligned dry output also serves a staging failure (#3977).
        self.tail.input(frames);
        self.advance_dry(frames);
        if node.input_gain != 1.0 {
            for frame in &mut self.input[..frames.len()] {
                frame[0] *= node.input_gain;
                frame[1] *= node.input_gain;
            }
        }
        // The original joining worker retains processor parameter observations
        // for control queries; the native helper mirrors them to its controller.
        // Receiving that feedback here must not revoke an otherwise valid epoch.
        let playing =
            if node.prepared.is_instrument_profile() || node.automation.is_some() || lane.is_some()
            {
                clock.and_then(|clock| clock.transport_is_running())
            } else {
                None
            };
        let timing = SegmentTiming {
            playing,
            at: |index| {
                let clock = clock?;
                Some(ProcessTiming {
                    arrangement_frame: i64::try_from(clock.arrangement_frame_at_offset(index)?)
                        .ok()?,
                    playing: if node.automation.is_some()
                        && clock.authored_arrangement_frame_at_offset(index).is_some()
                    {
                        true
                    } else {
                        playing.or_else(|| clock.transport_is_running())?
                    },
                    arrangement_beat_raw: clock.arrangement_beat_at_offset(index),
                    tempo_micro_bpm: clock
                        .tempo_micro_bpm_at_offset(index)
                        .and_then(std::num::NonZeroU32::new),
                    meter: clock.arrangement_meter_at_offset(index),
                    cycle_beats_raw: clock.arrangement_cycle_at_offset(index),
                })
            },
        };
        let input = &self.input[..frames.len()];
        let authored_program = node.automation.map(automation::TransportProgram);
        let authored_origin = |index| clock?.authored_arrangement_frame_at_offset(index);
        let manual_origin = |index| {
            let clock = clock?;
            Some(ManualParameterOrigin {
                arrangement_frame: clock.authored_arrangement_frame_at_offset(index)?,
                performance_frame: clock.performance_frame_at_offset(index)?,
                loop_pass: clock.loop_pass_at_offset(index)?,
            })
        };
        let result = self.transport.stage_effect_pcm_segment(
            offset,
            input,
            events,
            parameters,
            frames,
            timing,
            EffectSegmentOrigins {
                authored: authored_program.as_ref().map(|program| AuthoredSegment {
                    program,
                    origin: &authored_origin,
                }),
                manual: Some(&manual_origin),
            },
            lane.map(|lane| hex_plugin_host::transport::DenseParameterLane {
                id: lane.id,
                normalized: lane.normalized,
            }),
        );
        let Ok(concealed_output) = result else {
            // The transport revoked only this endpoint's epoch. Leave the
            // aligned dry input so a containing carrier can keep rendering.
            frames.copy_from_slice(&self.dry_output[..frames.len()]);
            if !node.bypassed {
                for frame in frames.iter_mut() {
                    frame[0] *= node.output_gain;
                    frame[1] *= node.output_gain;
                }
            }
            return Err(DeviceContractError::ForeignProcessingFailed);
        };
        if let Some((start, end)) = concealed_output {
            // The transport still owns this epoch. Replace just the missing
            // wet samples with the same latency-aligned dry history used for
            // bypass. An instrument's dry history is already silence.
            frames[start..end].copy_from_slice(&self.dry_output[start..end]);
            self.concealed_output = true;
        }
        // Both paths advance. Refusal irreversibly revokes this epoch, so dry
        // and foreign history are never rolled back or replayed.
        self.bypass.process(
            frames,
            &self.dry_output[..frames.len()],
            node.bypassed,
            node.sample_rate,
            node.output_gain,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dense_tests;
