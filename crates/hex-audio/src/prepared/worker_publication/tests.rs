use super::*;
use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
use crate::prepared::candidate_tests::{candidate, finish, serial};
use hex_project::{Command, Created, Editor};

#[test]
fn nonextracting_preparation_operations_keep_actual_worker_cell_through_ready_cancel_and_retirement()
 {
    let _serial = serial();
    let Some(mut seed) = candidate() else { return };
    eprintln!(
        "SUPPORTED actual rooted WorkerPublication nonextracting preparation, cancellation, PCM and terminal retirement"
    );
    let ledger = hex_engine::AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 2048,
        root_rows: 1,
        max_retained_bytes: 96 * 1024 * 1024,
        max_construction_peak_bytes: 96 * 1024 * 1024,
    };
    let summary = seed.seal_constructor_on_control(ledger).unwrap();
    let limits = crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: summary.member_rows * 3,
        transfer_rows: summary.transfer_rows * 3,
        receipt_rows: 4,
        data_rows: ledger.data_rows,
        handle_rows: ledger.handle_rows,
        max_retained_data_bytes: ledger.max_retained_bytes,
        max_construction_peak_data_bytes: ledger.max_construction_peak_bytes,
    };
    drop(seed);
    for cancelled in [false, true] {
        let (control, callback) = channel(48_000).unwrap();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        {
            let mut loan = worker.control().unwrap();
            loan.enable_native_custody().unwrap();
            loan.configure_audio_candidate_publication(limits).unwrap();
            client.activate();
            let token = loan
                .reserve_audio_candidate_preparation(
                    crate::AudioCandidatePreparationReservation {
                        source_capacity: 1,
                        member_rows: limits.member_rows / 3,
                        transfer_rows: limits.transfer_rows / 3,
                        handle_rows: limits.handle_rows / 3,
                        storage: hex_engine::AudioStorageReservation {
                            allocation_items: 256,
                            handle_obligations: 512,
                            max_prepared_data_bytes: limits.max_retained_data_bytes / 3,
                            max_construction_data_bytes: limits.max_construction_peak_data_bytes
                                / 3,
                            caller_retained_bytes: 0,
                            caller_construction_bytes: 0,
                        },
                    },
                    Vec::new(),
                )
                .ok()
                .unwrap();
            let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: "audio".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor.execute(Command::RemoveInstrument { track }).unwrap();
            let Created::Asset(asset) = editor
                .execute(Command::AddAsset {
                    path: "actual.wav".into(),
                    sample_rate: 48_000,
                    frames: 8,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: 0,
                    duration: 8,
                    source_offset: 0,
                    source_length: 8,
                    looping: false,
                })
                .unwrap();
            let source = hex_engine::Audio::new(48_000, vec![[0.75, -0.25]; 8]).unwrap();
            let mut identity = None;
            assert_eq!(
                loan.prepare_audio_candidate_on_control(token, |mut operations| {
                    assert!(operations.prepared_identity().is_none());
                    operations.copy_resident_source_on_control(asset, &source)?;
                    operations.prepare_on_control(
                        editor.project(),
                        11,
                        hex_engine::AudioSourceContext {
                            project_generation: 2,
                            editor_revision: 8,
                        },
                    )?;
                    identity = operations.prepared_identity();
                    assert!(identity.is_some());
                    let measured = allocation_counter::measure(|| {
                        for _ in 0..128 {
                            assert_eq!(operations.prepared_identity(), identity);
                        }
                    });
                    assert_eq!(
                        (
                            measured.count_total,
                            measured.bytes_total,
                            measured.count_current,
                            measured.bytes_current
                        ),
                        (0, 0, 0, 0)
                    );
                    Ok(())
                })
                .unwrap(),
                crate::AudioCandidatePreparationProgress::Ready
            );
            if cancelled {
                assert!(loan.cancel_audio_candidate_preparation(token));
                let mut entered = false;
                let result = loan.prepare_audio_candidate_on_control(token, |_| {
                    entered = true;
                    Ok(())
                });
                assert!(!entered);
                assert!(
                    result.is_err()
                        || matches!(
                            result,
                            Ok(crate::AudioCandidatePreparationProgress::Retiring)
                        )
                );
                assert!(loan.stage_audio_candidate_preparation(token, 2).is_err());
                assert_eq!(loan.submitted_revision(), 0);
            } else {
                loan.stage_audio_candidate_preparation(token, 2).unwrap();
                client.command(PlaybackCommand::Play).unwrap();
                let mut output = [[0.0; 2]; 4];
                let measured = allocation_counter::measure(|| {
                    data.owner_mut().unwrap().run(|callback, _| {
                        callback.begin_block();
                        callback.render(&mut output).unwrap();
                    })
                });
                assert_eq!(output, [[0.75, -0.25]; 4]);
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                assert_eq!(loan.status().applied_revision.load(Acquire), 1);
            }
        }
        worker.close();
        client.close();
        drop(data);
        drop(errors);
        // Headless fixture only: follow the existing explicit backend-completion
        // witness rather than bypassing final-active release authority.
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        let _ = crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(backend.is_none() && disposed);
        finish(&mut retirement);
        assert!(worker.control().is_none());
    }
}
