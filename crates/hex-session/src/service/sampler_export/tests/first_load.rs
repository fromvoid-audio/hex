use super::*;
pub(super) fn authored(s: &Service) -> (hex_project::TrackId, Event) {
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
pub(super) fn request(before: &Event, track: hex_project::TrackId, path: PathBuf) -> Request {
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
        slot: SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::First),
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
fn insert_empty(s: &Service, track: hex_project::TrackId) -> Event {
    s.submit(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
            "hex.instrument.sampler",
        )),
        before: None,
    })
    .unwrap();
    let inserted = event(s);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let device = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    assert!(hex_engine::is_empty_sampler_state_on_control(
        device.state()
    ));
    assert_eq!(inserted.state.project.assets().count(), 0);
    inserted
}
fn empty_request(before: &Event, track: hex_project::TrackId, path: PathBuf) -> Request {
    let mut request = request(before, track, path);
    let device = before
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    if let Request::LoadFirstSampler { slot, .. } = &mut request {
        *slot = SamplerLoadSlot::Existing {
            device: device.id(),
            expected_state: device.state().clone(),
        };
    }
    request
}
#[test]
fn catalog_sampler_ordinary_insert_history_save_reopen_and_fill_preserve_device_identity() {
    let files = Files::new(true);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    let (track, before) = authored(&service);
    let inserted = insert_empty(&service, track);
    let device = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    assert!(inserted.playback.is_some());
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.tracks(), before.state.project.tracks());
    assert_eq!(undone.state.project.assets().count(), 0);
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, inserted.state.project);
    let saved = files.source.with_file_name("empty.hex");
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    let saved_event = event(&service);
    assert!(saved_event.save_complete && saved_event.error.is_none());
    assert_eq!(
        hex_storage::load(&saved)
            .unwrap()
            .track(track)
            .unwrap()
            .instrument(),
        Some(&device)
    );
    service.submit(Request::Open(saved)).unwrap();
    let reopened = event(&service);
    assert!(
        reopened.open_complete && reopened.error.is_none(),
        "{:?}",
        reopened.error
    );
    assert_eq!(
        reopened.state.project.track(track).unwrap().instrument(),
        Some(&device)
    );
    assert!(reopened.state.project.assets().next().is_none());
    service
        .submit(empty_request(&reopened, track, files.source.clone()))
        .unwrap();
    let loaded = event(&service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    let filled = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(filled.id(), device.id());
    assert_eq!(filled.state().type_id(), device.state().type_id());
    assert_eq!(filled.state().asset_references().len(), 1);
    assert_eq!(loaded.state.project.assets().count(), 1);
    assert!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::EndFrame,
            filled.state()
        )
        .is_ok()
    );
}
#[test]
fn empty_sampler_load_refuses_stale_cancelled_bad_source_and_wrong_device_without_mutation() {
    for case in [
        "cancel", "revision", "device", "state", "missing", "corrupt",
    ] {
        let files = Files::new(false);
        let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
        let (track, _) = authored(&service);
        let before = insert_empty(&service, track);
        let mut load = empty_request(&before, track, files.source.clone());
        if let Request::LoadFirstSampler {
            revision,
            slot,
            cancel,
            path,
            ..
        } = &mut load
        {
            match case {
                "cancel" => cancel.store(true, Ordering::Release),
                "revision" => *revision += 1,
                "device" => {
                    let mut model =
                        hex_project::Editor::new((*before.state.project).clone(), 1).unwrap();
                    let Created::Device(other) = model
                        .execute(Command::InsertDevice {
                            track,
                            before: None,
                            state: hex_engine::default_native_device_state(
                                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                            )
                            .unwrap(),
                        })
                        .unwrap()
                    else {
                        panic!("effect")
                    };
                    if let SamplerLoadSlot::Existing { device, .. } = slot {
                        *device = other
                    }
                }
                "state" => {
                    if let SamplerLoadSlot::Existing { expected_state, .. } = slot {
                        *expected_state = hex_project::default_instrument_state().unwrap();
                    }
                }
                "missing" => *path = path.with_file_name("missing.wav"),
                "corrupt" => fs::write(path, b"invalid wav").unwrap(),
                _ => unreachable!(),
            }
        }
        service.submit(load).unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some(), "{case}");
        assert_eq!(refused.state.project, before.state.project, "{case}");
        assert_eq!(refused.state.revision, before.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (before.state.can_undo, before.state.can_redo)
        );
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
                format: hex_export::ExportFormat::Pcm16,
                overwrite: false,
            })
            .unwrap();
        assert!(event(&service).error.is_none());
        let complete = export_event(&service);
        assert!(complete.result.is_ok(), "{:?}", complete.result);
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
        assert_pcm_near(
            &pcm,
            &left
                .into_iter()
                .zip(right)
                .map(|(l, r)| [l, r])
                .collect::<Vec<_>>(),
            "first load pcm",
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

#[test]
fn first_load_save_projects_actual_original_catalog_without_history_or_owner_escape() {
    let mono = [
        126, 31, 236, 78, 247, 231, 248, 213, 2, 55, 14, 76, 198, 151, 192, 55, 230, 94, 138, 21,
        200, 118, 132, 213, 113, 69, 163, 43, 23, 252, 230, 61,
    ];
    let stereo_digest = [
        37, 182, 86, 33, 141, 194, 149, 144, 249, 172, 95, 152, 63, 100, 83, 174, 58, 120, 112,
        231, 74, 206, 50, 3, 45, 58, 172, 97, 20, 192, 131, 8,
    ];
    for stereo in [false, true] {
        let files = Files::new(stereo);
        let unassigned = Files::new(false);
        let saved = files.source.with_file_name("sampler.hex");
        let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
        let (track, before) = authored(&service);
        service
            .submit(request(&before, track, files.source.clone()))
            .unwrap();
        let loaded = event(&service);
        assert!(loaded.error.is_none());
        let Some(Created::Asset(first)) = loaded.created else {
            panic!("first asset")
        };
        service
            .submit(Request::ImportDeviceAsset {
                path: unassigned.source.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let before = event(&service);
        assert!(before.error.is_none(), "{:?}", before.error);
        assert_eq!(before.state.project.assets().count(), 2);
        assert!(
            before
                .state
                .project
                .assets()
                .all(|a| a.original_source().is_none())
        );
        for save in [Request::SaveNew(saved.clone()), Request::SaveCurrent] {
            service.submit(save).unwrap();
            let result = event(&service);
            assert!(
                result.save_complete && result.error.is_none(),
                "{:?}",
                result.error
            );
            assert_eq!(
                result.state.project.snapshot(),
                before.state.project.snapshot()
            );
            assert_eq!(result.state.revision, before.state.revision);
            assert_eq!(
                (result.state.can_undo, result.state.can_redo),
                (before.state.can_undo, before.state.can_redo)
            );
            assert!(!result.state.unsaved);
            assert_eq!(result.state.document_path.as_ref(), Some(&saved));
            assert!(result.playback.is_none());
            assert!(
                result
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .sources()
                    .is_empty()
            );
            let project = hex_storage::load(&saved).unwrap();
            assert_eq!(project.tuning(), before.state.project.tuning());
            assert_eq!(
                project.track(track).unwrap().instrument().unwrap().state(),
                before
                    .state
                    .project
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .state()
            );
            for asset in project.assets() {
                let facts = asset
                    .original_source()
                    .expect("all actual native assets, including unassigned");
                assert_eq!(facts.law_version(), 1);
                assert_eq!(
                    facts.content_digest(),
                    if asset.id() == first && stereo {
                        stereo_digest
                    } else {
                        mono
                    }
                );
                assert_eq!(
                    (asset.source_sample_rate(), asset.source_frames()),
                    (8000, 4)
                );
            }
        }
        // Retain the ordinary conflict-checked save law with a private sampler.
        fs::write(&saved, b"external writer").unwrap();
        service.submit(Request::SaveCurrent).unwrap();
        let conflict = event(&service);
        assert!(conflict.save_complete && conflict.error.is_some());
        assert_eq!(
            conflict.state.project.snapshot(),
            before.state.project.snapshot()
        );
        assert_eq!(conflict.state.document_path.as_ref(), Some(&saved));
        assert_eq!(fs::read(&saved).unwrap(), b"external writer");
    }
}

#[test]
fn new_sampler_drop_replaces_the_captured_instrument_and_reopens() {
    let files = Files::new(false);
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    let (track, _) = authored(&service);
    edit(
        &service,
        Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        },
    );
    let before = edit(
        &service,
        Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        },
    );
    let instrument = before
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let mut load = request(&before, track, files.source.clone());
    if let Request::LoadFirstSampler { slot, .. } = &mut load {
        *slot = SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::Replace(instrument));
    }
    service.submit(load).unwrap();
    let loaded = event(&service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    assert_eq!(loaded.state.revision, before.state.revision + 1);
    let target = loaded.state.project.track(track).unwrap();
    assert_eq!(
        target.devices(),
        before.state.project.track(track).unwrap().devices()
    );
    assert_eq!(target.instrument_position(), Some(0));
    let sampler = target.instrument().unwrap();
    assert_eq!(sampler.id(), instrument);
    assert_eq!(
        sampler.state().type_id().as_str(),
        Ok("hex.instrument.sampler")
    );
    assert_eq!(sampler.state().asset_references().len(), 1);
    assert_eq!(loaded.state.project.assets().count(), 1);
    // A late duplicate still names the pre-import revision.
    let mut stale = request(&before, track, files.source.clone());
    if let Request::LoadFirstSampler { slot, .. } = &mut stale {
        *slot = SamplerLoadSlot::New(hex_project::InstrumentLoadPlacement::Replace(instrument));
    }
    service.submit(stale).unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, loaded.state.project);
    let saved = files.source.with_file_name("dropped-sampler.hex");
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    let saved_event = event(&service);
    assert!(
        saved_event.save_complete && saved_event.error.is_none(),
        "{:?}",
        saved_event.error
    );
    service.submit(Request::Open(saved)).unwrap();
    let reopened = event(&service);
    assert!(
        reopened.open_complete && reopened.error.is_none(),
        "{:?}",
        reopened.error
    );
    assert_eq!(
        reopened.state.project.track(track),
        loaded.state.project.track(track)
    );
    assert_eq!(reopened.state.project.assets().count(), 1);
}
