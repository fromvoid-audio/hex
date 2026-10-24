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
    fn begin_section_edit(&mut self, _: hex_engine::AudioSourceContext) -> Result<(), String> {
        if self.playing.load(Ordering::Acquire) {
            Err("stop playback before bouncing in place".into())
        } else {
            Ok(())
        }
    }
    fn section_edit_ready(&mut self) -> Option<Result<u64, String>> {
        Some(Ok(1))
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
        if matches!(policy, ResidentPublicationPolicy::StoppedArrangement) {
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
                target: hex_project::BounceTarget::Clips(vec![clip]),
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
        ResidentPublicationPolicy::StoppedArrangement
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
        ResidentPublicationPolicy::StoppedArrangement
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
        ResidentPublicationPolicy::StoppedArrangement
    ));
}
