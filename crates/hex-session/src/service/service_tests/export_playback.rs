use super::*;

#[test]
fn service_seek_worker_returns_exact_prepared_revision_and_destination() {
    let service = Service::new(48_000, 4, 0).expect("service");
    let initial = next_event(&service);
    let prepared = Arc::clone(
        initial
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("prepared startup playback"),
    );
    let request_id = service
        .request_arrangement_seek(ArrangementSeekSpec {
            note_chase: hex_engine::NoteChase::On,
            mode: ArrangementSeekMode::LinearArrangement,
            generation: 9,
            revision: 4,
            prepared_revision: Arc::clone(&prepared),
            destination_frame: 0,
            work_budget_frames: 0,
            start_playing: true,
        })
        .expect("admit seek");
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "seek result did not arrive");
        crate::test_wait::pause();
    };
    assert_eq!(result.request_id, request_id);
    assert_eq!(result.mode, ArrangementSeekMode::LinearArrangement);
    assert_eq!(result.generation, 9);
    assert_eq!(result.revision, 4);
    assert_eq!(result.destination_frame, 0);
    assert_eq!(result.work_budget_frames, 0);
    assert!(result.start_playing);
    assert!(Arc::ptr_eq(&result.prepared_revision, &prepared));
    let playback = result.result.expect("candidate");
    assert!(Arc::ptr_eq(
        playback.prepared_revision().expect("playback revision"),
        &prepared
    ));
    assert_eq!(playback.position(), 0);
    assert_eq!(playback.performance_frame(), 0);
    assert!(playback.playing());
}

#[test]
fn service_seek_worker_can_prepare_an_interactive_session_runtime() {
    let service = Service::new(48_000, 4, 0).expect("service");
    let initial = next_event(&service);
    let prepared = Arc::clone(
        initial
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("prepared startup playback"),
    );
    let request_id = service
        .request_arrangement_seek(ArrangementSeekSpec {
            note_chase: hex_engine::NoteChase::On,
            mode: ArrangementSeekMode::Interactive,
            generation: 2,
            revision: 1,
            prepared_revision: Arc::clone(&prepared),
            destination_frame: 0,
            work_budget_frames: 0,
            start_playing: false,
        })
        .expect("admit interactive seek");
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "interactive seek did not complete"
        );
        crate::test_wait::pause();
    };
    assert_eq!(result.request_id, request_id);
    assert_eq!(result.mode, ArrangementSeekMode::Interactive);
    let mut playback = result.result.expect("interactive candidate");
    assert!(Arc::ptr_eq(
        playback.prepared_revision().expect("playback revision"),
        &prepared
    ));
    playback
        .try_apply_action(hex_engine::playback::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Play,
        ))
        .expect("interactive candidate accepts Session transport");
    assert!(playback.playing());
}

#[test]
fn interactive_seek_worker_handles_stateful_arrangement_candidate() {
    let service = Service::new(48_000, 4, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "stateful seek".into(),
        }))
        .expect("add track");
    let track_event = next_event(&service);
    assert!(track_event.error.is_none());
    let track = track_event.state.project.tracks()[0].id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(
                hex_lattice_contract::DeviceTypeId::new("hex.utility").expect("utility id"),
            ),
            before: None,
        })
        .expect("add stateful effect");
    let added = next_event(&service);
    assert!(added.error.is_none());
    let utility = added.state.project.track(track).unwrap().devices()[0].id();
    service
        .submit(Request::SetDeviceParameter {
            device: utility,
            parameter: hex_lattice_contract::ParameterId::from_static(11),
            value: hex_lattice_contract::ParameterValue::Bool(true),
        })
        .expect("enable stateful utility filter");
    let installed = next_event(&service);
    assert!(installed.error.is_none());
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "stateful return".into(),
        }))
        .expect("add return");
    let return_added = next_event(&service);
    let return_track = return_added.state.project.return_tracks()[0].id();
    let utility_id =
        hex_lattice_contract::DeviceTypeId::new("hex.utility").expect("utility type id");
    let state = hex_engine::default_native_device_state(utility_id).expect("default utility state");
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state,
        }))
        .expect("insert return effect");
    let return_device_added = next_event(&service);
    assert!(return_device_added.error.is_none());
    let return_device = return_device_added.state.project.return_tracks()[0].devices()[0].id();
    service
        .submit(Request::SetDeviceParameter {
            device: return_device,
            parameter: hex_lattice_contract::ParameterId::from_static(11),
            value: hex_lattice_contract::ParameterValue::Bool(true),
        })
        .expect("enable stateful return filter");
    let return_filter_enabled = next_event(&service);
    assert!(return_filter_enabled.error.is_none());
    service
        .submit(Request::Edit(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        }))
        .expect("route track to return");
    let installed = next_event(&service);
    assert!(installed.error.is_none());
    let prepared = Arc::clone(
        installed
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("compiled stateful playback"),
    );
    assert!(prepared.arrangement_seek_requires_preroll());

    let destination = 8_192;
    let request_id = service
        .request_arrangement_seek(ArrangementSeekSpec {
            note_chase: hex_engine::NoteChase::On,
            mode: ArrangementSeekMode::Interactive,
            generation: 3,
            revision: installed.state.revision,
            prepared_revision: Arc::clone(&prepared),
            destination_frame: destination,
            work_budget_frames: destination,
            start_playing: false,
        })
        .expect("admit stateful interactive seek");
    let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "stateful seek did not complete");
        crate::test_wait::pause();
    };
    assert_eq!(result.request_id, request_id);
    assert_eq!(result.mode, ArrangementSeekMode::Interactive);
    assert_eq!(result.destination_frame, destination);
    let mut playback = result.result.expect("stateful interactive candidate");
    assert_eq!(playback.position(), destination);
    assert!(Arc::ptr_eq(
        playback.prepared_revision().expect("candidate revision"),
        &prepared
    ));
    playback
        .try_apply_action(hex_engine::playback::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Play,
        ))
        .expect("candidate remains Session-capable");
    assert!(playback.playing());
}

#[test]
fn keyboard_multitrack_copy_pastes_to_original_tracks_after_topology_change() {
    let service = Service::new(48_000, 32, 0).expect("service");
    next_event(&service);
    let mut tracks = Vec::new();
    for name in ["source a", "source b", "current target"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .expect("add track");
        let event = next_event(&service);
        tracks.push(event.state.project.tracks().last().expect("track").id());
    }
    let [source_a, source_b, target] = tracks.as_slice() else {
        panic!("expected three tracks")
    };
    for track in [*source_a, *source_b] {
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            }))
            .expect("place source clip");
        next_event(&service);
    }
    service
        .submit(Request::CopyTimeRange(
            hex_project::TimeRange::new(vec![*source_a, *source_b], 0, 48_000).expect("range"),
        ))
        .expect("copy rows");
    assert!(next_event(&service).error.is_none());
    service
        .submit(Request::Edit(Command::InsertTrack {
            name: "intervening".into(),
            before_track: Some(*source_b),
        }))
        .expect("insert between clipboard rows");
    let inserted = match next_event(&service).created {
        Some(Created::Track(track)) => track,
        other => panic!("unexpected result {other:?}"),
    };

    service
        .submit(Request::PasteTimeRange {
            track: inserted,
            frame: 120_000,
        })
        .expect("paste at current vertical target");
    let pasted = next_event(&service);
    assert!(pasted.error.is_none(), "{:?}", pasted.error);
    for track in [*source_a, *source_b] {
        let row = pasted.state.project.track(track).expect("source row");
        assert_eq!(row.midi_clips().len(), 2);
        assert!(row.midi_clips().iter().any(|clip| {
            pasted
                .state
                .project
                .tempo_map()
                .beat_to_frame(clip.start())
                .is_ok_and(|frame| frame == 120_000)
        }));
    }
    assert!(
        pasted
            .state
            .project
            .track(inserted)
            .expect("intervening")
            .midi_clips()
            .is_empty()
    );
    assert!(
        pasted
            .state
            .project
            .track(*target)
            .expect("current target")
            .midi_clips()
            .is_empty()
    );

    service.submit(Request::Undo).expect("undo paste");
    let undone = next_event(&service);
    assert_eq!(
        undone
            .state
            .project
            .track(*source_a)
            .unwrap()
            .midi_clips()
            .len(),
        1
    );
    service.submit(Request::Redo).expect("redo paste");
    let redone = next_event(&service);
    assert_eq!(
        redone
            .state
            .project
            .track(*source_b)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
}

#[test]
fn keyboard_single_row_paste_uses_current_target_track() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    let mut tracks = Vec::new();
    for name in ["source", "target"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .expect("add track");
        let event = next_event(&service);
        tracks.push(event.state.project.tracks().last().expect("track").id());
    }
    let [source, target] = tracks.as_slice() else {
        panic!("expected two tracks")
    };
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: *source,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).expect("length"),
        }))
        .expect("source clip");
    next_event(&service);
    service
        .submit(Request::CopyTimeRange(
            hex_project::TimeRange::new(vec![*source], 0, 48_000).expect("range"),
        ))
        .expect("copy one row");
    assert!(next_event(&service).error.is_none());
    service
        .submit(Request::PasteTimeRange {
            track: *target,
            frame: 96_000,
        })
        .expect("paste on current target");
    let pasted = next_event(&service);
    assert!(pasted.error.is_none(), "{:?}", pasted.error);
    assert_eq!(
        pasted
            .state
            .project
            .track(*source)
            .unwrap()
            .midi_clips()
            .len(),
        1
    );
    let target_clip = &pasted
        .state
        .project
        .track(*target)
        .expect("target")
        .midi_clips()[0];
    assert_eq!(
        pasted
            .state
            .project
            .tempo_map()
            .beat_to_frame(target_clip.start())
            .expect("frame"),
        96_000
    );
}

#[test]
fn keyboard_multitrack_cut_ripple_inserts_on_original_rows_only() {
    let service = Service::new(48_000, 32, 0).expect("service");
    next_event(&service);
    let mut tracks = Vec::new();
    for name in ["source a", "source b", "current target"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .expect("add track");
        let event = next_event(&service);
        tracks.push(event.state.project.tracks().last().expect("track").id());
    }
    let [source_a, source_b, target] = tracks.as_slice() else {
        panic!("expected three tracks")
    };
    for track in [*source_a, *source_b] {
        for (start, length) in [(0, 4), (8, 1)] {
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::from_quarters(start).expect("start"),
                    length: hex_project::BeatTime::from_quarters(length).expect("length"),
                }))
                .expect("place source clip");
            next_event(&service);
        }
    }
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: *target,
            start: hex_project::BeatTime::from_quarters(8).expect("target start"),
            length: hex_project::BeatTime::from_quarters(1).expect("target length"),
        }))
        .expect("place target clip");
    next_event(&service);
    service
        .submit(Request::CutTimeRange(
            hex_project::TimeRange::new(vec![*source_a, *source_b], 0, 24_000).expect("range"),
        ))
        .expect("cut rows");
    let cut = next_event(&service);
    assert!(cut.error.is_none());
    service
        .submit(Request::Edit(Command::InsertTrack {
            name: "intervening".into(),
            before_track: Some(*source_b),
        }))
        .expect("insert between clipboard rows");
    let inserted = match next_event(&service).created {
        Some(Created::Track(track)) => track,
        other => panic!("unexpected result {other:?}"),
    };

    service
        .submit(Request::InsertTimeRange {
            track: inserted,
            frame: 120_000,
        })
        .expect("ripple insert at current vertical target");
    let inserted_result = next_event(&service);
    assert!(
        inserted_result.error.is_none(),
        "{:?}",
        inserted_result.error
    );
    for track in [*source_a, *source_b] {
        let row = inserted_result
            .state
            .project
            .track(track)
            .expect("source row");
        assert!(row.midi_clips().iter().any(|clip| {
            inserted_result
                .state
                .project
                .tempo_map()
                .beat_to_frame(clip.start())
                .is_ok_and(|frame| frame == 120_000)
        }));
        assert!(row.midi_clips().iter().any(|clip| {
            inserted_result
                .state
                .project
                .tempo_map()
                .beat_to_frame(clip.start())
                .is_ok_and(|frame| frame == 216_000)
        }));
    }
    assert!(
        inserted_result
            .state
            .project
            .track(inserted)
            .expect("intervening")
            .midi_clips()
            .is_empty()
    );
    assert_eq!(
        inserted_result
            .state
            .project
            .track(*target)
            .expect("current target")
            .midi_clips()[0]
            .start(),
        hex_project::BeatTime::from_quarters(8).expect("target beat")
    );

    service.submit(Request::Undo).expect("undo ripple insert");
    let undone = next_event(&service);
    for track in [*source_a, *source_b] {
        assert!(
            undone
                .state
                .project
                .track(track)
                .expect("source row")
                .midi_clips()
                .iter()
                .all(|clip| undone
                    .state
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .is_ok_and(|frame| frame != 216_000))
        );
    }
    service.submit(Request::Redo).expect("redo ripple insert");
    let redone = next_event(&service);
    assert!(
        redone
            .state
            .project
            .track(*source_a)
            .expect("source row")
            .midi_clips()
            .iter()
            .any(|clip| redone
                .state
                .project
                .tempo_map()
                .beat_to_frame(clip.start())
                .is_ok_and(|frame| frame == 216_000))
    );
}

#[test]
fn export_keeps_edit_worker_responsive_and_returns_captured_revision_separately() {
    let directory = TestDir::new();
    let source = directory.0.join("source.wav");
    let output = directory.0.join("render.wav");
    fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let exporter: Exporter = {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Arc::new(
            move |path, project, start, frames, signal, format, control, publication| {
                entered.wait();
                release.wait();
                hex_export::render_prepared_with_policy(hex_export::ExportJobSpec {
                    path,
                    revision: Arc::clone(project),
                    resources: None,
                    start,
                    frames,
                    signal,
                    format,
                    control,
                    publication,
                })
                .map_err(ExportFailure::from)
            },
        )
    };
    let service = Service::with_exporter(48_000, 16, 8, exporter).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Import(source))
        .expect("submit import");
    let imported = next_event(&service);
    let revision = imported.state.revision;
    let track = imported.state.project.tracks()[0].id();

    service
        .submit(Request::Export(output.clone()))
        .expect("submit export");
    let accepted = next_event(&service);
    assert!(accepted.request_complete);
    assert_eq!(accepted.state.revision, revision);
    entered.wait();

    assert!(matches!(
        service.submit(Request::Export(directory.0.join("busy.wav"))),
        Err(SubmitError::Busy(
            request,
            BusyReason::ExportPending
        )) if matches!(*request, Request::Export(_))
    ));
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .expect("submit edit while rendering");
    let edited = next_event(&service);
    assert!(edited.request_complete);
    assert_eq!(edited.state.revision, revision + 1);
    assert!(edited.state.unsaved);
    assert!(edited.playback.is_some());

    release.wait();
    let completed = next_export_event(&service);
    assert_eq!(completed.revision, revision);
    assert_eq!(completed.path, output);
    assert!(completed.result.is_ok());
    let bytes = fs::read(completed.path).expect("read exported snapshot");
    assert_pcm16_near(&bytes, 44, 0x4000);
    assert_pcm16_near(&bytes, 46, 0x4000);
    assert_pcm16_near(&bytes, 48, 0x2000);
    assert_pcm16_near(&bytes, 50, 0x2000);
}

#[test]
fn explicit_export_range_uses_canonical_track_capture_and_releases_cancelled_reservation() {
    let directory = TestDir::new();
    let source = directory.0.join("selected.wav");
    fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
    let service = Service::new(48_000, 16, 8).unwrap();
    next_event(&service);
    service.submit(Request::Import(source)).unwrap();
    let imported = next_event(&service);
    let track = imported.state.project.tracks()[0].id();
    let output = directory.0.join("selected-output.wav");
    let cancelled = ExportControl::default();
    cancelled.cancel();
    service
        .submit(Request::ExportRange {
            path: output.clone(),
            start: 1,
            frames: Some(1),
            signal: ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]),
            control: cancelled,
            format: hex_export::ExportFormat::Pcm16,
            publication: ExportPublication::CreateNew,
        })
        .unwrap();
    next_event(&service);
    let refused = next_export_event(&service);
    assert_eq!(refused.result.unwrap_err().message(), "export cancelled");
    assert!(!output.exists());
    let control = ExportControl::default();
    service
        .submit(Request::ExportRange {
            path: output.clone(),
            start: 1,
            frames: Some(1),
            signal: ExportSignal::CapturedMix(vec![hex_engine::OutputCaptureId::Track(track)]),
            control: control.clone(),
            format: hex_export::ExportFormat::Pcm16,
            publication: ExportPublication::confirmed_save(&output).unwrap(),
        })
        .unwrap();
    next_event(&service);
    let completed = next_export_event(&service);
    assert_eq!(completed.revision, imported.state.revision);
    assert_eq!(completed.result.unwrap().frames, 1);
    let bytes = fs::read(output).unwrap();
    assert_eq!(bytes.len(), 48);
    assert_pcm16_near(&bytes, 44, 0x2000);
    assert_pcm16_near(&bytes, 46, 0x2000);
    assert_eq!(
        control.progress(),
        ExportProgress {
            processed_frames: 1,
            total_frames: 1
        }
    );
}

#[test]
fn queued_export_keeps_the_confirmed_destination_policy() {
    let directory = TestDir::new();
    let source = directory.0.join("source.wav");
    fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
    let service = Service::new(48_000, 16, 8).unwrap();
    next_event(&service);
    service.submit(Request::Import(source)).unwrap();
    let imported = next_event(&service);
    let change = directory.0.join("other.wav");

    for initially_exists in [false, true] {
        let path = directory.0.join(if initially_exists {
            "replace.wav"
        } else {
            "new.wav"
        });
        if initially_exists {
            fs::write(&path, b"original file").unwrap();
        }
        let publication = ExportPublication::confirmed_save(&path).unwrap();
        let (entered, wait_for_worker) = mpsc::channel();
        let (release, wait_for_release) = mpsc::channel();
        *service.export_test_hook.lock().unwrap() = Some(Box::new(move || {
            entered.send(()).unwrap();
            let _ = wait_for_release.recv_timeout(crate::test_wait::HANG_BOUND);
        }));
        service
            .submit(Request::ExportRange {
                path: path.clone(),
                start: 0,
                frames: Some(2),
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: ExportFormat::Pcm16,
                publication,
            })
            .unwrap();
        assert_eq!(next_event(&service).state.revision, imported.state.revision);
        wait_for_worker
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        let changed = b"another file!";
        fs::write(&change, changed).unwrap();
        fs::rename(&change, &path).unwrap();
        release.send(()).unwrap();

        let completed = next_export_event(&service);
        assert_eq!(completed.path, path);
        let error = completed
            .result
            .expect_err("destination changed while queued");
        let cause = error
            .source_error()
            .map(hex_export::ExportError::original_failure);
        if initially_exists {
            assert!(
                matches!(
                    cause,
                    Some(hex_export::ExportError::Invalid(
                        "confirmed overwrite destination changed"
                    ))
                ),
                "{error}"
            );
        } else {
            assert!(
                matches!(cause, Some(hex_export::ExportError::Io { source, .. }) if source.kind() == std::io::ErrorKind::AlreadyExists),
                "{error}"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), changed);
    }
}

#[test]
fn queued_export_publishes_a_confirmed_unchanged_replacement() {
    let directory = TestDir::new();
    let source = directory.0.join("source.wav");
    fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
    let path = directory.0.join("replace.wav");
    let previous = b"previous export";
    fs::write(&path, previous).unwrap();
    let service = Service::new(48_000, 16, 8).unwrap();
    next_event(&service);
    service.submit(Request::Import(source)).unwrap();
    next_event(&service);

    service
        .submit(Request::ExportRange {
            path: path.clone(),
            start: 0,
            frames: Some(2),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: ExportFormat::Pcm16,
            publication: ExportPublication::confirmed_save(&path).unwrap(),
        })
        .unwrap();
    next_event(&service);
    let report = next_export_event(&service).result.unwrap();
    assert_eq!(report.frames, 2);
    assert_eq!(fs::read(report.recovery_path.unwrap()).unwrap(), previous);
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_pcm16_near(&bytes, 44, 0x4000);
}

#[test]
fn export_range_writes_the_requested_format_from_one_render() {
    let directory = TestDir::new();
    let source = directory.0.join("formats.wav");
    fs::write(&source, wav_mono(&[16_384, -8_192])).unwrap();
    let service = Service::new(48_000, 16, 8).unwrap();
    next_event(&service);
    service.submit(Request::Import(source)).unwrap();
    next_event(&service);
    let export = |format: hex_export::ExportFormat| {
        let path = directory.0.join(format!("{format:?}.wav"));
        service
            .submit(Request::ExportRange {
                path: path.clone(),
                start: 0,
                frames: Some(2),
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format,
                publication: ExportPublication::CreateNew,
            })
            .unwrap();
        next_event(&service);
        let report = next_export_event(&service).result.unwrap();
        assert_eq!(report.clipped_samples, 0);
        fs::read(path).unwrap()
    };
    let float = export(hex_export::ExportFormat::Float32);
    let pcm24 = export(hex_export::ExportFormat::Pcm24);
    let pcm16 = export(hex_export::ExportFormat::Pcm16);
    // Header format tag and bits per sample follow the request.
    let word = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    assert_eq!((word(&float, 20), word(&float, 34)), (3, 32));
    assert_eq!((word(&pcm24, 20), word(&pcm24, 34)), (1, 24));
    assert_eq!((word(&pcm16, 20), word(&pcm16, 34)), (1, 16));
    let samples: Vec<f32> = float[58..]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    assert_eq!(samples.len(), 4);
    assert!((samples[0] - 0.5).abs() < 1e-4 && (samples[2] + 0.25).abs() < 1e-4);
    for (index, sample) in samples.iter().enumerate() {
        let at = 44 + index * 3;
        let packed = i32::from_le_bytes([0, pcm24[at], pcm24[at + 1], pcm24[at + 2]]) >> 8;
        let expected = (f64::from(*sample) * 8_388_607.0).round() as i32;
        assert_eq!(packed, expected, "pcm24 sample {index}");
        assert_pcm16_near(&pcm16, 44 + index * 2, (sample * 32_767.0).round() as i16);
    }
}

/// PCM16 export applies ±1 LSB TPDF dither: within one LSB of `expected`.

#[test]
fn empty_export_returns_a_separate_refusal_completion() {
    let directory = TestDir::new();
    let service = Service::new(48_000, 8, 8).expect("service");
    let _ = next_event(&service);
    let path = directory.0.join("empty.wav");
    service
        .submit(Request::Export(path.clone()))
        .expect("submit export");
    let accepted = next_event(&service);
    assert!(accepted.request_complete);
    assert_eq!(accepted.state.revision, 0);
    let result = next_export_event(&service);
    assert_eq!(result.revision, 0);
    assert_eq!(result.path, path);
    assert_eq!(
        result.result.expect_err("empty project refusal").message(),
        "invalid wav export: invalid export range"
    );
    assert!(!result.path.exists());
}

#[test]
fn export_reservation_rolls_back_when_request_admission_fails() {
    let (requests, _request_receiver) = mpsc::sync_channel(0);
    let (_event_sender, events) = mpsc::sync_channel(1);
    let (_export_sender, export_events) = mpsc::sync_channel(1);
    let (_track_switch_sender, track_switch_views) = track_switch_views::channel();
    let (_arrangement_edit_sender, arrangement_edit_views) = arrangement_edit_views::channel();
    let service = Service {
        diagnostics: Arc::new(diagnostics::Hooks::default()),
        export_test_hook: Arc::new(std::sync::Mutex::new(None)),
        clip_audition: Arc::new(clip_audition::Mailbox::default()),
        requests: Some(request_cancellation::Sender::new(
            requests,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        )),
        events: diagnostics::EventReceiver::new(events, Arc::new(diagnostics::Hooks::default())),
        wake_hook: client::WakeHook::default(),
        track_switch_views,
        arrangement_edit_views,
        export_events,
        export_busy: Arc::new(AtomicBool::new(false)),
        shutdown: Arc::new(AtomicBool::new(false)),
        arrangement_seek: None,
        resident_seek_results: None,
        resident_seek_count: AtomicUsize::new(0),
        recovery_slot: None,
        _recovery_claim: None,
        startup_failure: Default::default(),
        worker: Some(thread::spawn(|| {})),
    };
    assert!(matches!(
        service.submit(Request::Export(PathBuf::from("busy.wav"))),
        Err(SubmitError::Full(request)) if matches!(*request, Request::Export(_))
    ));
    assert!(!service.export_busy.load(Ordering::Acquire));
}

#[test]
fn midi_edge_resize_compiles_before_commit_and_undo_restores_source_window() {
    let service = Service::new(48_000, 8, 0).expect("service");
    let _initial = next_event(&service);
    let beat = |n| hex_project::BeatTime::from_quarters(n).expect("small beat");
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "keys".into(),
        }))
        .expect("track");
    let added = next_event(&service);
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(8),
        }))
        .expect("clip");
    let placed = next_event(&service);
    let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: beat(4),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("note");
    let before = next_event(&service);
    let original = before.state.project.midi_clip(clip).expect("clip").clone();
    service
        .submit(Request::Edit(Command::TrimMidiClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            beat: beat(7),
        }))
        .expect("resize");
    let resized = next_event(&service);
    assert!(resized.error.is_none());
    assert!(resized.state.revision > before.state.revision);
    assert!(
        resized
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .is_some()
    );
    let result = resized.state.project.midi_clip(clip).expect("clip");
    assert_eq!(result.start(), beat(7));
    assert_eq!(result.source_start(), original.source_start());
    assert_eq!(result.playback_offset(), beat(3));
    assert_eq!(
        result
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(hex_project::BeatTime::ZERO),
        Some(beat(3))
    );
    assert_eq!(result.notes(), original.notes());
    service.submit(Request::Undo).expect("undo");
    let undone = next_event(&service);
    assert!(undone.error.is_none());
    assert_eq!(undone.state.project.midi_clip(clip), Some(&original));
}

#[test]
fn rejected_loop_expansion_preserves_committed_graph_revision_dirty_state_and_redo() {
    let service = Service::new(48_000, 8, 0).expect("service");
    let initial = next_event(&service);
    assert_eq!(initial.state.revision, 0);
    assert!(!initial.state.unsaved);
    assert!(initial.playback.is_some());

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keys".into(),
        }))
        .expect("add instrument track");
    let track_state = next_event(&service);
    let track = track_state.state.project.tracks()[0].id();

    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).expect("arrangement length"),
        }))
        .expect("place midi clip");
    let clip_state = next_event(&service);
    let clip = clip_state.state.project.tracks()[0].midi_clips()[0].id();

    service
        .submit(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_raw(2),
            source_start: hex_project::BeatTime::from_raw(1),
            loop_start: hex_project::BeatTime::ZERO,
            loop_end: hex_project::BeatTime::from_raw(1),
            looping: false,
            length: hex_project::BeatTime::from_raw(65_538),
        }))
        .expect("set hidden one-shot source");
    let source_state = next_event(&service);
    assert!(source_state.error.is_none());

    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_raw(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add source-local note hidden by source offset");
    let note_state = next_event(&service);
    assert!(note_state.error.is_none());

    service
        .submit(Request::Edit(Command::AddTrack {
            name: "redo branch".into(),
        }))
        .expect("add redo branch");
    let branch = next_event(&service);
    service.submit(Request::Undo).expect("undo branch edit");
    let before = next_event(&service);
    assert!(before.state.unsaved);
    assert!(before.state.can_redo);
    assert!(before.state.revision > branch.state.revision);
    let committed_artifact = Arc::clone(
        before
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("edit playback retains exact prepared revision"),
    );
    let saved_revision = before.state.revision;
    let saved_project = before.state.project.as_ref().clone();

    service
        .submit(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_raw(2),
            source_start: hex_project::BeatTime::ZERO,
            loop_start: hex_project::BeatTime::ZERO,
            loop_end: hex_project::BeatTime::from_raw(1),
            looping: true,
            length: hex_project::BeatTime::from_raw(65_538),
        }))
        .expect("submit over-budget local loop");
    let refused = next_event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.ends_with("compiled midi occurrence limit reached"))
    );
    assert_eq!(refused.state.revision, saved_revision);
    assert_eq!(refused.state.project.as_ref(), &saved_project);
    assert!(refused.state.unsaved);
    assert!(refused.state.can_redo);
    assert!(refused.playback.is_none());

    service
        .submit(Request::RestorePlayback { generation: 7 })
        .expect("restore prior committed playback");
    let restored = next_event(&service);
    assert_eq!(restored.restore_complete, Some(7));
    assert_eq!(restored.state.revision, saved_revision);
    let restored_artifact = restored
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("restore returns committed artifact");
    assert!(Arc::ptr_eq(&committed_artifact, restored_artifact));

    service
        .submit(Request::Redo)
        .expect("redo survives refusal");
    let redone = next_event(&service);
    assert_eq!(redone.state.revision, saved_revision + 1);
    assert!(!redone.state.can_redo);
    assert_eq!(
        redone.state.project.tracks().len(),
        saved_project.tracks().len() + 1
    );
}

#[test]
fn dropping_service_detaches_an_active_export_worker() {
    let directory = TestDir::new();
    let source = directory.0.join("source.wav");
    fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (finished_sender, finished_receiver) = mpsc::channel();
    let exporter: Exporter = {
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Arc::new(move |path, project, _, frames, _, _, _, _| {
            entered.wait();
            release.wait();
            let _ = finished_sender.send(());
            Ok(ExportReport {
                path: path.to_path_buf(),
                sample_rate: project.arrangement().sample_rate(),
                frames,
                data_bytes: u32::try_from(frames * 4).unwrap_or(u32::MAX),
                clipped_samples: 0,
                recovery_path: None,
                applied_gain_linear: 1.0,
                applied_gain_db: 0.0,
            })
        })
    };
    let service = Service::with_exporter(48_000, 8, 8, exporter).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Import(source))
        .expect("submit import");
    let imported = next_event(&service);
    assert!(imported.error.is_none());
    service
        .submit(Request::Export(directory.0.join("detached.wav")))
        .expect("submit export");
    let _ = next_event(&service);
    entered.wait();

    let (dropped_sender, dropped_receiver) = mpsc::channel();
    let drop_thread = thread::spawn(move || {
        drop(service);
        let _ = dropped_sender.send(());
    });
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("dropping endpoints must not join the active exporter");
    release.wait();
    finished_receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("detached export worker should finish after release");
    drop_thread.join().expect("drop thread");
}

#[test]
fn computer_keyboard_worker_preserves_revision_history_and_rejects_stale_activation() {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let edited = next_event(&service);
    let track = edited.state.project.tracks()[0].id();
    let state = Arc::clone(&edited.state);
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: state.revision,
            request_id: 7,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let event = next_event(&service);
    assert_eq!(event.computer_keyboard_complete, Some(7));
    assert!(event.error.is_none());
    assert!(event.playback.is_some());
    assert!(Arc::ptr_eq(&event.state, &state));
    assert_eq!(event.state.revision, state.revision);
    assert_eq!(event.state.can_undo, state.can_undo);
    assert_eq!(event.state.unsaved, state.unsaved);
    input.cancel();
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: state.revision,
            request_id: 8,
            track,
            input,
        })
        .unwrap();
    let event = next_event(&service);
    assert_eq!(event.computer_keyboard_complete, Some(8));
    assert!(event.error.is_some());
    assert!(event.playback.is_none());
    assert!(Arc::ptr_eq(&event.state, &state));
}

#[test]
fn desktop_export_refuses_open_project_and_source_media_including_aliases_without_plugins() {
    let directory = TestDir::new();
    let media_dir = directory.0.join("media");
    fs::create_dir_all(&media_dir).unwrap();
    let source = media_dir.join("source.wav");
    let source_bytes = wav_mono(&[16_384; 16]);
    fs::write(&source, &source_bytes).unwrap();

    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: std::path::PathBuf::from("media/source.wav"),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap();
    let project_path = directory.0.join("open.hex");
    hex_storage::save_new(&project_path, editor.project()).unwrap();
    let project_bytes = fs::read(&project_path).unwrap();

    let source_hardlink = directory.0.join("source-hardlink.wav");
    fs::hard_link(&source, &source_hardlink).unwrap();
    let project_hardlink = directory.0.join("open-hardlink.hex");
    fs::hard_link(&project_path, &project_hardlink).unwrap();
    #[cfg(unix)]
    let (alias_dir, source_via_dir_alias, project_via_dir_alias) = {
        let alias_dir = TestDir::new();
        let linked = alias_dir.0.join("linked");
        std::os::unix::fs::symlink(&directory.0, &linked).unwrap();
        let source_alias = linked.join("media").join("source.wav");
        let project_alias = linked.join("open.hex");
        (alias_dir, source_alias, project_alias)
    };
    #[cfg(unix)]
    let _keep_alias_dir = alias_dir;

    let service = Service::new(48_000, 16, 1024).unwrap();
    let _ = next_event(&service);
    service.submit(Request::Open(project_path.clone())).unwrap();
    let opened = next_event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut protected = vec![
        (source.clone(), "original media"),
        (source_hardlink, "original media"),
        (project_path.clone(), "open project"),
        (project_hardlink, "open project"),
    ];
    #[cfg(unix)]
    {
        protected.push((source_via_dir_alias, "original media"));
        protected.push((project_via_dir_alias, "open project"));
    }

    for (destination, expected) in protected {
        service
            .submit(Request::ExportRange {
                path: destination.clone(),
                start: 0,
                frames: Some(16),
                signal: ExportSignal::Main,
                control: ExportControl::default(),
                format: ExportFormat::Pcm16,
                publication: ExportPublication::confirmed_save(&destination).unwrap(),
            })
            .unwrap();
        let _ = next_event(&service);
        let completion = next_export_event(&service);
        let error = completion
            .result
            .expect_err("protected export destination must be refused");
        assert!(
            error.message().contains(expected),
            "expected {expected:?} in {:?} for {}",
            error.message(),
            destination.display()
        );
    }

    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(fs::read(&project_path).unwrap(), project_bytes);

    // An unrelated existing WAV remains replaceable with its recovery receipt.
    let unrelated = directory.0.join("unrelated.wav");
    let unrelated_before = b"previous unrelated output";
    fs::write(&unrelated, unrelated_before).unwrap();
    service
        .submit(Request::ExportRange {
            path: unrelated.clone(),
            start: 0,
            frames: Some(16),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: ExportFormat::Pcm16,
            publication: ExportPublication::confirmed_save(&unrelated).unwrap(),
        })
        .unwrap();
    let _ = next_event(&service);
    let completion = next_export_event(&service);
    let report = completion
        .result
        .expect("unrelated confirmed replacement must succeed");
    assert_eq!(report.frames, 16);
    let recovery = report
        .recovery_path
        .expect("unrelated confirmed replacement must preserve recovery copy");
    assert_eq!(fs::read(recovery).unwrap(), unrelated_before);
}
