//! Client-facing `Service` handle, request submission, shutdown and export job wiring.
use super::*;

#[cfg(test)]
pub(super) type ExportTestHook = Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// Optional control-side wake callback notified whenever the session worker
/// (or its export/seek lanes) enqueues an `Event` or status observation.
/// Never invoked from the audio callback.
pub type WakeCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
pub(super) struct WakeHook {
    installed: Arc<AtomicBool>,
    callback: Arc<std::sync::Mutex<Option<WakeCallback>>>,
}

impl WakeHook {
    pub(super) fn set(&self, callback: Option<WakeCallback>) {
        let mut slot = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.installed.store(callback.is_some(), Ordering::Release);
        *slot = callback;
    }

    pub(super) fn wake(&self) {
        if !self.installed.load(Ordering::Acquire) {
            return;
        }
        let callback = self
            .callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

/// UI-owned endpoints. `shutdown` stops and joins the session worker, which
/// joins the export worker it owns; dropping calls it with a short bound.
pub struct Service {
    #[cfg(test)]
    pub(super) export_test_hook: ExportTestHook,
    /// `None` after shutdown, which disconnects an idle blocking worker.
    pub(super) requests: Option<request_cancellation::Sender>,
    pub(super) clip_audition: Arc<clip_audition::Mailbox>,
    pub(super) events: diagnostics::EventReceiver,
    pub(super) wake_hook: WakeHook,
    pub(super) track_switch_views: track_switch_views::Receiver,
    pub(super) arrangement_edit_views: arrangement_edit_views::Receiver,
    pub(super) diagnostics: Arc<diagnostics::Hooks>,
    pub(super) export_events: Receiver<ExportEvent>,
    pub(super) export_recovery: Arc<crate::ExportRecoveryJournal>,
    pub(super) export_drain_signal: Arc<WakeSignal>,
    pub(super) outcome_drain_signal: Arc<WakeSignal>,
    pub(super) worker_wake_signal: Arc<WakeSignal>,
    pub(super) worker_exited: Arc<WakeSignal>,
    pub(super) export_busy: Arc<AtomicBool>,
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) arrangement_seek: Option<ArrangementSeekWorker>,
    pub(super) resident_seek_results: Option<Receiver<ResidentArrangementSeekResult>>,
    pub(super) resident_seek_count: AtomicUsize,
    pub(super) recovery_slot: Option<PathBuf>,
    /// Exclusive per-instance ownership of `recovery_slot` (Pulse #3948).
    /// `Drop` stops the worker before fields drop, so the slot is released only
    /// after checkpointing ends (or the bounded shutdown detached the worker).
    pub(super) _recovery_claim: Option<hex_storage::UntitledSlotClaim>,
    pub(super) startup_failure: Arc<std::sync::OnceLock<String>>,
    /// `None` once joined; kept after a timed-out shutdown for a later retry.
    pub(super) worker: Option<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        // Bounded: an uncancellable job past the bound stays detached rather
        // than blocking the dropping thread. Quit paths call `shutdown` first.
        let _ = self.shutdown(DROP_SHUTDOWN_BOUND);
        // Persist any unconsumed recovery-bearing export terminal sitting in
        // `export_events` before dropping the receiver on application exit.
        while let Ok(event) = self.export_events.try_recv() {
            self.export_recovery.record_event(&event);
        }
    }
}

/// Ordinary controlled range renderer: path, revision, start, frames, signal,
/// format, control and publication. Tests substitute it to observe worker custody.
pub(super) type Exporter = Arc<
    dyn Fn(
            &Path,
            &Arc<PreparedRevision>,
            u64,
            u64,
            hex_export::ExportSignal,
            hex_export::ExportFormat,
            &hex_export::ExportControl,
            &hex_export::Publication,
        ) -> Result<ExportReport, ExportFailure>
        + Send
        + Sync,
>;

pub(super) fn canonical_exporter() -> Exporter {
    Arc::new(
        |path, prepared, start, frames, signal, format, control, publication| {
            hex_export::render_prepared_with_policy(hex_export::ExportJobSpec {
                path,
                revision: Arc::clone(prepared),
                resources: None,
                start,
                frames,
                signal,
                format,
                control,
                publication,
            })
            .map_err(ExportFailure::from)
        },
    )
}

pub(super) struct ExportJob {
    pub(super) job_id: u64,
    pub(super) streamed: Option<streamed_export::StreamedExportJob>,
    pub(super) foreign: Option<Box<dyn OfflineBounceSource>>,
    pub(super) sources: Option<hex_engine::native_device_resources::ControlSamplerSources>,
    pub(super) resources: Option<hex_engine::native_device_resources::PreparedDeviceResources>,
    pub(super) revision: u64,
    pub(super) path: PathBuf,
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) frames: u64,
    pub(super) controlled: Option<ControlledExport>,
}

pub(super) enum ExportWork {
    Render(ExportJob),
}

impl Service {
    /// Configure fresh catalog creation without filesystem work. Accepted FIFO
    /// requests retain their captured directory; explicit saved/copied state
    /// never receives it. Unconfigured services use provider factory settings.
    pub fn set_device_defaults_directory(&mut self, root: PathBuf) -> Result<(), String> {
        preset::validate_default_root(&root)?;
        self.requests
            .as_mut()
            .ok_or("session worker is closed")?
            .set_device_defaults_directory(root);
        Ok(())
    }
    pub fn new(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            canonical_exporter(),
        )
    }

    /// All journal I/O, including directory creation, stays on the worker.
    pub fn with_recovery(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        untitled_path: PathBuf,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            Some(untitled_path),
            canonical_exporter(),
        )
    }

    #[cfg(test)]
    pub(super) fn with_exporter(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        exporter: Exporter,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter_and_recovery(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            exporter,
        )
    }

    /// Compose the desktop audio adapter without opening a device on the caller.
    /// The adapter supplies the initial project rate without waiting for device
    /// opening. Delayed results use its existing polling contract; failed output
    /// setup retains canonical editing/saving in offline mode.
    pub fn with_audio_sink(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        audio: Box<dyn WorkerAudioSink>,
    ) -> Result<Self, ServiceError> {
        Self::start_with_audio(
            48_000,
            history_capacity,
            max_resident_frames,
            recovery,
            canonical_exporter(),
            Some(audio),
        )
    }

    pub fn with_audio_sink_and_native_reservation(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        audio: Box<dyn WorkerAudioSink>,
        native: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            48_000,
            history_capacity,
            max_resident_frames,
            recovery,
            canonical_exporter(),
            Some(audio),
            Some(native),
        )
    }

    /// Desktop composition with an untitled slot this instance exclusively
    /// owns; the service keeps the claim until it is dropped (Pulse #3948).
    pub fn with_audio_sink_and_claimed_recovery(
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: hex_storage::UntitledSlotClaim,
        audio: Box<dyn WorkerAudioSink>,
        native: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        let mut service = Self::with_audio_sink_and_native_reservation(
            history_capacity,
            max_resident_frames,
            Some(recovery.path().to_path_buf()),
            audio,
            native,
        )?;
        service._recovery_claim = Some(recovery);
        Ok(service)
    }

    pub(super) fn with_exporter_and_recovery(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
    ) -> Result<Self, ServiceError> {
        Self::start_with_audio(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            None,
        )
    }

    pub fn with_native_reservation(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        reservation: crate::ResidentNativeReservation,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            sample_rate,
            history_capacity,
            max_resident_frames,
            None,
            canonical_exporter(),
            None,
            Some(reservation),
        )
    }

    pub(super) fn start_with_audio(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Result<Self, ServiceError> {
        Self::start_configured(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            audio,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Internal composition; public constructors stay explicit.
    pub(super) fn start_configured(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
        native: Option<crate::ResidentNativeReservation>,
    ) -> Result<Self, ServiceError> {
        Self::start_configured_in_overview_domain(
            sample_rate,
            history_capacity,
            max_resident_frames,
            recovery,
            exporter,
            audio,
            native,
            crate::source_overview::OverviewBudget::standard().map_err(ServiceError::Session)?,
        )
    }
    #[allow(clippy::too_many_arguments)] // Existing worker composition plus one shared derived-data domain.
    pub(super) fn start_configured_in_overview_domain(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        recovery: Option<PathBuf>,
        exporter: Exporter,
        audio: Option<Box<dyn WorkerAudioSink>>,
        native: Option<crate::ResidentNativeReservation>,
        overview_budget: crate::source_overview::OverviewBudget,
    ) -> Result<Self, ServiceError> {
        let recovery_slot = recovery.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let session = if audio.is_none() {
            Some(
                Session::new_with_overview_budget(
                    sample_rate,
                    history_capacity,
                    max_resident_frames,
                    overview_budget.clone(),
                )
                .and_then(|session| match native {
                    Some(reservation) => session.with_native_reservation(reservation),
                    None => Ok(session),
                })
                .map_err(ServiceError::Session)?
                .with_shutdown(Arc::clone(&shutdown)),
            )
        } else {
            None
        };
        let wake_hook = WakeHook::default();
        let arrangement_seek = ArrangementSeekWorker::new().map_err(ServiceError::Worker)?;
        let (seek_results_tx, seek_results_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (arrangement_seek, resident_seek) = if audio
            .as_ref()
            .is_some_and(|sink| sink.resident_publication_enabled())
        {
            (
                None,
                Some(
                    resident_seek::Lane::new(arrangement_seek, seek_results_tx)
                        .with_wake(wake_hook.clone()),
                ),
            )
        } else {
            (Some(arrangement_seek), None)
        };
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let diagnostics = Arc::new(diagnostics::Hooks::default());
        let worker_diagnostics = Arc::clone(&diagnostics);
        let document_intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_document_intent = Arc::clone(&document_intent);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let (track_switch_view_tx, track_switch_views) = track_switch_views::channel();
        let track_switch_view_tx = track_switch_view_tx.with_wake(wake_hook.clone());
        let (arrangement_edit_view_tx, arrangement_edit_views) = arrangement_edit_views::channel();
        let arrangement_edit_view_tx = arrangement_edit_view_tx.with_wake(wake_hook.clone());
        let (export_work_tx, export_work_rx) = mpsc::sync_channel(1);
        let (export_event_tx, export_event_rx) = mpsc::sync_channel(EXPORT_EVENT_CAPACITY);
        let actor_export_events = export_event_tx.clone();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let export_busy = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let export_test_hook = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let worker_test_hook = Arc::clone(&export_test_hook);
        let export_shutdown = Arc::clone(&shutdown);
        let export_wake_hook = wake_hook.clone();
        let export_drain_signal = Arc::new(WakeSignal::default());
        let worker_export_drain = Arc::clone(&export_drain_signal);
        let outcome_drain_signal = Arc::new(WakeSignal::default());
        let worker_outcome_drain = Arc::clone(&outcome_drain_signal);
        let worker_wake_signal = Arc::new(WakeSignal::default());
        let worker_wake = Arc::clone(&worker_wake_signal);
        let worker_exited = Arc::new(WakeSignal::default());
        let worker_exit_notify = Arc::clone(&worker_exited);
        let export_recovery = crate::ExportRecoveryJournal::new(
            recovery_slot
                .as_deref()
                .and_then(crate::ExportRecoveryJournal::default_path_for_recovery_slot),
        );
        let worker_export_recovery = Arc::clone(&export_recovery);
        let export_worker = thread::Builder::new()
            .name("hex-export".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                run_export_worker_with_recovery(
                    export_work_rx,
                    export_event_tx,
                    exporter,
                    finished_tx,
                    export_shutdown,
                    export_wake_hook,
                    worker_export_drain,
                    Some(worker_export_recovery),
                    #[cfg(test)]
                    worker_test_hook,
                )
            })
            .map_err(ServiceError::Worker)?;
        let worker_shutdown = Arc::clone(&shutdown);
        let startup_failure = Arc::new(std::sync::OnceLock::new());
        let worker_startup_failure = Arc::clone(&startup_failure);
        let events_closed = Arc::new(AtomicBool::new(false));
        let worker_events_closed = Arc::clone(&events_closed);
        let worker_wake_hook = wake_hook.clone();
        let clip_audition = Arc::new(clip_audition::Mailbox::default());
        let worker_clip_audition = Arc::clone(&clip_audition);
        let worker = thread::Builder::new()
            .name("hex-session".into())
            // Structural stopped publications reuse the bounded preroll constructor.
            .stack_size(crate::arrangement_seek::SEEK_WORKER_STACK_BYTES)
            .spawn(move || {
                struct ExitNotify(Arc<WakeSignal>);
                impl Drop for ExitNotify {
                    fn drop(&mut self) {
                        self.0.notify();
                    }
                }
                let _exit_notify = ExitNotify(worker_exit_notify);
                // Establish export-worker custody before fallible session startup,
                // so an early return still closes and joins that worker.
                let export_lane = ExportLane::new(export_work_tx, finished_rx, export_worker);
                let mut events = WorkerEvents::new_with_diagnostics(
                    event_tx,
                    Arc::clone(&worker_shutdown),
                    audio,
                    worker_diagnostics,
                )
                .with_events_closed(worker_events_closed)
                .with_wake_signals(worker_outcome_drain, worker_wake)
                .with_wake_hook(worker_wake_hook);
                let session = match session {
                    Some(session) => session,
                    None => {
                        let (rate, error) = match events.initialize() {
                            Ok(rate) => (rate, None),
                            Err(error) => (48_000, Some(error)),
                        };
                        let session = match Session::new_with_audio_mode_in_overview_domain(
                            rate,
                            history_capacity,
                            max_resident_frames,
                            DocumentAudioMode::Resident,
                            events.ordinary_source_reservation(),
                            overview_budget.clone(),
                        ) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = worker_startup_failure
                                    .set(format!("project session could not start: {error}"));
                                return;
                            }
                        };
                        if let Some(error) = error {
                            events.startup_failed(error);
                        }
                        let session = match native {
                            Some(limits) => match session.with_native_reservation(limits) {
                                Ok(session) => session,
                                Err(reason) => {
                                    events.startup_failed(format!(
                                        "native resident reservation unavailable: {reason}"
                                    ));
                                    match Session::new_with_audio_mode_in_overview_domain(
                                        rate,
                                        history_capacity,
                                        max_resident_frames,
                                        DocumentAudioMode::Resident,
                                        events.ordinary_source_reservation(),
                                        overview_budget.clone(),
                                    ) {
                                        Ok(session) => session,
                                        Err(error) => {
                                            let _ = worker_startup_failure.set(format!(
                                                "project session could not start: {error}"
                                            ));
                                            return;
                                        }
                                    }
                                }
                            },
                            None => session,
                        };
                        let session = match project_rate::new_following_interface(
                            session,
                            history_capacity,
                        ) {
                            Ok(session) => session,
                            Err(error) => {
                                let _ = worker_startup_failure
                                    .set(format!("project session could not start: {error}"));
                                return;
                            }
                        };
                        session.with_shutdown(worker_shutdown)
                    }
                };
                run_worker(
                    session,
                    history_capacity,
                    request_rx,
                    worker_document_intent,
                    events,
                    export_lane,
                    actor_export_events,
                    recovery,
                    resident_seek,
                    track_switch_view_tx,
                    arrangement_edit_view_tx,
                    worker_clip_audition,
                )
            })
            .map_err(ServiceError::Worker)?;
        Ok(Self {
            #[cfg(test)]
            export_test_hook,
            requests: Some(request_cancellation::Sender::new(
                request_tx,
                document_intent,
            )),
            clip_audition,
            diagnostics: Arc::clone(&diagnostics),
            events: diagnostics::EventReceiver::with_closed(
                event_rx,
                diagnostics,
                events_closed,
                Arc::clone(&outcome_drain_signal),
            ),
            wake_hook,
            track_switch_views,
            arrangement_edit_views,
            export_events: export_event_rx,
            export_recovery,
            export_drain_signal,
            outcome_drain_signal,
            worker_wake_signal,
            worker_exited,
            export_busy,
            shutdown,
            arrangement_seek,
            resident_seek_results: Some(seek_results_rx),
            resident_seek_count: AtomicUsize::new(0),
            recovery_slot,
            _recovery_claim: None,
            startup_failure,
            worker: Some(worker),
        })
    }

    /// Install or replace an optional control-side wake callback invoked whenever
    /// the session worker (or its export/seek lanes) enqueues an `Event` or status
    /// observation. Never called from the audio callback.
    pub fn set_wake_callback(&self, callback: Option<WakeCallback>) {
        let installed = callback.is_some();
        self.wake_hook.set(callback);
        if installed {
            self.wake_hook.wake();
        }
    }

    /// Construction failure recorded before the initial-state sender disconnects.
    pub fn startup_failure(&self) -> Option<&str> {
        self.startup_failure.get().map(String::as_str)
    }

    /// Receive a bounded track mute or solo display observation. A full channel
    /// drops the observation; the ordinary terminal remains authoritative.
    pub fn try_track_switch_view(&self) -> Result<TrackSwitchViewObservation, TryRecvError> {
        self.track_switch_views.try_recv()
    }

    /// Receive an optional display observation without consuming a terminal.
    pub fn try_arrangement_edit_view(
        &self,
    ) -> Result<ArrangementEditViewObservation, TryRecvError> {
        self.arrangement_edit_views.try_recv()
    }

    /// Replace the one held stretch audition target outside the canonical FIFO.
    pub fn audition_arrangement_stretch(&self, target: Option<ArrangementStretchAudition>) {
        if let Some(target) = target {
            self.clip_audition
                .set(clip_audition::Target::Stretch(target));
        } else {
            self.clip_audition
                .clear(Some(clip_audition::Kind::Stretch), false);
        }
    }

    /// Hold the audible stretch preview until the ordinary edit's terminal.
    pub fn finish_arrangement_stretch_audition(&self) {
        self.clip_audition.release(clip_audition::Kind::Stretch);
    }

    /// Replace the held play-start target, or cancel held input. Released audio
    /// remains owned by the pending clip edit until its exact terminal.
    pub fn audition_arrangement_play_start(&self, target: Option<ArrangementPlayStartAudition>) {
        if let Some(target) = target {
            self.clip_audition
                .set(clip_audition::Target::PlayStart(target));
        } else {
            self.clip_audition
                .clear(Some(clip_audition::Kind::PlayStart), true);
        }
    }

    /// Retain the last applied play-start preview through a single commit.
    pub fn finish_arrangement_play_start_audition(&self) {
        self.clip_audition.release(clip_audition::Kind::PlayStart);
    }

    /// Retire the audition after refusal or installation of the authored edit.
    pub fn retire_arrangement_play_start_audition(&self) {
        self.clip_audition
            .clear(Some(clip_audition::Kind::PlayStart), false);
    }

    /// Replace only the latest held gain target. The worker coalesces pointer
    /// values before preparation, and the preview does not enter history.
    pub fn audition_arrangement_gain(&self, target: Option<ArrangementGainAudition>) {
        match target {
            Some(target) if target.edit.valid() => {
                self.clip_audition.set(clip_audition::Target::Gain(target));
            }
            _ => self
                .clip_audition
                .clear(Some(clip_audition::Kind::Gain), true),
        }
    }

    pub fn finish_arrangement_gain_audition(&self) {
        self.clip_audition.release(clip_audition::Kind::Gain);
    }

    pub fn retire_arrangement_gain_audition(&self) {
        self.clip_audition
            .clear(Some(clip_audition::Kind::Gain), false);
    }

    /// Replace the complete held group target. An invalid target cancels the
    /// held audition; it can never admit only part of a selection.
    /// Returns true when a valid target was stored, not when audio applied it.
    /// Passing None cancels held input and returns false.
    pub fn audition_arrangement_move(&self, target: Option<ArrangementMoveAudition>) -> bool {
        match target {
            Some(target) if target.valid() => {
                self.clip_audition.set(clip_audition::Target::Move(target));
                true
            }
            _ => {
                self.clip_audition
                    .clear(Some(clip_audition::Kind::Move), true);
                false
            }
        }
    }

    /// Retain the admitted group preview until its matching canonical request.
    pub fn finish_arrangement_move_audition(&self) {
        self.clip_audition.release(clip_audition::Kind::Move);
    }

    /// Retire the preview after the matching terminal or refusal.
    pub fn retire_arrangement_move_audition(&self) {
        self.clip_audition
            .clear(Some(clip_audition::Kind::Move), false);
    }

    #[cfg(feature = "source-overview-test-support")]
    pub fn held_arrangement_stretch_audition_for_test(&self) -> Option<ArrangementStretchAudition> {
        match self.clip_audition.held()? {
            clip_audition::Target::Stretch(target) => Some(target),
            _ => None,
        }
    }

    #[cfg(feature = "source-overview-test-support")]
    pub fn held_arrangement_play_start_audition_for_test(
        &self,
    ) -> Option<ArrangementPlayStartAudition> {
        match self.clip_audition.held()? {
            clip_audition::Target::PlayStart(target) => Some(target),
            _ => None,
        }
    }

    /// Submit without waiting for worker capacity.
    pub fn submit(&self, request: Request) -> Result<(), SubmitError> {
        let replaces_audition_source = matches!(
            &request,
            Request::NewWithAudioMode { .. }
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::RestartAudio { .. }
        );
        let diagnostic = self.diagnostics.begin(&request);
        let result = self.submit_traced(request, diagnostic);
        if result.is_ok() && replaces_audition_source {
            self.clip_audition.clear(None, false);
        }
        self.diagnostics.returned(diagnostic, &result);
        result
    }

    /// Submit one save bound to the current document generation and revision.
    /// Rejection returns the original request; a delivered terminal echoes the
    /// identity, including validation or persistence failure. A stopped worker
    /// remains a separate disconnection, never a successful save terminal.
    pub fn submit_correlated_save(
        &self,
        request: Request,
        correlation: SaveCorrelation,
    ) -> Result<(), SubmitError> {
        if !matches!(
            request,
            Request::SaveNew(_)
                | Request::SaveProjectAs { .. }
                | Request::SaveCurrent
                | Request::SaveCurrentWithFormatUpgrade { .. }
        ) || correlation.request_id == 0
        {
            return Err(SubmitError::busy(
                request,
                BusyReason::InvalidSaveCorrelation,
            ));
        }
        let diagnostic = self.diagnostics.begin(&request);
        let result = match self.requests.as_ref() {
            Some(requests) => requests.submit_traced_save(request, diagnostic, Some(correlation)),
            None => Err(SubmitError::closed(request)),
        };
        self.diagnostics.returned(diagnostic, &result);
        result
    }

    /// Iterations of the worker's request-wait loop since start (#4574): each
    /// is one bounded native/control poll. A read-only diagnostic scalar for
    /// idle wake measurement; it grants no ordering or completion evidence.
    pub fn worker_wakes(&self) -> u64 {
        self.diagnostics.worker_wakes()
    }

    /// Install one read-only control observer. Logging has no effect on request
    /// admission, canonical state, cancellation or publication authority.
    pub fn set_diagnostic_observer(
        &self,
        observer: Arc<diagnostics::DiagnosticObserver>,
    ) -> Result<(), &'static str> {
        self.diagnostics.install(observer)
    }

    pub(super) fn submit_traced(
        &self,
        request: Request,
        diagnostic: Option<diagnostics::DiagnosticRequest>,
    ) -> Result<(), SubmitError> {
        if let Request::BounceArrangement {
            target:
                hex_project::BounceTarget::Clips(ids)
                | hex_project::BounceTarget::AudioGroup { clips: ids, .. },
            ..
        } = &request
            && (ids.is_empty()
                || ids.len() > hex_project::MAX_CLIPS
                || ids.capacity() > hex_project::MAX_CLIPS)
        {
            return Err(SubmitError::busy(request, BusyReason::InvalidBounceTarget));
        }
        if matches!(&request, Request::ResidentArrangementSeek(_)) {
            if self.arrangement_seek.is_some() {
                return Err(SubmitError::busy(request, BusyReason::SeekWorkerActive));
            }
            if self
                .resident_seek_count
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < REQUEST_CAPACITY).then(|| n + 1)
                })
                .is_err()
            {
                return Err(SubmitError::busy(request, BusyReason::SeekQueueFull));
            }
            return match try_submit_traced(self.requests.as_ref(), request, diagnostic) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.resident_seek_count.fetch_sub(1, Ordering::AcqRel);
                    Err(error)
                }
            };
        }
        if matches!(&request, Request::Export(_) | Request::ExportRange { .. }) {
            if self
                .export_busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(SubmitError::busy(request, BusyReason::ExportPending));
            }
            match try_submit_traced(self.requests.as_ref(), request, diagnostic) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.export_busy.store(false, Ordering::Release);
                    Err(error)
                }
            }
        } else {
            try_submit_traced(self.requests.as_ref(), request, diagnostic)
        }
    }

    /// Poll for one committed state or operation result without waiting.
    /// The untitled automatic-recovery slot, when this service checkpoints one.
    pub fn recovery_slot(&self) -> Option<&Path> {
        self.recovery_slot.as_deref()
    }

    pub fn try_event(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }

    /// Configure the durable path where undelivered export recovery notices
    /// are recorded on shutdown, saturated/disconnected channels, or exit.
    pub fn set_export_recovery_path(&self, path: Option<PathBuf>) {
        self.export_recovery.set_path(path);
    }

    /// Drain undelivered export recovery notices captured in memory for this
    /// service (e.g. during shutdown with a saturated result channel).
    pub fn take_export_recovery_notices(&self) -> Vec<crate::ExportRecoveryNotice> {
        self.export_recovery.take_notices()
    }

    /// Consume one separate export result and release the single-job reservation.
    pub fn try_export_event(&self) -> Result<ExportEvent, TryRecvError> {
        match self.export_events.try_recv() {
            Ok(event) => {
                self.export_busy.store(false, Ordering::Release);
                self.export_drain_signal.notify();
                Ok(event)
            }
            Err(TryRecvError::Disconnected) => {
                self.export_busy.store(false, Ordering::Release);
                self.export_drain_signal.notify();
                Err(TryRecvError::Disconnected)
            }
            Err(TryRecvError::Empty) => Err(TryRecvError::Empty),
        }
    }

    /// Prepare an independent Arrangement candidate on its dedicated worker.
    /// This does not perform live route/Stop admission or publish to audio.
    pub fn request_arrangement_seek(
        &self,
        spec: ArrangementSeekSpec,
    ) -> Result<u64, ArrangementSeekSubmitError> {
        self.arrangement_seek
            .as_ref()
            .ok_or(ArrangementSeekSubmitError::Closed)?
            .submit(spec)
    }

    /// Submit scalar native seek intent without publishing an owning graph through the UI.
    pub fn submit_resident_arrangement_seek(
        &self,
        intent: ResidentArrangementSeekIntent,
    ) -> Result<(), SubmitError> {
        if self.arrangement_seek.is_some() {
            return Err(SubmitError::busy(
                Request::ResidentArrangementSeek(intent),
                BusyReason::SeekWorkerActive,
            ));
        }
        self.submit(Request::ResidentArrangementSeek(intent))
    }
    pub fn try_resident_arrangement_seek_result(&self) -> Option<ResidentArrangementSeekResult> {
        let result = self.resident_seek_results.as_ref()?.try_recv().ok()?;
        self.resident_seek_count.fetch_sub(1, Ordering::AcqRel);
        Some(result)
    }

    /// Consume a completed candidate without waiting.
    pub fn try_arrangement_seek_result(&self) -> Option<ArrangementSeekResult> {
        self.arrangement_seek.as_ref()?.try_result()
    }

    /// Return a stale result for control-side destruction on the seek worker.
    /// On backpressure, ownership is returned unchanged for a later retry.
    #[allow(clippy::result_large_err)] // Preserve the candidate intact on backpressure.
    pub fn retire_arrangement_seek_result(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        match self.arrangement_seek.as_ref() {
            Some(worker) => worker.retire_result(result),
            None => Err(result),
        }
    }

    /// Return a UI-held seek result immediately before this service is dropped.
    /// The worker drains it during shutdown, avoiding final graph destruction
    /// on the UI thread.
    #[allow(clippy::result_large_err)] // Preserve the candidate if worker handback is full.
    pub fn retire_arrangement_seek_result_for_shutdown(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        match self.arrangement_seek.as_ref() {
            Some(worker) => worker.retire_result_for_shutdown(result),
            None => Err(result),
        }
    }

    /// Cancel admitted seek preparation and discard its ready candidate.
    pub fn invalidate_arrangement_seek(&self) {
        if let Some(worker) = &self.arrangement_seek {
            worker.invalidate();
        }
    }
}

#[cfg(test)]
pub(super) fn try_submit(
    sender: &request_cancellation::Sender,
    request: Request,
) -> Result<(), SubmitError> {
    sender.submit(request)
}
pub(super) fn try_submit_traced(
    sender: Option<&request_cancellation::Sender>,
    request: Request,
    diagnostic: Option<diagnostics::DiagnosticRequest>,
) -> Result<(), SubmitError> {
    match sender {
        Some(sender) => sender.submit_traced(request, diagnostic),
        None => Err(SubmitError::closed(request)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_starts_and_shuts_down_cleanly() {
        let mut service = Service::new(48_000, 4, 0).expect("service");
        let initial = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .expect("initial event");
        assert_eq!(initial.state.revision, 0);
        service
            .shutdown(std::time::Duration::from_secs(2))
            .expect("clean shutdown");
    }
}
