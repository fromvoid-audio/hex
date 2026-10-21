use super::*;
fn authored(s: &Service) -> (hex_project::TrackId, Event) {
    event(s);
    let Some(Created::Track(track)) = edit(
        s,
        Command::AddMidiTrack {
            name: "target".into(),
        },
    )
    .created
    else {
        panic!("track")
    };
    let Some(Created::Clip(clip)) = edit(
        s,
        Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        },
    )
    .created
    else {
        panic!("clip")
    };
    let before = edit(
        s,
        Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        },
    );
    (track, before)
}
fn request(before: &Event, track: hex_project::TrackId, path: PathBuf) -> Request {
    Request::LoadFirstSampler {
        request_id: 9,
        project_generation: before
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: before.state.revision,
        track,
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
#[test]
fn first_load_worker_commits_one_source_and_binding_and_exports_original_pcm() {
    for stereo in [false, true] {
        let files = Files::new(stereo);
        let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
        let (track, before) = authored(&service);
        service
            .submit(request(&before, track, files.source.clone()))
            .unwrap();
        let loaded = event(&service);
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        assert_eq!(loaded.state.revision, before.state.revision + 1);
        let completion = loaded.sampler_load_complete.unwrap();
        assert_eq!((completion.request_id, completion.track), (9, track));
        let Some(Created::Asset(asset)) = loaded.created else {
            panic!("asset")
        };
        assert_eq!(loaded.state.project.assets().count(), 1);
        assert_eq!(
            loaded
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state()
                .asset_references()[0]
                .asset
                .value(),
            asset.value()
        );
        assert!(loaded.playback.is_none());
        assert!(
            loaded
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .is_empty()
        );
        // A rapid duplicate still carries the old revision. It cannot leave a
        // second source, change the captured target, or create another undo step.
        service
            .submit(request(&before, track, files.source.clone()))
            .unwrap();
        let duplicate = event(&service);
        assert!(duplicate.error.is_some());
        assert_eq!(duplicate.state.revision, loaded.state.revision);
        assert_eq!(
            duplicate.state.project.snapshot(),
            loaded.state.project.snapshot()
        );
        fs::remove_file(&files.source).unwrap();
        service
            .submit(Request::ExportRange {
                path: files.output.clone(),
                start: 0,
                frames: Some(12),
                signal: hex_session_export_signal(),
                control: Default::default(),
                overwrite: false,
            })
            .unwrap();
        assert!(event(&service).error.is_none());
        assert!(export_event(&service).result.is_ok());
        let bytes = fs::read(&files.output).unwrap();
        let pcm = bytes[44..]
            .chunks_exact(4)
            .map(|f| {
                [
                    i16::from_le_bytes([f[0], f[1]]),
                    i16::from_le_bytes([f[2], f[3]]),
                ]
            })
            .collect::<Vec<_>>();
        let left = [4096, 10240, 16384, 6144, -4096, -2048, 0, 0, 0, 0, 0, 0];
        let right = if stereo {
            [-8192, -4096, 0, 4096, 8192, 4096, 0, 0, 0, 0, 0, 0]
        } else {
            left
        };
        assert_eq!(
            pcm,
            left.into_iter()
                .zip(right)
                .map(|(l, r)| [l, r])
                .collect::<Vec<_>>()
        );
    }
}
fn hex_session_export_signal() -> ExportSignal {
    ExportSignal::Main
}
#[test]
fn first_load_refusals_preserve_canonical_history_sources_and_original_target() {
    for case in [
        "cancel",
        "generation",
        "revision",
        "occupied",
        "no-notes",
        "missing",
        "corrupt",
        "oversize",
        "capacity",
    ] {
        let files = Files::new(false);
        let service = Service::with_native_reservation(
            16000,
            16,
            if case == "capacity" { 3 } else { 1024 },
            reservation(),
        )
        .unwrap();
        let (track, mut before) = authored(&service);
        if case == "occupied" {
            before = edit(
                &service,
                Command::InsertInstrument {
                    track,
                    state: hex_project::default_instrument_state().unwrap(),
                },
            );
        }
        if case == "no-notes" {
            let clip = before.state.project.track(track).unwrap().midi_clips()[0].id();
            before = edit(&service, Command::RemoveMidiClip(clip));
        }
        let mut path = files.source.clone();
        if case == "missing" {
            path = path.with_file_name("missing.wav");
        }
        if case == "corrupt" {
            fs::write(&path, b"invalid wav").unwrap();
        }
        if case == "oversize" {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(u64::try_from(IMPORT_FILE_LIMIT + 1).unwrap())
                .unwrap();
        }
        let mut load = request(&before, track, path);
        if let Request::LoadFirstSampler {
            project_generation,
            revision,
            cancel,
            ..
        } = &mut load
        {
            if case == "cancel" {
                cancel.store(true, Ordering::Release);
            }
            if case == "generation" {
                *project_generation += 1;
            }
            if case == "revision" {
                *revision += 1;
            }
        }
        service.submit(load).unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some(), "{case}");
        assert_eq!(refused.sampler_load_complete.unwrap().request_id, 9);
        assert_eq!(
            refused.state.project.snapshot(),
            before.state.project.snapshot(),
            "{case}"
        );
        assert_eq!(refused.state.revision, before.state.revision, "{case}");
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (before.state.can_undo, before.state.can_redo),
            "{case}"
        );
    }
}
