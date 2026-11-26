//! Ordinary precommit file binding uses the existing two Root domain cells.
//! Polling, verification and cancellation stay on the Session worker. A sealed
//! catalog is insufficient: publication waits for actual callback delivery.
use super::*;
use hex_engine::playback::Playback;
use hex_session::service::{
    ResidentFileInput, ResidentFilePublication, ResidentFileSet, ResidentPublicationMetadata,
    ResidentPublicationPolicy,
};

impl NativeAudioSink {
    pub(super) fn prepare_resident_publication_files(
        &mut self,
        playback: &Playback,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        files: Option<&ResidentFilePublication<'_>>,
    ) -> Result<(), String> {
        let Some(files) = files else {
            return if playback.contains_streamed_sources() {
                Err("ordinary file publication facts missing".into())
            } else {
                Ok(())
            };
        };
        // Authoring aliases may advance the revision while retaining these
        // exact dependencies. File capture counters are provenance, not a fence.
        let matches_sources = playback
            .streamed_sources_on_control()
            .is_ok_and(|mut sources| {
                sources.all(|source| files.sources.metadata().matches_source(source))
            });
        if files.sources.context().project_generation != metadata.context.project_generation
            || !matches_sources
        {
            return Err("ordinary file publication source scope differs".into());
        }
        let private_output = self.lifecycle.replacement_output.is_some();
        if self.output.is_none()
            || (!private_output
                && matches!(policy, ResidentPublicationPolicy::ReplaceDocument { .. }))
        {
            return Ok(());
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        let timeout = self.hooks.resident_files_deadline;
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let timeout = STARTUP_OPEN_DEADLINE;
        let deadline = std::time::Instant::now() + timeout;
        let cancellation = self.request_cancellation.clone();
        let is_cancelled = || {
            files.shutdown.load(Acquire)
                || files.cancel.is_some_and(|cancel| cancel.load(Acquire))
                || cancellation.as_ref().is_some_and(|cancel| {
                    if private_output {
                        cancel.is_shutting_down()
                    } else {
                        cancel.is_cancelled()
                    }
                })
        };
        loop {
            if is_cancelled() {
                self.retire_resident_files(files.sources);
                return Err("ordinary file publication cancelled".into());
            }
            if private_output && let Err(error) = self.check_replacement_deadline() {
                self.retire_resident_files(files.sources);
                return Err(error);
            }
            if std::time::Instant::now() >= deadline {
                self.retire_resident_files(files.sources);
                return Err("timed out preparing ordinary audio files".into());
            }
            if self.poll_resident_files(files.sources, &is_cancelled)? {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    pub(super) fn poll_resident_files(
        &mut self,
        files: &ResidentFileSet,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<bool, String> {
        let refs = files
            .files()
            .iter()
            .map(|(id, file)| (*id, file))
            .collect::<Vec<_>>();
        let input = ResidentFileInput {
            context: files.context(),
            semantic_source_epoch: files.semantic_source_epoch(),
            files: &refs,
            pitches: files.pitches(),
            repitches: files.repitches().requirements(),
            granulars: files.granulars().requirements(),
        };
        let policy = self.policy.as_ref().map_err(Clone::clone)?;
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("audio publication endpoint is closed")?;
        control.reclaim();
        if control.status.device_fault.load(Acquire)
            || control.status.render_fault.load(Acquire)
            || control.status.shutdown_complete.load(Acquire)
        {
            return Err("ordinary source preparation lost its audio output".into());
        }
        for row in &mut self.verified.domains {
            if row.as_ref().is_some_and(|domain| {
                domain.retiring
                    && control
                        .recover_audio_candidate_domain(domain.token)
                        .is_some()
            }) {
                *row = None;
            }
        }
        let catalog = files.metadata().catalog_generation();
        let index = match self.verified.domains.iter().position(|row| {
            row.as_ref().is_some_and(|domain| {
                !domain.retiring
                    && domain.sources.catalog_generation() == catalog
                    && domain.sources.matches_files(&input)
            })
        }) {
            Some(index) => index,
            None => {
                let Some(index) = self.verified.domains.iter().position(Option::is_none) else {
                    // Retirement remains charged and cannot stop a domain while
                    // an actual ordinary graph retains a use. Never evict the
                    // newest cached domain while making room for its successor.
                    if !self
                        .verified
                        .domains
                        .iter()
                        .flatten()
                        .any(|domain| domain.retiring)
                        && let Some(domain) = self
                            .verified
                            .domains
                            .iter_mut()
                            .flatten()
                            .filter(|domain| {
                                control.audio_candidate_domain_has_no_graph_users(domain.token)
                            })
                            .min_by_key(|domain| domain.sources.catalog_generation())
                    {
                        domain.request_retirement(control);
                    }
                    return Ok(false);
                };
                let sources = DomainSources::capture_files(&input, catalog)?;
                let (definitions, granular) = sources.prepare_for_files(
                    &input,
                    policy.stream,
                    policy.granular,
                    &mut cancelled,
                )?;
                let started = match granular {
                    Some(bank) => control.begin_audio_candidate_granular_domain_preparation(
                        policy.stream,
                        definitions,
                        bank,
                        hex_stream::StreamReadinessReservation {
                            extents_per_role: policy
                                .readiness
                                .extents_per_role
                                .min(policy.stream.page_buffers.saturating_sub(2)),
                        },
                    ),
                    None => control.begin_audio_candidate_domain_preparation(
                        policy.stream,
                        definitions,
                        policy.readiness,
                    ),
                };
                let token = started
                    .map_err(|error| format!("ordinary source preparation refused: {error:?}"))?;
                self.verified.domains[index] = Some(Domain {
                    token,
                    sources,
                    ready: false,
                    retiring: false,
                });
                index
            }
        };
        let domain = self.verified.domains[index]
            .as_mut()
            .ok_or("ordinary source domain missing")?;
        if !domain.ready {
            match control.poll_audio_candidate_domain_preparation(domain.token) {
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Ready) => {
                    domain.ready = true
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying)
                | Err("candidate publication queue is full") => return Ok(false),
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Retiring) => {
                    return Err("ordinary source verification is retiring".into());
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Refused(error)) => {
                    domain.request_retirement(control);
                    return Err(format!("ordinary source verification refused: {error:?}"));
                }
                Err(error) => {
                    domain.request_retirement(control);
                    return Err(format!("ordinary source verification refused: {error}"));
                }
            }
        }
        if !domain.sources.covers_files(&input, domain.token, control) {
            domain.request_retirement(control);
            return Err("ordinary retained source catalog differs".into());
        }
        Ok(control.audio_candidate_domain_endpoint_is_installed(domain.token))
    }
    pub(super) fn retire_resident_files(&mut self, files: &ResidentFileSet) {
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return;
        };
        for domain in self.verified.domains.iter_mut().flatten().filter(|domain| {
            domain.sources.catalog_generation() == files.metadata().catalog_generation()
        }) {
            domain.request_retirement(control);
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use crate::native_audio::tests::{Files, native_fixture};
    use hex_audio::worker_fixture::{HeadlessBackend, open_with_reliable_action_outcomes};
    use hex_session::service::{Event, Request, Service};
    use std::{
        io::Write,
        path::Path,
        sync::Mutex,
        time::{Duration, Instant},
    };

    fn write_wav(path: &Path, frames: u32) {
        let bytes = frames * 2;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&96_000u32.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        for index in 0..frames {
            let sample = if index % 2 == 0 { 4096i16 } else { -4096i16 };
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    struct StalledHarness {
        service: Service,
        audio_events: std::sync::mpsc::Receiver<EditorAudioEvent>,
        audio: Option<EditorAudio>,
        backend: Arc<Mutex<Option<HeadlessBackend>>>,
    }

    impl StalledHarness {
        fn new(resident_files_deadline: Duration) -> Self {
            let (send, audio_events) = std::sync::mpsc::sync_channel(8);
            let mut sink = NativeAudioSink::new(send);
            sink.hooks.resident_files_deadline = resident_files_deadline;
            let backend = Arc::new(Mutex::new(None));
            let callback = Arc::clone(&backend);
            sink.hooks.project_rate_probe = Box::new(|| Some(48_000));
            sink.hooks.output_factory = Some(Box::new(move |rate| {
                let project_hz = rate.unwrap_or(48_000);
                let (mut output, native) = match open_with_reliable_action_outcomes(project_hz) {
                    Ok(pair) => pair,
                    Err(error) if error == "audio output is open or still shutting down" => {
                        return Ok(None);
                    }
                    Err(error) => return Err(error.into()),
                };
                output.label = "resident-files test".into();
                output.route = Some(hex_audio::OutputRoute {
                    device: "fixture".into(),
                    nominal_hz: 48_000,
                });
                *callback.lock().unwrap() = Some(native);
                Ok(Some(output))
            }));
            let service = Service::with_audio_sink_and_native_reservation(
                16,
                64,
                None,
                Box::new(sink),
                AudioPolicy::new().unwrap().resident_native,
            )
            .unwrap();
            let mut harness = Self {
                service,
                audio_events,
                audio: None,
                backend,
            };
            harness.wait_initial_ready();
            harness
        }

        fn pump_audio_events(&mut self) {
            while let Ok(event) = self.audio_events.try_recv() {
                if let EditorAudioEvent::Opened { audio, .. } = event {
                    self.audio = Some(audio);
                }
            }
        }

        fn render_once(&self) {
            let mut pcm = [[0.0; 2]; 64];
            let mut host = [0.0; 128];
            if let Some(callback) = self.backend.lock().unwrap().as_mut() {
                callback.render_direct(&mut pcm, &mut host).unwrap();
            }
        }

        fn wait_initial_ready(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut got_service_event = false;
            loop {
                self.pump_audio_events();
                self.render_once();
                if let Ok(event) = self.service.try_event() {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    got_service_event = true;
                }
                let applied = self
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .and_then(|status| status.recording_source())
                    .is_some();
                if got_service_event && applied {
                    return;
                }
                assert!(Instant::now() < deadline, "initial service ready timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn recv_event(&mut self, render: bool, timeout: Duration) -> Event {
            let deadline = Instant::now() + timeout;
            loop {
                if render {
                    self.render_once();
                }
                if let Ok(event) = self.service.try_event() {
                    return event;
                }
                assert!(Instant::now() < deadline, "timed out waiting for event");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    #[test]
    fn same_rate_publication_times_out_and_retires_domain_when_callback_stalls() {
        let _native = native_fixture();
        let files = Files::new();
        let wav = files.0.join("stalled.wav");
        write_wav(&wav, 512);
        let mut h = StalledHarness::new(Duration::from_millis(150));

        // Do not render the callback: endpoint installation stalls on a same-rate publication.
        let started = Instant::now();
        h.service
            .submit(Request::ImportToNewTrack {
                path: wav.clone(),
                before_track: None,
                start: 0,
            })
            .unwrap();
        let event = h.recv_event(false, Duration::from_secs(2));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "stalled same-rate publication took too long: {elapsed:?}"
        );
        let error = event
            .error
            .as_deref()
            .expect("expected error event on timeout");
        assert!(
            error.contains("timed out preparing ordinary audio files"),
            "unexpected timeout error: {error}"
        );

        // Once the callback resumes, the retired candidate domain is reclaimed and a new
        // same-rate streamed import succeeds cleanly.
        for _ in 0..8 {
            h.render_once();
            std::thread::sleep(Duration::from_millis(2));
        }
        h.service
            .submit(Request::ImportToNewTrack {
                path: wav,
                before_track: None,
                start: 0,
            })
            .unwrap();
        let recovered = h.recv_event(true, Duration::from_secs(3));
        assert!(
            recovered.error.is_none(),
            "expected import recovery after retired domain reclamation, got {:?}",
            recovered.error
        );
        assert_eq!(recovered.state.project.tracks().len(), 1);
    }

    #[test]
    fn same_rate_publication_honors_request_cancellation_without_plugin_edit() {
        let _native = native_fixture();
        let files = Files::new();
        let wav = files.0.join("cancelled.wav");
        write_wav(&wav, 512);
        // Long deadline ensures cancellation—not timeout—exits the poll loop.
        let mut h = StalledHarness::new(Duration::from_secs(5));

        let started = Instant::now();
        h.service
            .submit(Request::ImportToNewTrack {
                path: wav,
                before_track: None,
                start: 0,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        // Advancing document intent cancels the in-flight same-rate import unconditionally
        // even with no plugin edit active.
        h.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();

        let first = h.recv_event(false, Duration::from_secs(2));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancellation waited for full 5s deadline"
        );
        let error = first
            .error
            .as_deref()
            .expect("expected error event on cancellation");
        assert!(
            error.contains("ordinary file publication cancelled"),
            "unexpected cancellation error: {error}"
        );

        let second = h.recv_event(true, Duration::from_secs(3));
        assert!(
            second.project_replaced && second.error.is_none(),
            "expected NewWithAudioMode to complete after cancellation, got {:?}",
            second.error
        );
    }
}
