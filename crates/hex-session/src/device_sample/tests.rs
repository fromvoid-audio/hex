use super::*;
use hex_engine::AudioStorageReservation;
use hex_engine::native_device_resources::ResourceCatalogReservation;
use hex_project::{Created, Editor};

fn fixture() -> (Session, DeviceId) {
    let mut session = Session::new(48_000, 8, 1024).unwrap();
    let Created::Track(track) = session
        .editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap();
    (session, device)
}
fn limits() -> ResidentResourcePlanReservation {
    ResidentResourcePlanReservation {
        inputs: AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: 1_000_000,
            max_construction_data_bytes: 1_000_000,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 65_536,
            hash_frames: 8,
        },
        max_preparation_metadata_bytes: 65_536,
        max_retained_data_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

#[test]
fn actual_worker_source_binds_candidate_without_publishing_and_survives_staging_retirement() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for stereo in [false, true] {
        let (session, device) = fixture();
        let original = session.project().clone();
        let source = if stereo {
            OwnedAudioSource::try_resident(24_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let prepared = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "sample.wav".into(),
            source,
        )
        .unwrap();
        assert_eq!(prepared.source_sample_rate(), 24_000);
        assert_eq!(prepared.source_frames(), 4);
        let mut candidate = Editor::new(original.clone(), 8).unwrap();
        let Created::Asset(asset) = candidate
            .execute(prepared.assignment_command_on_control())
            .unwrap()
        else {
            panic!("asset")
        };
        assert_eq!(
            candidate.project().asset(asset).unwrap().playback_frames(),
            8
        );
        let cancel = AtomicBool::new(false);
        assert!(
            prepared
                .prepare_resource_bank_for_candidate_on_control(
                    &session,
                    candidate.project(),
                    &[],
                    limits(),
                    &cancel,
                )
                .is_err()
        );
        let bank = prepared
            .prepare_resource_bank_for_candidate_on_control(
                &session,
                candidate.project(),
                &[asset],
                limits(),
                &cancel,
            )
            .unwrap();
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 64,
                root_rows: 16,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
            .unwrap();
        // Staging is a real field, not merely an asset identity.
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        prepared
            .visit_original_pcm_on_control(&mut ledger, AudioStorageRole::Caller)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
        // A newly retained lease is another actual owner, even when its PCM
        // is already present. Selected closure must notice the omitted field.
        let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        drop(ledger);
        drop(retained);
        let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert_eq!(*session.project(), original);
        assert_eq!(session.resident_frames(), 0);
        assert!(!session.media.contains_key(&asset));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            prepared
                .prepare_resource_bank_for_candidate_on_control(
                    &session,
                    candidate.project(),
                    &[asset],
                    limits(),
                    &cancel,
                )
                .is_err()
        );
        drop(prepared);
        drop(bank);
        drop(candidate);
        drop(session);
        let view = retained.view().unwrap();
        if stereo {
            assert_eq!(view.resident_frames().unwrap(), &[[0.25, -0.5]; 4]);
        } else {
            assert_eq!(view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
    }
}

#[test]
fn bounded_file_worker_preserves_original_rate_and_reports_cancellation_and_missing_source() {
    let (session, device) = fixture();
    let state = hex_engine::empty_sampler_state_on_control().unwrap();
    let path = std::env::temp_dir().join(format!("hex-sampler-worker-{}.wav", std::process::id()));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&44_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&24_000_u32.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&8_u32.to_le_bytes());
    for sample in [8192_i16, -16384, 0, 32767] {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let cancel = AtomicBool::new(false);
    let limits = hex_media::Limits::new(1024, 8);
    let prepared = session
        .prepare_device_sample_on_worker(device, &state, &path, limits, &cancel)
        .unwrap();
    assert_eq!(prepared.path(), path);
    assert_eq!(
        (prepared.source_sample_rate(), prepared.source_frames()),
        (24_000, 4)
    );
    match &prepared.source {
        OriginalAudio::Mono(source) => assert_eq!(
            source.view().resident_mono_samples().unwrap(),
            &[0.25, -0.5, 0.0, 32767.0 / 32768.0]
        ),
        OriginalAudio::Stereo(source) => assert_eq!(source.frames()[0], [0.25, 0.25]),
    }
    assert!(
        PreparedDeviceSample::decode_on_worker(
            device,
            &state,
            &path,
            hex_media::Limits::new(1024, 3),
            &cancel
        )
        .is_err()
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        PreparedDeviceSample::decode_on_worker(device, &state, &path, limits, &cancel).is_err()
    );
    std::fs::remove_file(&path).unwrap();
    cancel.store(false, Ordering::Relaxed);
    assert!(
        PreparedDeviceSample::decode_on_worker(device, &state, &path, limits, &cancel).is_err()
    );
}

#[test]
fn bounded_file_worker_target_is_checked_before_missing_file_io_and_never_edits_history() {
    let (mut session, device) = fixture();
    let captured = session.project().device(device).unwrap().state().clone();
    let original = session.project().clone();
    let missing =
        std::env::temp_dir().join(format!("hex-sampler-absent-{}.wav", std::process::id()));
    let cancel = AtomicBool::new(false);
    let limits = hex_media::Limits::new(1024, 8);
    // A genuine target reaches I/O and reports missing media, not stale target.
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Media(_))
    ));
    assert_eq!(*session.project(), original);
    let replaced = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.other.instrument"),
        1,
        vec![],
    )
    .unwrap();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: replaced.clone(),
        })
        .unwrap();
    let before = session.project().clone();
    let undo = session.editor.can_undo();
    let redo = session.editor.can_redo();
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Invalid(
            "sample loading canceled · target instrument changed"
        ))
    ));
    // Matching identity with the wrong actual instrument type also refuses
    // before file access; an arbitrary state is never treated as a Sampler.
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &replaced, &missing, limits, &cancel),
        Err(SessionError::Engine(_))
    ));
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Media(hex_media::ImportError::Cancelled))
    ));
    assert_eq!(*session.project(), before);
    assert_eq!(
        (session.editor.can_undo(), session.editor.can_redo()),
        (undo, redo)
    );
    assert_eq!(session.resident_frames(), 0);
}

#[test]
fn stale_target_and_wrong_candidate_are_refused_before_publication() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (mut session, device) = fixture();
    let prepared = PreparedDeviceSample::from_native_on_control(
        device,
        session.project().device(device).unwrap().state(),
        "sample.wav".into(),
        OwnedAudioSource::try_resident_mono(48_000, vec![0.5; 4])
            .ok()
            .unwrap(),
    )
    .unwrap();
    assert!(
        prepared
            .prepare_resource_bank_for_candidate_on_control(
                &session,
                session.project(),
                &[],
                limits(),
                &AtomicBool::new(false),
            )
            .is_err()
    );
    let changed = session
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_millidb(-600)
        .unwrap();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: changed,
        })
        .unwrap();
    assert!(
        prepared
            .validate_target_on_control(session.project())
            .is_err()
    );
    assert!(
        session
            .editor
            .execute(prepared.assignment_command_on_control())
            .is_err()
    );
    assert_eq!(session.project().assets().count(), 0);
}

#[test]
fn real_worker_waveform_reuses_original_root_and_keeps_deep_zoom_after_control_retirement() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for stereo in [false, true] {
        let (session, device) = fixture();
        let source = if stereo {
            OwnedAudioSource::try_resident(
                48_000,
                vec![[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]],
            )
            .ok()
            .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(48_000, vec![0.25, -0.5, 0.75, -1.0])
                .ok()
                .unwrap()
        };
        let prepared = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "sample.wav".into(),
            source,
        )
        .unwrap();
        let mut candidate = Editor::new(session.project().clone(), 8).unwrap();
        let Created::Asset(asset) = candidate
            .execute(prepared.assignment_command_on_control())
            .unwrap()
        else {
            panic!("asset")
        };
        let cancel = AtomicBool::new(false);
        let waveform_limits = DeviceSampleWaveformLimits {
            bins: 1,
            scan_frames: 4,
            source_metadata_bytes: 4096,
        };
        assert!(
            prepared
                .prepare_resources_and_waveform_on_worker(
                    &session,
                    candidate.project(),
                    &[asset],
                    limits(),
                    DeviceSampleWaveformLimits {
                        scan_frames: 3,
                        ..waveform_limits
                    },
                    &cancel
                )
                .is_err()
        );
        let (bank, waveform) = prepared
            .prepare_resources_and_waveform_on_worker(
                &session,
                candidate.project(),
                &[asset],
                limits(),
                waveform_limits,
                &cancel,
            )
            .unwrap();
        let lease = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert!(waveform.shares_original_with(&lease));
        assert!(waveform.audio_source().is_none());
        assert_eq!(waveform.source_frames, 4);
        // One cached bin cannot explain this one-frame extrema; it must use
        // the actual retained original source, not fabricated/stretched peaks.
        drop(lease);
        drop(bank);
        drop(prepared);
        drop(candidate);
        drop(session);
        let peaks = waveform.peaks_for_frames(1, 2).unwrap();
        assert_eq!((peaks[0].min, peaks[0].max), (-0.5, -0.5));
        let right = if stereo { 1.0 } else { -0.5 };
        assert_eq!((peaks[1].min, peaks[1].max), (right, right));
    }
}

#[test]
fn pending_device_sample_inventory_joins_actual_history_command_states_path_and_original_pcm() {
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::canonical_native_state_storage_layout_supported()
    {
        eprintln!("unsupported pending sample storage constructors; no positive proof");
        return;
    }
    eprintln!(
        "SUPPORTED actual pending device decode, assignment command, model/history and original PCM inventory"
    );
    for stereo in [false, true] {
        let (session, device) = fixture();
        let source = if stereo {
            OwnedAudioSource::try_resident(24_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let mut path = PathBuf::with_capacity(311);
        path.push("pending-original.wav");
        let pending = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            path,
            source,
        )
        .unwrap();
        let command = pending.assignment_command_on_control();
        let Command::ImportDeviceAsset {
            expected_state,
            state,
            path,
            ..
        } = &command
        else {
            panic!("assignment")
        };
        assert_eq!(expected_state, &pending.expected_state);
        assert_eq!(state, &pending.assignment.state);
        let budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        };
        let collect = |budget| -> Result<_, AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            pending.visit_original_pcm_on_control(&mut ledger, AudioStorageRole::Caller)?;
            let pcm = ledger.summary()?.data_bytes;
            pending.visit_selected_storage_on_control(&mut ledger)?;
            let staging = ledger.summary()?.data_bytes;
            let requested = |length| {
                std::alloc::Layout::array::<std::sync::atomic::AtomicUsize>(2)
                    .unwrap()
                    .extend(std::alloc::Layout::array::<u8>(length).unwrap())
                    .unwrap()
                    .0
                    .pad_to_align()
                    .size()
            };
            assert_eq!(
                staging - pcm,
                pending.path.capacity()
                    + requested(pending.expected_state.payload().len())
                    + requested(pending.assignment.state.payload().len())
            );
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err(),
                "actual model/history and pending command fields have not joined yet"
            );
            ledger.borrow_editor_native_states_on_control(&session.editor)?;
            ledger.borrow_canonical_native_state_on_control(expected_state)?;
            ledger.borrow_canonical_native_state_on_control(state)?;
            ledger.borrow_metadata_path_backing_on_control(path)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let measured = ledger.summary()?;
            pending.visit_selected_storage_on_control(&mut ledger)?;
            ledger.borrow_editor_native_states_on_control(&session.editor)?;
            assert_eq!(ledger.summary()?, measured);
            assert!(measured.require_complete().is_err());
            Ok(measured)
        };
        // The standalone root API stays STRICT; using it twice is still an
        // error. Only the embedded-field visitor is idempotent for this field.
        let mut direct = AudioRootStorageLedger::try_new_on_control(budget).unwrap();
        pending
            .visit_original_pcm_on_control(&mut direct, AudioStorageRole::Caller)
            .unwrap();
        assert_eq!(
            pending.visit_original_pcm_on_control(&mut direct, AudioStorageRole::Caller),
            Err(AudioStorageError::SourceMismatch)
        );
        drop(direct);
        let measured = collect(budget).unwrap();
        let mut exact = budget;
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        assert_eq!(pending.source_frames(), 4);
        assert_eq!(pending.source_sample_rate(), 24_000);
        assert_eq!(session.project().assets().count(), 0);
        assert!(
            session
                .project()
                .device(device)
                .unwrap()
                .state()
                .asset_references()
                .is_empty()
        );
    }
}

#[test]
fn sampler_snapshot_waveform_reads_actual_bound_original_and_retains_exact_old_display() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported original waveform constructors; no positive proof");
        return;
    }
    eprintln!("SUPPORTED exact Sampler snapshot read over actual decoded original waveform");
    let (session, device) = fixture();
    let source = OwnedAudioSource::try_resident_mono(24_000, vec![0.25, -0.5, 0.75, -1.0])
        .ok()
        .unwrap();
    let prepared = PreparedDeviceSample::from_native_on_control(
        device,
        session.project().device(device).unwrap().state(),
        "sample.wav".into(),
        source,
    )
    .unwrap();
    let mut candidate = Editor::new(session.project().clone(), 8).unwrap();
    let Created::Asset(asset) = candidate
        .execute(prepared.assignment_command_on_control())
        .unwrap()
    else {
        panic!("asset")
    };
    let (bank, waveform) = prepared
        .prepare_resources_and_waveform_on_worker(
            &session,
            candidate.project(),
            &[asset],
            limits(),
            DeviceSampleWaveformLimits {
                bins: 1,
                scan_frames: 4,
                source_metadata_bytes: 4096,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    let mut view = crate::service::view_state_owned(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        7,
        1,
        false,
        None,
    );
    // A prospective device preview, not a fabricated canonical source snapshot.
    view.canonical_audio_source = None;
    view.project = crate::SharedProject::new_on_control(candidate.project().clone());
    view.waveforms
        .try_insert_on_control(asset, crate::SharedWaveform::new_on_control(waveform), 4096)
        .unwrap();
    let mut next = crate::service::view_state_owned(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        8,
        1,
        false,
        None,
    );
    next.canonical_audio_source = None;
    next.project = crate::SharedProject::new_on_control(candidate.project().clone());
    // A real pending summary has no display yet, not peaks borrowed from old UI.
    assert!(next.waveforms.is_empty());
    let mut records = crate::snapshot_exports::SnapshotExportRecords::try_new_on_control(
        crate::snapshot_exports::SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        },
    )
    .unwrap();
    let old = records.insert_on_control(&mut Some(view)).unwrap();
    let old_read = records.reserve_read(old).unwrap();
    let new = records.insert_on_control(&mut Some(next)).unwrap();
    let new_read = records.reserve_read(new).unwrap();
    records.retire_on_control(old).unwrap();
    drop(bank);
    drop(prepared);
    drop(candidate);
    drop(session);
    assert_eq!(
        records
            .read_with(&old_read, |snapshot| {
                let read = snapshot.sampler_waveform(device).unwrap().unwrap();
                (
                    snapshot.revision(),
                    read.source_frames(),
                    read.peaks_for_frames(1, 2).unwrap(),
                )
            })
            .unwrap(),
        (
            7,
            4,
            [crate::CachedPeak {
                min: -0.5,
                max: -0.5
            }; 2]
        )
    );
    assert_eq!(
        records
            .read_with(&new_read, |snapshot| (
                snapshot.revision(),
                snapshot.sampler_waveform(device).unwrap().is_none()
            ))
            .unwrap(),
        (8, true)
    );
    records.release_read(old_read).unwrap();
    records.release_read(new_read).unwrap();
}

#[test]
fn disjoint_pending_resource_projection_works_beside_actual_editor_transition_and_refusal() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported disjoint pending resource projection");
        return;
    }
    for stereo in [false, true] {
        eprintln!("SUPPORTED disjoint pending resource projection stereo={stereo}");
        let (mut session, device) = fixture();
        let original_project = session.project().clone();
        let original_undo = session.editor.undo_storage_on_control().clone();
        let original_redo = session.editor.redo_storage_on_control().clone();
        let original = if stereo {
            OwnedAudioSource::try_resident(24000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let pending = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "pending.wav".into(),
            original,
        )
        .unwrap();
        let expected_pointer = match &pending.source {
            OriginalAudio::Mono(source) => {
                source.view().resident_mono_samples().unwrap().as_ptr() as usize
            }
            OriginalAudio::Stereo(audio) => audio.frames().as_ptr() as usize,
        };
        let projection =
            crate::resource_plan::producer::ResidentResourceSources::new(&session.media);
        let mut retained_bank = None;
        let mut asset_id = None;
        // The real editor callback borrows source/candidate while editor is
        // mutable. This compiles because projection borrows ONLY actual media.
        let result: Result<(Created, ()), SessionError> =
            session.editor.execute_prepared_transition(
                pending.assignment_command_on_control(),
                |source, candidate| {
                    assert_eq!(source, &original_project);
                    let reference = candidate.device(device).unwrap().state().asset_references()[0];
                    let asset = candidate
                        .assets()
                        .find(|asset| asset.id().value() == reference.asset.value())
                        .unwrap();
                    asset_id = Some(asset.id());
                    retained_bank = Some(
                        pending
                            .prepare_resource_bank_with_sources_on_control(
                                source,
                                candidate,
                                &projection,
                                &[asset.id()],
                                limits(),
                                &AtomicBool::new(false),
                            )
                            .map_err(|_| SessionError::Invalid("resource preparation refused"))?,
                    );
                    // Simulate a later, still-fallible preparation refusal. No
                    // native queue/media publication is performed by this test.
                    Err(SessionError::Invalid("later preparation refused"))
                },
            );
        assert!(result.is_err());
        assert_eq!(session.project(), &original_project);
        assert_eq!(session.editor.undo_storage_on_control(), &original_undo);
        assert_eq!(session.editor.redo_storage_on_control(), &original_redo);
        assert!(session.media.is_empty());
        assert!(session.media_bank().unwrap().resident.is_empty());
        let bank = retained_bank.unwrap();
        let lease = bank
            .borrow_original_source_on_control(asset_id.unwrap(), 4096)
            .unwrap();
        let view = lease.view().unwrap();
        assert_eq!(view.sample_rate(), 24000);
        if stereo {
            assert_eq!(
                view.resident_frames().unwrap().as_ptr() as usize,
                expected_pointer
            );
            assert_eq!(view.resident_frames().unwrap(), &[[0.25, -0.5]; 4]);
        } else {
            assert_eq!(
                view.resident_mono_samples().unwrap().as_ptr() as usize,
                expected_pointer
            );
            assert_eq!(view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
        drop(bank);
        drop(pending);
        let surviving_view = lease.view().unwrap();
        if stereo {
            assert_eq!(
                surviving_view.resident_frames().unwrap(),
                &[[0.25, -0.5]; 4]
            );
        } else {
            assert_eq!(surviving_view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
    }
}

#[test]
fn resolved_pending_plan_installs_into_actual_preparation_custody_beside_editor_refusal() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported resolved pending affine install");
        return;
    }
    for stereo in [false, true] {
        eprintln!("SUPPORTED resolved pending affine install stereo={stereo}");
        let (mut session, device) = fixture();
        let before = session.project().clone();
        let undo = session.editor.undo_storage_on_control().clone();
        let redo = session.editor.redo_storage_on_control().clone();
        let original = if stereo {
            OwnedAudioSource::try_resident(24000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let pending = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "pending.wav".into(),
            original,
        )
        .unwrap();
        let pcm_closed = || {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 32,
                    handle_rows: 16,
                    root_rows: 8,
                    max_retained_bytes: 1_000_000,
                    max_construction_peak_bytes: 1_000_000,
                },
            )
            .unwrap();
            pending
                .visit_original_pcm_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger.validate_selected_shared_handles_on_control()
        };
        pcm_closed().unwrap();
        let mut outer = limits().inputs;
        outer.allocation_items = 256;
        outer.handle_obligations = 128;
        outer.max_prepared_data_bytes = 16 * 1024 * 1024;
        outer.max_construction_data_bytes = 16 * 1024 * 1024;
        let mut cell = hex_engine::AudioCandidatePreparation::try_new_on_control(0, outer).unwrap();
        let sources = crate::resource_plan::producer::ResidentResourceSources::new(&session.media);
        let result: Result<(Created, ()), SessionError> =
            session.editor.execute_prepared_transition(
                pending.assignment_command_on_control(),
                |source, candidate| {
                    assert_eq!(source, &before);
                    let reference = candidate.device(device).unwrap().state().asset_references()[0];
                    let asset = candidate
                        .assets()
                        .find(|asset| asset.id().value() == reference.asset.value())
                        .unwrap();
                    let resolved = pending
                        .resolve_resource_plan_with_sources_on_control(
                            source,
                            candidate,
                            &sources,
                            &[asset.id()],
                            limits(),
                            &AtomicBool::new(false),
                        )
                        .map_err(|_| SessionError::Invalid("input resolution refused"))?;
                    // Resolution carries only borrowed rows: it has not minted a
                    // source alias, bank or lease and cannot expose an owning bank.
                    pcm_closed().unwrap();
                    cell.operations_on_control()
                        .install_original_resources_on_control(
                            &resolved.as_engine_plan(),
                            limits(),
                            &AtomicBool::new(false),
                        )
                        .map_err(|_| SessionError::Invalid("affine install refused"))?;
                    // The actual bank is now inside Engine custody. Omitting its
                    // actual source field independently refuses alias closure.
                    assert!(pcm_closed().is_err());
                    assert!(cell.owner_on_control().is_none());
                    Err(SessionError::Invalid("later preparation refused"))
                },
            );
        assert!(result.is_err());
        assert_eq!(session.project(), &before);
        assert_eq!(session.editor.undo_storage_on_control(), &undo);
        assert_eq!(session.editor.redo_storage_on_control(), &redo);
        assert!(session.media.is_empty());
        assert!(!cell.failed_incomplete());
        assert!(
            pcm_closed().is_err(),
            "editor refusal cannot lose installed custody"
        );
        cell.cleanup_on_control().unwrap();
        cell.cleanup_on_control().unwrap();
        pcm_closed().unwrap();
        match &pending.source {
            OriginalAudio::Stereo(audio) => assert_eq!(audio.frames(), &[[0.25, -0.5]; 4]),
            OriginalAudio::Mono(source) => {
                assert_eq!(source.view().resident_mono_samples().unwrap(), &[0.25; 4])
            }
        }
    }
}

struct PendingWaveformInterrupt {
    calls: std::sync::atomic::AtomicUsize,
    panic: bool,
}
impl hex_engine::CancellationQuery for PendingWaveformInterrupt {
    fn is_cancelled(&self) -> bool {
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if call == 4 {
            assert!(!self.panic, "injected pending waveform query panic");
            return true;
        }
        false
    }
}
fn native_waveform_limits(frames: usize) -> hex_media::waveform::NativeOriginalWaveformLimits {
    hex_media::waveform::NativeOriginalWaveformLimits {
        bins: 3,
        scan_frames: frames,
        metadata_bytes: hex_media::waveform::native_original_metadata_bytes(frames, 3).unwrap(),
    }
}
fn pending_native(stereo: bool, frames: usize) -> (Session, PreparedDeviceSample) {
    let (session, device) = fixture();
    let source = if stereo {
        OwnedAudioSource::try_resident(24_000, vec![[0.25, -0.5]; frames])
            .ok()
            .unwrap()
    } else {
        OwnedAudioSource::try_resident_mono(24_000, vec![0.25; frames])
            .ok()
            .unwrap()
    };
    let pending = PreparedDeviceSample::from_native_on_control(
        device,
        session.project().device(device).unwrap().state(),
        "pending.wav".into(),
        source,
    )
    .unwrap();
    (session, pending)
}
#[test]
fn pending_native_waveform_installs_then_retains_completed_or_cancelled_state_beside_atomic_editor_refusal()
 {
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::canonical_native_state_storage_layout_supported()
    {
        eprintln!("unsupported pending native waveform custody");
        return;
    }
    for stereo in [false, true] {
        for cancel_scan in [false, true] {
            eprintln!("SUPPORTED pending native waveform stereo={stereo} cancel={cancel_scan}");
            let (mut session, mut pending) = pending_native(stereo, 1025);
            let before = session.project().clone();
            let undo = session.editor.undo_storage_on_control().clone();
            let redo = session.editor.redo_storage_on_control().clone();
            let mut resource_limits = limits();
            resource_limits.catalog.hash_frames = 1025;
            let mut outer = resource_limits.inputs;
            outer.allocation_items = 256;
            outer.handle_obligations = 128;
            outer.max_prepared_data_bytes = 16 * 1024 * 1024;
            outer.max_construction_data_bytes = 16 * 1024 * 1024;
            let mut cell =
                hex_engine::AudioCandidatePreparation::try_new_on_control(0, outer).unwrap();
            let sources =
                crate::resource_plan::producer::ResidentResourceSources::new(&session.media);
            let result: Result<(Created, ()), SessionError> =
                session.editor.execute_prepared_transition(
                    pending.assignment_command_on_control(),
                    |source, candidate| {
                        let reference = candidate
                            .device(pending.device)
                            .unwrap()
                            .state()
                            .asset_references()[0];
                        let asset = candidate
                            .assets()
                            .find(|asset| asset.id().value() == reference.asset.value())
                            .unwrap();
                        {
                            let resolved = pending
                                .resolve_resource_plan_with_sources_on_control(
                                    source,
                                    candidate,
                                    &sources,
                                    &[asset.id()],
                                    resource_limits,
                                    &AtomicBool::new(false),
                                )
                                .unwrap();
                            cell.operations_on_control()
                                .install_original_resources_on_control(
                                    &resolved.as_engine_plan(),
                                    resource_limits,
                                    &AtomicBool::new(false),
                                )
                                .unwrap();
                        }
                        let interrupt = PendingWaveformInterrupt {
                            calls: std::sync::atomic::AtomicUsize::new(0),
                            panic: false,
                        };
                        let result = if cancel_scan {
                            pending.prepare_native_waveform_for_candidate_on_worker(
                                source,
                                candidate,
                                native_waveform_limits(1025),
                                &interrupt,
                            )
                        } else {
                            pending.prepare_native_waveform_for_candidate_on_worker(
                                source,
                                candidate,
                                native_waveform_limits(1025),
                                &AtomicBool::new(false),
                            )
                        };
                        if cancel_scan {
                            assert!(matches!(
                                result,
                                Err(hex_media::DeviceSourceImportError::Media(
                                    hex_media::ImportError::Cancelled
                                ))
                            ));
                            assert_eq!(interrupt.calls.load(Ordering::Relaxed), 4);
                            assert!(pending.native_waveform_read_on_control().unwrap().is_none());
                        } else {
                            result.unwrap();
                            let read = pending.native_waveform_read_on_control().unwrap().unwrap();
                            assert_eq!(read.source_frames(), 1025);
                            let expected = if stereo { -0.5 } else { 0.25 };
                            assert_eq!(read.peaks_for_frames(700, 701).unwrap()[1].min, expected);
                        }
                        assert!(pending.native_waveform_attempted);
                        assert!(pending.native_waveform_failure.is_none());
                        assert!(
                            pending
                                .prepare_native_waveform_for_candidate_on_worker(
                                    source,
                                    candidate,
                                    native_waveform_limits(1025),
                                    &interrupt
                                )
                                .is_err()
                        );
                        Err(SessionError::Invalid("later host preparation refused"))
                    },
                );
            assert!(result.is_err());
            assert_eq!(session.project(), &before);
            assert_eq!(session.editor.undo_storage_on_control(), &undo);
            assert_eq!(session.editor.redo_storage_on_control(), &redo);
            assert!(session.media.is_empty());
            drop(before);
            drop(undo);
            drop(redo);
            let export = pending.native_waveform.as_ref().cloned();
            let collect = || {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(
                    hex_engine::AudioStorageLedgerReservation {
                        data_rows: 256,
                        handle_rows: 128,
                        root_rows: 16,
                        max_retained_bytes: 2_000_000,
                        max_construction_peak_bytes: 2_000_000,
                    },
                )
                .unwrap();
                pending
                    .visit_selected_storage_on_control(&mut ledger)
                    .unwrap();
                ledger
                    .borrow_editor_native_states_on_control(&session.editor)
                    .unwrap();
                if let Some(export) = &export {
                    hex_media::waveform::borrow_shared_storage_on_control(export, &mut ledger)
                        .unwrap();
                }
                ledger.validate_selected_shared_handles_on_control()
            };
            assert!(collect().is_err(), "actual installed bank remains omitted");
            cell.cleanup_on_control().unwrap();
            collect().unwrap();
            pending.retire_native_waveform_on_control().unwrap();
            pending.retire_native_waveform_on_control().unwrap();
            assert!(pending.native_waveform_read_on_control().unwrap().is_none());
            drop(pending);
            drop(session);
            if let Some(export) = export {
                assert_eq!(export.read().source_frames(), 1025);
                assert_eq!(
                    export.read().peaks_for_frames(700, 701).unwrap()[0].max,
                    0.25
                );
            }
        }
    }
}
#[test]
fn pending_native_waveform_binding_and_prequote_refusal_precede_attempt() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    eprintln!("SUPPORTED pending native waveform actual binding refusals");
    let (session, mut pending) = pending_native(false, 1025);
    let mut candidate = Editor::new(session.project().clone(), 1).unwrap();
    candidate
        .execute(pending.assignment_command_on_control())
        .unwrap();
    let query = PendingWaveformInterrupt {
        calls: std::sync::atomic::AtomicUsize::new(0),
        panic: true,
    };
    let mut too_small = native_waveform_limits(1025);
    too_small.metadata_bytes -= 1;
    assert!(
        pending
            .prepare_native_waveform_for_candidate_on_worker(
                session.project(),
                candidate.project(),
                too_small,
                &query
            )
            .is_err()
    );
    assert!(!pending.native_waveform_attempted);
    assert_eq!(query.calls.load(Ordering::Relaxed), 0);
    let mut wrong = candidate.project().snapshot();
    wrong.assets[0].path = "different.wav".into();
    let wrong = Project::from_snapshot(wrong).unwrap();
    assert!(
        pending
            .prepare_native_waveform_for_candidate_on_worker(
                session.project(),
                &wrong,
                native_waveform_limits(1025),
                &query
            )
            .is_err()
    );
    assert!(!pending.native_waveform_attempted);
    assert_eq!(query.calls.load(Ordering::Relaxed), 0);
    // Same DeviceId/state/asset in a different model role is not the captured
    // candidate instrument, even though the neutral model can retain it.
    let mut wrong_role = candidate.project().snapshot();
    let instrument = wrong_role.tracks[0].instrument.take().unwrap();
    wrong_role.tracks[0].instrument_position = None;
    wrong_role.tracks[0].devices.push(instrument);
    let wrong_role = Project::from_snapshot(wrong_role).unwrap();
    assert!(
        pending
            .prepare_native_waveform_for_candidate_on_worker(
                session.project(),
                &wrong_role,
                native_waveform_limits(1025),
                &query
            )
            .is_err()
    );
    assert!(!pending.native_waveform_attempted);
    assert_eq!(query.calls.load(Ordering::Relaxed), 0);
    pending
        .prepare_native_waveform_for_candidate_on_worker(
            session.project(),
            candidate.project(),
            native_waveform_limits(1025),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(pending.native_waveform_read_on_control().unwrap().is_some());
    pending.retire_native_waveform_on_control().unwrap();
}
#[test]
fn pending_native_waveform_caught_panic_retains_first_payload_and_refuses_retry_inspection_cleanup()
{
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    eprintln!("SUPPORTED pending native waveform first panic retained");
    let (session, mut pending) = pending_native(false, 1025);
    let mut candidate = Editor::new(session.project().clone(), 1).unwrap();
    candidate
        .execute(pending.assignment_command_on_control())
        .unwrap();
    let query = PendingWaveformInterrupt {
        calls: std::sync::atomic::AtomicUsize::new(0),
        panic: true,
    };
    assert!(
        pending
            .prepare_native_waveform_for_candidate_on_worker(
                session.project(),
                candidate.project(),
                native_waveform_limits(1025),
                &query
            )
            .is_err()
    );
    assert_eq!(query.calls.load(Ordering::Relaxed), 4);
    assert!(pending.native_waveform.is_none());
    assert_eq!(
        pending
            .native_waveform_failure
            .as_ref()
            .unwrap()
            .downcast_ref::<&str>(),
        Some(&"injected pending waveform query panic")
    );
    assert_eq!(
        pending.native_waveform_read_on_control().err(),
        Some(AudioStorageError::UnknownStorage)
    );
    assert_eq!(
        pending.retire_native_waveform_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(
        pending
            .prepare_native_waveform_for_candidate_on_worker(
                session.project(),
                candidate.project(),
                native_waveform_limits(1025),
                &query
            )
            .is_err()
    );
    assert_eq!(query.calls.load(Ordering::Relaxed), 4);
    let mut ledger =
        AudioRootStorageLedger::try_new_on_control(hex_engine::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 32,
            root_rows: 16,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        })
        .unwrap();
    assert_eq!(
        pending.visit_selected_storage_on_control(&mut ledger),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(pending.source_frames(), 1025);
    // Fixture teardown on this control test thread is not successful production
    // retirement or a permit to clear/retry the retained failed parent slot.
}
