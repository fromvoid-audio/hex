use super::first_load::{authored, request};
use super::*;

fn export_pcm(service: &Service, path: PathBuf) -> Vec<[i16; 2]> {
    service
        .submit(Request::ExportRange {
            path: path.clone(),
            start: 0,
            frames: Some(8),
            signal: ExportSignal::Main,
            control: Default::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        })
        .unwrap();
    assert!(event(service).error.is_none());
    let result = export_event(service);
    assert!(
        result.result.is_ok(),
        "{}: {:?}",
        path.display(),
        result.result
    );
    fs::read(path).unwrap()[44..]
        .chunks_exact(4)
        .map(|f| {
            [
                i16::from_le_bytes([f[0], f[1]]),
                i16::from_le_bytes([f[2], f[3]]),
            ]
        })
        .collect()
}
fn loop_envelope_state(
    original: &hex_lattice_contract::NativeDeviceState,
) -> hex_lattice_contract::NativeDeviceState {
    // Version follows the actual loaded template. Payload/PCM remain independent
    // literals: Classic loop, region1..4, loop1..3, gain0.5 and two-frame
    // attack/release at16k. This is not a second codec or UI.
    let mut bytes = vec![1, 0, 0, 60, 16, 0, 0, 0];
    for n in [1_u32, 4, 1, 3] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    for n in [
        0.0_f64,
        0.0,
        0.5,
        0.0,
        1.0,
        2.0 / 16000.0,
        0.0,
        1.0,
        2.0 / 16000.0,
    ] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    assert_eq!(bytes.len(), 96);
    hex_lattice_contract::NativeDeviceState::new(
        original.type_id(),
        original.state_version(),
        bytes,
    )
    .unwrap()
    .with_host_metadata_from(original)
    .unwrap()
    .with_input_gain_millidb(6000)
    .unwrap()
    .with_output_gain_millidb(-6000)
    .unwrap()
}
#[test]
fn save_new_current_new_open_reconstructs_original_content_state_tuning_and_actual_file_pcm() {
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
        let device = loaded
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        let expected_state = loop_envelope_state(device.state());
        let changed = edit(
            &service,
            Command::ReplaceDeviceState {
                device: device.id(),
                state: expected_state.clone(),
            },
        );
        assert!(changed.error.is_none(), "{:?}", changed.error);
        let mut scale = hex_tuning::ProjectTuning::default().scale();
        scale
            .set_reference(scale.root_key(), 880.0, scale.root_degree())
            .unwrap();
        let tuning =
            hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default()).unwrap();
        assert!(edit(&service, Command::SetTuning(tuning)).error.is_none());
        service
            .submit(Request::ImportDeviceAsset {
                path: unassigned.source.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let baseline = event(&service);
        assert!(baseline.error.is_none(), "{:?}", baseline.error);
        // The instrument input trim sees no input signal; generated PCM is
        // attenuated by the persisted -6 dB output trim.
        let left: [i16; 8] = [0, -513, 4106, -1026, 4106, -1026, 4106, -1026];
        let right = if stereo {
            [0, 1026, 0, 2053, 0, 2053, 0, 2053]
        } else {
            left
        };
        let expected = left
            .into_iter()
            .zip(right)
            .map(|(l, r)| [l, r])
            .collect::<Vec<_>>();
        let before_pcm = export_pcm(&service, files.output.clone());
        for (got, want) in before_pcm.iter().zip(&expected) {
            for channel in 0..2 {
                assert!(
                    (i32::from(got[channel]) - i32::from(want[channel])).abs() <= 1,
                    "{got:?} != {want:?}"
                );
            }
        }
        for request in [Request::SaveNew(saved.clone()), Request::SaveCurrent] {
            service.submit(request).unwrap();
            let result = event(&service);
            assert!(
                result.save_complete && result.error.is_none(),
                "{:?}",
                result.error
            );
            assert_eq!(result.state.revision, baseline.state.revision);
        }
        let persisted = hex_storage::load(&saved).unwrap();
        assert!(persisted.assets().all(|a| a.original_source().is_some()));
        service
            .submit(Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
        assert!(event(&service).error.is_none());
        service.submit(Request::Open(saved.clone())).unwrap();
        let opened = event(&service);
        assert!(
            opened.open_complete && opened.project_replaced && opened.error.is_none(),
            "{:?}",
            opened.error
        );
        assert_eq!(opened.state.project.as_ref(), &persisted);
        assert_eq!(opened.state.project.tuning(), tuning);
        assert_eq!(
            opened
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &expected_state
        );
        assert_eq!(opened.state.document_path.as_ref(), Some(&saved));
        assert!(!opened.state.unsaved && !opened.state.can_undo && !opened.state.can_redo);
        assert!(opened.playback.is_none());
        assert!(
            opened
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .sources()
                .is_empty()
        );
        fs::remove_file(&files.source).unwrap();
        fs::remove_file(&unassigned.source).unwrap();
        assert_eq!(
            export_pcm(&service, files.output.with_file_name("reopened.wav")),
            before_pcm
        );
        // Both referenced and unassigned originals were actually reconstructed,
        // not merely left as metadata claims. Save can project their catalogs
        // without rereading the now absent source files or creating an edit.
        service.submit(Request::SaveCurrent).unwrap();
        let saved_again = event(&service);
        assert!(saved_again.error.is_none(), "{:?}", saved_again.error);
        assert_eq!(saved_again.state.revision, opened.state.revision);
    }
}

#[test]
fn failed_original_open_preserves_document_history_media_and_existing_sound() {
    for case in [
        "changed",
        "longer",
        "missing",
        "corrupt",
        "oversize",
        "unassigned-missing",
        "missing-facts",
        "cap-before-decode",
        "future-project",
        "malformed-project",
    ] {
        let files = Files::new(false);
        let unassigned = Files::new(false);
        let saved = files.source.with_file_name("sampler.hex");
        let service = Service::with_native_reservation(
            16000,
            16,
            if case == "cap-before-decode" { 8 } else { 1024 },
            reservation(),
        )
        .unwrap();
        let (track, before) = authored(&service);
        service
            .submit(request(&before, track, files.source.clone()))
            .unwrap();
        assert!(event(&service).error.is_none());
        service
            .submit(Request::ImportDeviceAsset {
                path: unassigned.source.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        assert!(event(&service).error.is_none());
        service.submit(Request::SaveNew(saved.clone())).unwrap();
        let before = event(&service);
        assert!(before.error.is_none());
        let saved_bytes = fs::read(&saved).unwrap();
        let original = fs::read(&files.source).unwrap();
        let mut open_path = saved.clone();
        match case {
            "changed" => {
                let mut changed = original.clone();
                changed[44..46].copy_from_slice(&8192_i16.to_le_bytes());
                fs::write(&files.source, changed).unwrap();
            }
            "longer" => {
                let mut changed = original.clone();
                changed.extend_from_slice(&[0, 32, 0, 32]);
                let riff = u32::try_from(changed.len() - 8).unwrap();
                let data = u32::try_from(changed.len() - 44).unwrap();
                changed[4..8].copy_from_slice(&riff.to_le_bytes());
                changed[40..44].copy_from_slice(&data.to_le_bytes());
                fs::write(&files.source, changed).unwrap();
            }
            "missing" => fs::remove_file(&files.source).unwrap(),
            "unassigned-missing" => fs::remove_file(&unassigned.source).unwrap(),
            "corrupt" | "cap-before-decode" => fs::write(&files.source, b"invalid wav").unwrap(),
            "oversize" => {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&files.source)
                    .unwrap()
                    .set_len(u64::try_from(IMPORT_FILE_LIMIT + 1).unwrap())
                    .unwrap();
            }
            "missing-facts" => {
                let mut snapshot = hex_storage::load(&saved).unwrap().snapshot();
                snapshot.assets[0].original_source = None;
                open_path = saved.with_file_name("unqualified.hex");
                hex_storage::save_new(
                    &open_path,
                    &hex_project::Project::from_snapshot(snapshot).unwrap(),
                )
                .unwrap();
            }
            "future-project" => {
                let mut document: serde_json::Value = serde_json::from_slice(&saved_bytes).unwrap();
                document["version"] = (hex_storage::NEWEST_SCHEMA_VERSION + 1).into();
                open_path = saved.with_file_name("future.hex");
                fs::write(&open_path, serde_json::to_vec(&document).unwrap()).unwrap();
            }
            "malformed-project" => {
                open_path = saved.with_file_name("malformed.hex");
                fs::write(&open_path, b"{ invalid project").unwrap();
            }
            _ => panic!("case"),
        }
        let rejected_bytes = fs::read(&open_path).unwrap();
        service.submit(Request::Open(open_path.clone())).unwrap();
        let refused = event(&service);
        assert!(
            refused.open_complete && !refused.project_replaced && refused.error.is_some(),
            "{case}"
        );
        if case == "changed" {
            assert!(refused.error.as_ref().unwrap().contains("content differs"));
        }
        if case == "longer" {
            assert!(
                refused
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("media limit exceeded: decoded frames"),
                "{:?}",
                refused.error
            );
        }

        if case == "cap-before-decode" {
            assert!(
                refused.error.as_ref().unwrap().contains("frame limit"),
                "{:?}",
                refused.error
            );
        }
        if case == "future-project" {
            assert!(refused.error.as_ref().unwrap().contains("newer schema"));
        }
        assert_eq!(fs::read(&open_path).unwrap(), rejected_bytes, "{case}");
        assert_eq!(fs::read(&saved).unwrap(), saved_bytes, "{case}");
        assert_eq!(
            refused.state.project.snapshot(),
            before.state.project.snapshot(),
            "{case}"
        );
        assert_eq!(refused.state.revision, before.state.revision, "{case}");
        assert_eq!(
            (
                refused.state.can_undo,
                refused.state.can_redo,
                refused.state.unsaved
            ),
            (
                before.state.can_undo,
                before.state.can_redo,
                before.state.unsaved
            ),
            "{case}"
        );
        assert_eq!(
            refused.state.document_path, before.state.document_path,
            "{case}"
        );
        assert!(refused.playback.is_none());
        let expected = [4096, 10240, 16384, 6144, -4096, -2048, 0, 0].map(|n| [n, n]);
        assert_pcm_near(
            &export_pcm(&service, files.output.clone()),
            &expected,
            &format!("old actual sound after {case}"),
        );
    }
}

#[test]
fn original_relative_media_and_save_as_keep_the_original_base_and_content() {
    let files = Files::new(false);
    let saved = files.source.with_file_name("first.hex");
    let relative = files.source.with_file_name("relative.hex");
    let service = Service::with_native_reservation(16000, 16, 1024, reservation()).unwrap();
    let (track, before) = authored(&service);
    service
        .submit(request(&before, track, files.source.clone()))
        .unwrap();
    assert!(event(&service).error.is_none());
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    assert!(event(&service).error.is_none());
    let mut snapshot = hex_storage::load(&saved).unwrap().snapshot();
    snapshot.assets[0].path = "original.wav".into();
    hex_storage::save_new(
        &relative,
        &hex_project::Project::from_snapshot(snapshot).unwrap(),
    )
    .unwrap();
    service.submit(Request::Open(relative)).unwrap();
    let opened = event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert_eq!(
        opened.state.project.assets().next().unwrap().path(),
        std::path::Path::new("original.wav")
    );
    let other_directory = files.source.parent().unwrap().join("other");
    fs::create_dir(&other_directory).unwrap();
    let relocated = other_directory.join("saved.hex");
    let mut wrong = fs::read(&files.source).unwrap();
    wrong[44..46].copy_from_slice(&8192_i16.to_le_bytes());
    fs::write(other_directory.join("original.wav"), wrong).unwrap();
    service.submit(Request::SaveNew(relocated.clone())).unwrap();
    let saved = event(&service);
    assert!(saved.error.is_none());
    let persisted = hex_storage::load(&relocated).unwrap();
    assert_eq!(persisted.assets().next().unwrap().path(), files.source);
    assert_eq!(
        saved.state.project.assets().next().unwrap().path(),
        std::path::Path::new("original.wav")
    );
    service.submit(Request::SaveCurrent).unwrap();
    assert!(event(&service).error.is_none());
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    service.submit(Request::Open(relocated)).unwrap();
    let reopened = event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    let expected = [4096, 10240, 16384, 6144, -4096, -2048, 0, 0].map(|n| [n, n]);
    assert_pcm_near(
        &export_pcm(&service, files.output.clone()),
        &expected,
        "relocated original",
    );
}

#[test]
fn held_export_and_new_original_reconstruction_share_one_finite_union() {
    for frames in [7, 8] {
        let files = Files::new(false);
        let saved = files.source.with_file_name("held.hex");
        let service = Service::with_native_reservation(16000, 16, frames, reservation()).unwrap();
        let (track, before) = authored(&service);
        service
            .submit(request(&before, track, files.source.clone()))
            .unwrap();
        assert!(event(&service).error.is_none());
        service.submit(Request::SaveNew(saved.clone())).unwrap();
        assert!(event(&service).error.is_none());
        let (entered, release) = block_export(&service);
        queue_export(&service, &files, Default::default());
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        service
            .submit(Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
        let empty = event(&service);
        assert!(empty.error.is_none());
        service.submit(Request::Open(saved.clone())).unwrap();
        let opened = event(&service);
        if frames == 7 {
            assert!(
                opened
                    .error
                    .as_ref()
                    .is_some_and(|e| e.contains("frame limit")),
                "{:?}",
                opened.error
            );
            assert!(!opened.project_replaced);
            assert_eq!(
                opened.state.project.snapshot(),
                empty.state.project.snapshot()
            );
            assert_eq!(opened.state.revision, empty.state.revision);
        } else {
            assert!(
                opened.project_replaced && opened.error.is_none(),
                "{:?}",
                opened.error
            );
        }
        release.send(()).unwrap();
        assert!(export_event(&service).result.is_ok());
        if frames == 7 {
            let end = Instant::now() + crate::test_wait::HANG_BOUND;
            loop {
                service.submit(Request::Open(saved.clone())).unwrap();
                let retried = event(&service);
                if retried.error.is_none() {
                    assert!(retried.project_replaced);
                    break;
                }
                assert!(
                    Instant::now() < end,
                    "export hold did not retire: {:?}",
                    retried.error
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
fn original_open_caps_ordinary_decode_at_saved_frames_before_alias_construction() {
    let native = Files::new(false);
    let ordinary = Files::new(false);
    let mut current = Session::new(16000, 16, 1024)
        .unwrap()
        .with_native_reservation(reservation())
        .unwrap();
    let (_, snapshot) = current
        .import_service_snapshot(
            decode_wav_file(
                &native.source,
                Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
            )
            .unwrap(),
            &AtomicBool::new(false),
            Admission::default(),
        )
        .unwrap();
    import(
        &mut current,
        ordinary.source.clone(),
        None,
        Admission {
            current: Some(snapshot.selection()),
            ..Admission::default()
        },
        &mut Default::default(),
    )
    .unwrap();
    let saved = native.source.with_file_name("mixed.hex");
    hex_storage::save_new(&saved, &recovery::project_for_save(&current, None).unwrap()).unwrap();
    let before = current.project().clone();
    let history = (current.can_undo(), current.can_redo());
    let mut bytes = fs::read(&ordinary.source).unwrap();
    bytes.extend_from_slice(&[0, 32, 0, 32]);
    let riff = u32::try_from(bytes.len() - 8).unwrap();
    let data = u32::try_from(bytes.len() - 44).unwrap();
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    bytes[40..44].copy_from_slice(&data.to_le_bytes());
    fs::write(&ordinary.source, bytes).unwrap();
    let old = compile(&current, None).unwrap();
    let admission = Admission {
        current: Some(ControlSamplerSelection {
            sources: None,
            prepared: Some(&old),
        }),
        ..Admission::default()
    };
    let refused = recovery::open_source_with_limits(
        &current,
        16,
        &ProjectSource::Saved(saved),
        &BTreeMap::new(),
        DocumentAudioMode::Resident,
        None,
        Some(reservation()),
        admission.replacement_frame_allowance(&current).unwrap(),
        admission,
    )
    .err()
    .unwrap();
    assert!(
        refused.contains("media limit exceeded: decoded frames"),
        "{refused}"
    );
    assert_eq!(current.project(), &before);
    assert_eq!((current.can_undo(), current.can_redo()), history);
}

#[test]
fn replacement_snapshot_counts_old_original_and_legacy_pcm_before_compiler_entry() {
    for original in [false, true] {
        let old_files = Files::new(false);
        let mut bytes = fs::read(&old_files.source).unwrap();
        let pcm = bytes[44..].repeat(1024);
        bytes.truncate(44);
        bytes.extend_from_slice(&pcm);
        bytes[4..8].copy_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        bytes[40..44].copy_from_slice(&(pcm.len() as u32).to_le_bytes());
        fs::write(&old_files.source, bytes).unwrap();
        let mut limits = reservation();
        limits.max_scan_frames = 8192;
        let mut old = Session::new(16000, 16, 16384)
            .unwrap()
            .with_native_reservation(limits)
            .unwrap();
        let decoded = decode_wav_file(
            old_files.source.clone(),
            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
        )
        .unwrap();
        let old_snapshot = if original {
            old.import_service_snapshot(decoded, &AtomicBool::new(false), Admission::default())
                .unwrap()
                .1
        } else {
            drop(decoded);
            import(
                &mut old,
                old_files.source.clone(),
                None,
                Admission::default(),
                &mut Default::default(),
            )
            .unwrap();
            ServiceExportSnapshot {
                prepared: compile(&old, None).unwrap(),
                sources: None,
            }
        };
        let before = old.project().snapshot();
        let history = (old.can_undo(), old.can_redo());
        let files = Files::new(false);
        let (mut candidate, _) = direct_sampler(&files);
        let sources = ControlSamplerSources::prepare_on_control(
            &candidate.native_assets,
            candidate.project(),
            limits.storage.max_retained_bytes,
            limits.max_scan_frames,
            &candidate.shutdown,
        )
        .unwrap();
        let allowance = sources.compiler_factory_allowance().unwrap();
        // This precompiler allowance includes future factory/proof owners;
        // both the one-under refusal and exact-budget admission happen below.
        let admission = Admission {
            current: Some(old_snapshot.selection()),
            ..Admission::default()
        };
        let quote = admission
            .check(
                &old,
                &candidate,
                ControlSamplerSelection {
                    sources: Some(&sources),
                    prepared: None,
                },
                allowance * 2,
                None,
            )
            .unwrap();
        let exact = quote.construction.construction_peak_bytes;
        candidate
            .native_reservation
            .as_mut()
            .unwrap()
            .storage
            .max_construction_peak_bytes = exact - 1;
        SNAPSHOT_COMPILER_ENTRIES.with(|n| n.set(0));
        let result =
            candidate.prepare_service_snapshot_against(&old, candidate.project(), admission);
        assert!(
            result.is_err(),
            "old unassigned PCM must participate in the pre-compiler peak"
        );
        assert_eq!(SNAPSHOT_COMPILER_ENTRIES.with(|n| n.get()), 0);
        candidate
            .native_reservation
            .as_mut()
            .unwrap()
            .storage
            .max_construction_peak_bytes = exact;
        let result =
            candidate.prepare_service_snapshot_against(&old, candidate.project(), admission);
        assert!(
            result.is_ok(),
            "exact pre-compiler union: {:?}",
            result.err()
        );
        assert_eq!(SNAPSHOT_COMPILER_ENTRIES.with(|n| n.get()), 1);
        assert_eq!(old.project().snapshot(), before);
        assert_eq!((old.can_undo(), old.can_redo()), history);
    }
}

#[test]
fn retained_sampler_source_reopens_unassigned_and_rebinds_without_losing_original_pcm_or_state() {
    let files = Files::new(true);
    let saved = files.source.with_file_name("retained-source.hex");
    let service = Service::with_native_reservation(16000, 32, 1024, reservation()).unwrap();
    let (track, before) = authored(&service);
    service
        .submit(request(&before, track, files.source.clone()))
        .unwrap();
    let loaded = event(&service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    let source = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .unwrap();
    let definition = loaded.state.project.native_source(source).unwrap().clone();
    let device = definition.instrument().id();
    let mut expected_definition = hex_project::NativeSourceSnapshot::from(&definition);
    expected_definition.receiver = None;
    let expected = export_pcm(&service, files.source.with_file_name("before.wav"));
    assert!(expected.iter().any(|f| *f != [0; 2]));
    let unlinked = edit(
        &service,
        Command::BindTrackSource {
            track,
            source: None,
        },
    );
    assert!(unlinked.error.is_none(), "{:?}", unlinked.error);
    assert_eq!(
        unlinked
            .state
            .project
            .native_source(source)
            .map(hex_project::NativeSourceSnapshot::from),
        Some(expected_definition.clone())
    );
    assert_eq!(
        export_pcm(&service, files.source.with_file_name("unlinked.wav")),
        vec![[0; 2]; 8]
    );
    let removed = edit(&service, Command::RemoveTrack(track));
    assert!(removed.error.is_none(), "{:?}", removed.error);
    assert_eq!(
        removed
            .state
            .project
            .native_source(source)
            .map(hex_project::NativeSourceSnapshot::from),
        Some(expected_definition.clone())
    );
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    assert!(event(&service).error.is_none());
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(event(&service).error.is_none());
    service.submit(Request::Open(saved)).unwrap();
    let reopened = event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(
        reopened
            .state
            .project
            .native_source(source)
            .map(hex_project::NativeSourceSnapshot::from),
        Some(expected_definition.clone())
    );
    let new_track = edit(
        &service,
        Command::AddMidiTrack {
            name: "reuse".into(),
        },
    );
    let Some(Created::Track(new_track)) = new_track.created else {
        panic!("new track")
    };
    let rebound = edit(
        &service,
        Command::BindTrackSource {
            track: new_track,
            source: Some(source),
        },
    );
    assert!(rebound.error.is_none(), "{:?}", rebound.error);
    expected_definition.receiver = Some(new_track.value());
    assert_eq!(
        rebound
            .state
            .project
            .native_source(source)
            .map(hex_project::NativeSourceSnapshot::from),
        Some(expected_definition.clone())
    );
    assert_eq!(
        rebound
            .state
            .project
            .track(new_track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    assert!(
        rebound
            .state
            .project
            .track(new_track)
            .unwrap()
            .devices()
            .is_empty()
    );
    let placed = edit(
        &service,
        Command::PlaceMidiClip {
            track: new_track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    assert!(
        edit(
            &service,
            Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127
            }
        )
        .error
        .is_none()
    );
    assert_eq!(
        export_pcm(&service, files.source.with_file_name("reused.wav")),
        expected
    );
    service
        .submit(Request::Edit(Command::RemoveNativeSource(source)))
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some(), "bound source removal must refuse");
    assert_eq!(
        refused
            .state
            .project
            .native_source(source)
            .map(hex_project::NativeSourceSnapshot::from),
        Some(expected_definition)
    );
}
