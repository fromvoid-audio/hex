//! Desktop-injected native publication, driven only by the canonical worker.
//! Session owns no device or page endpoints and never interprets their tokens.
use super::*;
use std::time::Duration;
pub(super) mod native_sampler;
pub use native_sampler::{
    NativeSamplerInput, NativeSamplerReservationHold, NativeSamplerRetryFacts,
};

/// Scalar identity for one worker-private ordinary resident graph. Neither preview correlation
/// nor a requested freshness fence grants source custody or acknowledges callback application.
#[derive(Clone, Copy, Debug)]
pub struct ResidentPublicationMetadata {
    pub context: hex_engine::AudioSourceContext,
    pub device_preview: Option<DevicePreviewIdentity>,
    pub tuning_preview: Option<TuningPreviewIdentity>,
    pub seek_request: Option<u64>,
    pub stopped_restore: bool,
    pub computer_keyboard_request: Option<u64>,
    pub gain_recording_track: Option<hex_project::TrackId>,
    pub requires_preroll: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct ResidentFreshRuntimeFence {
    pub output_identity: u64,
    pub active_audio_revision: u64,
    pub route_generation: u64,
    pub admitted_command_ticket: u64,
}
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationPolicy {
    InheritRuntime,
    /// Existing explicit fresh installation used after priority Stop restoration.
    InstallFreshRuntime,
    /// New/Open only: admit one exact pending owner before canonical generation replacement.
    /// Acceptance is OfflineRetained; existing teardown/open then queues it on the fresh output.
    /// This never converts ordinary queue pressure into offline acceptance.
    ReplaceDocument,
    FreshIfUnchanged(ResidentFreshRuntimeFence),
}
/// Successful worker acceptance is either an actual queue submission or explicit offline custody.
/// Only the callback's exact output/revision observation establishes application.
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationAcceptance {
    Submitted {
        output_identity: u64,
        audio_revision: u64,
    },
    OfflineRetained,
}
pub struct ResidentPublicationRefusal {
    pub playback: Playback,
    pub error: String,
}
impl std::fmt::Debug for ResidentPublicationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidentPublicationRefusal")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A single worker-owned desktop adapter. Initialization and every method run
/// on the existing Session worker, including while the UI event queue is full.
/// Construction must not start device work or hold an opened output: a failed
/// thread spawn may drop the uninitialized adapter on the constructing thread.
pub trait WorkerAudioSink: Send {
    /// Output ownership policy, independent of device presence or availability. A native consumer
    /// opts in only after routing every resident graph through this worker-private boundary.
    fn resident_publication_enabled(&self) -> bool {
        false
    }
    /// Synchronously borrow actual active/retiring/unpublished owners for every precommit check.
    /// No borrow may survive this worker call; each actual root is selected once per union.
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        [hex_engine::native_device_resources::DeviceResourceAdmission::default(); 3]
    }
    /// Final fallible prepared-transition operation. An active output must refuse queue/resource
    /// pressure unchanged; explicit unavailable/opening state may accept one admitted latest owner
    /// privately with no submitted/applied revision. Cancellation checks precede this call.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Exact graph/resources/observation custody and unchanged refusal remain explicit.
    fn publish_resident(
        &mut self,
        playback: Playback,
        _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        _observation: Arc<hex_engine::PreparedPlaybackObservation>,
        _metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        _metadata_limit: usize,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        Err(ResidentPublicationRefusal {
            playback,
            error: "ordinary worker publication is unavailable".into(),
        })
    }
    /// Retry an already admitted offline candidate only while the canonical worker lends the
    /// current source/export/control union. Pending custody is not released by cancellation/ACK.
    fn poll_resident_pending(
        &mut self,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
    }
    /// Negotiate the initial device rate on the lifecycle lane. An error leaves
    /// editing available at the explicit offline rate of 48000 Hz. Observe the
    /// shutdown flag while awaiting that lane; never wait for a UI response.
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String>;
    /// Called before the corresponding canonical event can block on UI delivery.
    /// On generation replacement relinquish the previous publication/output
    /// first. Replacement opening must be polled, not awaited in this method.
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>);
    /// Synchronous private canonical/original PCM input; no borrow may survive.
    fn native_sampler_input(&mut self, _input: NativeSamplerInput<'_>) {}
    fn begin_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit requires native audio".into())
    }
    /// None while awaiting the bounded callback fence; never wait here.
    fn sampler_edit_ready(&mut self) -> Option<Result<(), String>> {
        None
    }
    fn prepare_sampler_edit(&mut self, _input: NativeSamplerInput<'_>) -> Result<(), String> {
        Err("sampler edit preparation is unavailable".into())
    }
    fn commit_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit publication is unavailable".into())
    }
    fn cancel_sampler_edit(&mut self) {}
    /// Scalar activation only; implementations must verify the actual installed
    /// native CURRENT before accepting this exact output/context/track/device.
    fn prepare_sampler_keyboard(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _output: u64,
        _epoch: u64,
        _track: u64,
        _device: u64,
    ) -> Result<(), String> {
        Err("sampler keyboard is unavailable".into())
    }
    /// Hold remains charged through actual native teardown, never a UI ACK.
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        NativeSamplerReservationHold::default()
    }
    /// Advance bounded preparation/return work; no UI polling is required.
    fn poll(&mut self);
    /// Start a correlated reopen on the existing lifecycle lane. Implementations
    /// must relinquish the old output before returning; readiness is reported
    /// separately without waiting for a Client or UI event delivery.
    fn restart_audio(&mut self, _project_generation: u64, _request_id: u64) -> Result<(), String> {
        Err("audio restart is unavailable".into())
    }
    /// Relinquish output/publication custody without waiting for UI collection.
    /// Idempotent; called on the Session worker on every ordinary exit.
    fn close(&mut self);
}

pub(super) struct WorkerEvents {
    sender: SyncSender<Event>,
    shutdown: Arc<AtomicBool>,
    audio: Option<Box<dyn WorkerAudioSink>>,
    last_context: Option<hex_engine::AudioSourceContext>,
    startup_error: Option<String>,
}
impl WorkerEvents {
    pub(super) fn new(
        sender: SyncSender<Event>,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Self {
        Self {
            sender,
            shutdown,
            audio,
            last_context: None,
            startup_error: None,
        }
    }
    pub(super) fn initialize(&mut self) -> Result<u32, String> {
        let rate = self
            .audio
            .as_mut()
            .ok_or("audio adapter unavailable")?
            .initialize(&self.shutdown)?;
        if !(8_000..=192_000).contains(&rate) {
            if let Some(audio) = self.audio.as_mut() {
                audio.close();
            }
            return Err("unsupported negotiated audio rate".into());
        }
        Ok(rate)
    }
    pub(super) fn prepare_sampler_keyboard(
        &mut self,
        context: hex_engine::AudioSourceContext,
        output: u64,
        epoch: u64,
        track: hex_project::TrackId,
        device: hex_project::DeviceId,
    ) -> Result<(), String> {
        self.audio
            .as_mut()
            .ok_or("sampler keyboard requires native audio")?
            .prepare_sampler_keyboard(context, output, epoch, track.value(), device.value())
    }
    pub(super) fn canonical_source_changed(&mut self, source: &Arc<CanonicalAudioSourceSnapshot>) {
        if self.last_context != Some(source.context()) {
            if let Some(audio) = self.audio.as_mut() {
                audio.canonical_changed(Arc::clone(source));
            }
            self.last_context = Some(source.context());
        }
    }
    pub(super) fn resident_publication_enabled(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.resident_publication_enabled())
    }
    pub(super) fn resident_audio(&mut self) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio
            .as_mut()
            .filter(|audio| audio.resident_publication_enabled())
            .map(Box::as_mut)
    }
    pub(super) fn native_audio_requested(&self) -> bool {
        self.audio.is_some()
    }
    pub(super) fn sampler_edit_audio(&mut self) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio.as_deref_mut()
    }
    pub(super) fn startup_failed(&mut self, error: String) {
        self.startup_error = Some(format!("audio unavailable: {error}"));
    }
    pub(super) fn take_startup_error(&mut self) -> Option<String> {
        self.startup_error.take()
    }
    pub(super) fn restart_audio(
        &mut self,
        project_generation: u64,
        request_id: u64,
    ) -> Result<(), String> {
        self.audio
            .as_mut()
            .ok_or_else(|| "audio adapter unavailable".to_owned())?
            .restart_audio(project_generation, request_id)
    }
    fn poll(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.poll();
        }
    }
    pub(super) fn send_with_control_poll(
        &mut self,
        mut event: Event,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>),
    ) -> bool {
        if let Some(source) = &event.state.canonical_audio_source {
            self.canonical_source_changed(source);
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return false;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            match self.sender.try_send(event) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(returned)) => event = returned,
            }
            // Control-side event backpressure retains this exact event. It does
            // not strand native returns or require dropping a canonical ACK.
            thread::sleep(Duration::from_millis(5));
        }
    }
    pub(super) fn next_request_with_control_poll(
        &mut self,
        requests: &Receiver<Request>,
        bounded: bool,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>),
    ) -> Option<Request> {
        control_poll(
            self.audio
                .as_mut()
                .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
        );
        if self.audio.is_none() && !bounded {
            let result = requests.recv().ok();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            return result;
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            // Preserve queued New/Open/edit ordering before a callback-ready
            // continuation. No worker wait can strand project replacement.
            match requests.try_recv() {
                Ok(request) => return Some(request),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if self
                .audio
                .as_mut()
                .is_some_and(|a| a.sampler_edit_ready().is_some())
            {
                return Some(Request::ContinueSamplerEdit);
            }
            match requests.recv_timeout(Duration::from_millis(5)) {
                Ok(request) => {
                    control_poll(
                        self.audio
                            .as_mut()
                            .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                    );
                    return Some(request);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for WorkerEvents {
    fn drop(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicUsize};
    use std::time::Instant;
    #[derive(Default)]
    struct Observation {
        sources: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
        polls: AtomicUsize,
        closed: Mutex<Option<thread::ThreadId>>,
        restarts: Mutex<Vec<(u64, u64, thread::ThreadId)>>,
    }
    struct Sink {
        observed: Arc<Observation>,
        fail: bool,
    }
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            if self.fail {
                Err("device unavailable".into())
            } else {
                Ok(44100)
            }
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            let context = source.context();
            self.observed.sources.lock().unwrap().push((
                context.project_generation,
                context.editor_revision,
                thread::current().id(),
            ));
        }
        fn restart_audio(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
            self.observed.restarts.lock().unwrap().push((
                generation,
                request_id,
                thread::current().id(),
            ));
            Ok(())
        }
        fn poll(&mut self) {
            self.observed.polls.fetch_add(1, Ordering::SeqCst);
        }
        fn close(&mut self) {
            *self.observed.closed.lock().unwrap() = Some(thread::current().id());
        }
    }
    fn wait(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }
    #[test]
    fn worker_uses_negotiated_rate_and_keeps_cleanup_live_under_event_backpressure_and_close() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: false,
            }),
        )
        .unwrap();
        let caller = thread::current().id();
        // Initial + first edit fill the two event slots; the second edit must
        // notify native custody before waiting for its canonical event delivery.
        for name in ["one", "two"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
        }
        wait(|| observed.sources.lock().unwrap().len() == 3);
        let polls = observed.polls.load(Ordering::SeqCst);
        wait(|| observed.polls.load(Ordering::SeqCst) > polls + 2);
        for revision in 0..=2 {
            let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(event.state.project.sample_rate(), 44100);
            assert_eq!(event.state.revision, revision);
        }
        let sources = observed.sources.lock().unwrap();
        assert_eq!(
            sources.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(),
            [(0, 0), (0, 1), (0, 2)]
        );
        assert!(sources.iter().all(|s| s.2 != caller));
        drop(sources);
        // Close while the event queue is full again. No UI receive or progress
        // poll is needed to relinquish the worker's publication/output bundle.
        for _ in 0..3 {
            service.submit(Request::Undo).unwrap();
        }
        wait(|| observed.sources.lock().unwrap().len() >= 5);
        drop(service);
        wait(|| observed.closed.lock().unwrap().is_some());
        assert_ne!(observed.closed.lock().unwrap().unwrap(), caller);
    }
    #[test]
    fn restart_is_generation_checked_worker_only_and_does_not_edit_canonical_state() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: false,
            }),
        )
        .unwrap();
        let initial = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
        for (generation, id, valid) in [(1, 1, false), (0, 0, false), (0, 2, true)] {
            service
                .submit(Request::RestartAudio {
                    project_generation: generation,
                    request_id: id,
                })
                .unwrap();
            let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(event.state.revision, initial.state.revision);
            assert_eq!(
                event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation,
                initial
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation
            );
            assert_eq!(event.error.is_none(), valid);
            assert!(event.playback.is_none());
        }
        let rows = observed.restarts.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0, rows[0].1), (0, 2));
        assert_ne!(rows[0].2, thread::current().id());
    }
    #[test]
    fn failed_device_initialization_keeps_offline_canonical_edits_available() {
        let observed = Arc::new(Observation::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(Sink {
                observed: observed.clone(),
                fail: true,
            }),
        )
        .unwrap();
        let event = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(event.state.project.sample_rate(), 48000);
        assert_eq!(
            event.error.as_deref(),
            Some("audio unavailable: device unavailable")
        );
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "offline".into(),
            }))
            .unwrap();
        let edited = service.events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(edited.error.is_none());
        assert_eq!(edited.state.project.tracks().len(), 1);
        assert!(edited.state.unsaved);
        drop(service);
        wait(|| observed.closed.lock().unwrap().is_some());
    }
}
