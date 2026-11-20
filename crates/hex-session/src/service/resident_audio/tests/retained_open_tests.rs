use super::*;

#[test]
fn resident_sink_opens_and_publishes_audible_retained_receiver() {
    let files = sampler_export::tests::Files::new(false);
    let path = files.source().with_file_name("retained.hex");
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    editor
        .execute(Command::InsertInstrument {
            track: receiver,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let hex_project::Created::Track(peer) = editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer")
    };
    editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let hex_project::Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: peer,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let original = hex_storage::encode(&project).unwrap();
    std::fs::write(&path, &original).unwrap();

    let observed = Arc::new(Observed::default());
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        Box::new(OfflineSink {
            observed: Arc::clone(&observed),
            pending: None,
        }),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 8_000_000,
        },
    )
    .unwrap();
    sampler_export::tests::event(&service);
    observed
        .render_for_switch_test
        .store(true, Ordering::Release);
    service.submit(Request::Open(path.clone())).unwrap();
    let opened = sampler_export::tests::event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert_eq!(opened.state.project.snapshot(), project.snapshot());
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(
        observed
            .switch_peaks
            .lock()
            .unwrap()
            .iter()
            .any(|(_, peak)| *peak > 0.0)
    );
}
