//! Pure editor state tests; no GPUI window or service is created.

#[path = "../src/editor/cpu_meter.rs"]
mod cpu_meter_state;
#[path = "../src/editor/disk_overload.rs"]
mod disk_overload_state;
#[path = "../src/editor/pending_edit.rs"]
mod receipt;
#[path = "../src/editor/viewport.rs"]
mod viewport_state;

mod editor {
    mod pending_edit {
        use crate::receipt::*;

        mod tests {
            use super::*;

            #[test]
            fn queued_and_earlier_events_cannot_retire_released_display() {
                let mut receipt = Receipt::new(7);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                assert!(!receipt.submitted(0));
                assert!(receipt.submitted(2));
                assert!(!receipt.submitted(1));
                assert_eq!(receipt.observe(7, false, false), Disposition::Waiting);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                // Success/refusal and revision equality are deliberately not inputs:
                // the same exact terminal releases paint for either canonical outcome.
                assert_eq!(receipt.observe(7, false, true), Disposition::Terminal);
                assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
                assert!(!receipt.submitted(1));
            }

            #[test]
            fn project_replacement_invalidates_both_queued_and_submitted_receipts() {
                for submitted in [false, true] {
                    for replaced in [false, true] {
                        let mut receipt = Receipt::new(7);
                        if submitted {
                            assert!(receipt.submitted(3));
                        }
                        assert_eq!(
                            receipt.observe(if replaced { 7 } else { 8 }, replaced, false),
                            Disposition::Invalidated
                        );
                        assert!(!receipt.submitted(1));
                    }
                }
            }
        }
    }
    mod viewport {
        use crate::viewport_state::*;

        mod tests {
            use super::Viewport;

            #[test]
            fn fit_and_end_are_checked() {
                assert_eq!(
                    Viewport::fit(100),
                    Viewport {
                        start: 0,
                        length: 100
                    }
                );
                assert_eq!(
                    Viewport {
                        start: u64::MAX,
                        length: 1
                    }
                    .end(),
                    None
                );
            }

            #[test]
            fn zoom_anchors_at_playhead_or_center_and_clamps() {
                let viewport = Viewport {
                    start: 100,
                    length: 400,
                };
                assert_eq!(
                    viewport.zoom(1_000, 200, true),
                    Viewport {
                        start: 150,
                        length: 200
                    }
                );
                assert_eq!(
                    viewport.zoom(1_000, 900, true),
                    Viewport {
                        start: 200,
                        length: 200
                    }
                );
                assert_eq!(
                    Viewport {
                        start: 900,
                        length: 100
                    }
                    .zoom(2_000, 950, false),
                    Viewport {
                        start: 850,
                        length: 200
                    }
                );
            }

            #[test]
            fn pan_moves_by_half_and_stays_inside_horizon() {
                let viewport = Viewport {
                    start: 100,
                    length: 200,
                };
                assert_eq!(
                    viewport.pan_half(1_000, true),
                    Viewport {
                        start: 200,
                        length: 200
                    }
                );
                assert_eq!(
                    viewport.pan_half(1_000, false),
                    Viewport {
                        start: 0,
                        length: 200
                    }
                );
                assert_eq!(
                    Viewport {
                        start: 990,
                        length: 30
                    }
                    .pan_half(1_000, true),
                    Viewport {
                        start: 970,
                        length: 30
                    }
                );
            }

            #[test]
            fn zoom_and_pan_handle_large_horizons_without_overflow() {
                let viewport = Viewport {
                    start: u64::MAX / 2,
                    length: u64::MAX / 4,
                };
                assert!(viewport.zoom(u64::MAX, u64::MAX / 2, false).end().is_some());
                assert!(viewport.pan_half(u64::MAX, true).end().is_some());
                assert!(viewport.zoom(10, 5, false).end().is_some());
            }

            #[test]
            fn wheel_zoom_keeps_pointer_frame_at_the_same_fraction_and_clamps() {
                let viewport = Viewport {
                    start: 100,
                    length: 800,
                };
                let zoomed = viewport.zoom_at(2_000, 300, true);
                assert_eq!(zoomed.length, 640);
                let old_fraction = (300 - viewport.start) as f64 / viewport.length as f64;
                let new_fraction = (300 - zoomed.start) as f64 / zoomed.length as f64;
                assert!((old_fraction - new_fraction).abs() < 0.002);
                assert_eq!(zoomed.zoom_at(2_000, 300, false), viewport);
                assert_eq!(
                    Viewport {
                        start: 1_800,
                        length: 200
                    }
                    .zoom_at(2_000, 1_999, false),
                    Viewport {
                        start: 1_750,
                        length: 250
                    }
                );
            }

            #[test]
            fn frame_pan_preserves_scale_and_respects_both_horizon_edges() {
                let viewport = Viewport {
                    start: 100,
                    length: 200,
                };
                assert_eq!(
                    viewport.pan_by_frames(1_000, 50),
                    Viewport {
                        start: 150,
                        length: 200
                    }
                );
                assert_eq!(
                    viewport.pan_by_frames(1_000, i128::MAX),
                    Viewport {
                        start: 800,
                        length: 200
                    }
                );
                assert_eq!(
                    viewport.pan_by_frames(1_000, i128::MIN),
                    Viewport {
                        start: 0,
                        length: 200
                    }
                );
            }

            #[test]
            fn fraction_rejects_outside_empty_and_overflowing_ranges() {
                let viewport = Viewport {
                    start: 100,
                    length: 200,
                };
                assert_eq!(viewport.fraction(100), Some(0.0));
                assert_eq!(viewport.fraction(200), Some(0.5));
                assert_eq!(viewport.fraction(300), Some(1.0));
                assert_eq!(viewport.fraction(99), None);
                assert_eq!(viewport.fraction(301), None);
                assert_eq!(Viewport::fit(0).fraction(0), None);
                assert_eq!(
                    Viewport {
                        start: u64::MAX,
                        length: 1
                    }
                    .fraction(u64::MAX),
                    None
                );
            }
        }
    }
    mod disk_overload {
        use crate::disk_overload_state::*;
        mod tests {
            use super::{DiskOverload, HOLD, description};
            use std::time::{Duration, Instant};

            #[test]
            fn rise_lights_and_decays_after_hold() {
                let start = Instant::now();
                let mut state = DiskOverload::default();
                assert!(!state.observe(Some((1, 0)), start));
                assert!(!state.lit(start));
                assert!(state.observe(Some((1, 2)), start));
                assert!(state.lit(start));
                assert_eq!(state.count(), 2);
                // A later rise extends the hold from that rise.
                let later = start + Duration::from_secs(2);
                assert!(state.observe(Some((1, 5)), later));
                assert!(!state.observe(Some((1, 5)), start + HOLD));
                assert!(state.lit(start + HOLD));
                assert!(state.observe(Some((1, 5)), later + HOLD));
                assert!(!state.lit(later + HOLD));
                assert!(!state.observe(Some((1, 5)), later + HOLD * 2));
            }

            #[test]
            fn first_sight_and_output_replacement_only_rebaseline() {
                let start = Instant::now();
                let mut state = DiskOverload::default();
                assert!(!state.observe(Some((1, 9)), start));
                assert!(!state.lit(start));
                assert!(state.observe(Some((1, 10)), start));
                // Output closed: indicator clears immediately.
                assert!(state.observe(None, start));
                assert!(!state.lit(start));
                assert!(!state.observe(Some((2, 4)), start));
                assert!(state.observe(Some((2, 5)), start));
                assert!(state.observe(Some((2, 0)), start));
                assert!(!state.lit(start));
                assert!(state.observe(Some((2, 1)), start));
                assert!(state.lit(start));
            }

            #[test]
            fn description_is_lowercase() {
                let text = description(3);
                assert_eq!(text, text.to_lowercase());
                assert!(text.contains("3 callbacks"));
            }

            #[test]
            fn shown_tracks_the_displayed_indicator() {
                let now = std::time::Instant::now();
                let mut state = DiskOverload::default();
                assert!(!state.shown());
                state.observe(Some((1, 0)), now);
                state.observe(Some((1, 1)), now);
                assert!(state.shown());
                state.observe(None, now);
                assert!(!state.shown());
            }
        }
    }
    mod cpu_meter {
        use crate::cpu_meter_state::*;
        use hex_audio::cpu_meter::CpuSample;
        use std::time::{Duration, Instant};

        // These durations are the observable meter update/hold contract.
        const INTERVAL: Duration = Duration::from_millis(100);
        const HOLD: Duration = Duration::from_secs(3);

        mod tests {
            use super::*;
            fn sample(elapsed: u64, budget: u64, peak: u32) -> CpuSample {
                CpuSample {
                    elapsed,
                    budget,
                    peak,
                    frames: 256,
                    rate: 44_100,
                }
            }
            #[test]
            fn smoothing_throttling_peak_reset_and_overload_hold() {
                let now = Instant::now();
                let mut meter = CpuMeter::default();
                assert_eq!(meter.label(), "cpu —");
                assert!(meter.observe(Some((1, sample(10, 100, 1000), 0)), now));
                assert_eq!(meter.label(), "cpu 10%");
                assert!((meter.fraction() - 0.1).abs() < f32::EPSILON);
                assert!(!meter.observe(
                    Some((1, sample(60, 200, 5000), 0)),
                    now + Duration::from_millis(20)
                ));
                assert!(meter.observe(Some((1, sample(60, 200, 5000), 1)), now + INTERVAL));
                assert_eq!(meter.label(), "cpu 22%");
                assert!(meter.warning());
                assert!(meter.description().contains("1 callbacks over budget"));
                assert!(meter.observe(Some((1, sample(110, 300, 5000), 1)), now + INTERVAL + HOLD));
                assert!(!meter.warning());
                meter.reset(1);
                assert!(meter.description().contains("0 callbacks over budget"));
                assert!(meter.observe(None, now + INTERVAL + HOLD));
                assert_eq!(meter.label(), "cpu —");
            }
            #[test]
            fn replacement_output_resets_the_old_peak_and_miss_count() {
                let now = Instant::now();
                let mut meter = CpuMeter::default();
                meter.observe(Some((1, sample(100, 100, 10_000), 3)), now);
                assert!(meter.description().contains("3 callbacks over budget"));
                meter.observe(Some((1, sample(200, 200, 10_000), 4)), now + INTERVAL);
                meter.observe(Some((2, sample(5, 100, 500), 0)), now + INTERVAL * 2);
                assert_eq!(meter.label(), "cpu 5%");
                assert!(!meter.warning());
                assert!(meter.description().contains("0 callbacks over budget"));
            }
        }
    }
}
