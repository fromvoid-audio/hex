//! Actual resident prepare/publication, including final sink refusal and stale intent.
use super::*;
#[test]
fn arrangement_bounce_admits_native_candidate_once_and_refusals_preserve_history() {
    let files = sampler_export::tests::Files::new(true);
    let root = files.source().parent().unwrap();
    let observed = Arc::new(Observed::default());
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        2_000_000,
        Some(root.join("recovery/untitled.hex-journal")),
        Box::new(OfflineSink {
            observed: observed.clone(),
            pending: None,
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
    for (stale, refuse, cancel) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (false, false, false),
    ] {
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
        if stale || refuse || cancel {
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
    service.submit(Request::Undo).unwrap();
    let undo = receive();
    assert!(undo.error.is_none(), "{:?}", undo.error);
    assert_eq!(
        undo.state.project.snapshot().tracks,
        original.state.project.snapshot().tracks
    );
    service.submit(Request::Redo).unwrap();
    let redo = receive();
    assert!(redo.error.is_none(), "{:?}", redo.error);
    assert!(redo.state.project.clip(clip).is_none());
}
