use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_project::{BeatTime, Command, Created};

#[test]
fn original_producer_to_persistent_sampler_candidate_keeps_target_and_reader_custody() {
    exercise(ConstructorPurpose::NativeFresh, false, false, false);
}
#[test]
fn main_original_sampler_control_work_matches_linear_pcm_and_reader_cleanup() {
    exercise(ConstructorPurpose::MainExport, false, false, false);
}
#[test]
fn main_original_input_destruction_failure_stays_in_same_anchor_once() {
    exercise(ConstructorPurpose::MainExport, true, false, false);
}
#[test]
fn main_cancel_with_unacknowledged_block_retains_reader_and_retires_original_once() {
    exercise(ConstructorPurpose::MainExport, false, true, false);
}
#[test]
fn retained_native_sampler_graph_binds_original_mono_stereo_and_pruned_target_while_rendering() {
    exercise(ConstructorPurpose::NativeFresh, false, false, true);
}
fn exercise(purpose: ConstructorPurpose, fault: bool, cancel: bool, transferred_graph: bool) {
    exercise_with_writers(purpose, fault, cancel, transferred_graph, false)
}
#[test]
fn main_original_sampler_same_writers_complete_registered_birth_pcm_cancel_and_fault() {
    for (fault, cancel) in [(false, false), (false, true), (true, false)] {
        exercise_with_writers(ConstructorPurpose::MainExport, fault, cancel, false, true);
    }
}
fn exercise_with_writers(
    purpose: ConstructorPurpose,
    fault: bool,
    cancel: bool,
    transferred_graph: bool,
    lending: bool,
) {
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
        if transferred_graph {
            for n in 0..16 {
                editor
                    .execute(Command::SetTempoPoint {
                        beat: BeatTime::ZERO,
                        tempo: hex_project::Tempo::new(100_000_000 + n * 1_000_000).unwrap(),
                    })
                    .unwrap();
            }
            assert!(editor.project().tracks().is_empty());
        }

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
            let result = if lending {
                let mut writers = transforms
                    .original_sampler_transaction_on_control()
                    .unwrap();
                cell.prepare_original_sampler_with_writers_on_control(
                    &target,
                    &plan,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    too_small,
                    ConstructorPurpose::MainExport,
                    Some(&mut writers),
                    None,
                )
            } else {
                cell.prepare_original_sampler_for_purpose_on_control(
                    &target,
                    &plan,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    too_small,
                    purpose,
                )
            };
            assert_eq!(result, Err(expected), "{refusal}");
            assert_eq!(crate::owned_shared::birth_count(), 0, "{refusal}");
            assert_eq!(crate::owned_shared::clone_count(), 0, "{refusal}");
            assert!(cell.owner.is_none() && cell.token.is_none());
        }
        if lending {
            let mut writers = transforms
                .original_sampler_transaction_on_control()
                .unwrap();
            let construction = cell
                .inspect_original_sampler_with_writers_on_control(
                    None, &target, &plan, None, &editor, None, &original, &writers, limits, purpose,
                )
                .unwrap();
            let mut foreign = AudioCandidatePreparation::try_new_on_control(0, outer()).unwrap();
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            let mut refused = None;
            let allocations = allocation_counter::measure(|| {
                refused = Some(foreign.construct_original_sampler_with_writers_on_control(
                    &target,
                    None,
                    &mut writers,
                    construction,
                ));
            });
            assert_eq!(refused, Some(Err(AudioStorageError::SourceMismatch)));
            assert_eq!(allocations.count_total, 0);
            assert_eq!(allocations.count_current, 0);
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
            assert!(foreign.token.is_none() && foreign.owner.is_none());
            assert!(cell.token.is_none() && cell.owner.is_none());
            foreign.cleanup_on_control().unwrap();
            let construction = cell
                .inspect_original_sampler_with_writers_on_control(
                    None, &target, &plan, None, &editor, None, &original, &writers, limits, purpose,
                )
                .unwrap();
            // SAME immutable allocation, DIFFERENT actual owning handle/field.
            // A copied owner identity cannot substitute for the inspected loan.
            let foreign_target_field = target.clone();
            crate::owned_shared::reset_birth_count();
            crate::owned_shared::reset_clone_count();
            let mut refused = None;
            let allocations = allocation_counter::measure(|| {
                refused = Some(cell.construct_original_sampler_with_writers_on_control(
                    &foreign_target_field,
                    None,
                    &mut writers,
                    construction,
                ));
            });
            assert_eq!(refused, Some(Err(AudioStorageError::SourceMismatch)));
            assert_eq!(allocations.count_total, 0);
            assert_eq!(allocations.count_current, 0);
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
            assert!(cell.token.is_none() && cell.owner.is_none());
            drop(foreign_target_field);
            // The actual original cell and SAME writers remain usable after
            // refusing the unrelated physical destination before any birth.
            drop(writers);
        }
        if purpose == ConstructorPurpose::NativeFresh {
            if transferred_graph {
                let mut foreign =
                    AudioCandidatePreparation::try_new_on_control(0, outer()).unwrap();
                foreign
                    .operations_on_control()
                    .install_original_resources_on_control(&plan, allowance(), &query(false, false))
                    .unwrap();
                let actual = cell.custody.as_ref().unwrap();
                let unrelated = foreign.custody.as_ref().unwrap();
                let mut shell = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
                crate::owned_shared::reset_birth_count();
                crate::owned_shared::reset_clone_count();
                let measured = allocation_counter::measure(|| {
                    cell.require_original_sampler_constructor_custody(actual)
                        .unwrap();
                    cell.visit_original_sampler_constructor_shell(actual, &mut shell)
                        .unwrap();
                    assert_eq!(
                        cell.require_original_sampler_constructor_custody(unrelated),
                        Err(AudioStorageError::SourceMismatch)
                    );
                });
                assert_eq!((measured.count_total, measured.count_current), (0, 0));
                assert_eq!(
                    (
                        crate::owned_shared::birth_count(),
                        crate::owned_shared::clone_count()
                    ),
                    (0, 0)
                );
                drop(shell);
                foreign.cleanup_on_control().unwrap();
            }
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
        } else if lending {
            let mut writers = transforms
                .original_sampler_transaction_on_control()
                .unwrap();
            cell.prepare_original_sampler_with_writers_on_control(
                &target,
                &plan,
                &editor,
                None,
                &original,
                &transforms,
                limits,
                ConstructorPurpose::MainExport,
                Some(&mut writers),
                None,
            )
            .unwrap();
            // End the SAME writers before runtime loans, DSP or File turns.
            drop(writers);
        } else {
            cell.operations_on_control()
                .prepare_main_original_sampler_on_control(
                    &target,
                    &plan,
                    &editor,
                    &original,
                    &transforms,
                    limits,
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
        if transferred_graph {
            {
                let actual = &cell.owner.as_ref().unwrap().inner.custody;
                let mut shell = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
                let measured = allocation_counter::measure(|| {
                    cell.require_original_sampler_constructor_custody(actual)
                        .unwrap();
                    cell.visit_original_sampler_constructor_shell(actual, &mut shell)
                        .unwrap();
                });
                assert_eq!((measured.count_total, measured.count_current), (0, 0));
            }
            let (mut runtime, mut anchor) = cell.split_for_publication_on_control().unwrap();
            runtime
                .apply_action(crate::PlaybackAction::Transport(
                    crate::PlaybackCommand::Play,
                ))
                .unwrap();
            let mut output = [[0.0; 2]; 16];
            let mut rendered = None;
            let measured =
                allocation_counter::measure(|| rendered = Some(runtime.render(&mut output)));
            rendered.unwrap().unwrap();
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            let gain = if with_return { 1.25 } else { 1.0 };
            let source: Vec<[f32; 2]> = if stereo {
                crate::native_device_resources::tests::PCM.to_vec()
            } else {
                vec![[0.25; 2], [-0.5; 2], [0.75; 2], [-1.0; 2]]
            };
            for (index, frame) in output.iter().enumerate() {
                let expected = source.get(index).copied().unwrap_or([0.0; 2]);
                assert_eq!(
                    *frame,
                    [expected[0] * gain, expected[1] * gain],
                    "stereo={stereo} return={with_return} frame={index}"
                );
            }
            transforms
                .with_pcm_inventory_on_control(|inventory| {
                    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
                    anchor.visit_transferred_sampler_inputs(
                        &target,
                        None,
                        Some(&plan),
                        &editor,
                        None,
                        &original,
                        inventory,
                        &mut ledger,
                    )?;
                    reader.visit_shared_storage_on_control(&mut ledger)?;
                    ledger.validate_selected_shared_handles_on_control()?;
                    assert_eq!(ledger.summary()?.unknown_storage, 0);
                    Ok::<_, AudioStorageError>(())
                })
                .unwrap();
            let proof = runtime.dispose_on_control().unwrap();
            let mut anchors = [Some(anchor)];
            proof.record_anchor_cells_on_control(&mut anchors).unwrap();
            drop(proof);
            anchor = anchors[0].take().unwrap();
            assert_eq!(
                anchor.finish_disposal_on_control().err(),
                Some(AudioStorageError::ExternalReadersPending)
            );
            drop(runtime); // Actual wrapper storage, separate from DSP proof.
            assert_eq!(
                anchor.finish_disposal_on_control().err(),
                Some(AudioStorageError::ExternalReadersPending),
                "actual external compiled reader STILL retains its own constructor obligation"
            );
            drop(reader);
            anchor.finish_disposal_on_control().unwrap();
            drop(anchor.recover_inputs_on_control().unwrap());
            cell.cleanup_on_control().unwrap();
            assert!(cell.complete);
            eprintln!(
                "retained native sampler graph completed stereo={stereo} return={with_return} target-pruned=true"
            );
            continue;
        }
        if purpose == ConstructorPurpose::NativeFresh {
            assert_eq!(
                cell.operations_on_control()
                    .begin_main_control_work_on_control(24001),
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
            cell.operations_on_control()
                .begin_main_control_work_on_control(24001)
                .unwrap();
            let mut actual = [[0.0; 2]; 24001];
            assert!(cell.test_work_native_claim_refuses());
            let mut previous = None;
            let mut offset = 0;
            loop {
                let mut step = Ok(crate::AudioCandidateControlProgress::Complete);
                let measured = allocation_counter::measure(|| {
                    step = cell
                        .operations_on_control()
                        .step_main_control_work_on_control([1, 17, 251, 83][offset % 4]);
                });
                assert_eq!(measured.count_total, 0);
                assert_eq!(measured.count_current, 0);
                let crate::AudioCandidateControlProgress::Block(block) = step.unwrap() else {
                    break;
                };
                assert_eq!(block.offset(), offset as u64);
                if let Some(old) = previous {
                    assert_eq!(
                        cell.operations_on_control()
                            .acknowledge_main_control_block_on_control(old),
                        Err(AudioStorageError::SourceMismatch)
                    );
                }
                assert!(cell.test_work_native_claim_refuses());
                assert_eq!(
                    cell.operations_on_control()
                        .step_main_control_work_on_control(256)
                        .unwrap(),
                    crate::AudioCandidateControlProgress::Block(block)
                );
                let mut copy = [[0.0; 2]; 256];
                cell.operations_on_control()
                    .copy_main_control_block_on_control(block, &mut copy)
                    .unwrap();
                actual[offset..offset + block.frames()].copy_from_slice(&copy[..block.frames()]);
                offset += block.frames();
                if cancel {
                    break;
                }
                cell.operations_on_control()
                    .acknowledge_main_control_block_on_control(block)
                    .unwrap();
                previous = Some(block);
                assert_eq!(
                    cell.operations_on_control()
                        .acknowledge_main_control_block_on_control(block),
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
