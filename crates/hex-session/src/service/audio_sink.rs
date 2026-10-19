//! Desktop-injected native publication, driven only by the canonical worker.
//! Session owns no device or page endpoints and never interprets their tokens.
use super::*;
use std::time::Duration;

/// A single worker-owned desktop adapter. Initialization and every method run
/// on the existing Session worker, including while the UI event queue is full.
/// Construction must not start device work or hold an opened output: a failed
/// thread spawn may drop the uninitialized adapter on the constructing thread.
pub trait WorkerAudioSink: Send {
    /// Negotiate the initial device rate on the lifecycle lane. An error leaves
    /// editing available at the explicit offline rate of 48000 Hz. Observe the
    /// shutdown flag while awaiting that lane; never wait for a UI response.
    fn initialize(&mut self, shutdown: &dyn hex_engine::CancellationQuery) -> Result<u32, String>;
    /// Called before the corresponding canonical event can block on UI delivery.
    /// On generation replacement relinquish the previous publication/output
    /// first. Replacement opening must be polled, not awaited in this method.
    fn canonical_changed(&mut self, source: SharedCanonicalAudioSource);
    /// Advance bounded preparation/return work; no UI polling is required.
    fn poll(&mut self);
    /// Fresh actual same-worker roots for this synchronous call only. The
    /// default preserves existing asset-free adapters; it grants no admission.
    /// Implementations must retain final ledger/guards through their commit,
    /// never cache this borrow or turn selected totals into a saved permit.
    fn poll_with_worker_roots(&mut self, _roots: &WorkerRootView<'_>) {
        self.poll();
    }
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
    shutdown: SessionCancellation,
    audio: Option<Box<dyn WorkerAudioSink>>,
    last_source: Option<(
        hex_engine::AudioSourceContext,
        hex_engine::SharedMetadataCacheIdentity,
    )>,
    startup_error: Option<String>,
}
impl WorkerEvents {
    pub(super) fn cancelled(&self) -> bool {
        hex_engine::CancellationQuery::is_cancelled(&self.shutdown)
    }
    pub(super) fn cancellation_on_control(&self) -> &SessionCancellation {
        &self.shutdown
    }
    pub(super) fn new(
        sender: SyncSender<Event>,
        shutdown: SessionCancellation,
        audio: Option<Box<dyn WorkerAudioSink>>,
    ) -> Self {
        Self {
            sender,
            shutdown,
            audio,
            last_source: None,
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
    fn poll(
        &mut self,
        state: &WorkerDispatchState<'_>,
        event: Option<&Event>,
        dispatch: WorkerRootDispatch,
    ) {
        if let Some(audio) = self.audio.as_mut() {
            let roots = WorkerRootView::new(state, &self.shutdown, event, dispatch);
            audio.poll_with_worker_roots(&roots);
        }
    }
    pub(super) fn send(&mut self, mut event: Event, state: &WorkerDispatchState<'_>) -> bool {
        if let Some(source) = &event.state.canonical_audio_source {
            // SaveNew/SaveCurrent replace the immutable source allocation at
            // unchanged musical counters. A nonowning immutable version only
            // suppresses redundant notification; it grants no storage/custody.
            // Unavailable/exhausted versions conservatively notify every Event.
            let identity = source.cache_identity().map(|id| (source.context(), id));
            if identity.is_none() || self.last_source != identity {
                if let Some(audio) = self.audio.as_mut() {
                    audio.canonical_changed(source.clone());
                }
                self.last_source = identity;
            }
        }
        let mut dispatch = WorkerRootDispatch::ChangedEvent;
        loop {
            if self.cancelled() {
                return false;
            }
            // This exact Event is borrowed only during the call, then moved.
            // Full returns ownership and the next iteration makes a fresh view.
            self.poll(state, Some(&event), dispatch);
            dispatch = WorkerRootDispatch::PendingEvent;
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
    pub(super) fn complete_request(
        &mut self,
        requests: &RequestReceiver,
        stamp: Option<crate::snapshot_exports::NativeRequestStamp>,
    ) -> bool {
        loop {
            match requests.complete(stamp) {
                Ok(()) => return true,
                Err(crate::snapshot_exports::SnapshotTransportError::Busy) => {
                    if self.cancelled() {
                        if let Some(audio) = self.audio.as_mut() {
                            audio.close();
                        }
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return false,
            }
        }
    }
    pub(super) fn next_request(
        &mut self,
        requests: &RequestReceiver,
        state: &WorkerDispatchState<'_>,
    ) -> Option<crate::snapshot_exports::NativeRequestDelivery> {
        if self.audio.is_none()
            && let Some(receiver) = requests.unqualified()
        {
            return receiver
                .recv()
                .ok()
                .map(crate::snapshot_exports::NativeRequestDelivery::unqualified);
        }
        loop {
            if self.cancelled() {
                return None;
            }
            self.poll(state, None, WorkerRootDispatch::Idle);
            match requests.receive_timeout(Duration::from_millis(5)) {
                Ok(request) => return Some(request),
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
        fn initialize(&mut self, _: &dyn hex_engine::CancellationQuery) -> Result<u32, String> {
            if self.fail {
                Err("device unavailable".into())
            } else {
                Ok(44100)
            }
        }
        fn canonical_changed(&mut self, source: SharedCanonicalAudioSource) {
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

#[cfg(test)]
mod root_dispatch_tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Clone, Copy, Debug)]
    struct Read {
        context: hex_engine::AudioSourceContext,
        dispatch: WorkerRootDispatch,
        event_revision: Option<u64>,
        old_source_refused: bool,
        pending_export: bool,
        source_version: Option<hex_engine::SharedMetadataCacheIdentity>,
    }
    #[derive(Default)]
    struct Observed {
        reads: Mutex<Vec<Read>>,
        closed: AtomicBool,
    }
    struct RootSink {
        observed: Arc<Observed>,
        latest: Option<SharedCanonicalAudioSource>,
        first: Option<SharedCanonicalAudioSource>,
        independent: Option<SharedCanonicalAudioSource>,
    }
    impl WorkerAudioSink for RootSink {
        fn initialize(&mut self, _: &dyn hex_engine::CancellationQuery) -> Result<u32, String> {
            Ok(44100)
        }
        fn canonical_changed(&mut self, source: SharedCanonicalAudioSource) {
            if self.first.is_none() {
                self.first = Some(source.clone());
            }
            let context = source.context();
            let independent_session = Session::new(source.sample_rate(), 8, 0).unwrap();
            self.independent = Some(SharedCanonicalAudioSource::new_on_control(
                CanonicalAudioSourceSnapshot::capture(
                    source.project_owner_on_control(),
                    context.project_generation,
                    context.editor_revision,
                    &independent_session,
                ),
            ));
            self.latest = Some(source);
        }
        fn poll(&mut self) {
            panic!("worker must dispatch actual roots, including idle and full-queue polls");
        }
        fn poll_with_worker_roots(&mut self, roots: &WorkerRootView<'_>) {
            // Notification must happen before the first inspection/send. These
            // intentionally retained test aliases do not claim whole-root fit.
            let latest = self.latest.as_ref().unwrap();
            assert!(roots.matches_current_source_on_control(latest));
            assert_eq!(roots.context(), latest.context());
            // Equal counters and shared Project do not substitute for the
            // actual current physical snapshot allocation.
            let independent = self.independent.as_ref().unwrap();
            assert_eq!(independent.context(), latest.context());
            assert!(
                independent
                    .project_owner_on_control()
                    .shares_owner_with(latest.project_owner_on_control())
            );
            assert!(!roots.matches_current_source_on_control(independent));
            let first = self.first.as_ref().unwrap();
            let old_source_refused = first.context() != roots.context();
            if old_source_refused {
                assert!(!roots.matches_current_source_on_control(first));
            }
            assert_eq!(
                roots.with_original_child_inventory_on_control(|inventory| {
                    inventory.require_complete_root_storage_on_control()
                }),
                Err(hex_engine::AudioStorageError::UnknownStorage),
            );
            self.observed.reads.lock().unwrap().push(Read {
                context: roots.context(),
                dispatch: roots.dispatch(),
                event_revision: roots.pending_event_revision(),
                old_source_refused,
                pending_export: roots.has_pending_export(),
                source_version: latest.cache_identity(),
            });
        }
        fn close(&mut self) {
            self.observed.closed.store(true, Ordering::Release);
        }
    }
    fn wait(mut ready: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(3);
        while !ready() {
            assert!(Instant::now() < until, "worker dispatch did not progress");
            thread::yield_now();
        }
    }
    #[test]
    fn fresh_worker_root_dispatch_tracks_actual_edit_idle_and_full_event_retry_without_saved_authority()
     {
        let observed = Arc::new(Observed::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(RootSink {
                observed: observed.clone(),
                latest: None,
                first: None,
                independent: None,
            }),
        )
        .unwrap();
        for name in ["one", "two"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
        }
        // Initial and first edit fill both slots. Revision two is retained as
        // an exact local Event and inspected again on each Full retry.
        wait(|| {
            observed
                .reads
                .lock()
                .unwrap()
                .iter()
                .filter(|read| {
                    read.dispatch == WorkerRootDispatch::PendingEvent
                        && read.event_revision == Some(2)
                })
                .count()
                >= 2
        });
        {
            let reads = observed.reads.lock().unwrap();
            for revision in 0..=2 {
                assert!(
                    reads
                        .iter()
                        .any(|read| read.dispatch == WorkerRootDispatch::ChangedEvent
                            && read.context.project_generation == 0
                            && read.context.editor_revision == revision
                            && read.event_revision == Some(revision))
                );
            }
            assert!(
                reads
                    .iter()
                    .filter(|read| read.context.editor_revision == 2)
                    .all(|read| read.old_source_refused)
            );
        }
        for revision in 0..=2 {
            assert_eq!(
                service
                    .events
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .state
                    .revision,
                revision
            );
        }
        wait(|| {
            observed.reads.lock().unwrap().iter().any(|read| {
                read.dispatch == WorkerRootDispatch::Idle
                    && read.context.editor_revision == 2
                    && read.event_revision.is_none()
            })
        });
        service.submit(Request::Undo).unwrap();
        assert_eq!(
            service
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .state
                .revision,
            3
        );
        wait(|| {
            observed.reads.lock().unwrap().iter().any(|read| {
                read.context.editor_revision == 3
                    && read.old_source_refused
                    && read.event_revision == Some(3)
            })
        });
        // A refused export still has an ACTUAL pending completion path on
        // the worker stack until the canonical Event has been delivered.
        service
            .submit(Request::Export(PathBuf::from(
                "sampler-root-empty-export.wav",
            )))
            .unwrap();
        let event = service.events.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(event.request_complete);
        assert_eq!(event.state.revision, 3);
        let completion = service
            .export_events
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert_eq!(completion.result.unwrap_err(), "nothing to export");
        wait(|| {
            observed
                .reads
                .lock()
                .unwrap()
                .iter()
                .any(|read| read.event_revision == Some(3) && read.pending_export)
        });
        wait(|| {
            observed.reads.lock().unwrap().last().is_some_and(|read| {
                read.dispatch == WorkerRootDispatch::Idle && !read.pending_export
            })
        });
        // Close is independent of UI collection and no inspection guard is
        // carried through the receive/sleep loop or destructor.
        drop(service);
        wait(|| observed.closed.load(Ordering::Acquire));
    }
    #[test]
    fn successful_saves_refresh_actual_worker_source_at_unchanged_counters_and_idle() {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "hex-root-save-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        )));
        std::fs::create_dir(&directory.0).unwrap();
        let path = directory.0.join("saved.hex");
        let observed = Arc::new(Observed::default());
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(RootSink {
                observed: observed.clone(),
                latest: None,
                first: None,
                independent: None,
            }),
        )
        .unwrap();
        let _initial = service.events.recv_timeout(Duration::from_secs(3)).unwrap();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "keep".into(),
            }))
            .unwrap();
        let edited = service.events.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(edited.state.unsaved);
        let mut previous = edited
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .clone();
        for request in [Request::SaveNew(path.clone()), Request::SaveCurrent] {
            service.submit(request).unwrap();
            let saved = service.events.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(saved.save_complete);
            assert!(saved.error.is_none(), "{:?}", saved.error);
            assert!(!saved.state.unsaved);
            assert_eq!(saved.state.revision, edited.state.revision);
            let source = saved.state.canonical_audio_source.as_ref().unwrap();
            assert_eq!(source.context(), previous.context());
            assert_eq!(source.sample_rate(), previous.sample_rate());
            assert!(!source.shares_owner_with(&previous));
            assert_ne!(source.cache_identity(), previous.cache_identity());
            // RootSink checks EXACT current allocation on every poll. Ensure
            // later idle polls also observe this save, not an earlier revision.
            wait(|| {
                observed.reads.lock().unwrap().iter().any(|read| {
                    read.dispatch == WorkerRootDispatch::Idle
                        && read.context == source.context()
                        && read.source_version == source.cache_identity()
                })
            });
            previous = source.clone();
        }
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        drop(service);
        wait(|| observed.closed.load(Ordering::Acquire));
    }
}
