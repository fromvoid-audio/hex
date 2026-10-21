//! One constructor-owned client bundle and one publication bundle. Their guards
//! transfer whole endpoint Boxes to lifecycle custody; closing a client never
//! destroys a ring, preview PCM, or a native stream on the caller's thread.
use super::*;
use crate::native_callback_owner::{Guard, Terminal, reserve};

pub struct Client {
    endpoints: Guard<ClientEndpoints>,
}
pub struct WorkerControl {
    endpoint: Guard<PublicationControl>,
}
pub(crate) struct ClientEndpoints {
    output_identity: u64,
    pub(super) sample_preview: crate::sample_preview::Control,
    pub(super) resident_forwarding: Arc<resident_forwarding::ResidentForwarding>,
    spectrum_reader: Option<crate::spectrum_worker::SpectrumReader>,
    commands: Producer<QueuedCommand>,
    spectrum_target: spectrum_target::SpectrumTargetControl,
    processor_audition: processor_audition::ProcessorAuditionTargetControl,
    gain_recording: crate::gain_recording::Control,
    pub(super) status: Arc<Status>,
    command_ticket: u64,
    #[cfg(all(test, feature = "allocation-probe"))]
    probe: Option<crate::native_callback_owner::DropProbe>,
}
pub(crate) struct EndpointTerminals {
    pub(crate) client: Terminal<ClientEndpoints>,
    pub(crate) publication: Terminal<PublicationControl>,
}
impl Control {
    pub(crate) fn reserve_worker_endpoints(
        self,
        spectrum_reader: Option<crate::spectrum_worker::SpectrumReader>,
    ) -> (Client, WorkerControl, EndpointTerminals) {
        let Self {
            sample_preview,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            command_ticket,
            mut publication,
        } = self;
        publication.status.split_endpoints.store(true, Release);
        let output_identity = publication.candidate_root.root_id();
        let status = Arc::clone(&publication.status);
        let resident_forwarding = resident_forwarding::ResidentForwarding::new();
        publication.resident_forwarding = Some(Arc::clone(&resident_forwarding));
        let (endpoints, client) = reserve(ClientEndpoints {
            output_identity,
            sample_preview,
            resident_forwarding,
            spectrum_reader,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            status,
            command_ticket,
            #[cfg(all(test, feature = "allocation-probe"))]
            probe: None,
        });
        let (endpoint, publication) = reserve(publication);
        (
            Client { endpoints },
            WorkerControl { endpoint },
            EndpointTerminals {
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
            endpoints.status.client_exposed.store(true, Release);
        }
    }
    pub(crate) fn guard(&self) -> &Guard<ClientEndpoints> {
        &self.endpoints
    }
    pub(crate) fn owner_ref(&self) -> Option<&ClientEndpoints> {
        self.endpoints.owner_ref()
    }
    /// A borrowed observation, with no cloneable Status/endpoint owner escape.
    pub fn status(&self) -> Option<&Status> {
        self.endpoints.owner_ref().map(|e| e.status.as_ref())
    }
    /// Actual channel incarnation for view/request correlation only.
    pub fn output_identity(&self) -> Option<u64> {
        self.endpoints.owner_ref().map(|e| e.output_identity)
    }
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        capture_runtime_fence(self.status()?)
    }
    pub fn close(&mut self) {
        if let Some(endpoints) = self.endpoints.owner_ref() {
            endpoints.status.closing.store(true, Release);
            endpoints.stop();
        }
        self.endpoints.close();
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
            &mut endpoints.command_ticket,
            &endpoints.status,
            endpoints.status.submitted_revision.load(Acquire),
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
    fn stop(&self) {
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
    pub(crate) fn matches(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
    }
}
fn enqueue_action(
    commands: &mut Producer<QueuedCommand>,
    ticket: &mut u64,
    status: &Status,
    revision: u64,
    audition: &mut processor_audition::ProcessorAuditionTargetControl,
    action: PlaybackAction,
) -> Result<u64, PlaybackAction> {
    if status.closing.load(Acquire) || revision == 0 {
        return Err(action);
    }
    let Some(next) = ticket.checked_add(1).filter(|ticket| *ticket != u64::MAX) else {
        return Err(action);
    };
    commands
        .push(QueuedCommand {
            ticket: next,
            action,
        })
        .map_err(|PushError::Full(q)| q.action)?;
    *ticket = next;
    status.admitted_ticket.store(next, Release);
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
        &mut control.command_ticket,
        &control.publication.status,
        control.publication.revision,
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
        .checked_add(resident_forwarding::constructor_storage_quote())
}
