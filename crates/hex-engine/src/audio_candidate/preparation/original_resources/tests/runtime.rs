use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_project::{BeatTime, Command, Created};

#[test]
fn original_producer_to_persistent_sampler_candidate_keeps_target_and_reader_custody() {
    exercise(ConstructorPurpose::NativeFresh, false, false);
}
#[test]
fn main_original_sampler_control_work_matches_linear_pcm_and_reader_cleanup() {
    exercise(ConstructorPurpose::MainExport, false, false);
}
#[test]
fn main_original_input_destruction_failure_stays_in_same_anchor_once() {
    exercise(ConstructorPurpose::MainExport, true, false);
}
#[test]
fn main_cancel_with_unacknowledged_block_retains_reader_and_retires_original_once() {
    exercise(ConstructorPurpose::MainExport, false, true);
}
fn exercise(purpose: ConstructorPurpose, fault: bool, cancel: bool) {
    for (stereo, with_return) in [(false, false), (false, true), (true, false), (true, true)] {
        let (unused_bank, identity, mut editor) =
            crate::shared_analytic::sampler_tests::canonical_fixture().unwrap();
        drop(identity);
        drop(unused_bank);
        let track = editor.project().tracks()[0].id();
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
        if purpose == ConstructorPurpose::MainExport {
            editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 128,
                    enabled: true,
                })
                .unwrap();
            editor
                .execute(Command::SetTempoPoint {
                    beat: BeatTime::ONE,
                    tempo: hex_project::Tempo::new(90_000_000).unwrap(),
                })
                .unwrap();
        }
        if with_return {
            use hex_lattice_contract::NativeDeviceMetadata;
            editor
                .execute(Command::AddReturnTrack {
                    name: "main return".into(),
                })
                .unwrap();
            let return_track = editor.project().return_tracks()[0].id();
            editor
                .execute(Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
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
        let target = crate::SharedMetadata::new_on_control(editor.project().clone());
        editor.execute(Command::RemoveTrack(track)).unwrap();
        let original_source = if stereo {
            crate::OwnedAudioSource::try_resident(
                44_100,
                crate::native_device_resources::tests::PCM.to_vec(),
            )
            .ok()
            .unwrap()
        } else {
            crate::OwnedAudioSource::try_resident_mono(44_100, vec![0.25, -0.5, 0.75, -1.0])
                .ok()
                .unwrap()
        };
        let entries = vec![OriginalResourceInput::native(
            crate::native_device_resources::tests::ids()[0],
            &original_source,
        )];
        let plan = OriginalResourceBankPlan::borrow(&entries);
        let mut storage = outer();
        storage.max_prepared_data_bytes = 64 * 1024 * 1024;
        storage.max_construction_data_bytes = 64 * 1024 * 1024;
        let mut cell = AudioCandidatePreparation::try_new_on_control(0, storage).unwrap();
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &query(false, false))
            .unwrap();
        let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = crate::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let worker = signal.bind_worker_on_control(job.route());
        let transforms = crate::audio_transform::AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
        let limits = AudioStorageLedgerReservation {
            data_rows: 1024,
            handle_rows: 1024,
            root_rows: 128,
            max_retained_bytes: 48 * 1024 * 1024,
            max_construction_peak_bytes: 48 * 1024 * 1024,
        };
        let mut original = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
        original
            .borrow_native_source_on_control(&original_source, AudioStorageRole::Caller)
            .unwrap();
        signal.visit_storage_on_control(&mut original).unwrap();
        worker.visit_storage_on_control(&mut original).unwrap();
        for refusal in ["retained", "peak", "rows"] {
            let mut too_small = limits;
            let expected = match refusal {
                "retained" => {
                    too_small.max_retained_bytes = crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES - 1;
                    AudioStorageError::ReservationExceeded
                }
                "peak" => {
                    too_small.max_construction_peak_bytes =
                        crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES - 1;
                    AudioStorageError::ReservationExceeded
                }
                _ => {
                    too_small.handle_rows = 0;
                    AudioStorageError::Capacity
                }
            };
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            assert_eq!(
                cell.prepare_original_sampler_for_purpose_on_control(
                    &target,
                    &plan,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    too_small,
                    purpose
                ),
                Err(expected),
                "{refusal}"
            );
            assert_eq!(crate::owned_shared::birth_count(), 0, "{refusal}");
            assert_eq!(crate::owned_shared::clone_count(), 0, "{refusal}");
            assert!(cell.owner.is_none() && cell.token.is_none());
        }
        if purpose == ConstructorPurpose::NativeFresh {
            cell.prepare_original_sampler_on_control(
                &target,
                &plan,
                &editor,
                None,
                &original,
                &transforms,
                limits,
            )
            .unwrap();
        } else {
            cell.prepare_original_sampler_for_purpose_on_control(
                &target,
                &plan,
                &editor,
                None,
                &original,
                &transforms,
                limits,
                purpose,
            )
            .unwrap();
        }
        assert!(cell.custody.is_none() && cell.token.is_none() && cell.owner.is_some());
        assert!(
            cell.owner
                .as_ref()
                .unwrap()
                .inner
                .constructor_data
                .is_some()
        );
        // No scalar SourceContext is fabricated to make this component look
        // like an admitted Service source association.
        assert!(cell.owner.as_ref().unwrap().source_context().is_none());
        let reader = cell
            .operations_on_control()
            .register_compiled_reader_on_control()
            .unwrap();
        if purpose == ConstructorPurpose::NativeFresh {
            assert_eq!(
                cell.begin_main_control_work_on_control(24001),
                Err(AudioStorageError::SourceMismatch)
            );
        } else {
            assert_eq!(
                cell.owner
                    .as_mut()
                    .unwrap()
                    .seal_constructor_on_control(limits),
                Err(AudioStorageError::SourceMismatch)
            );
            assert!(cell.preallocate_publication_split_on_control().is_err());
            let owner = cell.owner.as_ref().unwrap();
            let revision = owner
                .inner
                .custody
                .candidate_root_view(&owner.inner.token)
                .unwrap();
            let mut oracle = crate::playback::Playback::from_arrangement_preroll(
                crate::SessionPlayback::new_linear_arrangement_export(std::sync::Arc::clone(
                    revision,
                ))
                .unwrap(),
            )
            .unwrap();
            let mut interactive =
                crate::playback::Playback::new_audio_candidate(std::sync::Arc::clone(revision))
                    .unwrap();
            interactive.apply(crate::PlaybackCommand::Play);
            let mut loop_output = [[0.0; 2]; 513];
            interactive.render(&mut loop_output).unwrap();
            assert!(
                interactive.position() < 128,
                "native still wraps authored loop"
            );
            drop(interactive);
            oracle.apply(crate::PlaybackCommand::Play);
            let mut expected = [[0.0; 2]; 24001];
            oracle.render(&mut expected).unwrap();
            drop(oracle);
            cell.begin_main_control_work_on_control(24001).unwrap();
            let mut actual = [[0.0; 2]; 24001];
            assert!(cell.test_work_native_claim_refuses());
            let mut previous = None;
            let mut offset = 0;
            loop {
                let mut step = Ok(crate::AudioCandidateControlProgress::Complete);
                let measured = allocation_counter::measure(|| {
                    step = cell.step_main_control_work_with_frames_on_control(
                        [1, 17, 251, 83][offset % 4],
                    );
                });
                assert_eq!(measured.count_total, 0);
                assert_eq!(measured.count_current, 0);
                let crate::AudioCandidateControlProgress::Block(block) = step.unwrap() else {
                    break;
                };
                assert_eq!(block.offset(), offset as u64);
                if let Some(old) = previous {
                    assert_eq!(
                        cell.acknowledge_main_control_block_on_control(old),
                        Err(AudioStorageError::SourceMismatch)
                    );
                }
                assert!(cell.test_work_native_claim_refuses());
                assert_eq!(
                    cell.step_main_control_work_on_control().unwrap(),
                    crate::AudioCandidateControlProgress::Block(block)
                );
                let mut copy = [[0.0; 2]; 256];
                cell.copy_main_control_block_on_control(block, &mut copy)
                    .unwrap();
                actual[offset..offset + block.frames()].copy_from_slice(&copy[..block.frames()]);
                offset += block.frames();
                if cancel {
                    break;
                }
                cell.acknowledge_main_control_block_on_control(block)
                    .unwrap();
                previous = Some(block);
                assert_eq!(
                    cell.acknowledge_main_control_block_on_control(block),
                    Err(AudioStorageError::SourceMismatch)
                );
            }
            if cancel {
                assert_eq!(&actual[..offset], &expected[..offset]);
                assert_eq!(offset, 1);
            } else {
                assert_eq!(actual, expected);
                assert_eq!(offset, 24001);
            }
            cell.test_main_work_inventory(
                &target,
                &plan,
                &editor,
                &original,
                &transforms,
                &reader,
                limits,
            )
            .unwrap();
        }
        let drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        if purpose == ConstructorPurpose::MainExport {
            let drops = drops.clone();
            cell.test_work_original_drop(Box::new(move || {
                drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(!fault, "injected original input destruction");
            }));
        }
        drop(original);
        assert_eq!(
            cell.cleanup_on_control(),
            Err(AudioStorageError::ExternalReadersPending)
        );
        assert!(!cell.failed_incomplete());
        drop(reader);
        eprintln!(
            "main control completed purpose={purpose:?} stereo={stereo} return={with_return} fault={fault}"
        );
        if fault {
            assert_eq!(
                cell.cleanup_on_control(),
                Err(AudioStorageError::UnknownStorage)
            );
            assert!(cell.test_work_kept_original_drop_failure());
            assert_eq!(
                cell.cleanup_on_control(),
                Err(AudioStorageError::UnknownStorage)
            );
            assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        } else {
            cell.cleanup_on_control().unwrap();
            assert!(cell.complete && cell.owner.is_none());
            if purpose == ConstructorPurpose::MainExport {
                assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
            }
        }
    }
}
