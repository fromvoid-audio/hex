//! Capture-side allocation regression; worker construction and join stay outside scope.

use allocation_counter::measure;
use hex_audio::spectrum_worker::{
    CaptureOutcome, SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker,
};
use std::hint::black_box;

#[test]
fn bounded_spectrum_capture_push_is_allocation_and_deallocation_free() {
    let stamp = SpectrumStamp {
        target: SpectrumTargetId(1),
        generation: 1,
        sample_rate_hz: 48_000,
        applied_revision: 1,
    };
    let (mut capture, _reader, reaper) = spawn_spectrum_worker(stamp).expect("valid test worker");
    let frames = [[0.25, -0.25]; 256];
    let mut outcome = None;
    let measured = measure(|| {
        outcome = Some(black_box(capture.submit(stamp, black_box(&frames))));
    });

    assert_eq!(measured.count_total, 0, "capture push allocated");
    assert_eq!(measured.bytes_total, 0, "capture push allocated bytes");
    assert_eq!(
        measured.count_current, 0,
        "capture push changed net allocation count"
    );
    assert_eq!(
        measured.bytes_current, 0,
        "capture push changed net allocation bytes"
    );

    drop(capture);
    reaper
        .shutdown_and_join()
        .expect("worker join after callback-side capture reclamation");
    assert_eq!(outcome, Some(CaptureOutcome::Queued { sequence: 0 }));
}
