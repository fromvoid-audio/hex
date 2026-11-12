//! Actual resident prepare/publication, including final sink refusal and stale intent.
use super::*;
struct FenceSink {
    inner: OfflineSink,
    playing: Arc<AtomicBool>,
}
impl WorkerAudioSink for FenceSink {
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        self.inner.native_sampler_hold()
    }
    fn initialize(&mut self, s: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(s)
    }
    fn canonical_changed(&mut self, s: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(s);
    }
    fn poll(&mut self) {
        self.inner.poll();
    }
    fn close(&mut self) {
        self.inner.close();
    }
    fn begin_section_edit(
        &mut self,
        _: hex_engine::AudioSourceContext,
        allow_playing: bool,
    ) -> Result<(), String> {
        assert!(allow_playing, "in-place bounces never ask for Stop");
        Ok(())
    }
    fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
        Some(ArrangementEditPosition::from_frames(
            1,
            self.playing.load(Ordering::Acquire),
            None,
        ))
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        if matches!(policy, ResidentPublicationPolicy::ArrangementEdit) {
            if self.playing.load(Ordering::Acquire) {
                assert!(
                    playback.live_restart_requested(),
                    "a playing bake restarts at the cursor, never carries tails"
                );
            } else {
                assert_eq!(playback.position(), 1, "keep the exact stopped cursor");
                assert!(!playback.playing());
                assert!(!playback.live_restart_requested());
            }
        }
        self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            admit,
        )
    }
}
#[test]
fn arrangement_bounce_admits_native_candidate_once_and_refusals_preserve_history() {
    for whole_track in [false, true] {
        arrangement_bounce_publication_case(whole_track);
    }
}

fn arrangement_bounce_publication_case(whole_track: bool) {
    let files = sampler_export::tests::Files::new(true);
    let root = files.source().parent().unwrap();
    let observed = Arc::new(Observed::default());
    let playing = Arc::new(AtomicBool::new(false));
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        2_000_000,
        Some(root.join("recovery/untitled.hex-journal")),
        Box::new(FenceSink {
            inner: OfflineSink {
                observed: observed.clone(),
                pending: None,
            },
            playing: playing.clone(),
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 128,
                root_rows: 128,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 2_000_000,
        },
    )
    .unwrap();
    let receive = || {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    };
    let _ = receive();
    service
        .submit(Request::Import(files.source().to_owned()))
        .unwrap();
    let mut original = receive();
    assert!(original.error.is_none(), "{:?}", original.error);
    let clip = original.state.project.tracks()[0].clips()[0].id();
    let console = if whole_track {
        let track = original.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddScene {
                name: "console".into(),
            }))
            .unwrap();
        let added = receive();
        assert!(added.error.is_none(), "{:?}", added.error);
        let scene = added.state.project.scenes()[0].id();
        service
            .submit(Request::Edit(Command::CopyArrangementAudioClipToSession {
                source: clip,
                scene,
            }))
            .unwrap();
        let copied = receive();
        assert!(copied.error.is_none(), "{:?}", copied.error);
        let hex_project::SessionCellContent::Audio(cell) =
            copied.state.project.session_cell(track, scene).unwrap()
        else {
            panic!("audio cell")
        };
        let id = cell.id();
        service
            .submit(Request::Edit(Command::SetSessionAudioClipLoop {
                clip: id,
                loop_range: Some(hex_project::SessionAudioLoop::new(1, 3).unwrap()),
            }))
            .unwrap();
        let looped = receive();
        assert!(looped.error.is_none(), "{:?}", looped.error);
        service
            .submit(Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap()
                .with_output_gain_db(6.0)
                .unwrap(),
            }))
            .unwrap();
        original = receive();
        assert!(original.error.is_none(), "{:?}", original.error);
        Some(id)
    } else {
        None
    };
    let revision = original.state.revision;
    // The accepted bake runs while playing (Pulse #3784).
    for (stale, refuse, cancel, active) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        playing.store(active, Ordering::Release);
        observed.refuse.store(refuse, Ordering::Release);
        let control = hex_export::ExportControl::default();
        if cancel {
            assert!(control.cancel());
        }
        service
            .submit(Request::BounceArrangement {
                project_generation: 0,
                revision: revision + u64::from(stale),
                target: hex_project::BounceTarget::Track(original.state.project.tracks()[0].id()),
                in_place: true,
                adoption: hex_project::BounceAdoption::NewTracks,
                control: control.clone(),
            })
            .unwrap();
        let event = receive();
        assert!(event.bounce_complete.as_ref().unwrap().same_job(&control));
        assert!(event.playback.is_none(), "native custody stays out of UI");
        if stale || refuse || cancel {
            assert!(event.error.is_some());
            assert_eq!(event.state.project, original.state.project);
            assert_eq!(event.state.revision, revision);
        } else {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.state.project.clip(clip).is_none());
            if let Some(id) = console {
                let cell = event.state.project.session_audio_clip(id).unwrap();
                assert_eq!(
                    cell.loop_range(),
                    Some(hex_project::SessionAudioLoop::new(3, 5).unwrap())
                );
                assert!(event.state.project.tracks()[0].devices().is_empty());
            }
            let bounced = &event.state.project.tracks()[0].clips()[0];
            assert_eq!(
                bounced.duration(),
                original.state.project.tracks()[0].clips()[0].duration()
            );
            assert!(
                event
                    .state
                    .project
                    .asset(bounced.asset())
                    .unwrap()
                    .path()
                    .starts_with(std::fs::canonicalize(root).unwrap())
            );
        }
    }
    assert!(matches!(
        observed.publications.lock().unwrap().last().unwrap().1,
        ResidentPublicationPolicy::ArrangementEdit
    ));
    playing.store(false, Ordering::Release);
    service.submit(Request::Undo).unwrap();
    let undo = receive();
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert_eq!(
        undo.state.project.snapshot().tracks,
        original.state.project.snapshot().tracks
    );
    assert_eq!(
        undo.state.project.snapshot().session,
        original.state.project.snapshot().session
    );
    assert!(matches!(
        observed.publications.lock().unwrap().last().unwrap().1,
        ResidentPublicationPolicy::ArrangementEdit
    ));
    playing.store(true, Ordering::Release);
    service.submit(Request::Redo).unwrap();
    let redo = receive();
    assert!(redo.error.is_none(), "{:?}", redo.error);
    assert!(redo.state.project.clip(clip).is_none());
    if let Some(id) = console {
        assert_eq!(
            redo.state
                .project
                .session_audio_clip(id)
                .unwrap()
                .loop_range(),
            Some(hex_project::SessionAudioLoop::new(3, 5).unwrap())
        );
    }
    assert!(matches!(
        observed.publications.lock().unwrap().last().unwrap().1,
        ResidentPublicationPolicy::ArrangementEdit
    ));
}

#[test]
fn resident_sampler_clip_and_console_bounces_keep_main_live_and_refusal_atomic() {
    for whole_track in [false, true] {
        resident_sampler_console_bounce_case(whole_track);
    }
}
fn resident_sampler_console_bounce_case(whole_track: bool) {
    fn context(revision: u64) -> hex_engine::AudioSourceContext {
        hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: revision,
        }
    }
    fn edit(
        session: &mut Session,
        sink: &mut FenceSink,
        current: &mut Snapshot,
        command: Command,
        revision: u64,
    ) -> Created {
        let (created, next) = session
            .command_resident_snapshot(
                command,
                context(revision),
                sink,
                sampler_export::Admission::default(),
                Some(current),
            )
            .unwrap();
        *current = next;
        created
    }
    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    if whole_track {
        // Whole-device bakes reserve the bounded 30-second tail/decode horizon;
        // the tiny clip-conversion fixture otherwise admits only 1024 frames.
        session.max_resident_frames = 2_000_000;
        let reservation = session.native_reservation.as_mut().unwrap();
        reservation.storage.max_retained_bytes = 128 * 1024 * 1024;
        reservation.storage.max_construction_peak_bytes = 256 * 1024 * 1024;
        reservation.max_scan_frames = 2_000_000;
    }
    let track = session.project().tracks()[0].id();
    let observed = Arc::new(Observed::default());
    let mut sink = FenceSink {
        inner: OfflineSink {
            observed: observed.clone(),
            pending: None,
        },
        playing: Arc::new(AtomicBool::new(false)),
    };
    let mut snapshot = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            context(0),
        )
        .unwrap();
    let length = hex_project::BeatTime::from_ratio(1, 100).unwrap();
    let Created::Clip(clip) = edit(
        &mut session,
        &mut sink,
        &mut snapshot,
        Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length,
        },
        1,
    ) else {
        panic!("clip")
    };
    edit(
        &mut session,
        &mut sink,
        &mut snapshot,
        Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: length,
            pitch: 60,
            channel: 0,
            velocity: 127,
        },
        2,
    );
    let main = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap()
    .with_output_gain_db(-6.0)
    .unwrap();
    edit(
        &mut session,
        &mut sink,
        &mut snapshot,
        Command::InsertMainDevice {
            before: None,
            state: main,
        },
        3,
    );
    let (render_clip, bounce_revision) = if whole_track {
        let Created::Scene(scene) = edit(
            &mut session,
            &mut sink,
            &mut snapshot,
            Command::AddScene {
                name: "console sampler".into(),
            },
            4,
        ) else {
            panic!("scene")
        };
        let Created::SessionClip(cell) = edit(
            &mut session,
            &mut sink,
            &mut snapshot,
            Command::CopyArrangementMidiToSession {
                source: clip,
                scene,
            },
            5,
        ) else {
            panic!("cell")
        };
        edit(
            &mut session,
            &mut sink,
            &mut snapshot,
            Command::SetSessionMidiClipSource {
                clip: cell,
                source_length: length,
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: length,
                looping: true,
            },
            6,
        );
        (cell, 7)
    } else {
        (clip, 4)
    };
    let original = session.project().clone();
    let original_bytes = std::fs::read(files.source()).unwrap();
    let frames = if whole_track {
        session
            .project()
            .tempo_map()
            .beat_to_frame(
                session
                    .project()
                    .console_midi_bounce_length(render_clip)
                    .unwrap(),
            )
            .unwrap() as usize
    } else {
        session.project().midi_bounce_range(clip).unwrap().1 as usize
    };
    let render = |snapshot: &Snapshot| {
        let mut runtime = super::super::playback_with_loop(snapshot, None).unwrap();
        if whole_track {
            runtime
                .try_apply_action(hex_engine::PlaybackAction::Session(
                    hex_engine::SessionPlaybackCommand::LaunchClip {
                        clip: render_clip,
                        quantization: hex_engine::LaunchQuantization::Immediate,
                    },
                ))
                .unwrap();
        }
        runtime
            .try_apply_action(hex_engine::PlaybackAction::Transport(
                hex_engine::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut pcm = vec![[0.0; 2]; frames];
        runtime.render(&mut pcm).unwrap();
        pcm
    };
    let expected = render(&snapshot);
    assert!(expected.iter().flatten().any(|v| v.abs() > 0.001));
    let mut current = Some(snapshot);
    let bounce = |session: &mut Session, sink: &mut FenceSink, current: &mut Option<Snapshot>| {
        if whole_track {
            super::super::bounce::bounce_arrangement(
                session,
                hex_project::BounceTarget::Track(track),
                true,
                hex_project::BounceAdoption::NewTracks,
                files.source().parent().unwrap(),
                &hex_export::ExportControl::default(),
                &crate::service::RequestCancellation::new(
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    0,
                ),
                context(bounce_revision),
                sink,
                sampler_export::Admission::default(),
                current,
            )
        } else {
            super::super::bounce::bounce_clip(
                session,
                clip,
                files.source().parent().unwrap(),
                context(bounce_revision),
                sink,
                sampler_export::Admission::default(),
                current,
            )
        }
    };
    observed.refuse.store(true, Ordering::Release);
    assert!(bounce(&mut session, &mut sink, &mut current).is_err());
    assert_eq!(session.project(), &original);
    assert_eq!(std::fs::read(files.source()).unwrap(), original_bytes);
    observed.refuse.store(false, Ordering::Release);
    bounce(&mut session, &mut sink, &mut current).unwrap();
    let actual = render(current.as_ref().unwrap());
    let error = actual
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        error < 1e-6,
        "resource-backed clip conversion changed PCM: {error}"
    );
    if whole_track {
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert!(
            session
                .project()
                .session_audio_clip(render_clip)
                .unwrap()
                .loop_range()
                .is_some()
        );
    }
    assert_eq!(session.project().main_devices(), original.main_devices());
    let mut retained_sources = original.snapshot().native_sources;
    if whole_track {
        for source in &mut retained_sources {
            if source.receiver == Some(track.value()) {
                source.receiver = None;
            }
        }
    }
    assert_eq!(
        session.project().snapshot().native_sources,
        retained_sources
    );
}
