//! Exercise ordinary saved-file open with actual Session and callback custody.
//! The physical device and its post-callback conversion are substituted.
use super::*;
use crate::native_audio::tests::{Files, native_fixture};
use hex_audio::worker_fixture::{HeadlessBackend, open_with_reliable_action_outcomes};
use hex_engine::{Frame, PlaybackCommand};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_session::service::{Event, Request};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU8, AtomicU32, Ordering::Release},
    },
};

struct Harness {
    service: Service,
    events: mpsc::Receiver<EditorAudioEvent>,
    backend: Arc<Mutex<Option<HeadlessBackend>>>,
    audio: Option<EditorAudio>,
    rebound: Option<(u64, u64)>,
    nominal_device_rate: Arc<AtomicU32>,
    fail: Arc<AtomicU8>,
}
impl Harness {
    fn new() -> Self {
        let (send, events) = mpsc::sync_channel(4);
        let mut sink = NativeAudioSink::new(send);
        let backend = Arc::new(Mutex::new(None));
        let callback = Arc::clone(&backend);
        let nominal_device_rate = Arc::new(AtomicU32::new(48_000));
        let device = Arc::clone(&nominal_device_rate);
        let probe = Arc::clone(&nominal_device_rate);
        sink.project_rate_probe = Box::new(move || Some(probe.load(Acquire)));
        let fail = Arc::new(AtomicU8::new(0));
        let failure = Arc::clone(&fail);
        sink.output_factory = Some(Box::new(move |rate| {
            let device_hz = device.load(Acquire);
            let project_hz = rate.unwrap_or(device_hz);
            let fault = failure.load(Acquire);
            if fault == 1 && project_hz != device_hz {
                return Err(hex_audio::RateMismatch {
                    device_hz,
                    project_hz,
                }
                .into());
            }
            match open_with_reliable_action_outcomes(project_hz) {
                Ok((mut output, mut native)) => {
                    if fault == 2 && project_hz != device_hz {
                        native.report_device_error().unwrap();
                    }
                    output.label = format!("{project_hz} hz client on {device_hz} hz fixture");
                    output.route = Some(hex_audio::OutputRoute {
                        device: "fixture".into(),
                        nominal_hz: device_hz,
                    });
                    *callback.lock().unwrap() = Some(native);
                    Ok(Some(output))
                }
                Err(error) if error == "audio output is open or still shutting down" => Ok(None),
                Err(error) => Err(error.into()),
            }
        }));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            16_384,
            None,
            Box::new(sink),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            service,
            events,
            backend,
            audio: None,
            rebound: None,
            nominal_device_rate,
            fail,
        }
    }

    fn render(&self) -> [Frame; 64] {
        let mut pcm = [[0.0; 2]; 64];
        let mut host = [0.0; 128];
        if let Some(callback) = self.backend.lock().unwrap().as_mut() {
            let counts = allocation_counter::measure(|| {
                callback.render_direct(&mut pcm, &mut host).unwrap();
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0),
            );
        }
        pcm
    }

    fn step(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                EditorAudioEvent::OutputReleased {
                    old_output_identity,
                    ..
                } => {
                    if self
                        .audio
                        .as_ref()
                        .and_then(|a| a.control.output_identity())
                        == Some(old_output_identity)
                    {
                        self.audio = None;
                    }
                }
                EditorAudioEvent::Opened { audio, .. } => self.audio = Some(audio),
                EditorAudioEvent::OutputRebound {
                    project_generation,
                    output_identity,
                } => {
                    let audio = self.audio.as_mut().unwrap();
                    assert_eq!(audio.control.output_identity(), Some(output_identity));
                    assert_eq!(
                        audio
                            .status()
                            .unwrap()
                            .recording_source()
                            .unwrap()
                            .context
                            .project_generation,
                        project_generation
                    );
                    audio.control.bind_resident_transport(project_generation);
                    self.rebound = Some((project_generation, output_identity));
                }
                _ => {}
            }
        }
        let mut backend = self.backend.lock().unwrap();
        if backend
            .as_ref()
            .is_some_and(HeadlessBackend::shutdown_requested)
        {
            *backend = None;
        }
        drop(backend);
        // Simulate the physical callback even before the editor gets the Client.
        if self.fail.load(Acquire) != 2 || self.audio.is_some() {
            self.render();
        }
    }

    fn wait<T>(&mut self, mut ready: impl FnMut(&Self) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            self.step();
            if let Some(result) = ready(self) {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "fixture did not reach its terminal"
            );
            std::thread::yield_now();
        }
    }

    fn submit(&mut self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        self.wait(|h| h.service.try_event().ok())
    }

    fn await_playback(&mut self, event: &Event, rate: u32) {
        let context = event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context();
        self.wait(|h| {
            let audio = h.audio.as_ref()?;
            assert_eq!(audio.sample_rate, rate);
            let applied = audio.status()?.recording_source()?;
            (applied.context == context).then_some(())
        });
    }
}

#[test]
fn same_rate_native_open_and_new_keep_the_delivered_output() {
    let _native = native_fixture();
    let files = Files::new();
    let (path, _, project) = saved_fixture(&files, 48_000);
    let mut h = Harness::new();
    let initial = h.wait(|h| h.service.try_event().ok());
    h.await_playback(&initial, 48_000);
    let identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();

    let opened = h.submit(Request::Open(path.clone()));
    assert!(opened.project_replaced, "{:?}", opened.error);
    assert_eq!(*opened.state.project, project);
    h.await_playback(&opened, 48_000);
    let generation = opened
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(identity)
    );
    h.wait(|h| (h.rebound == Some((generation, identity))).then_some(()));
    let refused = h.submit(Request::Open(files.0.join("missing.hex")));
    assert!(refused.open_complete && !refused.project_replaced && refused.error.is_some());
    assert_eq!(refused.state.project, opened.state.project);
    h.await_playback(&opened, 48_000);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(identity)
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert!((0..8).flat_map(|_| h.render()).any(|frame| frame[0] > 0.30));
    let replacement = h.submit(Request::Open(path));
    assert!(replacement.project_replaced, "{:?}", replacement.error);
    h.await_playback(&replacement, 48_000);
    let replacement_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    assert_ne!(replacement_identity, identity);
    h.audio.as_ref().unwrap().control.stop();
    h.render();

    let new = h.submit(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced, "{:?}", new.error);
    h.await_playback(&new, 48_000);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(replacement_identity)
    );
    h.wait(|h| (h.rebound == Some((generation + 2, replacement_identity))).then_some(()));
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

fn saved_fixture(files: &Files, rate: u32) -> (PathBuf, PathBuf, Project) {
    let wav = files.wav(rate, rate / 20);
    let mut editor = Editor::new(Project::new(rate).unwrap(), 8).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: wav.clone(),
            sample_rate: rate,
            frames: u64::from(rate / 20),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let path = files.0.join("source.hex");
    let project = editor.project().clone();
    hex_storage::save_new(&path, &project).unwrap();
    (path, wav, project)
}

#[test]
fn new_project_uses_current_interface_rate() {
    let _native = native_fixture();
    let mut h = Harness::new();
    let initial = h.wait(|h| h.service.try_event().ok());
    h.await_playback(&initial, 48_000);

    h.nominal_device_rate.store(44_100, Release);
    let opened = h.submit(Request::NewWithAudioMode {
        mode: hex_session::DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(opened.project_replaced, "{:?}", opened.error);
    assert_eq!(opened.state.project.sample_rate(), 44_100);
    h.await_playback(&opened, 44_100);
}

#[test]
fn ordinary_open_uses_saved_rate_in_both_directions_and_round_trips() {
    let _native = native_fixture();
    let mut h = Harness::new();
    let initial = h.wait(|h| h.service.try_event().ok());
    h.await_playback(&initial, 48_000);
    for (device, rate) in [(48_000, 44_100), (44_100, 48_000)] {
        let previous_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.nominal_device_rate.store(device, Release);
        let files = Files::new();
        let (path, wav, expected) = saved_fixture(&files, rate);
        let original = std::fs::read(&wav).unwrap();
        let original_document = std::fs::read(&path).unwrap();

        // File > Open and Open Recent both submit this ordinary request.
        let opened = h.submit(Request::Open(path.clone()));
        assert!(
            opened.project_replaced && opened.open_complete,
            "{:?}",
            opened.error
        );
        assert_eq!(*opened.state.project, expected);
        assert!(!opened.state.unsaved);
        h.await_playback(&opened, rate);
        let audio = h.audio.as_mut().unwrap();
        assert_ne!(audio.control.output_identity(), Some(previous_identity));
        assert_eq!(
            audio.label,
            format!("{rate} hz client on {device} hz fixture")
        );
        audio.control.command(PlaybackCommand::Play).unwrap();
        let frames: Vec<_> = (0..(rate / 640 + 1)).flat_map(|_| h.render()).collect();
        // The aligned audio clip and A4 note start at the same musical origin.
        // After the audio clip ends exactly at 50 ms, only A4 remains.
        let end = (rate / 20) as usize;
        assert!((frames[end - 1][0] - frames[end][0]).abs() > 0.15);
        assert!(frames[256..512].iter().any(|frame| frame[0] > 0.30));
        let crossings = frames[end..2 * end]
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert_eq!(crossings, 22, "440 hz at the project-rate callback");
        h.audio.as_ref().unwrap().control.stop();
        h.render();

        let copy = files.0.join("copy.hex");
        let saved = h.submit(Request::SaveNew(copy.clone()));
        assert!(saved.error.is_none(), "{:?}", saved.error);
        assert_eq!(hex_storage::load(&copy).unwrap(), expected);
        // Reopen the copy at the same session rate on the converting output.
        let reopened = h.submit(Request::Open(copy));
        assert!(reopened.project_replaced, "{:?}", reopened.error);
        assert_eq!(*reopened.state.project, expected);
        h.await_playback(&reopened, rate);
        assert_eq!(std::fs::read(wav).unwrap(), original);
        assert_eq!(std::fs::read(path).unwrap(), original_document);
    }
}

#[test]
fn refused_output_open_or_install_keeps_unsaved_document_and_can_retry() {
    let _native = native_fixture();
    let mut h = Harness::new();
    let initial = h.wait(|h| h.service.try_event().ok());
    h.await_playback(&initial, 48_000);
    let edited = h.submit(Request::Edit(Command::AddTrack {
        name: "unsaved".into(),
    }));
    h.await_playback(&edited, 48_000);
    let files = Files::new();
    let (path, _, expected) = saved_fixture(&files, 44_100);
    for (fault, message) in [
        (1, "default output rate is 48000 hz"),
        (2, "output failed during publication"),
    ] {
        h.fail.store(fault, Release);
        let refused = h.submit(Request::Open(path.clone()));
        assert!(refused.open_complete && !refused.project_replaced);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|error| error.contains(message)),
            "{:?}",
            refused.error
        );
        assert_eq!(refused.state.project, edited.state.project);
        assert_eq!(refused.state.revision, edited.state.revision);
        assert!(refused.state.unsaved && refused.state.can_undo);
        h.fail.store(0, Release);
        let restarted = h.submit(Request::RestartAudio {
            project_generation: 0,
            request_id: u64::from(fault),
        });
        assert!(restarted.error.is_none(), "{:?}", restarted.error);
        h.await_playback(&edited, 48_000);
    }
    let opened = h.submit(Request::Open(path));
    assert!(opened.project_replaced, "{:?}", opened.error);
    assert_eq!(*opened.state.project, expected);
    h.await_playback(&opened, 44_100);
}
