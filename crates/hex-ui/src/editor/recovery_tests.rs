use super::*;
use crate::{EditorAudioEvent, EditorPublicationState, editor_backend::EditorAudio};
use hex_audio::prepared::{AdmittedPublishError, PublicationPolicy};
use hex_engine::{
    PreparedPlaybackObservation, native_device_resources::DeviceResourceAdmission,
    playback::Playback,
};
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{
    CanonicalAudioSourceSnapshot, ResidentPublicationAcceptance, ResidentPublicationMetadata,
    ResidentPublicationPolicy, ResidentPublicationRefusal, Service, WorkerAudioSink,
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
            ResidentPublicationPolicy::InstallFreshRuntime => {
                PublicationPolicy::InstallFreshRuntime
            }
            ResidentPublicationPolicy::FreshIfUnchanged(expected) => {
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
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(actual)
            }
            ResidentPublicationPolicy::StoppedArrangement => {
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
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
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
