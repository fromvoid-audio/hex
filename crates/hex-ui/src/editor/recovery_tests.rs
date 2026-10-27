use super::*;
use crate::{EditorAudioEvent, EditorPublicationState, editor_backend::EditorAudio};
use hex_audio::prepared::{AdmittedPublishError, PublicationPolicy};
use hex_engine::{
    PreparedPlaybackObservation, native_device_resources::DeviceResourceAdmission,
    playback::Playback,
};
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{
    CanonicalAudioSourceSnapshot, Request, ResidentPublicationAcceptance,
    ResidentPublicationMetadata, ResidentPublicationPolicy, ResidentPublicationRefusal, Service,
    WorkerAudioSink,
};
use std::sync::{
    Arc, Mutex,
    atomic::AtomicBool,
    mpsc::{SyncSender, TryRecvError, sync_channel},
};
use std::time::{Duration, Instant};

// An already-open actual worker endpoint, without a second lifecycle implementation. The native
// desktop tests own reopen/teardown; this fixture exercises UI recovery through the real Service
// seek worker, ordinary resource book, callback and protected scalar observation channel.
struct Sink {
    output: hex_audio::WorkerPreparedOutput,
    events: SyncSender<EditorAudioEvent>,
    hold: Arc<AtomicBool>,
    held: Arc<Mutex<Option<EditorAudioEvent>>>,
}
impl WorkerAudioSink for Sink {
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.output
                .publication
                .device_resource_admission_on_control()
                .unwrap(),
            DeviceResourceAdmission::default(),
            DeviceResourceAdmission::default(),
        ]
    }
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {
        self.output.publication.control().unwrap().reclaim();
    }
    fn close(&mut self) {
        self.output.publication.close();
    }
    fn publish_resident(
        &mut self,
        playback: Playback,
        _: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        _: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let control = self.output.publication.control().unwrap();
        let output_identity = control.output_identity().unwrap();
        let policy = match policy {
            ResidentPublicationPolicy::InheritRuntime => PublicationPolicy::InheritRuntime,
            ResidentPublicationPolicy::InheritRuntimeAt(_) => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not capture resident loop witnesses".into(),
                });
            }
            ResidentPublicationPolicy::InstallFreshRuntime => {
                PublicationPolicy::InstallFreshRuntime
            }
            ResidentPublicationPolicy::RestorePluginState => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture has no fresh native plugin restoration".into(),
                });
            }
            ResidentPublicationPolicy::FreshIfUnchanged(expected)
            | ResidentPublicationPolicy::SectionWhenReady {
                fence: expected, ..
            } => {
                let Some(actual) = control.capture_fresh_runtime_fence().filter(|actual| {
                    output_identity == expected.output_identity
                        && actual.expected_active_revision() == expected.active_audio_revision
                        && actual.route_state_generation() == expected.route_generation
                        && actual.admitted_command_ticket() == expected.admitted_command_ticket
                }) else {
                    return Err(ResidentPublicationRefusal {
                        playback,
                        error: "fixture fence changed".into(),
                    });
                };
                match policy {
                    ResidentPublicationPolicy::SectionWhenReady { quantization, .. } => {
                        PublicationPolicy::InstallSectionWhenReady {
                            fence: actual,
                            quantization,
                        }
                    }
                    _ => PublicationPolicy::InstallFreshRuntimeIfUnchanged(actual),
                }
            }
            ResidentPublicationPolicy::ArrangementEdit => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not publish stopped arrangement replacements".into(),
                });
            }
            ResidentPublicationPolicy::ReplaceDocument => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not replace documents".into(),
                });
            }
        };
        let revision = control
            .publish_with_admission_on_control(
                playback,
                policy,
                metadata.context.project_generation,
                |view| {
                    admit(&[
                        view,
                        DeviceResourceAdmission::default(),
                        DeviceResourceAdmission::default(),
                    ])
                },
            )
            .map_err(|error| match error {
                AdmittedPublishError::Admission { reason, playback } => {
                    ResidentPublicationRefusal {
                        playback,
                        error: reason,
                    }
                }
                AdmittedPublishError::Publication(error) => ResidentPublicationRefusal {
                    playback: error.playback,
                    error: error.reason.into(),
                },
            })?;
        let event = EditorAudioEvent::Publication {
            context: metadata.context,
            output_identity,
            state: EditorPublicationState::ResidentSubmitted {
                audio_revision: revision,
                metadata,
                observation,
            },
        };
        if self.hold.load(Acquire) {
            assert!(self.held.lock().unwrap().replace(event).is_none());
        } else {
            self.events.try_send(event).unwrap();
        }
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity,
            audio_revision: revision,
        })
    }
}

#[gpui::test]
fn seek_during_latency_rebuild_cancels_the_automatic_resume(cx: &mut gpui::TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, _| {
        view.latency_rebuild = Some(super::super::audio_admission::LatencyRebuild::Restoring {
            project: super::super::audio_admission::ProjectRevision {
                generation: 0,
                revision: 0,
            },
            output: 1,
            frame: 123,
        });
        view.stopped_playback_restore =
            Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
        view.request_seek(456);
        assert!(view.latency_rebuild.is_none());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("wait before seeking")
        );
    });
}

#[gpui::test]
fn replaced_project_cannot_resume_an_older_latency_rebuild(cx: &mut gpui::TestAppContext) {
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.latency_rebuild = Some(super::super::audio_admission::LatencyRebuild::Restoring {
            project: super::super::audio_admission::ProjectRevision {
                generation: 0,
                revision: 0,
            },
            output: 1,
            frame: 123,
        });
        view.project_generation = 1;
        assert!(!view.tick_latency_rebuild());
        assert!(view.latency_rebuild.is_none());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn rejected_second_limiter_rebuilds_and_resumes_at_the_captured_position(
    cx: &mut gpui::TestAppContext,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: bytes.checked_mul(2).unwrap(),
            max_construction_peak_bytes: bytes.checked_mul(3).unwrap(),
        },
        max_scan_frames: 100_000,
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(reservation.storage)
        .unwrap();
    let client = output.take_client().unwrap();
    let (events, receiver) = sync_channel(64);
    let hold = Arc::new(AtomicBool::new(false));
    let held = Arc::new(Mutex::new(None));
    let release = events.clone();
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        Box::new(Sink {
            output,
            events,
            hold: Arc::clone(&hold),
            held: Arc::clone(&held),
        }),
        reservation,
    )
    .unwrap();
    let mut receive = || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            match service.try_event() {
                Ok(event) => {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    break event;
                }
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    receive();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "audio".into(),
        }))
        .unwrap();
    let track = match receive().created.unwrap() {
        Created::Track(track) => track,
        other => panic!("{other:?}"),
    };
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "second".into(),
        }))
        .unwrap();
    let second_track = match receive().created.unwrap() {
        Created::Track(track) => track,
        other => panic!("{other:?}"),
    };
    let add_limiter = |track| Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    };
    service.submit(add_limiter(track)).unwrap();
    let current = receive().state;
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        current.project.as_ref().clone(),
        Arc::clone(&current.midi_preview),
    );
    editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(current);
        view.project_generation = 0;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "latency fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.pending_playback = None;
        view.tick(cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|s| view.applied_project_revision == Some(s.revision))
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "initial graph did not apply");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        view.queue_audio_command(PlaybackCommand::PlayFrom(123))
    });
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    hold.store(true, std::sync::atomic::Ordering::Release);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(add_limiter(second_track)));
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let rejected_event = loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(cx, |view, cx| {
            view.tick(cx);
        });
        if let Some(event) = held.lock().unwrap().take() {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "candidate observation was not held"
        );
        std::thread::yield_now();
    };
    let rejected_revision = match &rejected_event {
        EditorAudioEvent::Publication {
            state: EditorPublicationState::ResidentSubmitted { audio_revision, .. },
            ..
        } => *audio_revision,
        _ => panic!("unexpected held publication"),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            view.observed_rejected_revision == rejected_revision
                && matches!(view.latency_rebuild, Some(super::super::audio_admission::LatencyRebuild::Unresolved { audio_revision, .. }) if audio_revision == rejected_revision)
        }) { break; }
        assert!(
            Instant::now() < deadline,
            "rejection did not arrive before its metadata"
        );
        std::thread::yield_now();
    }
    release.send(rejected_event).unwrap();
    let mut captured = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut released_restore = false;
    let mut stopped_seen = false;
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let complete = editor.update(cx, |view, cx| {
            view.tick(cx);
            if let Some(super::super::audio_admission::LatencyRebuild::Restoring {
                frame, ..
            }) = view.latency_rebuild
            {
                captured = Some(frame);
            }
            let status = view.audio_status().unwrap();
            stopped_seen |= captured.is_some() && !status.playing.load(Acquire);
            if stopped_seen && !released_restore {
                assert!(
                    !status.playing.load(Acquire),
                    "must not resume before restoration metadata"
                );
            }
            captured.is_some()
                && view.latency_rebuild.is_none()
                && view
                    .state
                    .as_ref()
                    .is_some_and(|s| view.applied_project_revision == Some(s.revision))
                && status.playing.load(Acquire)
        });
        if captured.is_some()
            && !released_restore
            && let Some(event) = held.lock().unwrap().take()
        {
            hold.store(false, std::sync::atomic::Ordering::Release);
            release.send(event).unwrap();
            released_restore = true;
        }
        if complete {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "rebuild did not resume (captured={captured:?}, released={released_restore}, stopped={stopped_seen}): {:?}",
            editor.read_with(cx, |view, _| (
                view.error.clone(),
                view.latency_rebuild,
                view.stopped_playback_restore,
                view.applied_project_revision,
                view.state.as_ref().map(|s| s.revision),
                view.playback_candidates
                    .iter()
                    .map(|c| (
                        c.audio_revision,
                        c.state.revision,
                        c.device_preview_identity.is_some()
                    ))
                    .collect::<Vec<_>>(),
                view.audio_status().map(|s| (
                    s.playing.load(Acquire),
                    s.applied_revision.load(Acquire),
                    s.rejected_revision()
                ))
            ))
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.position.load(Acquire) >= captured.unwrap());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(second_track)
                .unwrap()
                .devices()
                .len(),
            1
        );
    });

    // A later explicit Stop must win even if another rebuild is in flight.
    editor.update(cx, |view, _| {
        assert!(view.enqueue(add_limiter(second_track)));
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            matches!(
                view.latency_rebuild,
                Some(super::super::audio_admission::LatencyRebuild::Restoring { .. })
            )
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "second rebuild did not start");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| view.stop());
    for _ in 0..100 {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(cx, |view, cx| {
            view.tick(cx);
            assert!(view.latency_rebuild.is_none());
        });
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert!(!view.audio_status().unwrap().playing.load(Acquire))
    });
}

#[gpui::test]
fn recovered_mutable_cursor_uses_stopped_preroll_and_exact_callback_ack(
    cx: &mut gpui::TestAppContext,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    // Recovery constructs an interactive runtime with full release/handoff banks.
    let runtime_storage = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: runtime_storage.checked_mul(2).unwrap(),
            max_construction_peak_bytes: runtime_storage.checked_mul(3).unwrap(),
        },
        max_scan_frames: 100_000,
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(reservation.storage)
        .unwrap();
    let client = output.take_client().unwrap();
    let (events, receiver) = sync_channel(64);
    let hold = Arc::new(AtomicBool::new(false));
    let held = Arc::new(Mutex::new(None));
    let release = events.clone();
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        Box::new(Sink {
            output,
            events,
            hold: hold.clone(),
            held: held.clone(),
        }),
        reservation,
    )
    .unwrap();
    let mut receive = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            match service.try_event() {
                Ok(event) => {
                    backend.render(&mut [[0.0; 2]; 64]).unwrap();
                    assert!(event.error.is_none(), "{:?}", event.error);
                    break event;
                }
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    receive();
    service
        .submit(hex_session::service::Request::Edit(Command::AddMidiTrack {
            name: "recovery".into(),
        }))
        .unwrap();
    let event = receive();
    let track = event.state.project.tracks()[0].id();
    let path = std::env::temp_dir().join(format!(
        "hex-recovery-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&68_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&192_000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&32_u32.to_le_bytes());
    for _ in 0..16 {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    service
        .submit(hex_session::service::Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: event.state.revision,
            track,
            empty_sampler: None,
            path: path.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let loaded = receive();
    let Created::Asset(asset) = loaded.created.unwrap() else {
        panic!("asset")
    };
    let state = hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
        8, asset, 0.0,
    )
    .unwrap();
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    service
        .submit(hex_session::service::Request::Edit(
            Command::ReplaceDeviceState { device, state },
        ))
        .unwrap();
    receive();
    service
        .submit(hex_session::service::Request::Edit(
            Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            },
        ))
        .unwrap();
    let clip = receive();
    let Created::Clip(clip) = clip.created.unwrap() else {
        panic!("clip")
    };
    service
        .submit(hex_session::service::Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            velocity: 127,
            channel: 0,
        }))
        .unwrap();
    let current = receive().state;
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        current.project.as_ref().clone(),
        Arc::clone(&current.midi_preview),
    );
    editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(current);
        view.project_generation = 0;
        view.mode = super::super::EditorMode::Session;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "recovery fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.pending_playback = None;
        view.tick(cx);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| view.applied_project_revision == Some(state.revision))
                && view.current_seek_requires_preroll() == Some(true)
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "initial protected publication did not apply"
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, cx| {
        assert_eq!(view.current_seek_requires_preroll(), Some(true));
        let generation = view.recovery.begin(129).unwrap();
        view.recovery.opened(generation);
        view.recovery.restored(generation);
        view.pending_recovery_generation = Some(generation);
        view.tick(cx);
        let pending = view
            .prepared_seek
            .as_ref()
            .expect("recovery must submit a prepared seek, not raw Seek");
        assert_eq!(pending.recovery_generation, Some(generation));
        assert!(!pending.start_playing);
        assert!(pending.prepared_revision.is_none());
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.recovery.generation().is_none()
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "recovery did not finish");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let audible = view.audible_playback.as_ref().unwrap();
        assert!(audible.seek_request_id.is_some());
        let audio = view.audio.as_mut().unwrap();
        let status = audio.control.status().unwrap();
        assert_eq!(status.position.load(Relaxed), 129);
        assert!(!status.playing.load(Relaxed));
        assert_eq!(
            status.applied_revision.load(Acquire),
            audible.audio_revision
        );
        audio
            .control
            .command_with_ticket(PlaybackCommand::Play)
            .unwrap();
    });
    let mut sample = [[0.0; 2]; 1];
    backend.render(&mut sample).unwrap();
    assert_eq!(
        sample, [[0.25; 2]; 1],
        "prerolled sampler history must remain playable"
    );
    hold.store(true, std::sync::atomic::Ordering::Release);
    editor.update(cx, |view, cx| {
        let generation = view.recovery.begin(257).unwrap();
        view.recovery.opened(generation);
        view.recovery.restored(generation);
        view.pending_recovery_generation = Some(generation);
        view.tick(cx);
        let request = view.recovery.prepared_request().unwrap();
        view.request_seek(64);
        assert_eq!(
            view.recovery.prepared_request(),
            Some(request),
            "user seek cannot replace recovery's exact request"
        );
    });
    // Apply the real graph while its protected metadata is withheld from the UI.
    let deadline = Instant::now() + Duration::from_secs(5);
    let delayed_revision = loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let revision = held.lock().unwrap().as_ref().and_then(|event| match event {
            EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted { audio_revision, .. },
                ..
            } => Some(*audio_revision),
            _ => None,
        });
        let applied = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire)
        });
        if let Some(revision) = revision.filter(|revision| applied == *revision) {
            break revision;
        }
        assert!(Instant::now() < deadline, "withheld recovery did not apply");
        std::thread::yield_now();
    };
    editor.update(cx, |view, _| {
        view.stop();
        assert_eq!(view.recovery.position(), Some(0));
        assert_eq!(view.recovery.prepared_request(), None);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.recovery.generation().is_none()
        });
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Stop did not supersede prepared recovery"
        );
        std::thread::yield_now();
    }
    hold.store(false, std::sync::atomic::Ordering::Release);
    release
        .try_send(held.lock().unwrap().take().unwrap())
        .unwrap();
    for _ in 0..4 {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(cx, |view, cx| {
            view.tick(cx);
        });
    }
    editor.update(cx, |view, _| {
        let status = view.audio.as_ref().unwrap().control.status().unwrap();
        assert_eq!(status.position.load(Relaxed), 0);
        assert!(!status.playing.load(Relaxed));
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            delayed_revision,
            "late metadata must preserve the actual applied source after Stop"
        );
        assert!(
            view.current_preroll_source().is_some(),
            "normal seek must remain available"
        );
        view.audio.take().unwrap().control.close();
        let old = std::mem::replace(&mut view.service, Service::new(48_000, 1, 0).unwrap());
        drop(old);
    });
    backend.close();
    std::fs::remove_file(path).unwrap();
}
