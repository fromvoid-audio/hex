use super::*;
use hex_engine::{
    AudioCandidatePreparation, AudioRootStorageLedger, AudioStorageLedgerReservation,
    AudioStorageReservation, AudioStorageRole, OwnedAudioSource, SharedMetadata,
    native_device_resources::{
        OriginalResourceBankPlan, OriginalResourceInput, OriginalResourcePlanReservation,
        ResourceCatalogReservation,
    },
};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-main-parent-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Success,
    RetirementClosed,
    RetirementBusyAfterInvoke,
    Duplicate,
    BusyAfterInvoke,
    Closed,
    Panic,
}
fn exercise(stereo: bool, case: Case) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "main sampler".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let empty = editor.project().device(device).unwrap().state().clone();
    let assignment = hex_engine::sampler_replacement_state_on_control(&empty, 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            device,
            expected_state: empty,
            state: assignment.state,
            slot: assignment.slot,
            path: "main-sampler-original.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    if stereo {
        editor
            .execute(Command::AddReturnTrack {
                name: "local return".into(),
            })
            .unwrap();
        let return_track = editor.project().return_tracks()[0].id();
        editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.25,
            })
            .unwrap();
    }
    let target = SharedMetadata::new_on_control(editor.project().clone());
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let source = if stereo {
        OwnedAudioSource::try_resident(
            44_100,
            vec![[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]],
        )
        .ok()
        .unwrap()
    } else {
        OwnedAudioSource::try_resident_mono(44_100, vec![0.25, -0.5, 0.75, -1.0])
            .ok()
            .unwrap()
    };
    let entries = vec![OriginalResourceInput::native(asset, &source)];
    let producer = OriginalResourceBankPlan::borrow(&entries);
    let storage = AudioStorageReservation {
        allocation_items: 256,
        handle_obligations: 128,
        max_prepared_data_bytes: 64 * 1024 * 1024,
        max_construction_data_bytes: 64 * 1024 * 1024,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    let dir = Scratch::new();
    let path = dir.0.join("main.wav");
    let file =
        StagedWavFile::begin_on_control(&path, 44_100, 513, &hex_export::Publication::CreateNew)
            .unwrap();
    let mut job = MainControlFileJob::reserve_on_control(file).unwrap();
    assert!(
        job.advance_on_control(|_| panic!("unbound job cannot loan"))
            .is_err()
    );
    let mut parent = AudioCandidatePreparation::try_new_on_control(0, storage).unwrap();
    let allowance = OriginalResourcePlanReservation {
        inputs: AudioStorageReservation {
            allocation_items: 16,
            handle_obligations: 16,
            max_prepared_data_bytes: 65536,
            max_construction_data_bytes: 65536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 4096,
            hash_frames: 4,
        },
        max_preparation_metadata_bytes: 4096,
        max_retained_data_bytes: 65536,
        max_construction_peak_bytes: 65536,
    };
    parent
        .operations_on_control()
        .install_original_resources_on_control(&producer, allowance, &AtomicBool::new(false))
        .unwrap();
    let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let root = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let worker = signal.bind_worker_on_control(root.route());
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
    let limits = AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 128,
        max_retained_bytes: 48 * 1024 * 1024,
        max_construction_peak_bytes: 48 * 1024 * 1024,
    };
    let mut original = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    original
        .borrow_native_source_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    signal.visit_storage_on_control(&mut original).unwrap();
    worker.visit_storage_on_control(&mut original).unwrap();
    parent
        .operations_on_control()
        .prepare_main_original_sampler_on_control(
            &target,
            &producer,
            &editor,
            &original,
            &transforms,
            limits,
        )
        .unwrap();
    let identity = parent.operations_on_control().prepared_identity().unwrap();
    let reader = parent
        .operations_on_control()
        .register_compiled_reader_on_control()
        .unwrap();
    parent
        .operations_on_control()
        .begin_main_control_work_on_control(513)
        .unwrap();
    job.bind_main_on_control(identity).unwrap();
    assert!(job.bind_main_on_control(identity).is_err());
    drop(original);
    if !matches!(
        case,
        Case::Success | Case::RetirementClosed | Case::RetirementBusyAfterInvoke
    ) {
        let result = job.advance_on_control(|loan| {
            if case == Case::Closed {
                return Err(MainControlHostError::Closed);
            }
            loan.run(&mut parent.operations_on_control())
                .map_err(|_| MainControlHostError::Refused)?;
            match case {
                Case::Duplicate => {
                    let _ = loan.run(&mut parent.operations_on_control());
                }
                Case::BusyAfterInvoke => return Err(MainControlHostError::Busy),
                Case::Panic => panic!("injected main host loan"),
                _ => unreachable!(),
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(job.failed_incomplete());
        assert!(!path.exists());
        if case == Case::Panic {
            assert_eq!(
                job.first_panic.as_ref().unwrap().downcast_ref::<&str>(),
                Some(&"injected main host loan")
            );
        }
        assert!(
            job.advance_on_control(|_| panic!("failed job must not retry host"))
                .is_err()
        );
        drop(reader);
        parent.cleanup_on_control().unwrap();
        drop(job);
        assert!(!path.exists());
        return;
    }
    assert!(matches!(
        job.advance_on_control(|_| Err(MainControlHostError::Busy))
            .unwrap(),
        MainControlFileProgress::Busy
    ));
    assert_eq!(job.file.written_frames(), 0);
    let mut calls = 0;
    assert!(matches!(
        job.advance_on_control(|loan| {
            calls += 1;
            if calls == 2 {
                return Err(MainControlHostError::Busy);
            }
            loan.run(&mut parent.operations_on_control())
                .map_err(|_| MainControlHostError::Refused)
        })
        .unwrap(),
        MainControlFileProgress::Busy
    ));
    assert_eq!(calls, 2);
    assert!(job.phase == Phase::Written);
    assert_eq!(job.file.written_frames(), 256);
    let pending = job.pending.unwrap();
    assert!(
        matches!(parent.operations_on_control().step_main_control_work_on_control(256).unwrap(), Progress::Block(block) if block == pending)
    );
    assert!(matches!(
        job.advance_on_control(|_| Err(MainControlHostError::Busy))
            .unwrap(),
        MainControlFileProgress::Busy
    ));
    assert_eq!(job.file.written_frames(), 256);
    assert_eq!(job.pending, Some(pending));
    let mut published = false;
    for _ in 0..8 {
        let progress = job
            .advance_on_control(|loan| {
                loan.run(&mut parent.operations_on_control())
                    .map_err(|_| MainControlHostError::Refused)
            })
            .unwrap();
        assert!(!path.exists());
        if matches!(progress, MainControlFileProgress::Busy) {
            assert!(job.phase == Phase::Retiring);
            break;
        }
        if matches!(progress, MainControlFileProgress::Published(_)) {
            published = true;
        }
    }
    assert!(!published);
    assert!(job.phase == Phase::Retiring);
    assert_eq!(job.file.written_frames(), 513);
    assert!(!parent.failed_incomplete());
    if matches!(
        case,
        Case::RetirementClosed | Case::RetirementBusyAfterInvoke
    ) {
        let error = job.advance_on_control(|loan| {
            assert_eq!(
                loan.run(&mut parent.operations_on_control()),
                Err(AudioStorageError::ExternalReadersPending)
            );
            Err(if case == Case::RetirementClosed {
                MainControlHostError::Closed
            } else {
                MainControlHostError::Busy
            })
        });
        if case == Case::RetirementClosed {
            assert!(matches!(
                error,
                Err(MainControlFileError::Host(MainControlHostError::Closed))
            ));
        } else {
            assert!(matches!(error, Err(MainControlFileError::Protocol)));
        }
        assert!(job.failed_incomplete());
        assert!(!path.exists());
        drop(reader);
        parent.cleanup_on_control().unwrap();
        drop(job);
        assert!(!path.exists());
        return;
    }
    drop(reader);
    let result = job
        .advance_on_control(|loan| {
            loan.run(&mut parent.operations_on_control())
                .map_err(|_| MainControlHostError::Refused)
        })
        .unwrap();
    let MainControlFileProgress::Published(report) = result else {
        panic!("actual retired parent publishes")
    };
    assert_eq!(report.frames, 513);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 44 + 513 * 4);
    assert!(
        bytes[44..].iter().any(|value| *value != 0),
        "real sampler PCM, not a header-only oracle"
    );
    assert!(
        parent
            .operations_on_control()
            .finish_main_control_work_on_control(identity)
            .is_err(),
        "disposed parent is not a reusable completion flag"
    );
    assert!(
        job.advance_on_control(|_| panic!("published job must not retry host"))
            .is_err()
    );
    parent.cleanup_on_control().unwrap();
    eprintln!("actual Main parent File/ACK completed stereo={stereo} return={stereo} frames=513");
}
#[test]
fn real_main_parent_file_busy_ack_and_registered_reader_retirement() {
    exercise(false, Case::Success);
    exercise(true, Case::Success);
}
#[test]
fn real_main_parent_bad_host_cannot_publish_or_retry() {
    for case in [
        Case::Duplicate,
        Case::BusyAfterInvoke,
        Case::Closed,
        Case::Panic,
    ] {
        exercise(false, case);
    }
}

#[test]
fn reader_pending_cannot_hide_closed_or_busy_after_host_invocation() {
    exercise(false, Case::RetirementClosed);
    exercise(false, Case::RetirementBusyAfterInvoke);
}
