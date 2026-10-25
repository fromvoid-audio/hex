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
        assert!(!allow_playing);
        if self.playing.load(Ordering::Acquire) {
            Err("stop playback before bouncing in place".into())
        } else {
            Ok(())
        }
    }
    fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
        Some(ArrangementEditPosition::from_frames(1, false, None))
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
            assert_eq!(playback.position(), 1, "keep the exact stopped cursor");
            assert!(!playback.playing());
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
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    };
    let _ = receive();
    service
        .submit(Request::Import(files.source().to_owned()))
        .unwrap();
    let original = receive();
    assert!(original.error.is_none(), "{:?}", original.error);
    let clip = original.state.project.tracks()[0].clips()[0].id();
    let revision = original.state.revision;
    for (stale, refuse, cancel, active) in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
        (false, false, false, false),
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
                target: if whole_track {
                    hex_project::BounceTarget::Track(original.state.project.tracks()[0].id())
                } else {
                    hex_project::BounceTarget::Clips(vec![clip])
                },
                in_place: true,
                control: control.clone(),
            })
            .unwrap();
        let event = receive();
        assert!(event.bounce_complete.as_ref().unwrap().same_job(&control));
        assert!(event.playback.is_none(), "native custody stays out of UI");
        if stale || refuse || cancel || active {
            assert!(event.error.is_some());
            assert_eq!(event.state.project, original.state.project);
            assert_eq!(event.state.revision, revision);
        } else {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.state.project.clip(clip).is_none());
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
    playing.store(true, Ordering::Release);
    service.submit(Request::Undo).unwrap();
    let refused = receive();
    assert!(refused.error.is_some());
    assert!(refused.state.project.clip(clip).is_none());
    playing.store(false, Ordering::Release);
    service.submit(Request::Undo).unwrap();
    let undo = receive();
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert_eq!(
        undo.state.project.snapshot().tracks,
        original.state.project.snapshot().tracks
    );
    assert!(matches!(
        observed.publications.lock().unwrap().last().unwrap().1,
        ResidentPublicationPolicy::ArrangementEdit
    ));
    playing.store(true, Ordering::Release);
    service.submit(Request::Redo).unwrap();
    let refused = receive();
    assert!(refused.error.is_some());
    assert!(refused.state.project.clip(clip).is_some());
    playing.store(false, Ordering::Release);
    service.submit(Request::Redo).unwrap();
    let redo = receive();
    assert!(redo.error.is_none(), "{:?}", redo.error);
    assert!(redo.state.project.clip(clip).is_none());
    assert!(matches!(
        observed.publications.lock().unwrap().last().unwrap().1,
        ResidentPublicationPolicy::ArrangementEdit
    ));
}

#[test]
fn resident_sampler_clip_conversion_keeps_main_live_and_refusal_is_atomic() {
    fn context(revision: u64) -> hex_engine::AudioSourceContext {
        hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: revision,
        }
    }
    fn edit(
        session: &mut Session,
        sink: &mut OfflineSink,
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
    let (mut session, snapshot) = sampler_export::tests::direct_sampler(&files);
    drop(snapshot);
    let track = session.project().tracks()[0].id();
    let observed = Arc::new(Observed::default());
    let mut sink = OfflineSink {
        observed: observed.clone(),
        pending: None,
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
    let original = session.project().clone();
    let original_bytes = std::fs::read(files.source()).unwrap();
    let frames = session.project().midi_bounce_range(clip).unwrap().1 as usize;
    let render = |snapshot: &Snapshot| {
        let mut runtime = super::super::playback_with_loop(snapshot, None).unwrap();
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
    observed.refuse.store(true, Ordering::Release);
    assert!(
        super::super::bounce::bounce_clip(
            &mut session,
            clip,
            files.source().parent().unwrap(),
            context(4),
            &mut sink,
            sampler_export::Admission::default(),
            &mut current
        )
        .is_err()
    );
    assert_eq!(session.project(), &original);
    assert_eq!(std::fs::read(files.source()).unwrap(), original_bytes);
    observed.refuse.store(false, Ordering::Release);
    super::super::bounce::bounce_clip(
        &mut session,
        clip,
        files.source().parent().unwrap(),
        context(4),
        &mut sink,
        sampler_export::Admission::default(),
        &mut current,
    )
    .unwrap();
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
    assert_eq!(session.project().main_devices(), original.main_devices());
    assert_eq!(
        session.project().native_sources(),
        original.native_sources()
    );
}
