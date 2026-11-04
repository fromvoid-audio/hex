mod composition;
mod composition_ui;
mod console;
mod controls;
mod envelope;
mod gain;
mod history;
mod history_ui;
mod overview;
mod replacement;
mod replacement_ui;
use super::*;
mod keyboard;
use hex_project::{BeatTime, Command, Created};

/// The sampler's prepared ~2 ms declick release floor at this fixture's
/// 16 kHz output (#4087): no note-off releases in fewer frames.
const RELEASE_FLOOR_FRAMES: usize = 32;

/// Release level `steps` frames after a note-off whose prepared release is
/// `max(authored, RELEASE_FLOOR_FRAMES)` frames.
fn floored_release_level(steps: usize, authored: usize) -> f32 {
    let release = authored.max(RELEASE_FLOOR_FRAMES);
    if steps >= release {
        0.0
    } else {
        1.0 - steps as f32 / release as f32
    }
}

impl Harness {
    fn resident_sampler(frame_limit: u64) -> Self {
        Self::sampler_fixture(frame_limit, false)
    }
    fn rendered_resident_sampler(frame_limit: u64) -> Self {
        Self::sampler_fixture(frame_limit, true)
    }
    fn sampler_fixture(frame_limit: u64, rendered: bool) -> Self {
        let backend = Arc::new(Mutex::new(None));
        let factory_backend = Arc::clone(&backend);
        let (events, receiver) = mpsc::sync_channel(1);
        let setup_sender = rendered.then(|| events.clone());
        let mut sink = NativeAudioSink::new(events);
        sink.output_factory = Some(Box::new(move |rate| {
            let mut slot = factory_backend.lock().unwrap();
            if slot.is_some() {
                return Ok(None);
            }
            match hex_audio::worker_fixture::open_with_reliable_action_outcomes(
                rate.unwrap_or(16000),
            ) {
                Ok((output, callback)) => {
                    *slot = Some(callback);
                    Ok(Some(output))
                }
                Err(e) if e == "audio output is open or still shutting down" => Ok(None),
                Err(e) => Err(e.into()),
            }
        }));
        let observation = Arc::new(Mutex::new(Observation::default()));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            frame_limit,
            None,
            Box::new(if rendered {
                ObservedSink::rendered(sink, Arc::clone(&observation))
            } else {
                ObservedSink::legacy(sink, Arc::clone(&observation))
            }),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            rendered_setup: setup_sender.map(|sender| RenderedSetup {
                sender,
                opened: None,
                observation: None,
            }),
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
        let left = if tuned {
            vec![0.125, 0.5, -0.125, 0.0]
        } else {
            vec![0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0]
        };
        let right = if stereo {
            if tuned {
                vec![-0.25, 0.0, 0.25, 0.0]
            } else {
                vec![-0.25, -0.125, 0.0, 0.125, 0.25, 0.125, 0.0]
            }
        } else {
            left.clone()
        };
        let mut expected = [[0.0; 2]; 64];
        for (index, (l, r)) in left.into_iter().zip(right).enumerate() {
            // The short gate ends after frame 0; the prepared declick floor
            // then ramps the rest of the one-shot down instead of cutting it.
            let level = if short_gate && index > 0 {
                floored_release_level(index - 1, 0)
            } else {
                1.0
            };
            expected[index] = [l * level, r * level];
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
            format: hex_export::ExportFormat::Pcm16,
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
        // PCM16 export applies ±1 LSB TPDF dither.
        assert_eq!(pcm.len(), literal.len());
        for (actual, expected) in pcm.iter().flatten().zip(literal.iter().flatten()) {
            assert!(
                (i32::from(*actual) - i32::from(*expected)).abs() <= 1,
                "{pcm:?} vs {literal:?}"
            );
        }
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

pub(super) fn tuning(reference_hz: f64) -> hex_tuning::ProjectTuning {
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
        match hex_audio::worker_fixture::open_with_reliable_action_outcomes(rate.unwrap_or(16000)) {
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
            Err(e) => Err(e.into()),
        }
    }));
    let observation = Arc::new(Mutex::new(Observation::default()));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8,
        None,
        Box::new(ObservedSink::legacy(sink, observation.clone())),
        AudioPolicy::new().unwrap().resident_native,
    )
    .unwrap();
    let mut h = Harness {
        rendered_setup: None,
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
                assert_eq!(error.to_string(), "injected fresh output failure");
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
    rendered_sampler_save_open(cx, true, false, false, None);
}
#[gpui::test]
fn rendered_browser_mono_sampler_save_open_preserves_absolute_tuning(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, false, false, false, None);
}
#[gpui::test]
fn rendered_empty_midi_track_load_and_reopen_is_playable_from_computer_keyboard(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, true, false, None);
}
#[gpui::test]
fn rendered_sampler_key_release_and_focus_cancel_do_not_wait_for_blocked_service(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, false, true, None);
}
#[gpui::test]
fn rendered_mono_a880_sampler_keyboard_playing_and_blocked_release_are_literal(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, false, false, true, None);
}
#[gpui::test]
fn rendered_sampler_stopped_controls_rebuild_current_and_persist(cx: &mut gpui::TestAppContext) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::Scalar));
}
#[gpui::test]
fn rendered_sampler_envelope_controls_shape_current_keyboard_arrangement_and_persist(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::Envelope));
}
#[gpui::test]
fn rendered_sampler_trigger_release_is_visible_but_inactive_without_mode_conversion(
    cx: &mut gpui::TestAppContext,
) {
    for fixture in [
        ControlsFixture::TriggerOneShot,
        ControlsFixture::TriggerSlice,
    ] {
        rendered_sampler_save_open(cx, true, false, false, Some(fixture));
    }
}
#[gpui::test]
fn rendered_sampler_fresh_source_overview_and_controls_work_before_save_and_reconstruct_on_open(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::Overview));
}
#[gpui::test]
fn rendered_sampler_history_uses_global_order_cancels_drafts_and_persists(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::History));
}
#[gpui::test]
fn rendered_sampler_replacement_preserves_captured_target_cancel_sound_and_saved_sources(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::Replacement));
}
#[gpui::test]
fn rendered_sampler_console_cell_scene_and_stop_use_actual_current(cx: &mut gpui::TestAppContext) {
    rendered_sampler_save_open(cx, true, false, false, Some(ControlsFixture::Console));
}
#[gpui::test]
fn rendered_loaded_sampler_creates_arrange_midi_clip_through_fresh_current(
    cx: &mut gpui::TestAppContext,
) {
    rendered_sampler_save_open(cx, true, true, false, Some(ControlsFixture::Composition));
}
#[derive(Clone, Copy)]
enum ControlsFixture {
    Scalar,
    Envelope,
    TriggerOneShot,
    TriggerSlice,
    Overview,
    History,
    Replacement,
    Console,
    Composition,
}
fn rendered_sampler_save_open(
    cx: &mut gpui::TestAppContext,
    stereo: bool,
    empty: bool,
    looped: bool,
    controls: Option<ControlsFixture>,
) {
    use gpui::{Modifiers, MouseButton, VisualTestContext};
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let gain = controls.is_some();
    let files = Files::new();
    let samples = files.0.join("samples");
    fs::create_dir(&samples).unwrap();
    let source = wav(&files, stereo);
    let path = samples.join("original.wav");
    fs::rename(source, &path).unwrap();
    if matches!(controls, Some(ControlsFixture::Replacement)) {
        let incoming = replacement::incoming(&files);
        fs::rename(incoming, samples.join("replacement.wav")).unwrap();
    }
    let mut h = Harness::rendered_resident_sampler(
        if matches!(controls, Some(ControlsFixture::Replacement)) {
            24
        } else if gain {
            12
        } else {
            8
        },
    );
    h.next();
    let Some(Created::Track(track)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "target".into(),
        }))
        .created
    else {
        panic!("track")
    };
    if !empty {
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
    }
    let Some(Created::Track(other)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "other".into(),
        }))
        .created
    else {
        panic!("track")
    };
    if matches!(
        controls,
        Some(ControlsFixture::History | ControlsFixture::Replacement)
    ) {
        h.run(Request::AddNativeDevice {
            track: other,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_session::device_gesture::DeviceTypeId::from_static(
                "hex.utility",
            )),
            before: None,
        });
    }
    if matches!(controls, Some(ControlsFixture::Console)) {
        for name in ["first", "second"] {
            let Some(Created::Scene(scene)) = h
                .run(Request::Edit(Command::AddScene { name: name.into() }))
                .created
            else {
                panic!("scene")
            };
            let Some(Created::SessionClip(clip)) = h
                .run(Request::Edit(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: "pattern".into(),
                    source_length: BeatTime::ONE,
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
        }
    }
    let mut loaded_revision = if stereo && !gain {
        if empty { 3 } else { 5 }
    } else {
        let event = h.run(Request::Edit(Command::SetTuning(tuning(880.0))));
        assert!(event.error.is_none());
        event.state.revision + 1
    };
    // Give the actual view a current worker event after fixture authoring. No
    // browser action/request or source assignment is synthesized by the fixture.
    // Hold an older real observation so the scalar-controls fixture deterministically
    // starts handoff under pressure. The refresh below supplies the actual latest Submitted.
    let handoff_pressure = matches!(controls, Some(ControlsFixture::Scalar)).then(|| {
        h.rendered_setup
            .as_mut()
            .unwrap()
            .observation
            .take()
            .unwrap()
    });
    let before_refresh = h
        .observation
        .lock()
        .unwrap()
        .resident_submitted
        .last()
        .unwrap()
        .audio_revision;
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::Edit(Command::Gain { track, gain: 1.0 }))
        .unwrap();
    h.settle_rendered_setup(Some(before_refresh));
    if let Some(pressure) = handoff_pressure {
        match h.rendered_setup.as_ref().unwrap().sender.try_send(pressure) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                panic!("actual audio consumer disconnected before handoff")
            }
        }
        // No receiver drains until the UI is created: attach must tolerate this full row.
    }
    let (setup_sender, setup_events) = h.attach_rendered_setup();
    let backend = EditorBackend {
        service: h.service.take().unwrap(),
        audio_events: std::mem::replace(&mut h.events, mpsc::channel().1),
        source_reservation: AudioPolicy::source_reservation(),
    };
    let window =
        hex_ui::open_sampler_load_test_backend(cx, backend, files.0.clone(), samples).unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|w, _| w.activate_window());
    visual.run_until_parked();
    let target_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let other_selector = Box::leak(format!("track-name-{}", other.value()).into_boxed_str());
    let other_selected = Box::leak(format!("selected-track-{}", other.value()).into_boxed_str());
    let started = Instant::now();
    let end = started
        + if gain {
            Duration::from_secs(60)
        } else {
            h.wait
        };
    let pulse = |visual: &mut VisualTestContext| {
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                // Substitute the test backend's actual stop/drop, then let
                // normal lifecycle control establish terminal quiescence and
                // release the actual output lease. This is not a UI ACK.
                slot.take();
            } else {
                backend.render(&mut [[0.0; 2]; 64]).unwrap();
            }
        }
        drop(slot);
        std::thread::sleep(Duration::from_millis(1));
    };
    for mut setup_event in setup_events {
        loop {
            match setup_sender.try_send(setup_event) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(event)) => setup_event = event,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    panic!("actual UI audio consumer disconnected")
                }
            }
            pulse(&mut visual);
            assert!(
                Instant::now() < end,
                "actual retained event delivery stalled"
            );
        }
    }
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
                .and_then(|s| s.rendered_current(&h))
                .is_some_and(|applied| {
                    applied.context.project_generation == 0
                        && applied.context.editor_revision == loaded_revision
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
                .and_then(|s| s.rendered_current(&h));
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
    let before_output = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(&h)
        .unwrap()
        .output_identity;
    // The real Open picker cancellation must leave the actual current output
    // and installed source intact before any Save/New/reopen sequence.
    visual.simulate_keystrokes("cmd-o");
    while !cx.did_prompt_for_paths() {
        pulse(&mut visual);
        assert!(
            Instant::now() < end,
            "actual cancelled Open picker did not open"
        );
    }
    cx.simulate_paths_selection(None);
    for _ in 0..12 {
        pulse(&mut visual);
    }
    let after_cancel = h
        .backend
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rendered_current(&h)
        .unwrap();
    assert_eq!(after_cancel.output_identity, before_output);
    assert_eq!(after_cancel.context.project_generation, 0);
    assert_eq!(after_cancel.context.editor_revision, loaded_revision);
    let fresh_overview = if matches!(controls, Some(ControlsFixture::Overview)) {
        let fresh = overview::fresh(cx, &mut visual, &h, window, end, track);
        loaded_revision += 8;
        Some(fresh)
    } else {
        None
    };
    let saved = files.0.join("saved.hex");
    visual.simulate_keystrokes("cmd-s");
    while !cx.did_prompt_for_new_path() {
        pulse(&mut visual);
        assert!(Instant::now() < end, "actual SaveNew picker did not open");
    }
    cx.simulate_new_path_selection(|_| Some(saved.clone()));
    // Publication on disk precedes the worker's completion event. Both are
    // required before another rendered action can safely use the saved project.
    while !saved.exists()
        || !cx
            .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
            .unwrap()
    {
        pulse(&mut visual);
        assert!(Instant::now() < end, "actual SaveNew did not publish");
    }
    for _ in 0..12 {
        pulse(&mut visual);
    }
    let stored = hex_storage::load(&saved).unwrap();
    assert!(stored.assets().all(|a| a.original_source().is_some()));
    if !stereo || gain {
        assert_eq!(stored.tuning(), tuning(880.0));
    }
    let previous_save = hex_storage::recovery_path(&saved).unwrap();
    assert!(
        !previous_save.exists(),
        "SaveNew creates no previous-save recovery"
    );
    visual.simulate_keystrokes("cmd-s");
    while !previous_save.exists()
        || !cx
            .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
            .unwrap()
    {
        pulse(&mut visual);
        assert!(
            Instant::now() < end,
            "actual SaveCurrent did not use the ordinary recoverable publication"
        );
    }
    assert_eq!(hex_storage::load_recovery(&saved).unwrap().project, stored);
    for _ in 0..12 {
        pulse(&mut visual);
    }
    let open_path = if matches!(
        controls,
        Some(ControlsFixture::TriggerOneShot | ControlsFixture::TriggerSlice)
    ) {
        let mut editor = hex_project::Editor::new(stored.clone(), 8).unwrap();
        let device = stored.track(track).unwrap().instrument().unwrap();
        let asset = stored.assets().next().unwrap().id();
        let state = if matches!(controls, Some(ControlsFixture::TriggerSlice)) {
            hex_engine::native_device_resources::current_sampler_fixture::slice_trigger_state(
                4, asset,
            )
            .unwrap()
        } else {
            hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(4, asset)
                .unwrap()
        }
        .with_host_metadata_from(device.state())
        .unwrap();
        let state = hex_engine::patch_sampler_parameter_on_control(
            hex_engine::SamplerParameter::ReleaseMs,
            &state,
            0.125,
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state,
            })
            .unwrap();
        let path = files.0.join("trigger.hex");
        hex_storage::save_new(&path, editor.project()).unwrap();
        path
    } else if looped {
        let mut editor = hex_project::Editor::new(stored.clone(), 8).unwrap();
        let device = stored.track(track).unwrap().instrument().unwrap();
        let asset = stored.assets().next().unwrap().id();
        let state =
            hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                4,
                asset,
                2.0 / 16000.0,
            )
            .unwrap()
            .with_host_metadata_from(device.state())
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state,
            })
            .unwrap();
        let path = files.0.join("looped.hex");
        hex_storage::save_new(&path, editor.project()).unwrap();
        path
    } else {
        saved.clone()
    };
    visual.simulate_keystrokes("cmd-n");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(target_selector).is_some(),
        "first New must retain the existing explicit confirmation law"
    );
    visual.simulate_keystrokes("cmd-n");
    while visual.debug_bounds(target_selector).is_some() {
        pulse(&mut visual);
        assert!(
            Instant::now() < end,
            "actual New did not replace the saved document"
        );
    }
    for _ in 0..12 {
        pulse(&mut visual);
    }
    visual.simulate_keystrokes("cmd-o");
    while !cx.did_prompt_for_paths() {
        pulse(&mut visual);
        assert!(Instant::now() < end, "actual Open picker did not open");
    }
    cx.simulate_paths_selection(Some(vec![open_path.clone()]));
    let reopen_started = Instant::now();
    loop {
        pulse(&mut visual);
        if h.backend
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.status())
            .and_then(|s| s.rendered_current(&h))
            .is_some_and(|applied| {
                applied.context.project_generation == 2
                    && applied.context.editor_revision == loaded_revision + 2
                    && applied.output_identity != before_output
            })
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "actual Open did not reach a fresh matching native CURRENT: total={:?}, reopen={:?}, expected_revision={}, previous_output={before_output}, current={:?}, submitted={:?}, ui={:?}, prompt={:?}",
            started.elapsed(),
            reopen_started.elapsed(),
            loaded_revision + 2,
            h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.rendered_current(&h)),
            h.observation.lock().unwrap().resident_submitted.last(),
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx)),
            cx.pending_prompt()
        );
    }
    for _ in 0..12 {
        pulse(&mut visual);
    }
    if gain {
        match controls.unwrap() {
            ControlsFixture::Composition => {
                composition_ui::exercise(cx, &mut visual, &mut h, window, track, &saved)
            }
            ControlsFixture::Console => {
                console::exercise(cx, &mut visual, &mut h, window, track, &saved)
            }
            ControlsFixture::Replacement => {
                replacement_ui::exercise(cx, &mut visual, &mut h, window, track, &saved)
            }
            ControlsFixture::History => {
                history_ui::exercise(cx, &mut visual, &mut h, window, track, &saved)
            }
            ControlsFixture::Overview => {
                overview::reopened(
                    cx,
                    &mut visual,
                    window,
                    track,
                    fresh_overview.as_ref().unwrap(),
                );
            }
            ControlsFixture::Scalar => {
                let step = h.wait;
                controls::exercise(cx, &mut visual, &mut h, window, step, track, &saved)
            }
            ControlsFixture::TriggerOneShot | ControlsFixture::TriggerSlice => {
                envelope::trigger_release(cx, &mut visual, &mut h, track, &open_path);
            }
            ControlsFixture::Envelope => {
                envelope::exercise(cx, &mut visual, &mut h, window, end, track, &saved)
            }
        }
        visual.update(|w, _| w.remove_window());
        visual.run_until_parked();
        return;
    }
    fs::remove_file(path).unwrap();
    let target_bounds = visual.debug_bounds(target_selector).unwrap();
    visual.simulate_click(target_bounds.center(), Modifiers::default());
    let keyboard_toggle = visual.debug_bounds("computer midi keyboard").unwrap();
    visual.simulate_click(keyboard_toggle.center(), Modifiers::default());
    visual.simulate_keystrokes("v v");
    for _ in 0..12 {
        pulse(&mut visual);
    }
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    visual.run_until_parked();
    if looped {
        // Setup ends at this handoff. Keyboard operations must own their wait
        // budgets rather than borrow the source/file workflow's deadline.
        let setup_finished = Instant::now();
        keyboard::exercise_blocked_sampler_keyboard(
            cx,
            &mut visual,
            &mut h,
            window,
            setup_finished,
            stereo,
        );
        visual.update(|w, _| w.remove_window());
        visual.run_until_parked();
        return;
    }
    let keyboard = loop {
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
                let status = backend.status().unwrap();
                assert!(!status.playing.load(std::sync::atomic::Ordering::Acquire));
                assert_eq!(
                    status.position.load(std::sync::atomic::Ordering::Acquire),
                    0
                );
                assert_eq!(
                    status
                        .performance_frame
                        .load(std::sync::atomic::Ordering::Acquire),
                    0
                );
                break block;
            }
        }
        assert!(
            Instant::now() < end,
            "real UI keyboard did not reach stopped CURRENT: {:?}",
            cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let literal: &[[f32; 2]] = if stereo {
        &[
            [0.125, -0.25],
            [0.3125, -0.125],
            [0.5, 0.0],
            [0.1875, 0.125],
            [-0.125, 0.25],
            [-0.0625, 0.125],
            [0.0, 0.0],
        ]
    } else {
        &[[0.125, 0.125], [0.5, 0.5], [-0.125, -0.125], [0.0, 0.0]]
    };
    for (i, actual) in keyboard.iter().enumerate() {
        let expected = literal.get(i).copied().unwrap_or([0.0; 2]);
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                (*a - e).abs() < 1e-6,
                "keyboard {i}: {actual:?} != {expected:?}"
            );
        }
    }
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("a").unwrap(),
    });
    visual.simulate_keystrokes("m");
    if empty {
        visual.update(|w, _| w.remove_window());
        visual.run_until_parked();
        return;
    }
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
    let left = if stereo {
        [0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0]
    } else {
        [0.125, 0.5, -0.125, 0.0, 0.0, 0.0, 0.0]
    };
    let right = if stereo {
        [-0.25, -0.125, 0.0, 0.125, 0.25, 0.125, 0.0]
    } else {
        left
    };
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
        if stopped
            && cx
                .update(|cx| hex_ui::sampler_midi_test_ready(window, cx))
                .unwrap()
        {
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

#[test]
fn rejected_sampler_open_keeps_actual_native_current_document_and_original_sound() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for case in ["changed", "longer", "capacity"] {
        let capacity_refusal = case == "capacity";
        let files = Files::new();
        let mut h = Harness::resident_sampler(if capacity_refusal { 8 } else { 12 });
        let (track, asset) = authored(&mut h, &files, true, false, false);
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.run(Request::AssignSampler { track, asset });
        h.open_sampler(old_output);
        let context = h.context.unwrap();
        let applied = h.applied(context);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let original = h.render();
        let left = [0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0];
        let right = [-0.25, -0.125, 0.0, 0.125, 0.25, 0.125, 0.0];
        for i in 0..64 {
            let expected = if i < left.len() {
                [left[i], right[i]]
            } else {
                [0.0; 2]
            };
            for c in 0..2 {
                assert!((original[i][c] - expected[c]).abs() < 1e-6);
            }
        }
        h.audio.as_ref().unwrap().control.stop();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        let path = wav(&files, true); // Restore the fixture file removed after initial ingress.
        let saved = files.0.join("native.hex");
        let before = h.run(Request::SaveNew(saved.clone()));
        let mut bytes = fs::read(&path).unwrap();
        if capacity_refusal {
            bytes = b"invalid wav".to_vec();
        } else if case == "longer" {
            bytes.extend_from_slice(&[0, 32, 0, 224]);
            let riff = u32::try_from(bytes.len() - 8).unwrap();
            let data = u32::try_from(bytes.len() - 44).unwrap();
            bytes[4..8].copy_from_slice(&riff.to_le_bytes());
            bytes[40..44].copy_from_slice(&data.to_le_bytes());
        } else {
            bytes[44..46].copy_from_slice(&8192_i16.to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::Open(saved))
            .unwrap();
        let refused = h.next();
        assert!(refused.open_complete && !refused.project_replaced && refused.error.is_some());
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains(if capacity_refusal {
                    "frame limit"
                } else if case == "longer" {
                    "media limit exceeded: decoded frames"
                } else {
                    "content differs"
                }),
            "{:?}",
            refused.error
        );
        assert_eq!(
            refused.state.project.snapshot(),
            before.state.project.snapshot()
        );
        assert_eq!(refused.state.revision, before.state.revision);
        assert_eq!(refused.state.document_path, before.state.document_path);
        assert_eq!(h.context, Some(context));
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            Some(applied.output_identity)
        );
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            Some(applied)
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        assert_eq!(
            h.render(),
            original,
            "refused Open must preserve actual native PCM and phase reset"
        );
    }
}

#[test]
fn catalog_empty_sampler_fills_same_device_and_reaches_native_current_from_ordinary_output() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let files = Files::new();
    let mut h = Harness::resident_sampler(8);
    assert!(h.next().error.is_none());
    h.open();
    let Some(Created::Track(track)) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "sampler".into(),
        }))
        .created
    else {
        panic!("track")
    };
    let inserted = h.run(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static(
            "hex.instrument.sampler",
        )),
        before: None,
    });
    let device = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    assert!(hex_engine::is_empty_sampler_state_on_control(
        device.state()
    ));
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
    let before = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    let old_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 9,
        project_generation: h.context.unwrap().project_generation,
        revision: before.state.revision,
        track,
        empty_sampler: Some((device.id(), device.state().clone())),
        path: wav(&files, false),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert_eq!(
        loaded
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device.id()
    );
    assert_eq!(loaded.state.project.assets().count(), 1);
    h.open_sampler(old_identity);
    h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let output = h.render();
    let mut expected = [[0.0; 2]; 64];
    for (frame, sample) in [0.125, 0.3125, 0.5, 0.1875, -0.125, -0.0625, 0.0]
        .into_iter()
        .enumerate()
    {
        expected[frame] = [sample; 2];
    }
    assert_eq!(output, expected);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

#[test]
fn sampler_bypass_keeps_held_note_continuous_in_the_actual_callback() {
    use super::resident_preview::ResidentHarness;
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit(131_072);
    h.next();
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 99,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 65_536),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    for _ in 0..16 {
        h.render();
    }
    let before = h.render();
    assert!(before[63][0].abs() > 0.01, "held note must be audible");
    let toggled = h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
    assert!(toggled.error.is_none());
    let after = h.render();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire)
            > applied,
        "bypass must reach the callback"
    );
    assert!(
        (after[0][0] - before[63][0]).abs() < 0.01,
        "sampler bypass: {:?} -> {:?}",
        before[63],
        after[0]
    );
    let bypass_revision = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(std::sync::atomic::Ordering::Acquire);
    let enabled = h.run(Request::Edit(Command::ToggleDeviceBypass { device }));
    assert!(enabled.error.is_none(), "{:?}", enabled.error);
    let resumed = h.render();
    assert!(
        (resumed[0][0] - after[63][0]).abs() < 0.01,
        "sampler reenable: {:?} -> {:?}",
        after[63],
        resumed[0]
    );
    h.applied(bypass_revision);
    let settled = h.render();
    assert!(
        settled.iter().flatten().all(|sample| sample.abs() < 1e-6),
        "reenable must not retrigger the held note: {settled:?}"
    );
}
