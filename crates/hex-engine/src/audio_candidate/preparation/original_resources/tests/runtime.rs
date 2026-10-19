use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_project::{BeatTime, Command, Created};

#[test]
fn original_producer_to_persistent_sampler_candidate_keeps_target_and_reader_custody() {
    for stereo in [false, true] {
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
                cell.prepare_original_sampler_on_control(
                    &target,
                    &plan,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    too_small
                ),
                Err(expected),
                "{refusal}"
            );
            assert_eq!(crate::owned_shared::birth_count(), 0, "{refusal}");
            assert_eq!(crate::owned_shared::clone_count(), 0, "{refusal}");
            assert!(cell.owner.is_none() && cell.token.is_none());
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
        drop(original);
        assert_eq!(
            cell.cleanup_on_control(),
            Err(AudioStorageError::ExternalReadersPending)
        );
        assert!(!cell.failed_incomplete());
        drop(reader);
        cell.cleanup_on_control().unwrap();
        assert!(cell.complete && cell.owner.is_none());
    }
}
