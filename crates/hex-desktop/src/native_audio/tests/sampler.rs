use super::*;
use hex_project::{BeatTime, Command, Created};

impl Harness {
    fn resident_sampler(frame_limit: u64) -> Self {
        let backend = Arc::new(Mutex::new(None));
        let factory_backend = Arc::clone(&backend);
        let (events, receiver) = mpsc::sync_channel(1);
        let mut sink = NativeAudioSink::new(events);
        sink.output_factory = Some(Box::new(move |rate| {
            let mut slot = factory_backend.lock().unwrap();
            if slot.is_some() {
                return Ok(None);
            }
            match hex_audio::worker_fixture::open(rate.unwrap_or(16000)) {
                Ok((output, callback)) => {
                    *slot = Some(callback);
                    Ok(Some(output))
                }
                Err(e) if e == "audio output is open or still shutting down" => Ok(None),
                Err(e) => Err(e),
            }
        }));
        let observation = Arc::new(Mutex::new(Observation::default()));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            frame_limit,
            None,
            Box::new(ObservedSink {
                inner: sink,
                observation: Arc::clone(&observation),
            }),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            service: Some(service),
            events: receiver,
            backend,
            audio: None,
            context: None,
            observation,
            wait: Duration::from_secs(10),
        }
    }
    fn open_sampler(&mut self, old_identity: u64) {
        let end = Instant::now() + self.wait;
        let mut released = false;
        loop {
            match self.events.try_recv() {
                Ok(EditorAudioEvent::RelinquishClient {
                    old_output_identity,
                    target,
                }) => {
                    assert_eq!(old_output_identity, old_identity);
                    assert_eq!(Some(target), self.context);
                    assert!(!released);
                    self.audio = None;
                    self.backend.lock().unwrap().take();
                    released = true;
                }
                Ok(EditorAudioEvent::Opened { audio, .. }) => {
                    assert!(
                        released,
                        "a new client must not precede old endpoint release"
                    );
                    assert_ne!(audio.control.output_identity(), Some(old_identity));
                    self.audio = Some(audio);
                    return;
                }
                Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("{error}"),
                Ok(EditorAudioEvent::Publication {
                    state: EditorPublicationState::Refused(e) | EditorPublicationState::Fault(e),
                    ..
                }) => panic!("{e}"),
                _ => {}
            }
            assert!(Instant::now() < end, "sampler output did not open");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
fn wav(files: &Files, stereo: bool) -> PathBuf {
    let samples = if stereo {
        vec![4096_i16, -8192, 16384, 0, -4096, 8192, 0, 0]
    } else {
        vec![4096_i16, 16384, -4096, 0]
    };
    let channels = if stereo { 2u16 } else { 1 };
    let length = u32::try_from(samples.len() * 2).unwrap();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&(8000 * u32::from(channels) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&length.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let path = files.0.join("sampler.wav");
    fs::write(&path, bytes).unwrap();
    path
}
fn authored(
    h: &mut Harness,
    files: &Files,
    stereo: bool,
    tuned: bool,
    short_gate: bool,
) -> (hex_project::TrackId, hex_project::AssetId) {
    let initial = h.next();
    assert!(initial.error.is_none());
    h.open();
    authored_current(h, files, stereo, tuned, short_gate)
}
fn authored_current(
    h: &mut Harness,
    files: &Files,
    stereo: bool,
    tuned: bool,
    short_gate: bool,
) -> (hex_project::TrackId, hex_project::AssetId) {
    if tuned {
        h.run(Request::Edit(Command::SetTuning(tuning(880.0))));
    }
    let path = wav(files, stereo);
    let imported = h.run(Request::ImportDeviceAsset {
        path: path.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let Some(Created::Asset(asset)) = imported.created else {
        panic!("asset")
    };
    fs::remove_file(path).unwrap(); // Neither private preparation nor export may reread it.
    let Some(Created::Track(track)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "sampler".into(),
        }))
        .created
    else {
        panic!("track")
    };
    let Some(Created::Clip(clip)) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }))
        .created
    else {
        panic!("clip")
    };
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: if short_gate {
            BeatTime::from_raw(BeatTime::ONE.raw() / 8000)
        } else {
            BeatTime::ONE
        },
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (track, asset)
}
#[test]
fn service_sampler_original_wav_reaches_real_fresh_native_current_with_tuning_stop_and_export() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (stereo, tuned, short_gate) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let files = Files::new();
        let mut h = Harness::resident_sampler(8);
        let (track, asset) = authored(&mut h, &files, stereo, tuned, short_gate);
        let old_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let assigned = h.run(Request::AssignSampler { track, asset });
        assert!(assigned.playback.is_none());
        assert!(
            assigned
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .is_empty()
        );
        assert!(
            assigned
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .candidate_reservation_quote()
                .is_err(),
            "metadata alone must not construct a sampler"
        );
        let revision = assigned.state.revision;
        h.open_sampler(old_identity);
        let context = h.context.unwrap();
        h.applied(context);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let output = h.render();
        let left = if short_gate {
            vec![0.125]
        } else if tuned {
            vec![0.125, 0.5, -0.125, 0.0]
        } else {
            vec![0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0]
        };
        let right = if stereo {
            if short_gate {
                vec![-0.25]
            } else if tuned {
                vec![-0.25, 0.0, 0.25, 0.0]
            } else {
                vec![-0.25, -0.125, 0.0, 0.125, 0.25, 0.125, 0.0]
            }
        } else {
            left.clone()
        };
        let mut expected = [[0.0; 2]; 64];
        for (index, (l, r)) in left.into_iter().zip(right).enumerate() {
            expected[index] = [l, r];
        }
        for (a, b) in output.iter().zip(expected) {
            for i in 0..2 {
                assert!((a[i] - b[i]).abs() < 1e-6, "{a:?} != {b:?}");
            }
        }
        // Reach the authored gate-off through ordinary callback blocks; every
        // block also probes total and signed-net allocation/free counts.
        for _ in 0..128 {
            assert_eq!(h.render(), [[0.0; 2]; 64]);
        }
        h.audio.as_ref().unwrap().control.stop();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        assert_eq!(h.render(), output);
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::Edit(Command::Gain { track, gain: 0.5 }))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, revision);
        // Existing exporter captures a separate scope but shares/charges only its
        // actual original source owner, alongside the installed independent copy.
        let export_path = files.0.join("export.wav");
        h.run(Request::ExportRange {
            path: export_path.clone(),
            start: 0,
            frames: Some(12),
            signal: hex_session::service::ExportSignal::Main,
            overwrite: false,
            control: hex_export_control(),
        });
        let end = Instant::now() + h.wait;
        loop {
            match h.service.as_ref().unwrap().try_export_event() {
                Ok(event) => {
                    assert!(event.result.is_ok(), "{:?}", event.result);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(Instant::now() < end);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        }
        let bytes = fs::read(&export_path).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16000);
        let pcm = bytes[44..]
            .chunks_exact(4)
            .map(|f| {
                [
                    i16::from_le_bytes([f[0], f[1]]),
                    i16::from_le_bytes([f[2], f[3]]),
                ]
            })
            .collect::<Vec<_>>();
        let literal = expected[..12]
            .iter()
            .map(|f| [(f[0] * 32768.0) as i16, (f[1] * 32768.0) as i16])
            .collect::<Vec<_>>();
        assert_eq!(pcm, literal);
        assert_eq!(
            h.render(),
            [[0.0; 2]; 64],
            "offline export must not reset the installed native runtime"
        );
    }
}
fn hex_export_control() -> hex_session::service::ExportControl {
    Default::default()
}

fn tuning(reference_hz: f64) -> hex_tuning::ProjectTuning {
    let base = hex_tuning::ProjectTuning::default();
    let mut scale = base.scale();
    scale
        .set_reference(scale.root_key(), reference_hz, scale.root_degree())
        .unwrap();
    hex_tuning::ProjectTuning::new(true, scale, base.audition()).unwrap()
}

#[test]
fn sampler_copy_uses_the_same_frame_cap_and_preserves_old_output_on_refusal() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let mut h = Harness::resident_sampler(7);
    let (track, asset) = authored(&mut h, &files, false, false, false);
    let old = h.audio.as_ref().unwrap().control.output_identity();
    let assigned = h.run(Request::AssignSampler { track, asset });
    let end = Instant::now() + h.wait;
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::Publication {
                context,
                state: EditorPublicationState::Refused(e),
                ..
            }) if Some(context) == h.context => {
                assert!(e.contains("frame limit"), "{e}");
                break;
            }
            Ok(EditorAudioEvent::RelinquishClient { .. }) | Ok(EditorAudioEvent::Opened { .. }) => {
                panic!("budget refusal must precede old output disturbance")
            }
            _ => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), old);
    assert_eq!(h.observation.lock().unwrap().native_hold.frames, 0);
    assert!(
        assigned
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_some()
    );
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}
#[test]
fn new_during_sampler_drain_cleans_preflight_and_never_relabels_it_to_new_document() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let mut h = Harness::resident_sampler(8);
    let (track, asset) = authored(&mut h, &files, true, false, false);
    h.run(Request::AssignSampler { track, asset });
    let old_context = h.context.unwrap();
    let end = Instant::now() + h.wait;
    while h.observation.lock().unwrap().native_hold.frames != 4 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    // Do not service the release event: the old Client/backend still prevent
    // a fresh native open, while the original copy is already preflighted.
    let replacement = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(replacement.state.project.tracks().is_empty());
    assert_ne!(
        h.context.unwrap().project_generation,
        old_context.project_generation
    );
    h.open();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    while h.observation.lock().unwrap().native_hold.frames != 0 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn sampler_fresh_output_failure_cleans_unpublished_copy_and_allows_new_context_retry() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let backend = Arc::new(Mutex::new(None));
    let factory_backend = Arc::clone(&backend);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    let mut opens = 0;
    sink.output_factory = Some(Box::new(move |rate| {
        let mut slot = factory_backend.lock().unwrap();
        if slot.is_some() {
            return Ok(None);
        }
        match hex_audio::worker_fixture::open(rate.unwrap_or(16000)) {
            Ok((output, callback)) => {
                opens += 1;
                if opens == 2 {
                    drop(callback);
                    drop(output);
                    return Err("injected fresh output failure".into());
                }
                *slot = Some(callback);
                Ok(Some(output))
            }
            Err(e) if e == "audio output is open or still shutting down" => Ok(None),
            Err(e) => Err(e),
        }
    }));
    let observation = Arc::new(Mutex::new(Observation::default()));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8,
        None,
        Box::new(ObservedSink {
            inner: sink,
            observation: observation.clone(),
        }),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let mut h = Harness {
        service: Some(service),
        events: receiver,
        backend,
        audio: None,
        context: None,
        observation,
        wait: Duration::from_secs(10),
    };
    let (track, asset) = authored(&mut h, &files, false, false, false);
    let revision = h
        .run(Request::AssignSampler { track, asset })
        .state
        .revision;
    let end = Instant::now() + h.wait;
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::RelinquishClient { .. }) => {
                h.audio = None;
                h.backend.lock().unwrap().take();
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => {
                assert_eq!(error, "injected fresh output failure");
                break;
            }
            Ok(EditorAudioEvent::Opened { .. }) => {
                panic!("a failed fresh output must not expose a client")
            }
            _ => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    while h.observation.lock().unwrap().native_hold.frames != 0 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let renamed = h.run(Request::Edit(Command::RenameTrack {
        track,
        name: "retry".into(),
    }));
    assert!(renamed.state.revision > revision);
    // The failed unpublished copy was cleaned, so a fresh canonical borrow can
    // retry. No old owner is relabeled to this revision and no old client exists.
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::Opened { audio, .. }) => {
                h.audio = Some(audio);
                break;
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("{error}"),
            _ => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render()[0], [0.125; 2]);
}

#[test]
fn new_cancels_actual_draining_sampler_before_immediate_fresh_open_poll() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let mut h = Harness::resident_sampler(8);
    let (track, asset) = authored(&mut h, &files, false, false, false);
    h.run(Request::AssignSampler { track, asset });
    let end = Instant::now() + h.wait;
    while h.observation.lock().unwrap().native_hold.frames != 4 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    h.observation.lock().unwrap().fresh_before_input =
        Some((h.audio.take().unwrap(), Arc::clone(&h.backend)));
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(h.observation.lock().unwrap().fresh_before_input_checked);
    h.open();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

#[test]
fn actual_old_resident_current_pcm_remains_charged_across_new_and_sampler_preflight() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (cap, admitted) in [(15, false), (16, true)] {
        let files = Files::new();
        let mut h = Harness::resident_sampler(cap);
        h.next();
        h.open();
        let path = wav(&files, false);
        let mut imported = h.run(Request::Import(path));
        let playback = imported.playback.take().unwrap();
        assert_eq!(playback.resident_pcm_capacity_frames(), Some(8));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .forward_resident_publication(
                playback,
                hex_audio::prepared::PublicationPolicy::InheritRuntime,
                h.context.unwrap().project_generation,
            )
            .unwrap();
        let end = Instant::now() + h.wait;
        loop {
            if h.audio
                .as_mut()
                .unwrap()
                .control
                .try_resident_publication_response()
                .is_some()
            {
                break;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        h.render(); // actual old ordinary CURRENT now retains the graph/PCM
        let old_client = h.audio.take().unwrap();
        let old_backend = h.backend.lock().unwrap().take().unwrap();
        h.run(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        });
        let (track, asset) = authored_current(&mut h, &files, false, false, false);
        h.run(Request::AssignSampler { track, asset });
        loop {
            let state = h.observation.lock().unwrap();
            if admitted && state.native_hold.frames == 12 {
                break;
            }
            if !admitted && state.blocked == h.context {
                assert_eq!(
                    state.native_hold.frames, 8,
                    "old native PCM must survive Session replacement"
                );
                break;
            }
            drop(state);
            assert!(
                Instant::now() < end,
                "old PCM overlap admission did not settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // The original4 + newcopy4 + actual oldCurrent8 require16, not8.
        assert!(
            h.backend.lock().unwrap().is_none(),
            "old lease must still bar new output"
        );
        drop(old_client);
        drop(old_backend);
    }
}

#[gpui::test]
fn rendered_browser_first_load_reaches_real_service_and_native_current_without_retargeting(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Modifiers, MouseButton, VisualTestContext};
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let samples = files.0.join("samples");
    fs::create_dir(&samples).unwrap();
    let source = wav(&files, true);
    let path = samples.join("original.wav");
    fs::rename(source, &path).unwrap();
    let mut h = Harness::resident_sampler(8);
    h.next();
    let Some(Created::Track(track)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "target".into(),
        }))
        .created
    else {
        panic!("track")
    };
    let Some(Created::Clip(clip)) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        }))
        .created
    else {
        panic!("clip")
    };
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    let Some(Created::Track(other)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "other".into(),
        }))
        .created
    else {
        panic!("track")
    };
    // Give the actual view a current worker event after fixture authoring. No
    // browser action/request or source assignment is synthesized by the fixture.
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::Edit(Command::Gain { track, gain: 1.0 }))
        .unwrap();
    let backend = EditorBackend {
        service: h.service.take().unwrap(),
        audio_events: std::mem::replace(&mut h.events, mpsc::channel().1),
        source_reservation: AudioPolicy::source_reservation(),
    };
    let window =
        hex_ui::open_sampler_load_test_backend(cx, backend, files.0.clone(), samples).unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let other_selector = Box::leak(format!("track-name-{}", other.value()).into_boxed_str());
    let other_selected = Box::leak(format!("selected-track-{}", other.value()).into_boxed_str());
    let end = Instant::now() + h.wait;
    let pulse = |visual: &mut VisualTestContext| {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if backend.status().is_some_and(|s| {
                s.shutdown_complete
                    .load(std::sync::atomic::Ordering::Acquire)
            }) {
                // Emulate only the headless backend's actual stop/drop after
                // callback shutdown. This releases the real exclusive lease;
                // no UI acknowledgement is treated as native retirement.
                slot.take();
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(1));
    };
    loop {
        pulse(&mut visual);
        if visual.debug_bounds(target_selector).is_some()
            && visual.debug_bounds("browser-entry-0").is_some()
        {
            break;
        }
        assert!(Instant::now() < end, "actual browser/target did not render");
    }
    let target_bounds = visual.debug_bounds(target_selector).unwrap();
    visual.simulate_click(target_bounds.center(), Modifiers::default());
    let row = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let menu = visual
        .debug_bounds("context-menu-item")
        .expect("rendered sample context action");
    visual.simulate_click(menu.center(), Modifiers::default());
    // Navigate after dispatch, before the next worker-event UI tick. Completion
    // must preserve this newer selection/focus while loading the captured target.
    let other_bounds = visual.debug_bounds(other_selector).unwrap();
    visual.simulate_click(other_bounds.center(), Modifiers::default());
    let focus = visual.update(|w, cx| w.focused(cx));
    loop {
        pulse(&mut visual);
        if h.backend.lock().unwrap().as_ref().is_some_and(|b| {
            b.status()
                .and_then(|s| s.applied_audio_source())
                .is_some_and(|applied| {
                    applied.context.project_generation == 0 && applied.context.editor_revision == 5
                })
        }) {
            break;
        }
        if Instant::now() >= end {
            let ready = h.observation.lock().unwrap().command_ready;
            let slot = h.backend.lock().unwrap();
            let applied = slot
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.applied_audio_source());
            let shutdown = slot.as_ref().and_then(|b| b.status()).map(|s| {
                s.shutdown_complete
                    .load(std::sync::atomic::Ordering::Acquire)
            });
            drop(slot);
            panic!(
                "real first-load native owner not installed: ready={ready:?}, applied={applied:?}, shutdown={shutdown:?}"
            );
        }
    }
    for _ in 0..12 {
        pulse(&mut visual);
    }
    assert!(
        visual.debug_bounds(other_selected).is_some(),
        "completion retargeted the current selection"
    );
    assert_eq!(
        visual.update(|w, cx| w.focused(cx)),
        focus,
        "completion stole actual focus"
    );
    fs::remove_file(path).unwrap();
    visual.simulate_keystrokes("space");
    let output = loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        if let Some(backend) = h.backend.lock().unwrap().as_mut() {
            let mut block = [[0.0; 2]; 64];
            let measured = allocation_counter::measure(|| backend.render(&mut block).unwrap());
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            if block.iter().any(|f| *f != [0.0; 2]) {
                break block;
            }
        }
        assert!(
            Instant::now() < end,
            "actual UI Play did not reach sampler CURRENT"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let left = [0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0];
    let right = [-0.25, -0.125, 0.0, 0.125, 0.25, 0.125, 0.0];
    for i in 0..64 {
        let expected = if i < left.len() {
            [left[i], right[i]]
        } else {
            [0.0; 2]
        };
        for channel in 0..2 {
            assert!(
                (output[i][channel] - expected[channel]).abs() < 1e-6,
                "{i}: {:?} != {expected:?}",
                output[i]
            );
        }
    }
    visual.simulate_keystrokes("escape");
    loop {
        pulse(&mut visual);
        let stopped = h.backend.lock().unwrap().as_ref().is_some_and(|b| {
            b.status().is_some_and(|s| {
                !s.playing.load(std::sync::atomic::Ordering::Acquire)
                    && s.position.load(std::sync::atomic::Ordering::Acquire) == 0
            })
        });
        if stopped {
            break;
        }
        assert!(
            Instant::now() < end,
            "actual Stop did not reset native sampler"
        );
    }
    visual.simulate_keystrokes("space");
    loop {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            let mut replay = [[0.0; 2]; 64];
            let measured = allocation_counter::measure(|| backend.render(&mut replay).unwrap());
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            if replay.iter().any(|f| *f != [0.0; 2]) {
                assert_eq!(
                    replay, output,
                    "actual Stop/replay must reset phase and voices"
                );
                break;
            }
        }
        drop(slot);
        assert!(
            Instant::now() < end,
            "actual replay did not reach native sampler"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    visual.update(|w, _| w.remove_window());
    visual.run_until_parked();
}
