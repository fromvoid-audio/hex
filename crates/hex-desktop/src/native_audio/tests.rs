//! Output completion alone is substituted. Session, verified readers, source
//! domains, graph preparation, publication and callback are production paths.
use super::*;
use hex_audio::worker_fixture::HeadlessBackend;
use hex_engine::PlaybackCommand;
use hex_session::service::{Event, Request};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, atomic::AtomicU64},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
// The actual native constructor retains the process-wide output lease.
static NATIVE_FIXTURE: Mutex<()> = Mutex::new(());
#[derive(Default)]
struct Observation {
    latest: Option<(AudioSourceContext, AudioCandidateDomainToken, usize)>,
    command_ready: Option<(AudioSourceContext, u64)>,
    change_source_after_capture: Option<PathBuf>,
}
// A read-only test observer around the actual sink, not an alternate source,
// graph, publication or ownership path.
struct ObservedSink {
    inner: NativeAudioSink,
    observation: Arc<Mutex<Observation>>,
}
impl WorkerAudioSink for ObservedSink {
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(shutdown)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        if source
            .required_sources()
            .any(|(_, binding)| matches!(binding, AudioSourceBinding::VerifiedFile(_)))
            && let Some(path) = self
                .observation
                .lock()
                .unwrap()
                .change_source_after_capture
                .take()
        {
            // Session has captured the original fingerprint. The independent
            // native source worker must refuse replacement bytes, not relabel.
            let mut bytes = fs::read(&path).unwrap();
            let last = bytes.last_mut().unwrap();
            *last ^= 1;
            fs::write(path, bytes).unwrap();
        }
        self.inner.canonical_changed(source);
    }
    fn restart_audio(&mut self, generation: u64, request: u64) -> Result<(), String> {
        self.inner.restart_audio(generation, request)
    }
    fn poll(&mut self) {
        self.inner.poll();
        if let Some(context) = self.inner.last_context
            && let Some(domain) = self
                .inner
                .submitted
                .iter()
                .flatten()
                .find(|s| s.context == context)
                .and_then(|s| s.domain)
            && let Some(control) = self
                .inner
                .output
                .as_mut()
                .and_then(|o| o.publication.control())
            && let Some(usage) = control.audio_candidate_domain_usage()
        {
            let mut observation = self.observation.lock().unwrap();
            observation.latest = Some((context, domain, usage.occupied_domains));
            // The callback can consume a committed publication before the
            // publisher returns and records its command-admission revision.
            observation.command_ready = Some((context, control.submitted_revision()));
        }
    }
    fn close(&mut self) {
        self.inner.close();
    }
}
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-desktop-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn wav(&self, rate: u32, frames: u32) -> PathBuf {
        let path = self.0.join("mixed-rate.wav");
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&8192i16.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    service: Option<Service>,
    events: mpsc::Receiver<EditorAudioEvent>,
    backend: Arc<Mutex<Option<HeadlessBackend>>>,
    audio: Option<EditorAudio>,
    context: Option<AudioSourceContext>,
    observation: Arc<Mutex<Observation>>,
    wait: Duration,
}
impl Harness {
    fn new() -> Self {
        Self::with_rate(48000)
    }
    fn with_rate(default_rate: u32) -> Self {
        Self::with_settings(default_rate, Duration::from_secs(10))
    }
    fn with_wait(wait: Duration) -> Self {
        Self::with_settings(48000, wait)
    }
    fn with_settings(default_rate: u32, wait: Duration) -> Self {
        let backend = Arc::new(Mutex::new(None));
        let factory_backend = Arc::clone(&backend);
        // One queue row deliberately permits publication-event saturation.
        let (events, receiver) = mpsc::sync_channel(1);
        let mut sink = NativeAudioSink::new(events);
        sink.output_factory = Some(Box::new(move |rate| {
            let mut slot = factory_backend.lock().unwrap();
            if slot.is_some() {
                return Ok(None);
            }
            match hex_audio::worker_fixture::open(rate.unwrap_or(default_rate)) {
                Ok((output, callback)) => {
                    *slot = Some(callback);
                    Ok(Some(output))
                }
                Err(error) if error == "audio output is open or still shutting down" => Ok(None),
                Err(error) => Err(error),
            }
        }));
        let observation = Arc::new(Mutex::new(Observation::default()));
        let service = Service::with_audio_sink(
            16,
            0,
            None,
            Box::new(ObservedSink {
                inner: sink,
                observation: Arc::clone(&observation),
            }),
        )
        .unwrap();
        Self {
            service: Some(service),
            events: receiver,
            backend,
            audio: None,
            context: None,
            observation,
            wait,
        }
    }
    fn next(&mut self) -> Event {
        let deadline = Instant::now() + self.wait;
        loop {
            if let Ok(event) = self.service.as_ref().unwrap().try_event() {
                let context = event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context();
                if self
                    .context
                    .is_some_and(|old| old.project_generation != context.project_generation)
                {
                    // Same UI ordering: detach old endpoint on canonical
                    // generation change, BEFORE waiting for new Opened.
                    self.audio = None;
                    self.backend.lock().unwrap().take();
                }
                self.context = Some(context);
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "canonical worker did not respond"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn run(&mut self, request: Request) -> Event {
        self.service.as_ref().unwrap().submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }
    fn open(&mut self) {
        let deadline = Instant::now() + self.wait;
        loop {
            match self.events.try_recv() {
                Ok(EditorAudioEvent::Opened {
                    audio,
                    project_generation,
                    ..
                }) => {
                    assert_eq!(
                        Some(project_generation),
                        self.context.map(|c| c.project_generation)
                    );
                    self.audio = Some(audio);
                    return;
                }
                Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("{error}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "output did not open");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn render(&self) -> [hex_engine::Frame; 64] {
        let mut output = [[0.0; 2]; 64];
        let mut slot = self.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        // Construction, locking, verification and all disposal remain outside
        // the measured real callback entry.
        let measured = allocation_counter::measure(|| backend.render(&mut output).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        output
    }
    fn applied(&self, context: AudioSourceContext) -> hex_audio::prepared::AppliedAudioSource {
        let deadline = Instant::now() + self.wait;
        loop {
            self.render();
            if let Some(actual) = self
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                && actual.context == context
            {
                return actual;
            }
            if Instant::now() >= deadline {
                let status = self.audio.as_ref().unwrap().status().unwrap();
                let mut observations: Vec<_> = self
                    .events
                    .try_iter()
                    .map(|event| match event {
                        EditorAudioEvent::Publication { context, state, .. } => {
                            format!("{context:?}: {state:?}")
                        }
                        _ => "output event".into(),
                    })
                    .collect();
                if let Ok(EditorAudioEvent::Publication { context, state, .. }) =
                    self.events.recv_timeout(Duration::from_millis(100))
                {
                    observations.push(format!("pending {context:?}: {state:?}"));
                }
                panic!(
                    "actual install missing for {context:?}; applied={:?} rejected={:?} fault={} observations={observations:?}",
                    status.applied_audio_source(),
                    status.rejected_revision(),
                    status.render_fault.load(Acquire)
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn domain(&self, context: AudioSourceContext) -> (AudioCandidateDomainToken, usize) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some((observed, domain, occupied)) = self.observation.lock().unwrap().latest
                && observed == context
            {
                return (domain, occupied);
            }
            assert!(
                Instant::now() < deadline,
                "actual domain observation missing"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        // Service Drop only signals. Its sink/Publication destructor and all
        // graph/domain destruction run autonomously on existing workers.
        drop(self.service.take());
        self.audio = None;
        self.backend.lock().unwrap().take();
        while let Ok(event) = self.events.try_recv() {
            drop(event);
        }
    }
}

#[test]
fn failed_output_setup_keeps_canonical_editing_and_saving_available() {
    let files = Files::new();
    let (events, audio_events) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    sink.output_factory = Some(Box::new(|_| Err("fixture device unavailable".into())));
    let recovery = files.0.join("untitled.hex-journal");
    let service = Service::with_audio_sink(32, 0, Some(recovery.clone()), Box::new(sink)).unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let initial = next();
    assert!(
        initial
            .error
            .as_ref()
            .unwrap()
            .contains("audio unavailable")
    );
    assert!(matches!(
        audio_events.recv_timeout(Duration::from_secs(1)).unwrap(),
        EditorAudioEvent::Unavailable {
            restart_request: None,
            ..
        }
    ));
    service
        .submit(Request::Edit(hex_project::Command::MasterGain {
            gain: 0.5,
        }))
        .unwrap();
    let edited = next();
    assert!(edited.error.is_none());
    assert!(edited.state.revision > initial.state.revision);
    assert!(recovery.is_file()); // worker-backed startup retains automatic journals
    let path = files.0.join("offline.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    assert!(next().error.is_none());
    assert!(path.is_file());
}

#[test]
fn canonical_service_installs_verified_mixed_rate_and_empty_successors_despite_lost_submitted() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let path = files.wav(24000, 240000);
    let mut h = Harness::new();
    let initial = h.next();
    assert!(initial.playback.is_some()); // default Resident compatibility
    h.open();
    let empty = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(empty.playback.is_none());
    h.open();
    let empty_context = h.context.unwrap();
    let first = h.applied(empty_context);
    let (empty_domain, empty_occupied) = h.domain(empty_context);
    assert_eq!(empty_occupied, 1); // source-free opt-in still consumes actual N2
    assert_eq!(
        first.output_identity,
        h.audio.as_ref().unwrap().control.output_identity().unwrap()
    );
    let imported = h.run(Request::Import(path.clone()));
    assert!(imported.playback.is_none());
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(matches!(
        source.required_sources().next().unwrap().1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    assert!(imported.state.waveforms.is_empty());
    assert_eq!(
        imported
            .state
            .project
            .assets()
            .next()
            .unwrap()
            .playback_frames(),
        480000
    );
    let a = h.context.unwrap();
    let actual_a = h.applied(a);
    let (a_domain, a_occupied) = h.domain(a);
    assert_ne!(a_domain, empty_domain);
    assert_eq!(a_occupied, 2);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = h.render();
        if output.iter().any(|frame| frame[0] > 0.2) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "verified worker never supplied audible pages"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Do NOT drain publication events: the capacity-one queue and latest-only
    // pending observation cannot guarantee Submitted(A) ever reaches the UI.
    let refused = h.run(Request::Edit(hex_project::Command::AddMidiTrack {
        name: "unsupported".into(),
    }));
    let b = h.context.unwrap();
    assert_ne!(a, b);
    assert!(refused.playback.is_none());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            Some(actual_a)
        );
        if let Ok(EditorAudioEvent::Publication {
            context,
            state: EditorPublicationState::Refused(_),
            ..
        }) = h.events.try_recv()
            && context == b
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "refusal lost behind cleanup/busy"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Stop remains direct while canonical state is unsupported and actual A is
    // still audible. Undo returns through the same sealed source domain.
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    h.run(Request::Undo);
    h.applied(h.context.unwrap());
    assert_eq!(h.domain(h.context.unwrap()), (a_domain, 2));
    h.run(Request::Undo); // import removed: genuinely empty typed successor
    h.applied(h.context.unwrap());
    assert_eq!(h.domain(h.context.unwrap()), (empty_domain, 2));
    let client = &mut h.audio.as_mut().unwrap().control;
    let play = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    h.render();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .stream_applied_command_prefix
            .load(Acquire)
            >= play
    );
    h.audio.as_ref().unwrap().control.stop();
    h.render();

    // Ordinary sequential imports must evict the unused empty cache through
    // actual retirement/report consumption, not strand a valid {A,new} edit at
    // N2 or allocate a third pair. A's actual active domain stays protected.
    h.run(Request::Import(path.clone()));
    h.applied(h.context.unwrap());
    let (sequential_a_domain, occupied) = h.domain(h.context.unwrap());
    assert_eq!(occupied, 2);
    assert_ne!(sequential_a_domain, a_domain); // reimport has a new actual asset ID
    let second = files.0.join("second.wav");
    fs::copy(&path, &second).unwrap();
    h.run(Request::Import(second));
    h.applied(h.context.unwrap());
    let (combined_domain, occupied) = h.domain(h.context.unwrap());
    assert_eq!(occupied, 2);
    assert_ne!(combined_domain, empty_domain); // retired cell has a new incarnation
    assert_ne!(combined_domain, sequential_a_domain);

    // New document generation is emitted independently of output readiness.
    // Holding its old Client really bars the replacement's exclusive lease.
    let old_client = h.audio.take().unwrap();
    let old_generation = h.context.unwrap().project_generation;
    let old_identity = old_client.control.output_identity();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert!(h.context.unwrap().project_generation > old_generation);
    std::thread::sleep(Duration::from_millis(20));
    assert!(h.backend.lock().unwrap().is_none());
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::RestartAudio {
            project_generation: old_generation,
            request_id: 1,
        })
        .unwrap();
    assert!(h.next().error.is_some()); // stale client/request cannot reopen successor
    drop(old_client);
    h.open();
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        old_identity
    );
    h.applied(h.context.unwrap());
    let prior_actual = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_audio_source();
    h.observation.lock().unwrap().change_source_after_capture = Some(path.clone());
    let changed = h.run(Request::Import(path));
    assert!(changed.error.is_none()); // valid canonical edit remains editable/saveable
    let failed_context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            prior_actual
        );
        if let Ok(EditorAudioEvent::Publication {
            context,
            state: EditorPublicationState::Refused(reason),
            ..
        }) = h.events.try_recv()
            && context == failed_context
        {
            assert!(reason.contains("source preparation"), "{reason}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "changed source was not explicitly refused"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(20));
    assert!(!h.events.try_iter().any(|event| matches!(event,
        EditorAudioEvent::Publication { context, state: EditorPublicationState::Busy, .. } if context == failed_context
    ))); // ordinary retiring cleanup cannot overwrite the permanent refusal
    // No UI reclaim or return collection drives final cleanup after close.
    drop(h);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::worker_fixture::open(48000) {
            Ok((output, backend)) => {
                drop(output);
                drop(backend);
                break;
            }
            Err(error) => assert!(
                Instant::now() < deadline,
                "autonomous retirement failed: {error}"
            ),
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn verified_adapter_seek_pause_and_priority_stop_preserve_applied_source_and_domain() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let path = files.wav(24000, 24000);
    let mut bytes = fs::read(&path).unwrap();
    for sample in bytes[44 + 4096 * 2..].chunks_exact_mut(2) {
        sample.copy_from_slice(&16384i16.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let mut h = Harness::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    assert!(imported.playback.is_none());
    let context = h.context.unwrap();
    let actual = h.applied(context);
    let domain = h.domain(context);
    let play = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_applied_command_prefix.load(Acquire) == play {
            // The real upsampler has a source-start boundary response. The
            // end of this block and the next complete block are constant PCM.
            assert!(output[63].iter().all(|s| (*s - 0.25).abs() < 0.0001));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "first verified page did not play"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let before_seek = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    // Project/device frames, not source frames: 12288 at48k addresses6144
    // in the real24k file, beyond both the first page and the sample step.
    let seek = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Seek(12288))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.applied_audio_source(), Some(actual));
        assert!(!status.render_fault.load(Acquire));
        if status.stream_applied_command_prefix.load(Acquire) == seek {
            assert_eq!(status.position.load(Acquire), 12288 + 64);
            assert!(output.iter().flatten().all(|s| (*s - 0.5).abs() < 0.0001));
            break;
        }
        // A missing exact target may buffer, but cannot partially seek or ACK.
        assert_eq!(status.position.load(Acquire), before_seek);
        assert_eq!(output, [[0.0; 2]; 64]);
        assert!(Instant::now() < deadline, "verified seek did not commit");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(h.domain(context), domain);
    let pause = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Pause)
        .unwrap();
    for _ in 0..2 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.position.load(Acquire), 12288 + 64);
        assert_eq!(status.stream_applied_command_prefix.load(Acquire), pause);
        assert!(!status.playing.load(Acquire));
    }
    // Stop has its existing independent priority marker even when the actual
    // command ring is full; it must not wait for queued transport work.
    for _ in 0..32 {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    assert_eq!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play),
        Err(PlaybackCommand::Play)
    );
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.playing.load(Acquire));
    assert_eq!(status.position.load(Acquire), 0);
    assert_eq!(status.applied_audio_source(), Some(actual));
    assert!(!status.render_fault.load(Acquire));
}

#[path = "controller_input_tests.rs"]
mod controller_input_tests;
#[test]
fn native_verified_downsample_uses_source_window_and_applies_decoded_pcm() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let source = files.wav(48000, 9600);
    let mut h = Harness::with_rate(44100);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let event = h.run(Request::Import(source.clone()));
    assert!(event.playback.is_none());
    assert_eq!(event.state.project.sample_rate(), 44100);
    let context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .is_some_and(|a| a.context == context)
        {
            break;
        }
        // Fail at the actual preparation refusal, rather than waiting out an
        // unrelated large-workload hash/control deadline.
        for event in h.events.try_iter() {
            if let EditorAudioEvent::Publication {
                state: EditorPublicationState::Refused(error) | EditorPublicationState::Fault(error),
                ..
            } = event
            {
                panic!("downsample refused: {error}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "downsample install did not complete"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    loop {
        let output = h.render();
        if output
            .iter()
            .all(|frame| frame.iter().all(|sample| (*sample - 0.25).abs() < 0.0001))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual downsample worker PCM missing"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(!status.render_fault.load(Acquire));
    assert!(status.position.load(Acquire) >= 64);
    // An explicitly insufficient reader allowance must still fail at the worker
    // guard; composing the normal caller policy does not bypass that limit.
    let mut insufficient = AudioPolicy::source_reservation();
    insufficient.reader.reader.max_window_frames = hex_engine::STREAM_PAGE_FRAMES;
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(insufficient),
    });
    h.open();
    h.applied(h.context.unwrap());
    h.run(Request::Import(source));
    let refused_context = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.render();
        let refused = h.events.try_iter().any(|event| matches!(event,
            EditorAudioEvent::Publication { context, state: EditorPublicationState::Refused(ref error), .. }
            if context == refused_context && error.contains("CatalogFailed(CapacityExhausted)")));
        if refused {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "undersized reader window was not refused"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(
        !status
            .applied_audio_source()
            .is_some_and(|a| a.context == refused_context)
    );
    assert!(!status.render_fault.load(Acquire));
}

#[test]
fn rapid_new_verified_import_supersession_applies_decoded_pcm() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let path = files.wav(48000, 9600);
    let mut h = Harness::with_rate(44100);
    h.next();
    h.open();
    for attempt in 0..8 {
        h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        });
        h.open();
        // Deliberately no empty graph ACK: actual import supersedes preparation.
        h.run(Request::Import(path.clone()));
        let context = h.context.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observations = Vec::new();
        loop {
            h.render();
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source()
                .is_some_and(|a| a.context == context)
                && h.observation.lock().unwrap().command_ready.is_some_and(
                    |(submitted_context, revision)| submitted_context == context && revision > 0,
                )
            {
                break;
            }
            for event in h.events.try_iter() {
                if let EditorAudioEvent::Publication { context, state, .. } = event {
                    observations.push(format!("{context:?}: {state:?}"));
                }
            }
            assert!(
                Instant::now() < deadline,
                "rapid attempt={attempt} context={context:?} events={observations:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        loop {
            let output = h.render();
            if output
                .iter()
                .all(|frame| frame.iter().all(|sample| (*sample - 0.25).abs() < 0.0001))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "rapid successor PCM missing at {attempt}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn exact_root_domain_retirement_intent_survives_cleaning_and_recovers_before_reuse() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let path = files.wav(24000, 16);
    let service = Service::new(44100, 1, 0).unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    next();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(AudioPolicy::source_reservation()),
        })
        .unwrap();
    next();
    service.submit(Request::Import(path.clone())).unwrap();
    let event = next();
    let snapshot = event.state.canonical_audio_source.as_ref().unwrap();
    // Earlier fixtures transfer their terminal owners asynchronously. Wait for
    // the real exclusive lease, exactly as the production completion adapter.
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut output, _backend) = loop {
        match hex_audio::worker_fixture::open(44100) {
            Ok(pair) => break pair,
            Err(error) if error == "audio output is open or still shutting down" => {}
            Err(error) => panic!("fixture output unavailable: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "prior native lease never released"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let policy = AudioPolicy::new().unwrap();
    let control = output.publication.control().unwrap();
    policy.configure(control).unwrap();
    let (events, _receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let mut previous = None;
    for already_failed in [false, true] {
        let sources = DomainSources::capture(snapshot, if already_failed { 2 } else { 1 }).unwrap();
        let definitions = sources.definitions().unwrap();
        if already_failed {
            let mut bytes = fs::read(&path).unwrap();
            *bytes.last_mut().unwrap() ^= 1;
            fs::write(&path, bytes).unwrap();
        }
        let token = control
            .begin_audio_candidate_domain_preparation(policy.stream, definitions, policy.readiness)
            .unwrap();
        assert_ne!(Some(token), previous);
        let mut domain = Domain {
            token,
            sources,
            ready: false,
            retiring: false,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        if already_failed {
            // Verification moves Root custody into retirement before the adapter
            // has made its first explicit retirement request.
            loop {
                match control.poll_audio_candidate_domain_preparation(token) {
                    Ok(hex_audio::AudioCandidateDomainPreparationProgress::Refused(_)) => break,
                    Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying) => {}
                    other => panic!("unexpected failed-source state: {other:?}"),
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        } else {
            domain.request_retirement(control);
            assert!(domain.retiring);
        }
        loop {
            control.reclaim();
            if !control.retire_audio_candidate_domain(token) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // False is not completion, nor a cancellation of local cleanup intent.
        domain.request_retirement(control);
        assert!(
            domain.retiring,
            "retirement intent disappeared during actual Root cleanup"
        );
        domain.request_retirement(control);
        assert!(domain.retiring);
        sink.domains[0] = Some(domain);
        loop {
            sink.maintain_returns(control);
            if sink.domains[0].is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "exact Root report was not recovered"
            );
            std::thread::yield_now();
        }
        assert!(control.recover_audio_candidate_domain(token).is_none()); // consumed once
        loop {
            if let Some(usage) = control.audio_candidate_domain_usage() {
                assert_eq!(usage.occupied_domains, 0);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        previous = Some(token); // next accepted cell has a new actual incarnation
    }
}

/// Explicit ~1 GB generated workload smoke. Not part of ordinary CI and not a
/// physical-device, sustained real-time or listening result.
#[test]
#[ignore = "requires create-new generated workload and HEX_STREAMING_WORKLOAD_PROJECT"]
fn generated_100_track_project_uses_actual_verified_service_and_callback() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let path = PathBuf::from(
        std::env::var_os("HEX_STREAMING_WORKLOAD_PROJECT").expect("fixture project path"),
    );
    let mut h = Harness::with_wait(Duration::from_secs(120));
    h.next();
    h.open();
    let started = Instant::now();
    let event = h.run(Request::OpenWithAudioMode {
        source: hex_session::service::ProjectSource::Saved(path),
        relinks: Default::default(),
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    assert_eq!(event.state.project.tracks().len(), 100);
    assert_eq!(event.state.project.assets().count(), 100);
    assert_eq!(event.state.project.session_cells().len(), 200);
    assert!(event.playback.is_none());
    let snapshot = event.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(snapshot.required_sources().count(), 100);
    assert!(
        snapshot
            .required_sources()
            .all(|(_, source)| matches!(source, AudioSourceBinding::VerifiedFile(_)))
    );
    let rate = event.state.project.sample_rate();
    h.open();
    eprintln!(
        "canonical100-source load complete after {:?}; verifying native domain",
        started.elapsed()
    );
    let context = h.context.unwrap();
    let deadline = Instant::now() + h.wait;
    let actual = loop {
        h.render();
        if let Some(actual) = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            && actual.context == context
        {
            break actual;
        }
        for event in h.events.try_iter() {
            if let EditorAudioEvent::Publication {
                context: observed,
                state: EditorPublicationState::Refused(error) | EditorPublicationState::Fault(error),
                ..
            } = event
                && observed == context
            {
                panic!("full workload preparation refused: {error}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "full workload actual install missing"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + h.wait;
    let mut signal = false;
    loop {
        let output = h.render();
        signal |= output.iter().flatten().any(|sample| sample.abs() > 0.0001);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(!status.render_fault.load(Acquire));
        let frames = status.position.load(Acquire);
        if frames >= u64::from(rate) * 2 {
            assert!(signal, "actual decoded workload remained silent");
            eprintln!(
                "headless workload: sources=100 rate={rate} applied={} frames={frames} elapsed={:?}; physical timing/listening NOT measured",
                actual.audio_revision,
                started.elapsed()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "100-source worker made no bounded progress; frame={frames}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();
}
