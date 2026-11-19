use super::*;
use crate::arrangement_seek::{ArrangementSeekSpec, ArrangementSeekWorker, MAX_SEEK_WORK_FRAMES};

#[test]
fn native_intent_refuses_mislabeled_or_stale_worker_results() {
    let graph = crate::Session::new(48_000, 8, 0)
        .unwrap()
        .compile_prepared_revision(None)
        .unwrap();
    let worker = ArrangementSeekWorker::new().unwrap();
    worker
        .submit(ArrangementSeekSpec {
            mode: ArrangementSeekMode::HistoryPreservingInteractive,
            generation: 7,
            revision: 17,
            prepared_revision: Arc::clone(&graph),
            destination_frame: 17,
            work_budget_frames: MAX_SEEK_WORK_FRAMES,
            start_playing: false,
        })
        .unwrap();
    let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
    let mut result = loop {
        if let Some(result) = worker.try_result() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    let intent = ResidentArrangementSeekIntent {
        request_id: 99, // Native and internal worker request IDs are different namespaces.
        project_generation: 7,
        project_revision: 17,
        destination_frame: 17,
        start_playing: false,
        history: ResidentSeekHistory::Preserve,
        section: None,
        locator_quantization: None,
        fence: ResidentFreshRuntimeFence {
            output_identity: 1,
            active_audio_revision: 1,
            route_generation: 0,
            admitted_command_ticket: 0,
            output_fault_epoch: 0,
            consumed_stop_generation: 0,
        },
        device_preview: None,
        tuning_preview: None,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    assert!(result_matches_intent(&result, &intent, &graph));
    result.mode = ArrangementSeekMode::Interactive;
    assert!(!result_matches_intent(&result, &intent, &graph));
    result.mode = ArrangementSeekMode::HistoryPreservingInteractive;
    result.destination_frame += 1;
    assert!(!result_matches_intent(&result, &intent, &graph));
    result.destination_frame -= 1;
    result.start_playing = true;
    assert!(!result_matches_intent(&result, &intent, &graph));
    result.start_playing = false;
    let other = crate::Session::new(48_000, 8, 0)
        .unwrap()
        .compile_prepared_revision(None)
        .unwrap();
    assert!(
        !result_matches_intent(&result, &intent, &other),
        "equal-looking graphs do not share custody"
    );
    assert!(result_matches_intent(&result, &intent, &graph));
    worker.shutdown_and_join_on_control();
}
