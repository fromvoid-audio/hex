//! Pure viewport state tests; no GPUI window or service is created.

#[path = "../src/editor/viewport.rs"]
mod viewport_state;

mod editor {
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
}
