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
