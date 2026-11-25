//! Control-side audio status capture adapter over [`hex_ui_model::refresh_state`].
use hex_audio::prepared::Status;
pub(super) use hex_ui_model::refresh_state::{AudioFrame, RedrawGate};
use std::sync::atomic::Ordering::{Acquire, Relaxed};

/// One control-side observation of the actual callback status. Channel peaks
/// remain independent relaxed loads, paired with callback-latched between-poll
/// peak maxima and clip flags drained atomically from `Status`.
pub(super) fn capture_audio_frame(status: &Status) -> AudioFrame {
    let (latched_peaks, latched_clip) = status.take_latched_output_peaks();
    AudioFrame {
        source: 0,
        cpu: super::cpu_meter::sample_from_audio(status.cpu_meter.snapshot()),
        playing: status.playing.load(Relaxed),
        position: status.position.load(Relaxed),
        peak_left: status.peak_left.load(Relaxed),
        peak_right: status.peak_right.load(Relaxed),
        latched_peak_left: latched_peaks[0].to_bits(),
        latched_peak_right: latched_peaks[1].to_bits(),
        latched_clip,
        applied_revision: status.applied_revision.load(Relaxed),
        deadline_misses: status.deadline_misses.load(Relaxed),
        stream_starved: status.stream_starved_callbacks.load(Relaxed),
        callback_us: status.callback_max_ns.load(Relaxed) / 1_000,
        render_fault: status.render_fault.load(Relaxed),
        device_fault: status.device_fault.load(Relaxed),
        shutdown_complete: status.shutdown_complete.load(Acquire),
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod meter_snapshot_tests {
    use super::*;
    use crate::output_meter::MeterInput;
    use hex_ui_model::refresh_state::DIAGNOSTIC_REFRESH;
    use std::time::Instant;

    #[test]
    fn output_meter_capture_preserves_actual_status_bits_and_unknown_age() {
        let status = Status::default();
        status.playing.store(true, Relaxed);
        for (left, right, text) in [
            (0.0, 0.000_001, "last l -inf r <-60 dbfs"),
            (0.001, 0.1, "last l -60.0 r -20.0 dbfs"),
            (1.0, 2.0, "last l 0.0 r +6.0! dbfs"),
            (-1.0, f32::INFINITY, "last l invalid r invalid dbfs"),
            (f32::NAN, 0.5, "last l invalid r -6.0 dbfs"),
        ] {
            status.peak_left.store(left.to_bits(), Relaxed);
            status.peak_right.store(right.to_bits(), Relaxed);
            let snapshot = capture_audio_frame(&status);
            assert_eq!(snapshot.peak_left, left.to_bits());
            assert_eq!(snapshot.peak_right, right.to_bits());
            assert_eq!(crate::output_meter::readout(snapshot.meter_input()), text);
            assert!(
                crate::output_meter::description(snapshot.meter_input()).contains("age unknown")
            );
            // A producer update cannot silently alter an already cached frame.
            status.peak_right.store(0, Relaxed);
            assert_eq!(crate::output_meter::readout(snapshot.meter_input()), text);
        }
    }

    #[test]
    fn output_meter_stop_fault_shutdown_and_redraw_do_not_invent_freshness() {
        let status = Status::default();
        let start = Instant::now();
        let mut gate = RedrawGate::default();
        let stopped = capture_audio_frame(&status);
        assert_eq!(stopped.meter_input(), MeterInput::Stopped);
        gate.should_redraw(false, Some(stopped), start);
        status.playing.store(true, Relaxed);
        status.peak_left.store(2.0_f32.to_bits(), Relaxed);
        let overload = capture_audio_frame(&status);
        assert!(gate.should_redraw(false, Some(overload), start));
        assert!(!gate.should_redraw(false, Some(overload), start + DIAGNOSTIC_REFRESH * 10));
        assert_eq!(
            crate::output_meter::readout(overload.meter_input()),
            "last l +6.0! r -inf dbfs"
        );
        status.peak_left.store(0, Relaxed);
        let quiet = capture_audio_frame(&status);
        assert!(gate.should_redraw(false, Some(quiet), start));
        assert_eq!(
            crate::output_meter::readout(quiet.meter_input()),
            "last l -inf r -inf dbfs"
        );
        for fault in [&status.device_fault, &status.render_fault] {
            fault.store(true, Relaxed);
            let failed = capture_audio_frame(&status);
            assert_eq!(failed.meter_input(), MeterInput::Fault);
            assert!(gate.should_redraw(false, Some(failed), start));
            fault.store(false, Relaxed);
            assert!(gate.should_redraw(false, Some(capture_audio_frame(&status)), start));
        }
        status
            .shutdown_complete
            .store(true, std::sync::atomic::Ordering::Release);
        let offline = capture_audio_frame(&status);
        assert_eq!(offline.meter_input(), MeterInput::Offline);
        assert!(gate.should_redraw(false, Some(offline), start));
        assert!(gate.should_redraw(false, None, start));
        assert!(!gate.should_redraw(false, None, start));
    }
}
