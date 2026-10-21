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
            overwrite: false,
        })
        .unwrap();
    assert!(event(service).error.is_none());
    let result = export_event(service);
    assert!(result.result.is_ok(), "{:?}", result.result);
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
    // Literal current v1 authored state: Classic loop, region1..4, loop1..3,
    // gain0.5, two-frame attack/release at16k. This is not a second codec or UI.
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
    hex_lattice_contract::NativeDeviceState::new(original.type_id(), 1, bytes)
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
        "missing",
        "corrupt",
        "oversize",
        "unassigned-missing",
        "missing-facts",
        "cap-before-decode",
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
        let original = fs::read(&files.source).unwrap();
        let mut open_path = saved.clone();
        match case {
            "changed" => {
                let mut changed = original.clone();
                changed[44..46].copy_from_slice(&8192_i16.to_le_bytes());
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
            _ => panic!("case"),
        }
        service.submit(Request::Open(open_path)).unwrap();
        let refused = event(&service);
        assert!(
            refused.open_complete && !refused.project_replaced && refused.error.is_some(),
            "{case}"
        );
        if case == "changed" {
            assert!(refused.error.as_ref().unwrap().contains("content differs"));
        }
        if case == "cap-before-decode" {
            assert!(
                refused.error.as_ref().unwrap().contains("frame limit"),
                "{:?}",
                refused.error
            );
        }
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
        assert_eq!(
            export_pcm(&service, files.output.clone()),
            expected,
            "old actual sound after {case}"
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
    assert_eq!(export_pcm(&service, files.output.clone()), expected);
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
            let end = Instant::now() + Duration::from_secs(2);
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
