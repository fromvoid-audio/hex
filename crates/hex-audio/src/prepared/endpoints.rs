//! One constructor-owned client bundle and one publication bundle. Their guards
//! transfer whole endpoint Boxes to lifecycle custody; closing a client never
//! destroys a ring, preview PCM, or a native stream on the caller's thread.
use super::*;
use crate::native_callback_owner::{Guard, Terminal, reserve};

pub struct Client {
    pub(super) endpoints: Guard<ClientEndpoints>,
    pub(super) retired_receipts: Option<Arc<retired_scalar_receipts::Mailbox>>,
    action_outcomes: Option<ActionOutcomeReader>,
    worker_owned_publication: bool,
    project_generation: Option<u64>,
}
pub struct WorkerControl {
    endpoint: Guard<PublicationControl>,
}
pub(crate) struct ClientEndpoints {
    diagnostics: audio_diagnostics::TraceReader,
    action_admission: Option<action_outcomes::Admission>,
    pub(super) output_identity: u64,
    pub(super) sample_preview: crate::sample_preview::Control,
    sample_recording: crate::sample_recording::Control,
    pub(super) resident_forwarding: Arc<resident_forwarding::ResidentForwarding>,
    spectrum_reader: Option<crate::spectrum_worker::SpectrumReader>,
    commands: Producer<QueuedCommand>,
    spectrum_target: spectrum_target::SpectrumTargetControl,
    processor_audition: processor_audition::ProcessorAuditionTargetControl,
    pub(super) gain_recording: crate::gain_recording::Control,
    pub(super) native_parameter_input: crate::native_parameter_input::Control,
    pub(crate) retained_receipt_target: Option<ScalarReceiptTarget>,
    midi_recording: crate::midi_recording::Control,
    pub(super) status: Arc<Status>,
    command_ticket: u64,
    #[cfg(all(test, feature = "allocation-probe"))]
    probe: Option<crate::native_callback_owner::DropProbe>,
}
pub(crate) struct EndpointTerminals {
    pub(crate) receipts: retired_scalar_receipts::ReceiptPublisher,
    pub(crate) action_failure: Option<action_outcomes::FailureSignal>,
    pub(crate) client: Terminal<ClientEndpoints>,
    pub(crate) publication: Terminal<PublicationControl>,
}
impl Control {
    pub(crate) fn reserve_worker_endpoints(
        self,
        spectrum_reader: Option<crate::spectrum_worker::SpectrumReader>,
    ) -> (Client, WorkerControl, EndpointTerminals) {
        let Self {
            diagnostics,
            action_admission,
            sample_preview,
            sample_recording,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            native_parameter_input,
            midi_recording,
            command_ticket,
            mut publication,
        } = self;
        publication.status.split_endpoints.store(true, Release);
        let output_identity = publication.candidate_root.root_id();
        let status = Arc::clone(&publication.status);
        let resident_forwarding = publication
            .resident_forwarding
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(resident_forwarding::ResidentForwarding::new);
        publication.resident_forwarding = Some(Arc::clone(&resident_forwarding));
        let (retired_receipts, receipts) = retired_scalar_receipts::channel();
        let action_failure = action_admission
            .as_ref()
            .map(action_outcomes::Admission::failure_signal);
        let (endpoints, client) = reserve(ClientEndpoints {
            diagnostics,
            action_admission,
            output_identity,
            sample_preview,
            sample_recording,
            resident_forwarding,
            spectrum_reader,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            native_parameter_input,
            midi_recording,
            status,
            command_ticket,
            retained_receipt_target: None,
            #[cfg(all(test, feature = "allocation-probe"))]
            probe: None,
        });
        let (endpoint, publication) = reserve(publication);
        (
            Client {
                endpoints,
                retired_receipts: Some(retired_receipts),
                action_outcomes: None,
                worker_owned_publication: false,
                project_generation: None,
            },
            WorkerControl { endpoint },
            EndpointTerminals {
                receipts,
                action_failure,
                client,
                publication,
            },
        )
    }
}
impl WorkerControl {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_drop_probe(&mut self, probe: impl FnMut() + Send + 'static) {
        if let Some(control) = self.endpoint.owner_mut() {
            control.endpoint_probe = Some(crate::native_callback_owner::DropProbe(Box::new(probe)));
        }
    }

    /// Worker-side access only. The UI receives Client, never this endpoint.
    pub fn control(&mut self) -> Option<&mut PublicationControl> {
        self.endpoint.owner_mut()
    }
    /// Borrow active ordinary resource obligations without projecting the output, source owners
    /// or publication endpoint into a consumer. Used synchronously on its owning worker only.
    pub fn device_resource_admission_on_control(
        &self,
    ) -> Option<hex_engine::native_device_resources::DeviceResourceAdmission<'_>> {
        self.endpoint
            .owner_ref()?
            .device_resource_admission_on_control()
    }
    pub fn unreturned_resident_graphs_on_control(&self) -> Option<usize> {
        self.endpoint
            .owner_ref()?
            .unreturned_resident_graphs_on_control()
    }
    pub fn close(&mut self) {
        if let Some(control) = self.endpoint.owner_mut() {
            control.request_shutdown();
        }
        self.endpoint.close();
    }
    pub(crate) fn owner_ref(&self) -> Option<&PublicationControl> {
        self.endpoint.owner_ref()
    }
    pub(crate) fn guard(&self) -> &Guard<PublicationControl> {
        &self.endpoint
    }
}
impl Drop for WorkerControl {
    fn drop(&mut self) {
        self.close();
    }
}
impl Client {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_drop_probe(&mut self, probe: impl FnMut() + Send + 'static) {
        if let Some(endpoints) = self.endpoints.owner_mut() {
            endpoints.probe = Some(crate::native_callback_owner::DropProbe(Box::new(probe)));
        }
    }

    pub(crate) fn activate(&mut self) {
        if let Some(endpoints) = self.endpoints.owner_ref() {
            self.worker_owned_publication = endpoints
                .resident_forwarding
                .expose_client_on_control(&endpoints.status);
        }
    }
    /// Immutable publication policy captured under the existing exposure lock. Every graph on
    /// this output belongs to its control worker; transport and shared input clients remain
    /// ordinary. Reading this snapshot never locks or grants graph/resource ownership.
    pub fn worker_owns_graph_publication(&self) -> bool {
        self.worker_owned_publication
    }
    /// Bind queued actions to one Resident document on this output. The
    /// desktop binds a delivered worker-owned Client. A retained Client may
    /// change its binding after its successor graph is observed. Priority
    /// Stop remains an output-wide operation with its own cutoff.
    pub fn bind_resident_transport(&mut self, generation: u64) {
        self.project_generation = Some(generation);
    }
    pub(crate) fn guard(&self) -> &Guard<ClientEndpoints> {
        &self.endpoints
    }
    pub(crate) fn owner_ref(&self) -> Option<&ClientEndpoints> {
        self.endpoints.owner_ref()
    }
    /// Control/UI configuration only; trace data is never an audio ACK.
    pub fn set_audio_diagnostics(&self, flags: AudioDiagnosticsFlags) {
        if let Some(status) = self.status() {
            status.diagnostics_flags.store(flags.bits(), Relaxed);
        }
    }
    /// Take one Copy record without a wait. Callers must bound each poll.
    pub fn take_audio_diagnostic(&mut self) -> Option<AudioDiagnosticEvent> {
        self.endpoints.owner_mut()?.diagnostics.pop()
    }
    /// A borrowed observation, with no cloneable Status/endpoint owner escape.
    pub fn status(&self) -> Option<&Status> {
        self.endpoints.owner_ref().map(|e| e.status.as_ref())
    }
    /// Actual channel incarnation for view/request correlation only.
    pub fn output_identity(&self) -> Option<u64> {
        self.endpoints.owner_ref().map(|e| e.output_identity)
    }
    /// Attach only this output's unique reader. Refusal returns the original
    /// reader unchanged; the control owner must drain it before retirement.
    pub fn attach_action_outcomes(
        &mut self,
        reader: ActionOutcomeReader,
    ) -> Result<(), ActionOutcomeReader> {
        if self.action_outcomes.is_some()
            || self.output_identity() != Some(reader.output_identity())
        {
            return Err(reader);
        }
        self.action_outcomes = Some(reader);
        Ok(())
    }
    pub fn action_outcomes_mut(&mut self) -> Option<&mut ActionOutcomeReader> {
        self.action_outcomes.as_mut()
    }
    pub fn has_action_outcomes(&self) -> bool {
        self.action_outcomes.is_some()
    }
    /// Transfer the observer before dropping this client when retirement must
    /// continue to report terminals after the endpoint has been closed.
    pub fn take_action_outcomes(&mut self) -> Option<ActionOutcomeReader> {
        self.action_outcomes.take()
    }
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        capture_runtime_fence(self.status()?)
    }
    pub fn close(&mut self) {
        if let Some(endpoints) = self.endpoints.owner_ref() {
            endpoints.midi_recording.cancel();
            endpoints.sample_recording.stop();
            endpoints.status.closing.store(true, Release);
            endpoints.stop();
        }
        self.endpoints.close();
    }
    /// Exact scalar activation-clock correlation for retained UI input after output recovery.
    pub fn computer_keyboard_input_matches_output(
        &self,
        input: &hex_engine::computer_keyboard::ComputerKeyboardInput,
    ) -> bool {
        self.endpoints
            .owner_ref()
            .is_some_and(|endpoints| input.bound_to_clock(&endpoints.status.keyboard_activation))
    }
    /// Control-only keyboard allocation. The returned input retains a scalar epoch clock,
    /// never this client, Status, graph, resources or callback ownership.
    pub fn new_computer_keyboard_input_on_control(
        &self,
    ) -> Result<hex_engine::computer_keyboard::ComputerKeyboardInput, hex_engine::Invalid> {
        let endpoints = self
            .endpoints
            .owner_ref()
            .ok_or(hex_engine::Invalid("audio output is closed"))?;
        if endpoints.status.closing.load(Acquire) {
            return Err(hex_engine::Invalid("audio output is closing"));
        }
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_output_activation(Arc::clone(
            &endpoints.status.keyboard_activation,
        ))
    }
    pub fn stop(&self) {
        if let Some(endpoints) = self.endpoints.owner_ref() {
            endpoints.stop();
        }
    }
    pub fn action_with_ticket(&mut self, action: PlaybackAction) -> Result<u64, PlaybackAction> {
        let Some(endpoints) = self.endpoints.owner_mut() else {
            return Err(action);
        };
        if !endpoints.status.client_exposed.load(Acquire) {
            return Err(action);
        }
        enqueue_action(
            &mut endpoints.commands,
            &mut endpoints.action_admission,
            &mut endpoints.command_ticket,
            &endpoints.status,
            CommandSource {
                revision: endpoints.status.submitted_revision.load(Acquire),
                project_generation: self.project_generation,
            },
            &mut endpoints.processor_audition,
            action,
        )
    }
    pub fn command_with_ticket(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<u64, PlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Transport(command))
            .map_err(|_| command)
    }
    pub fn command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackCommand> {
        self.command_with_ticket(command).map(|_| ())
    }
    pub fn session_command_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Session(command))
            .map_err(|_| command)
    }
    pub fn launch_and_play_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::LaunchAndPlay(command))
            .map_err(|_| command)
    }
    /// Apply a linear level to the preview bus only.
    pub fn preview_set_gain(&mut self, gain: f32) -> Result<(), Invalid> {
        self.endpoints
            .owner_mut()
            .ok_or(Invalid("audio client is closed"))?
            .sample_preview
            .set_gain(gain)
    }
    pub fn preview_request(&mut self) -> Result<u64, Invalid> {
        self.endpoints
            .owner_mut()
            .ok_or(Invalid("audio client is closed"))?
            .sample_preview
            .request()
    }
    pub fn preview_publish(
        &mut self,
        generation: u64,
        audio: hex_engine::Audio,
    ) -> Result<(), crate::sample_preview::PublishRefusal> {
        self.preview_publish_from(generation, audio, 0)
    }
    pub fn preview_publish_from(
        &mut self,
        generation: u64,
        audio: hex_engine::Audio,
        frame: usize,
    ) -> Result<(), crate::sample_preview::PublishRefusal> {
        let Some(endpoints) = self.endpoints.owner_mut() else {
            return Err(crate::sample_preview::PublishRefusal {
                reason: Invalid("audio client is closed"),
                audio,
            });
        };
        endpoints
            .sample_preview
            .publish_from(generation, audio, frame)
    }
    pub fn preview_publish_loop(
        &mut self,
        generation: u64,
        audio: hex_engine::Audio,
    ) -> Result<(), crate::sample_preview::PublishRefusal> {
        let Some(endpoints) = self.endpoints.owner_mut() else {
            return Err(crate::sample_preview::PublishRefusal {
                reason: Invalid("audio client is closed"),
                audio,
            });
        };
        endpoints.sample_preview.publish_loop(generation, audio)
    }

    pub fn preview_cancel(&mut self) {
        if let Some(e) = self.endpoints.owner_mut() {
            e.sample_preview.cancel();
        }
    }
    pub fn preview_snapshot(&self) -> Option<crate::sample_preview::Snapshot> {
        self.endpoints
            .owner_ref()
            .map(|e| e.sample_preview.snapshot())
    }
    /// Resident compatibility only; the Verified constructor seals preview before
    /// this client becomes visible and can never place owning PCM here.
    pub fn reclaim_preview(&mut self) {
        if let Some(e) = self.endpoints.owner_mut() {
            e.sample_preview.reclaim();
        }
    }
    pub fn start_sample_recording(
        &mut self,
        project_generation: u64,
        outputs: &[crate::sample_recording::OutputCaptureId],
        main: bool,
    ) -> Result<crate::sample_recording::Reader, Invalid> {
        let endpoints = self
            .endpoints
            .owner_mut()
            .ok_or(Invalid("audio client is closed"))?;
        if endpoints.status.closing.load(Acquire) || endpoints.status.device_fault.load(Acquire) {
            return Err(Invalid("sample recording output is unavailable"));
        }
        endpoints
            .sample_recording
            .start(project_generation, outputs, main)
    }

    pub fn sample_recording_phase(&self) -> Option<crate::sample_recording::Phase> {
        self.endpoints.owner_ref()?.sample_recording.phase()
    }

    pub fn stop_sample_recording(&self) {
        if let Some(endpoints) = self.endpoints.owner_ref() {
            endpoints.sample_recording.stop();
        }
    }

    pub fn reclaim_sample_recording(&mut self) {
        if let Some(endpoints) = self.endpoints.owner_mut() {
            endpoints.sample_recording.reclaim();
        }
    }

    pub fn midi_target(&self) -> Option<crate::midi_recording::Target> {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.midi_recording.target())
    }
    pub fn midi_begin(
        &mut self,
        target: crate::midi_recording::Target,
    ) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .midi_recording
            .begin(target)
    }
    pub fn midi_finish(&self) {
        if let Some(e) = self.endpoints.owner_ref() {
            e.midi_recording.finish();
        }
    }
    pub fn midi_cancel(&self) {
        if let Some(e) = self.endpoints.owner_ref() {
            e.midi_recording.cancel();
        }
    }
    pub fn midi_receipt(&mut self) -> Option<crate::midi_recording::Receipt> {
        self.endpoints
            .owner_mut()
            .and_then(|e| e.midi_recording.try_receipt())
    }
    pub fn native_parameter_target(&self) -> Option<crate::native_parameter_input::Target> {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.native_parameter_input.target())
    }
    pub fn native_parameter_begin(
        &mut self,
        target: crate::native_parameter_input::Target,
        value: hex_lattice::NativeTransientValue,
    ) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .native_parameter_input
            .begin(target, value)
    }
    pub fn native_parameter_change(
        &mut self,
        value: hex_lattice::NativeTransientValue,
    ) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .native_parameter_input
            .change(value)
    }
    pub fn native_parameter_release(&mut self) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .native_parameter_input
            .release()
    }
    pub fn native_parameter_cancel(&mut self) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .native_parameter_input
            .cancel()
    }
    pub fn native_parameter_receipt(&mut self) -> Option<crate::native_parameter_input::Receipt> {
        self.endpoints
            .owner_mut()?
            .native_parameter_input
            .try_receipt()
    }
    pub fn gain_target(&self) -> Option<crate::gain_recording::Target> {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.gain_recording.target())
    }
    pub fn gain_begin(
        &mut self,
        target: crate::gain_recording::Target,
        gain: f32,
    ) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .gain_recording
            .begin(target, gain)
    }
    pub fn gain_change(&mut self, gain: f32) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .gain_recording
            .change(gain)
    }
    pub fn gain_release(&mut self) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .gain_recording
            .release()
    }
    pub fn gain_cancel(&mut self) -> Result<(), &'static str> {
        self.endpoints
            .owner_mut()
            .ok_or("audio client is closed")?
            .gain_recording
            .cancel()
    }
    pub fn gain_receipt(&mut self) -> Option<crate::gain_recording::Receipt> {
        self.endpoints
            .owner_mut()
            .and_then(|e| e.gain_recording.try_receipt())
    }
    pub fn spectrum_latest(&mut self) -> Option<crate::spectrum_worker::PublishedSpectrum> {
        self.endpoints
            .owner_mut()?
            .spectrum_reader
            .as_mut()?
            .try_latest()
    }
    pub fn project_spectrum_available(&self) -> bool {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.spectrum_reader.as_ref())
            .is_some_and(|r| r.project_available())
    }
    pub fn project_spectrum_select(
        &self,
        tracks: &[u64],
        generation: u64,
        floor: u64,
        rate: u32,
    ) -> Result<crate::spectrum_worker::ProjectSelection, &'static str> {
        if !self.project_spectrum_available() {
            return Err("project spectrum unavailable");
        }
        self.endpoints
            .owner_ref()
            .ok_or("audio client is closed")?
            .status
            .project_spectrum
            .select(tracks, generation, floor, rate)
    }
    pub fn project_spectrum_clear(&self) -> Result<(), &'static str> {
        self.endpoints
            .owner_ref()
            .ok_or("audio client is closed")?
            .status
            .project_spectrum
            .clear()
    }
    pub fn project_spectrum_desired(&self) -> Option<crate::spectrum_worker::ProjectSelection> {
        self.endpoints
            .owner_ref()?
            .status
            .project_spectrum
            .read()
            .filter(|s| s.active)
    }
    pub fn project_spectrum_latest(
        &mut self,
    ) -> Option<crate::spectrum_worker::PublishedProjectSpectrum> {
        let endpoints = self.endpoints.owner_mut()?;
        let selection = endpoints
            .status
            .project_spectrum
            .read()
            .filter(|s| s.active)?;
        endpoints
            .spectrum_reader
            .as_mut()?
            .try_latest_project_at(Instant::now(), selection)
    }
    /// One fresh, aligned pair; input is absent for legacy output-only captures.
    pub fn spectrum_latest_pair(&mut self) -> Option<crate::spectrum_worker::PublishedSpectra> {
        self.endpoints
            .owner_mut()?
            .spectrum_reader
            .as_mut()?
            .try_latest_pair()
    }
    pub fn spectrum_desired(&self) -> Option<spectrum_target::SpectrumTarget> {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.spectrum_target.desired())
    }
    /// Current complete analyzer membership for this guarded output incarnation.
    pub fn spectrum_desired_targets(&self) -> Option<&spectrum_target::SpectrumTargets> {
        self.endpoints
            .owner_ref()
            .map(|e| e.spectrum_target.desired_targets())
    }
    pub fn spectrum_set_targets(
        &mut self,
        requests: &[spectrum_target::SpectrumTargetRequest],
    ) -> Result<spectrum_target::TargetUpdate, spectrum_target::TargetError> {
        self.endpoints
            .owner_mut()
            .ok_or(spectrum_target::TargetError::OutputClosed)?
            .spectrum_target
            .set_targets(requests)
    }
    /// Fresh independent pairs, filtered by this endpoint's exact desired tokens,
    /// generation, rate and publication floors. The bounded cache lives on control.
    pub fn spectrum_latest_pairs(&mut self) -> Vec<crate::spectrum_worker::PublishedSpectra> {
        let mut pairs = Vec::new();
        self.spectrum_latest_pairs_into(&mut pairs);
        pairs
    }
    /// Replace `out` with the latest fresh pairs, reusing its capacity. A
    /// closed output or absent reader leaves it empty.
    pub fn spectrum_latest_pairs_into(
        &mut self,
        out: &mut Vec<crate::spectrum_worker::PublishedSpectra>,
    ) {
        out.clear();
        let Some(endpoints) = self.endpoints.owner_mut() else {
            return;
        };
        let Some(reader) = endpoints.spectrum_reader.as_mut() else {
            return;
        };
        reader.try_latest_pairs_into(
            Instant::now(),
            endpoints.spectrum_target.desired_targets(),
            out,
        );
    }
    pub fn spectrum_select(
        &mut self,
        identity: PostNodeIdentity,
        generation: u64,
        floor: u64,
        rate: u32,
    ) -> Result<
        (
            spectrum_target::SpectrumTarget,
            spectrum_target::TargetUpdate,
        ),
        spectrum_target::TargetError,
    > {
        self.endpoints
            .owner_mut()
            .ok_or(spectrum_target::TargetError::OutputClosed)?
            .spectrum_target
            .select(identity, generation, floor, rate)
    }
    pub fn spectrum_clear(
        &mut self,
    ) -> Result<spectrum_target::TargetUpdate, spectrum_target::TargetError> {
        self.endpoints
            .owner_mut()
            .ok_or(spectrum_target::TargetError::OutputClosed)?
            .spectrum_target
            .clear()
    }
    pub fn spectrum_retry_pending(&mut self) -> Option<spectrum_target::TargetUpdate> {
        self.endpoints
            .owner_mut()
            .map(|e| e.spectrum_target.retry_pending())
    }
    pub fn audition_desired(&self) -> Option<processor_audition::ProcessorAuditionTarget> {
        self.endpoints
            .owner_ref()
            .and_then(|e| e.processor_audition.desired())
    }
    pub fn audition_select(
        &mut self,
        target: processor_audition::ProcessorAuditionTarget,
    ) -> Result<processor_audition::TargetUpdate, processor_audition::TargetError> {
        self.endpoints
            .owner_mut()
            .ok_or(processor_audition::TargetError::OutputClosed)?
            .processor_audition
            .select(
                target.identity,
                target.project_generation,
                target.activation_floor,
                target.sample_rate_hz,
                target.selection,
            )
            .map(|(_, update)| update)
    }
    pub fn audition_release(
        &mut self,
    ) -> Result<processor_audition::TargetUpdate, processor_audition::TargetError> {
        self.endpoints
            .owner_mut()
            .ok_or(processor_audition::TargetError::OutputClosed)?
            .processor_audition
            .release()
    }
    pub fn audition_cancel(
        &mut self,
    ) -> Result<processor_audition::TargetUpdate, processor_audition::TargetError> {
        self.endpoints
            .owner_mut()
            .ok_or(processor_audition::TargetError::OutputClosed)?
            .processor_audition
            .cancel_immediately()
    }
    pub fn audition_retry_pending(&mut self) -> Option<processor_audition::TargetUpdate> {
        self.endpoints
            .owner_mut()
            .map(|e| e.processor_audition.retry_pending())
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}
impl ClientEndpoints {
    pub(crate) fn has_action_admission(&self) -> bool {
        self.action_admission.is_some()
    }
    pub(crate) fn action_admission_identity(&self) -> Option<usize> {
        self.action_admission
            .as_ref()
            .map(action_outcomes::Admission::identity)
    }
    pub(super) fn take_action_admission(&mut self) -> Option<action_outcomes::Admission> {
        self.action_admission.take()
    }
    pub(super) fn restore_action_admission(&mut self, admission: action_outcomes::Admission) {
        self.action_admission = Some(admission);
    }
    fn stop(&self) {
        self.midi_recording.finish();
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        self.status
            .pending_stop_marker
            .store(self.command_ticket.saturating_add(1), Release);
        self.processor_audition.invalidate_current_immediately();
    }
    pub(crate) fn native_spectrum_identity(
        &self,
    ) -> Option<Option<crate::spectrum_worker::NativeSpectrumIdentity>> {
        self.spectrum_reader
            .as_ref()
            .map_or(Some(None), |reader| reader.native_identity().map(Some))
    }
    pub(crate) fn matches_retired_scalar_output(
        &self,
        status_identity: usize,
        output: u64,
    ) -> bool {
        Arc::as_ptr(&self.status) as usize == status_identity && self.output_identity == output
    }
    pub(crate) fn matches(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
    }
}
struct CommandSource {
    revision: u64,
    project_generation: Option<u64>,
}

fn enqueue_action(
    commands: &mut Producer<QueuedCommand>,
    admission: &mut Option<action_outcomes::Admission>,
    ticket: &mut u64,
    status: &Status,
    source: CommandSource,
    audition: &mut processor_audition::ProcessorAuditionTargetControl,
    action: PlaybackAction,
) -> Result<u64, PlaybackAction> {
    // Stamp before admission so a command racing a fault is fenced even if
    // the output installs a replacement before the callback consumes it.
    let transport_epoch = status.transport_epoch.load(Acquire);
    let sampler_edit_gate = status.sampler_edit.gate();
    if sampler_edit_gate & 1 != 0 && !sampler_edit::is_stop(action) {
        return Err(action);
    }
    if status.closing.load(Acquire)
        || (status.device_fault.load(Acquire) && !sampler_edit::is_stop(action))
        || source.revision == 0
        || source.project_generation.is_some_and(|generation| {
            status
                .recording_source()
                .is_some_and(|active| active.context.project_generation != generation)
        })
    {
        return Err(action);
    }
    let Some(next) = ticket.checked_add(1).filter(|ticket| *ticket != u64::MAX) else {
        return Err(action);
    };
    if let Some(admission) = admission.as_mut()
        && !admission.reserve(next)
    {
        return Err(action);
    }
    if let Err(PushError::Full(queued)) = commands.push(QueuedCommand {
        ticket: next,
        action,
        project_generation: source.project_generation,
        sampler_edit_gate,
        transport_epoch,
    }) {
        if let Some(admission) = admission.as_mut() {
            admission.rollback(next);
        }
        return Err(queued.action);
    }
    *ticket = next;
    status.admitted_ticket.store(next, Release);
    if matches!(action, PlaybackAction::Transport(PlaybackCommand::Stop)) {
        status.sampler_keyboard.revoke_all();
    }

    if action_invalidates_processor_audition(action) {
        let _ = audition.cancel_immediately();
    }
    Ok(next)
}
pub(super) fn legacy_enqueue(
    control: &mut Control,
    action: PlaybackAction,
) -> Result<u64, PlaybackAction> {
    enqueue_action(
        &mut control.commands,
        &mut control.action_admission,
        &mut control.command_ticket,
        &control.publication.status,
        CommandSource {
            revision: control.publication.revision,
            project_generation: None,
        },
        &mut control.processor_audition,
        action,
    )
}

pub(super) fn constructor_endpoint_storage_quote() -> Option<usize> {
    std::mem::size_of::<ClientEndpoints>()
        .checked_add(std::mem::size_of::<PublicationControl>())?
        .checked_add(std::mem::size_of::<
            std::sync::OnceLock<std::sync::Mutex<Box<ClientEndpoints>>>,
        >())?
        .checked_add(std::mem::size_of::<
            std::sync::OnceLock<std::sync::Mutex<Box<PublicationControl>>>,
        >())?
        .checked_add(std::mem::size_of::<Client>())?
        .checked_add(std::mem::size_of::<WorkerControl>())?
        .checked_add(resident_forwarding::constructor_storage_quote())?
        .checked_add(retired_scalar_receipts::storage_bytes())
}
