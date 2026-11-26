//! Worker event channel, control-poll scheduler, and stopped-fence queue
//! coordinator (`WorkerEvents`) for the canonical Session worker.
use super::*;
use std::time::Duration;

/// The existing FIFO payload, optionally annotated. The plain form is retained
/// only for focused preexisting WorkerEvents unit fixtures; production always
/// exercises the annotated form through Service's unique EventReceiver.
pub(super) enum DiagnosticEventSender {
    Observed(SyncSender<super::diagnostics::ObservedEvent>),
    #[cfg(test)]
    Plain(SyncSender<Event>),
}
impl DiagnosticEventSender {
    #[allow(clippy::result_large_err)] // Failed send returns the same complete Event.
    fn try_send(
        &self,
        event: Event,
        request: Option<super::diagnostics::DiagnosticRequest>,
    ) -> Result<(), TrySendError<Event>> {
        match self {
            Self::Observed(sender) => sender
                .try_send(super::diagnostics::ObservedEvent { event, request })
                .map_err(|error| match error {
                    TrySendError::Full(event) => TrySendError::Full(event.event),
                    TrySendError::Disconnected(event) => TrySendError::Disconnected(event.event),
                }),
            #[cfg(test)]
            Self::Plain(sender) => sender.try_send(event),
        }
    }
}
/// Request-wait cadence while the sink or worker has pending native work.
/// The callback never wakes this thread, so this bounds native progress latency.
pub const BUSY_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Request-wait cadence once the sink and worker are idle (#4574). Requests
/// still end the wait immediately through the channel's blocking receive.
pub const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Consecutive idle iterations (about 50 ms at the busy cadence) before the
/// idle interval applies, so returns that follow the last busy poll (graph
/// retirement, observation release) are still collected at the busy cadence.
pub(in crate::service) const IDLE_GRACE_POLLS: u32 = 10;
/// Maximum time the Session worker will block on a full UI event channel before
/// parking/coalescing the latest canonical outcome and returning to service
/// native audio polling and shutdown (#13526).
pub(in crate::service) const OUTCOME_BACKPRESSURE_LIMIT: Duration = Duration::from_secs(5);
/// The wait after `idle_polls` consecutive idle iterations.
pub(in crate::service) fn poll_interval(idle_polls: u32) -> Duration {
    if idle_polls > IDLE_GRACE_POLLS {
        IDLE_POLL_INTERVAL
    } else {
        BUSY_POLL_INTERVAL
    }
}
pub(in crate::service) struct WorkerEvents {
    sender: DiagnosticEventSender,
    diagnostics: Arc<super::diagnostics::Hooks>,
    diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    // Observed-request timing only; absent without an installed observer.
    diagnostic_started: Option<std::time::Instant>,
    edit_phases: Option<(
        super::diagnostics::TimedEdit,
        super::diagnostics::SectionPhases,
    )>,
    recovery_timing: Option<(&'static str, Duration)>,
    shutdown: Arc<AtomicBool>,
    events_closed: Arc<AtomicBool>,
    wake_hook: super::client::WakeHook,
    audio: Option<Box<dyn WorkerAudioSink>>,
    last_context: Option<hex_engine::AudioSourceContext>,
    // Last delivered actor observation, not another editable gesture model.
    // It proves ownership when a terminal (including a retired refusal) arrives.
    last_verified_gain: Option<(crate::device_gesture::GestureKey, u64)>,
    last_verified_device: Option<(crate::device_gesture::GestureKey, u64)>,
    startup_error: Option<String>,
    // At most one request parked on the stopped Arrangement lease (#4544).
    stopped_fence: super::stopped_arrangement::Fence,
    publication_wait: Option<super::publication_wait::Wait>,
    // Park only the FIFO head while the callback owns a staged edit.
    device_insert_deferred: Option<QueuedRequest>,
    // Latest outcome retained when the UI event channel stayed full past
    // OUTCOME_BACKPRESSURE_LIMIT; flushed automatically as soon as space opens.
    stalled_outcome: Option<(Event, Option<super::diagnostics::DiagnosticRequest>)>,
    backpressure_coalesced: u64,
    // Consecutive request-wait iterations with no pending work (#4574).
    idle_polls: u32,
}
/// One request's unsent event and diagnostic identity stay together if the
/// control worker has to retain an outcome before delivering it.
pub(in crate::service) struct PendingOutcome {
    pub(in crate::service) event: Event,
    diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    diagnostic_started: Option<std::time::Instant>,
    edit_phases: Option<(
        super::diagnostics::TimedEdit,
        super::diagnostics::SectionPhases,
    )>,
    recovery_timing: Option<(&'static str, Duration)>,
}
/// One bounded control poll may make a previously admitted gesture ready.
/// The queued user FIFO retains priority over this copied continuation.
#[derive(Default)]
pub(in crate::service) struct ControlPoll {
    pub pending: bool,
    pub gesture: Option<GestureRequest>,
    pub editor_take: Option<u64>,
    pub recovery_ready: bool,
    pub editor_replay_ready: bool,
}

impl WorkerEvents {
    pub(in crate::service) fn diagnostics(&self) -> Arc<super::diagnostics::Hooks> {
        Arc::clone(&self.diagnostics)
    }
    pub(in crate::service) fn legacy_reader_limit(&self) -> usize {
        self.diagnostics.legacy_reader_limit()
    }
    pub(in crate::service) fn begin_diagnostics(
        &mut self,
        queued: &QueuedRequest,
        generation: u64,
        revision: u64,
    ) {
        let request = queued.diagnostic.or_else(|| {
            self.diagnostics
                .begin_at_worker(&queued.request, queued.diagnostic_origin)
        });
        self.diagnostic = request;
        self.diagnostic_started = request.map(|_| std::time::Instant::now());
        self.edit_phases = None;
        self.recovery_timing = None;
        self.diagnostics.emit(
            request,
            super::diagnostics::DiagnosticStage::WorkerBegin {
                generation,
                revision,
            },
            None,
        );
    }
    /// Attach section phases to this request's OutcomeReady observation.
    pub(in crate::service) fn record_section_phases(
        &mut self,
        phases: super::diagnostics::SectionPhases,
    ) {
        if self.diagnostic.is_some() {
            self.edit_phases = Some((super::diagnostics::TimedEdit::Section, phases));
        }
    }
    pub(in crate::service) fn observing_request(&self) -> bool {
        self.diagnostic.is_some()
    }

    pub(in crate::service) fn record_track_switch_phases(
        &mut self,
        phases: super::diagnostics::SectionPhases,
    ) {
        self.edit_phases = Some((super::diagnostics::TimedEdit::TrackSwitch, phases));
    }
    /// Time the request's post-transition view construction only when
    /// its diagnostic phase record exists. The work remains on the worker.
    pub(in crate::service) fn measure_edit_view<T>(&mut self, build: impl FnOnce() -> T) -> T {
        let started = self.edit_phases.as_ref().map(|_| std::time::Instant::now());
        let result = build();
        if let (Some((_, phases)), Some(started)) = (&mut self.edit_phases, started) {
            phases.view += started.elapsed();
        }
        result
    }
    /// Time recovery maintenance only for observed requests with a named phase.
    pub(in crate::service) fn measure_recovery<T>(
        &mut self,
        field: &'static str,
        maintain: impl FnOnce() -> T,
    ) -> T {
        let Some(_) = self.diagnostic else {
            return maintain();
        };
        let started = std::time::Instant::now();
        let result = maintain();
        self.recovery_timing = Some((field, started.elapsed()));
        result
    }
    pub(in crate::service) fn begin_request(
        &mut self,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        expected_intent: u64,
        publication_waited: bool,
    ) {
        if let Some(audio) = self.audio.as_mut() {
            audio.begin_request(
                RequestCancellation::new(
                    Arc::clone(&self.shutdown),
                    Arc::clone(document_intent),
                    expected_intent,
                )
                .after_publication_wait(publication_waited),
            );
        }
    }
    pub(in crate::service) fn default_project_sample_rate(&self) -> Option<u32> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.default_project_sample_rate())
            .filter(|rate| (8_000..=192_000).contains(rate))
    }

    pub(in crate::service) fn ordinary_source_reservation(
        &self,
    ) -> Option<crate::SourcePlanReservation> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.ordinary_source_reservation())
    }

    #[cfg(test)]
    pub(in crate::service) fn new(
        sender: SyncSender<Event>,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Self {
        Self::with_sender(
            DiagnosticEventSender::Plain(sender),
            shutdown,
            audio,
            Arc::new(super::diagnostics::Hooks::default()),
        )
    }
    pub(in crate::service) fn new_with_diagnostics(
        sender: SyncSender<super::diagnostics::ObservedEvent>,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
        diagnostics: Arc<super::diagnostics::Hooks>,
    ) -> Self {
        Self::with_sender(
            DiagnosticEventSender::Observed(sender),
            shutdown,
            audio,
            diagnostics,
        )
    }
    fn with_sender(
        sender: DiagnosticEventSender,
        shutdown: Arc<AtomicBool>,
        audio: Option<Box<dyn WorkerAudioSink>>,
        diagnostics: Arc<super::diagnostics::Hooks>,
    ) -> Self {
        Self {
            sender,
            diagnostics,
            diagnostic: None,
            diagnostic_started: None,
            edit_phases: None,
            recovery_timing: None,
            shutdown,
            events_closed: Arc::new(AtomicBool::new(false)),
            wake_hook: super::client::WakeHook::default(),
            audio,
            last_context: None,
            last_verified_gain: None,
            last_verified_device: None,
            startup_error: None,
            stopped_fence: Default::default(),
            publication_wait: None,
            device_insert_deferred: None,
            stalled_outcome: None,
            backpressure_coalesced: 0,
            idle_polls: 0,
        }
    }
    pub(in crate::service) fn with_events_closed(mut self, events_closed: Arc<AtomicBool>) -> Self {
        self.events_closed = events_closed;
        self
    }
    pub(in crate::service) fn with_wake_hook(mut self, wake_hook: super::client::WakeHook) -> Self {
        self.wake_hook = wake_hook;
        self
    }
    pub(in crate::service) fn wake_hook_clone(&self) -> super::client::WakeHook {
        self.wake_hook.clone()
    }
    pub(in crate::service) fn wake_ui(&self) {
        self.wake_hook.wake();
    }
    pub(in crate::service) fn initialize(&mut self) -> Result<u32, String> {
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
    pub(in crate::service) fn prepare_sampler_keyboard(
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
    pub(in crate::service) fn canonical_source_changed(
        &mut self,
        source: &Arc<CanonicalAudioSourceSnapshot>,
    ) {
        if self.last_context != Some(source.context()) {
            if let Some(audio) = self.audio.as_mut() {
                audio.canonical_changed(Arc::clone(source));
            }
            self.last_context = Some(source.context());
        }
    }
    pub(in crate::service) fn streamed_exporter(&self) -> Option<super::StreamedExporter> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.streamed_exporter())
    }
    pub(in crate::service) fn streamed_bouncer(&self) -> Option<crate::StreamedBouncer> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.streamed_bouncer())
    }
    pub(in crate::service) fn resident_publication_enabled(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.resident_publication_enabled())
    }
    pub(in crate::service) fn resident_audio(
        &mut self,
    ) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio
            .as_mut()
            .filter(|audio| audio.resident_publication_enabled())
            .map(Box::as_mut)
    }
    pub(in crate::service) fn native_audio_requested(&self) -> bool {
        self.audio.is_some()
    }
    pub(in crate::service) fn sampler_edit_audio(
        &mut self,
    ) -> Option<&mut (dyn WorkerAudioSink + 'static)> {
        self.audio.as_deref_mut()
    }
    pub(in crate::service) fn startup_failed(&mut self, error: String) {
        self.startup_error = Some(format!("audio unavailable: {error}"));
    }
    pub(in crate::service) fn take_startup_error(&mut self) -> Option<String> {
        self.startup_error.take()
    }
    pub(in crate::service) fn restart_audio(
        &mut self,
        project_generation: u64,
        request_id: u64,
    ) -> Result<(), String> {
        self.audio
            .as_mut()
            .ok_or_else(|| "audio adapter unavailable".to_owned())?
            .restart_audio(project_generation, request_id)
    }
    pub(in crate::service) fn poll(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.poll();
        }
        let _ = self.flush_stalled_outcome();
    }

    #[cfg(test)]
    pub(in crate::service) fn backpressure_coalesced(&self) -> u64 {
        self.backpressure_coalesced
    }

    #[cfg(test)]
    pub(in crate::service) fn has_stalled_outcome(&self) -> bool {
        self.stalled_outcome.is_some()
    }

    fn flush_stalled_outcome(&mut self) -> Result<bool, ()> {
        let Some((event, diagnostic)) = self.stalled_outcome.take() else {
            return Ok(true);
        };
        match self.sender.try_send(event, diagnostic) {
            Ok(()) => {
                self.wake_hook.wake();
                self.diagnostics.emit(
                    diagnostic,
                    super::diagnostics::DiagnosticStage::OutcomeQueued,
                    None,
                );
                Ok(true)
            }
            Err(TrySendError::Full(returned)) => {
                self.stalled_outcome = Some((returned, diagnostic));
                Ok(false)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.diagnostics.emit(
                    diagnostic,
                    super::diagnostics::DiagnosticStage::ConsumerClosed,
                    None,
                );
                Err(())
            }
        }
    }
    /// The control poll's result (pending lanes) is not needed here: event
    /// backpressure already retries at the busy cadence.
    pub(in crate::service) fn send_with_control_poll<R>(
        &mut self,
        event: Event,
        control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>) -> R,
    ) -> bool {
        let pending = self.capture_outcome(event);
        self.send_pending_with_control_poll(pending, control_poll)
    }

    pub(in crate::service) fn capture_outcome(&mut self, mut event: Event) -> PendingOutcome {
        if let Some(source) = &event.state.canonical_audio_source {
            self.canonical_source_changed(source);
        }
        match VerifiedTrackGainSource::capture(&event, self.last_verified_gain) {
            Ok(Some(source)) => {
                if let Some(audio) = self.audio.as_mut() {
                    audio.verified_track_gain_source(Arc::new(source));
                }
            }
            Ok(None) => {}
            Err(error) => {
                event.error = Some(match event.error.take() {
                    Some(previous) => format!("{previous}; {error}"),
                    None => error.to_string(),
                })
            }
        }
        match VerifiedDeviceParameterSource::capture(&event, self.last_verified_device) {
            Ok(Some(source)) => {
                self.last_verified_device = source
                    .preview_identity()
                    .map(|identity| (identity.key, identity.sequence));
                if let Some(audio) = self.audio.as_mut() {
                    audio.verified_device_parameter_source(Arc::new(source));
                }
            }
            Ok(None) => {}
            Err(error) => {
                event.error = Some(match event.error.take() {
                    Some(previous) => format!("{previous}; {error}"),
                    None => error.to_string(),
                });
            }
        }
        self.last_verified_gain = event
            .active_device_gesture
            .filter(|active| {
                matches!(
                    active.target,
                    crate::device_gesture::GestureTarget::TrackGain { .. }
                        | crate::device_gesture::GestureTarget::TrackGains { .. }
                ) && event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .is_some_and(|source| {
                        source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                    })
            })
            .map(|active| (active.key, active.sequence));
        PendingOutcome {
            event,
            diagnostic: self.diagnostic.take(),
            diagnostic_started: self.diagnostic_started.take(),
            edit_phases: self.edit_phases.take(),
            recovery_timing: self.recovery_timing.take(),
        }
    }

    pub(in crate::service) fn send_pending_with_control_poll<R>(
        &mut self,
        pending: PendingOutcome,
        control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>) -> R,
    ) -> bool {
        self.send_pending_with_control_poll_bounded(
            pending,
            OUTCOME_BACKPRESSURE_LIMIT,
            control_poll,
        )
    }

    pub(in crate::service) fn send_pending_with_control_poll_bounded<R>(
        &mut self,
        pending: PendingOutcome,
        max_wait: Duration,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>) -> R,
    ) -> bool {
        let PendingOutcome {
            mut event,
            diagnostic,
            diagnostic_started,
            edit_phases,
            recovery_timing,
        } = pending;
        let worker = diagnostic_started.map(|start| start.elapsed());
        self.diagnostics.outcome(
            diagnostic,
            &event,
            false,
            worker,
            edit_phases,
            recovery_timing,
        );
        let started = std::time::Instant::now();
        let mut reported_backpressure = false;
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                self.diagnostics.emit(
                    diagnostic,
                    super::diagnostics::DiagnosticStage::ShutdownAbandoned,
                    None,
                );
                return false;
            }
            if let Some(audio) = self.audio.as_mut() {
                audio.poll();
            }
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            );
            match self.flush_stalled_outcome() {
                Ok(true) => {}
                Ok(false) => {
                    if !reported_backpressure {
                        self.wake_hook.wake();
                        self.diagnostics.emit(
                            diagnostic,
                            super::diagnostics::DiagnosticStage::OutcomeBackpressure,
                            None,
                        );
                        reported_backpressure = true;
                    }
                    let elapsed = started.elapsed();
                    if elapsed >= max_wait {
                        self.stalled_outcome = Some((event, diagnostic));
                        self.backpressure_coalesced = self.backpressure_coalesced.saturating_add(1);
                        self.wake_hook.wake();
                        return true;
                    }
                    thread::sleep(BUSY_POLL_INTERVAL.min(max_wait.saturating_sub(elapsed)));
                    continue;
                }
                Err(()) => return false,
            }
            match self.sender.try_send(event, diagnostic) {
                Ok(()) => {
                    self.wake_hook.wake();
                    self.diagnostics.emit(
                        diagnostic,
                        super::diagnostics::DiagnosticStage::OutcomeQueued,
                        None,
                    );
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.diagnostics.emit(
                        diagnostic,
                        super::diagnostics::DiagnosticStage::ConsumerClosed,
                        None,
                    );
                    return false;
                }
                Err(TrySendError::Full(returned)) => {
                    event = returned;
                    if !reported_backpressure {
                        self.wake_hook.wake();
                        self.diagnostics.emit(
                            diagnostic,
                            super::diagnostics::DiagnosticStage::OutcomeBackpressure,
                            None,
                        );
                        reported_backpressure = true;
                    }
                }
            }
            let elapsed = started.elapsed();
            if elapsed >= max_wait {
                // Prolonged UI stall: park/coalesce the latest canonical outcome
                // so the worker stays responsive to audio polling and shutdown,
                // and flush automatically as soon as the UI drains the channel.
                self.stalled_outcome = Some((event, diagnostic));
                self.backpressure_coalesced = self.backpressure_coalesced.saturating_add(1);
                self.wake_hook.wake();
                return true;
            }
            // Transient event backpressure retries at the busy cadence so short
            // bursts retain every canonical outcome in FIFO order.
            thread::sleep(BUSY_POLL_INTERVAL.min(max_wait.saturating_sub(elapsed)));
        }
    }
    /// Park one edit ahead of the FIFO while the current publication settles.
    pub(in crate::service) fn defer_publication(
        &mut self,
        queued: QueuedRequest,
    ) -> Option<QueuedRequest> {
        // A request that just completed its wait must dispatch without re-parking.
        if queued.publication_wait_failure.is_some() {
            return Some(queued);
        }
        let readiness = self
            .audio
            .as_mut()
            .map_or(ResidentPublicationReadiness::NoOutput, |audio| {
                audio.resident_publication_readiness()
            });
        match super::publication_wait::Wait::begin(queued, readiness) {
            super::publication_wait::Step::Waiting(wait) => {
                self.publication_wait = Some(wait);
                None
            }
            super::publication_wait::Step::Ready(queued) => Some(queued),
        }
    }
    /// Begin the stopped Arrangement lease for a request that fences first;
    /// `None` parks it until the callback answers (see `stopped_arrangement`).
    pub(in crate::service) fn defer_stopped_fence(
        &mut self,
        queued: QueuedRequest,
        source: hex_engine::AudioSourceContext,
        recover: bool,
    ) -> Option<QueuedRequest> {
        let sink = self
            .audio
            .as_mut()
            .filter(|audio| audio.resident_publication_enabled())
            .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
        self.stopped_fence.defer(sink, queued, source, recover)
    }
    /// The lease outcome resolved before dispatch for `source`, if any.
    pub(in crate::service) fn take_stopped_fence(
        &mut self,
        source: hex_engine::AudioSourceContext,
    ) -> Option<super::stopped_arrangement::Prefence> {
        self.stopped_fence.take(source)
    }
    /// Release a resolved lease the finished request did not consume.
    pub(in crate::service) fn finish_stopped_fence(&mut self) {
        let sink = self
            .audio
            .as_mut()
            .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
        self.stopped_fence.finish_request(sink);
    }
    /// While a request is parked on the lease, only the fence is polled: no
    /// control poll or publication runs, unrelated requests are served and the
    /// first other request is held. Bounded by the fence deadline; the callback
    /// never wakes this thread. `None` resumes the ordinary loop.
    fn next_request_while_fenced(
        &mut self,
        requests: &Receiver<QueuedRequest>,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
    ) -> Option<QueuedRequest> {
        loop {
            let shutdown = self.shutdown.load(Ordering::Acquire);
            let intent = document_intent.load(Ordering::Acquire);
            let sink = self
                .audio
                .as_mut()
                .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink);
            if let Some(request) = self.stopped_fence.next(
                sink,
                shutdown,
                |parked| parked.document_intent != intent,
                || requests.try_recv().ok(),
            ) {
                return Some(request);
            }
            if !self.stopped_fence.active() {
                return None; // Resume the ordinary request loop.
            }
            thread::sleep(super::stopped_arrangement::FENCE_POLL);
        }
    }
    /// Requests that observe or save the committed predecessor without mutating
    /// project, history, transport or graph state, and without cancelling a
    /// pending device-insert confirmation. Window-close preflights stay out of
    /// this set so closing cancels an unanswered confirmation. When a parameter
    /// or tuning gesture is active, gesture-finalizing requests (`Save*`) must
    /// defer behind `pending_device_insert` so they never mutate canonical
    /// history underneath the staged graph edit.
    pub(in crate::service) fn served_during_device_insert(
        request: &Request,
        gesture_active: bool,
    ) -> bool {
        if gesture_active && request.finalizes_parameter_gesture() {
            return false;
        }
        matches!(
            request,
            Request::SaveNew(_)
                | Request::SaveProjectAs { .. }
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
                | Request::QueryPluginParameters(_)
                | Request::ContinuePluginParameters
                | Request::QueryPluginParameterConversion(_)
                | Request::ContinuePluginParameterConversion
                | Request::QueryFailedPluginAutomation { .. }
                | Request::InspectSealedAutomationRecovery { .. }
                | Request::PreparePausedInputMonitor { .. }
        )
    }
    #[allow(clippy::too_many_arguments)] // Each continuation has one worker owner.
    pub(in crate::service) fn next_request_with_control_poll(
        &mut self,
        requests: &Receiver<QueuedRequest>,
        document_intent: &Arc<std::sync::atomic::AtomicU64>,
        recording_barrier: &mut super::recording_barrier::State,
        bounded: bool,
        plugin_effect: Option<&plugin_effect::Pending>,
        device_gesture_active: bool,
        tuning_gesture_active: bool,
        device_insert: Option<&super::resident_audio::device_insert::Pending>,
        stopped_restore: Option<hex_engine::AudioSourceContext>,
        context: hex_engine::AudioSourceContext,
        mut control_poll: impl FnMut(Option<&mut dyn WorkerAudioSink>, bool) -> ControlPoll,
    ) -> Option<QueuedRequest> {
        if let Some(pending) = device_insert {
            let gesture_active = device_gesture_active || tuning_gesture_active;
            loop {
                let cancel = self.shutdown.load(Ordering::Acquire)
                    || pending.document_intent != document_intent.load(Ordering::Acquire);
                if self.device_insert_deferred.is_some() {
                    pending.cancel_confirmation();
                }
                if pending.poll(cancel).is_some() {
                    return Some(QueuedRequest::continuation(
                        Request::ContinueDeviceInsert,
                        document_intent,
                    ));
                }
                self.poll();
                if cancel || self.device_insert_deferred.is_some() {
                    thread::sleep(BUSY_POLL_INTERVAL);
                    continue;
                }
                if self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.plugin_parameters_ready())
                {
                    return Some(QueuedRequest::continuation(
                        Request::ContinuePluginParameters,
                        document_intent,
                    ));
                }
                if self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.plugin_parameter_conversion_ready())
                {
                    return Some(QueuedRequest::continuation(
                        Request::ContinuePluginParameterConversion,
                        document_intent,
                    ));
                }
                match requests.recv_timeout(BUSY_POLL_INTERVAL) {
                    Ok(queued)
                        if Self::served_during_device_insert(&queued.request, gesture_active) =>
                    {
                        return Some(queued);
                    }
                    Ok(queued) => {
                        if pending.awaiting_confirmation() {
                            pending.cancel_confirmation();
                        }
                        self.device_insert_deferred = Some(queued);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return None,
                }
            }
        }
        if let Some(queued) = self.device_insert_deferred.take() {
            return Some(queued);
        }
        // The original barrier retains its FIFO place. Continue polling SDK/disk
        // work without consuming a later request; accepted New/Open intent and
        // shutdown are observable independently of the bounded channel.
        while recording_barrier.active() {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.poll();
            let control = control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                false,
            );
            if let Some(request) =
                recording_barrier.poll(context, document_intent.load(Ordering::Acquire))
            {
                return Some(request);
            }
            if let Some(gesture) = control.gesture
                && let Some(request) = recording_barrier.continuation(gesture)
            {
                return Some(request);
            }
            thread::sleep(super::recording_barrier::POLL);
        }
        if self.stopped_fence.active()
            && let Some(request) = self.next_request_while_fenced(requests, document_intent)
        {
            return Some(request);
        }
        while let Some(wait) = self.publication_wait.take() {
            if self.shutdown.load(Ordering::Acquire) {
                return None;
            }
            self.poll();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                false,
            );
            let readiness = self
                .audio
                .as_mut()
                .map_or(ResidentPublicationReadiness::NoOutput, |audio| {
                    audio.resident_publication_readiness()
                });
            match wait.poll(readiness, document_intent.load(Ordering::Acquire)) {
                super::publication_wait::Step::Ready(request) => return Some(request),
                super::publication_wait::Step::Waiting(wait) => self.publication_wait = Some(wait),
            }
            thread::sleep(super::publication_wait::POLL);
        }
        control_poll(
            self.audio
                .as_mut()
                .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
            false,
        );
        if self.audio.is_none() && !bounded {
            let result = requests.recv().ok();
            control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                false,
            );
            return result;
        }
        loop {
            if self.shutdown.load(Ordering::Acquire) || self.events_closed.load(Ordering::Acquire) {
                return None;
            }
            self.diagnostics.record_worker_wake();
            self.poll();
            let control_pending = control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                false,
            );
            // Preserve queued New/Open/edit ordering before a callback-ready
            // continuation. No worker wait can strand project replacement.
            match requests.try_recv() {
                Ok(request) => return Some(request),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if control_pending.recovery_ready {
                return Some(QueuedRequest::continuation(
                    Request::ContinueRecovery,
                    document_intent,
                ));
            }
            if let Some(request_id) = control_pending.editor_take {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginEditorRecording { request_id },
                    document_intent,
                ));
            }
            if let Some(gesture) = control_pending.gesture {
                return Some(QueuedRequest::continuation(
                    Request::device_gesture(gesture),
                    document_intent,
                ));
            }
            if self
                .audio
                .as_mut()
                .is_some_and(|a| a.sampler_edit_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinueSamplerEdit,
                    document_intent,
                ));
            }
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.plugin_state_restore_ready().is_some())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginStateRestore,
                    document_intent,
                ));
            }
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.plugin_editor_ready())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginEditor,
                    document_intent,
                ));
            }
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.plugin_parameters_ready())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginParameters,
                    document_intent,
                ));
            }
            if control_pending.editor_replay_ready {
                return Some(QueuedRequest::continuation(
                    Request::ContinueRetiringEditorReplay,
                    document_intent,
                ));
            }
            if self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.plugin_parameter_conversion_ready())
            {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginParameterConversion,
                    document_intent,
                ));
            }
            if plugin_effect.is_some_and(|pending| {
                !pending.matches_context(context)
                    || pending.insertion.control.is_cancelled()
                    || (!device_gesture_active
                        && self.audio.as_mut().is_some_and(|audio| {
                            match pending.rebased_predecessor_ready(audio.as_mut()) {
                                Ok(ready) => ready && audio.plugin_effect_ready(),
                                Err(_) => true,
                            }
                        }))
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinuePluginEffect,
                    document_intent,
                ));
            }
            if stopped_restore.is_some_and(|captured| {
                captured != context
                    || self
                        .audio
                        .as_ref()
                        .is_some_and(|audio| audio.stopped_playback_restore_ready())
            }) {
                return Some(QueuedRequest::continuation(
                    Request::ContinueStoppedPlaybackRestore,
                    document_intent,
                ));
            }
            // Idle only when no worker continuation, control lane or sink work
            // is pending; any of them restores the busy cadence at once.
            let audition = control_poll(
                self.audio
                    .as_mut()
                    .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                true,
            );
            let idle = !control_pending.pending
                && !audition.pending
                && plugin_effect.is_none()
                && stopped_restore.is_none()
                && !self.stopped_fence.active()
                && self.stalled_outcome.is_none()
                && self.audio.as_mut().is_some_and(|audio| audio.worker_idle());
            self.idle_polls = if idle {
                self.idle_polls.saturating_add(1)
            } else {
                0
            };
            let wait = poll_interval(self.idle_polls);
            #[cfg(test)]
            let wait = self
                .audio
                .as_mut()
                .map_or(wait, |audio| audio.test_poll_wait(wait));
            match requests.recv_timeout(wait) {
                Ok(request) => {
                    self.idle_polls = 0;
                    control_poll(
                        self.audio
                            .as_mut()
                            .map(|a| a.as_mut() as &mut dyn WorkerAudioSink),
                        false,
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
        self.stopped_fence.close(
            self.audio
                .as_mut()
                .map(|audio| audio.as_mut() as &mut dyn WorkerAudioSink),
        );
        if let Some(audio) = self.audio.as_mut() {
            audio.close();
        }
    }
}
