//! Pure editor state tests; no GPUI window or service is created.

// Other audio-admission types are exercised by the UI library, not this target.
#[allow(dead_code)]
#[path = "../src/editor/audio_admission.rs"]
mod audio_admission_state;
#[path = "../src/editor/cpu_meter.rs"]
mod cpu_meter_state;
#[path = "../src/editor/disk_overload.rs"]
mod disk_overload_state;
// Render-only helpers are used by the library, not this geometry-only target.
#[allow(dead_code)]
#[path = "../src/editor/midi_thumbnail.rs"]
mod midi_thumbnail_state;
#[path = "../src/editor/navigation_profile/selection.rs"]
mod navigation_selection_state;
#[path = "../src/editor/play_intent.rs"]
mod play_intent_state;
#[path = "../src/editor/pending_edit.rs"]
mod receipt;
// Runtime-only methods remain in the library consumer.
#[allow(dead_code)]
#[path = "../src/editor/recovery_state.rs"]
mod recovery_state;
#[path = "../src/editor/track_meters/scale.rs"]
mod track_meter_scale_state;
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
    mod midi_thumbnail {
        use crate::midi_thumbnail_state::*;

        mod tests {
            use super::*;

            #[test]
            fn malformed_display_geometry_never_reaches_the_scene() {
                let bounds = gpui::Bounds::new(
                    gpui::point(gpui::px(1.0), gpui::px(2.0)),
                    gpui::size(gpui::px(100.0), gpui::px(50.0)),
                );
                let full = PitchFit::from_pitches([0, 127]);
                assert!(paint_bounds(bounds, 0.1, 0.2, 60, full, 2.0).is_some());
                for (x, width, pitch, scale) in [
                    (f32::NAN, 0.2, 60, 2.0),
                    (0.1, f32::INFINITY, 60, 2.0),
                    (0.9, 0.2, 60, 2.0),
                    (-0.1, 0.2, 60, 2.0),
                    (0.1, 0.0, 60, 2.0),
                    (0.1, 0.2, 128, 2.0),
                    (0.1, 0.2, 60, 0.0),
                    (0.1, 0.2, 60, f32::NAN),
                ] {
                    assert!(paint_bounds(bounds, x, width, pitch, full, scale).is_none());
                }
            }

            #[test]
            fn pitch_fit_zooms_rows_to_the_used_range() {
                let height = |fit: PitchFit, pitch| note_rows(fit, pitch, 30.0).unwrap().1;
                // A single pitch fills a centred third of the body.
                let single = PitchFit::from_pitches([60, 60]);
                assert_eq!(note_rows(single, 60, 30.0), Some((10.0, 9.0)));
                assert_eq!(note_rows(single, 72, 30.0), None);
                // An octave is thinner; the full keyboard bottoms out at 1px.
                let octave = PitchFit::from_pitches([60, 72]);
                assert!(height(octave, 60) < height(single, 60));
                assert_eq!(note_rows(octave, 72, 26.0).map(|(top, _)| top), Some(0.0));
                assert_eq!(height(PitchFit::from_pitches([0, 127]), 64), 1.0);
                // Highest pitch on top, lowest at the bottom, short ranges stay inside MIDI.
                let (top, row) = note_rows(octave, 60, 26.0).unwrap();
                assert!((top + row - 26.0).abs() < 1e-3);
                for edge in [0, 127] {
                    let fit = PitchFit::from_pitches([edge]);
                    assert!(note_rows(fit, edge, 30.0).is_some());
                }
                // No notes keeps the full-range scale.
                assert_eq!(PitchFit::from_pitches([]), PitchFit::from_pitches([0, 127]));
            }

            #[test]
            fn cropped_view_uses_visible_clip_coordinates_and_excludes_hidden_notes() {
                assert_eq!(note_shape(90, 120, 60, 100, 200), Some((0.0, 0.2, 60)));
                assert_eq!(note_shape(175, 225, 62, 100, 200), Some((0.75, 0.25, 62)));
                assert_eq!(note_shape(0, 100, 60, 100, 200), None);
                assert_eq!(note_shape(200, 220, 60, 100, 200), None);
            }

            #[test]
            fn tiny_samples_at_large_endpoints_stay_positive_and_inside_the_clip() {
                let (x, width, _) = note_shape(u64::MAX - 1, u64::MAX, 127, 0, u64::MAX).unwrap();
                assert!(x.is_finite() && width.is_finite());
                assert!((0.0..1.0).contains(&x) && width > 0.0 && x + width <= 1.0);
                assert_eq!(note_shape(10, 10, 60, 0, 20), None);
                assert_eq!(note_shape(0, 10, 128, 0, 20), None);
                assert_eq!(note_shape(0, 10, 60, 10, 10), None);
            }
        }
    }
    mod track_meters {
        mod scale {
            use crate::track_meter_scale_state::*;

            mod tests {
                use super::*;

                #[test]
                fn silence_floor_unity_and_headroom_use_the_display_scale() {
                    for (amplitude, expected) in [
                        (0.0, 0.0),
                        (FLOOR_AMPLITUDE, 0.0),
                        (0.001, 0.0565),
                        (10_f32.powf(-48.0 / 20.0), 0.1713),
                        (10_f32.powf(-30.0 / 20.0), 0.4009),
                        (10_f32.powf(-24.0 / 20.0), 0.5),
                        (10_f32.powf(-12.0 / 20.0), 0.6991),
                        (1.0, 0.8972),
                        (10_f32.powf(6.0 / 20.0), 1.0),
                        (100.0, 1.0),
                    ] {
                        assert!(
                            (fraction(amplitude) - expected).abs() < 0.0001,
                            "amplitude={amplitude}, expected={expected}, actual={}",
                            fraction(amplitude)
                        );
                    }
                    assert_eq!(readout(0.0), "−∞ dbfs");
                    assert_eq!(readout(FLOOR_AMPLITUDE / 2.0), "<−70 dbfs");
                    assert_eq!(readout(FLOOR_AMPLITUDE), "-70.0 dbfs");
                    assert_eq!(readout(1.0), "0.0 dbfs");
                    assert!((fraction(1.0) - UNITY_FRACTION).abs() < 0.0001);
                    assert_eq!(readout(10_f32.powf(6.0 / 20.0)), "+6.0 dbfs");
                    assert_eq!(readout(100.0), "+40.0 dbfs");
                    // An intermediate value stays between the adjacent labeled ticks.
                    assert!((0.5..0.5991).contains(&fraction(10_f32.powf(-21.0 / 20.0))));
                }

                #[test]
                fn malformed_and_extreme_inputs_have_bounded_display_geometry() {
                    for amplitude in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
                        assert_eq!(fraction(amplitude), 0.0);
                        assert_eq!(readout(amplitude), "invalid");
                    }
                    assert_eq!(fraction(f32::MAX), 1.0);
                    assert_eq!(fraction(f32::MIN_POSITIVE), 0.0);
                    assert_eq!(readout(-0.0), "−∞ dbfs");
                }
            }
        }
    }
    mod audio_admission {
        use crate::audio_admission_state::*;
        mod tests {
            use super::{AdmissionState, AudioAdmissionStatus, ProjectRevision};

            fn project(generation: u64, revision: u64) -> ProjectRevision {
                ProjectRevision {
                    generation,
                    revision,
                }
            }

            #[test]
            fn changed_media_rejection_reaches_the_admission_description() {
                let current = project(2, 17);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(current);
                status.published(current, 31);
                let reason = hex_engine::PlaybackHandoffRejection::StreamSourceChanged.message();
                assert!(status.rejected(current, 31, reason));
                assert!(status.description().contains("original media changed"));
                assert!(status.description().contains("reopen or relink"));
            }

            #[test]
            fn current_revision_transitions_pending_applied_and_rejected() {
                let current = project(2, 17);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(current);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);

                // Queue/full or deferred callback admission has no terminal event.
                status.published(current, 31);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
                assert_eq!(status.current().unwrap().audio_revision, Some(31));
                assert_eq!(status.last_audible(), None);

                let reason =
                    hex_engine::PlaybackHandoffRejection::SourceHistoryUnavailable.message();
                assert!(status.rejected(current, 31, reason));
                assert_eq!(
                    status.current().unwrap().state,
                    AdmissionState::Rejected(reason)
                );
                assert!(status.description().contains("previous audio continues"));
                assert!(status.description().contains("audio revision 31"));
                assert!(status.description().contains("rejected:"));

                // A later admission of the same canonical revision can recover it.
                status.published(current, 32);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
                status.applied(current, 32);
                assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
                assert_eq!(status.last_audible().unwrap().audio_revision, 32);
            }

            #[test]
            fn stale_rejection_cannot_clobber_newer_pending_or_replacement_revision() {
                let old = project(4, 20);
                let current = project(4, 21);
                let replacement = project(5, 0);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(old);
                status.published(old, 50);
                status.canonical_revision(current);
                status.published(current, 51);
                assert!(!status.rejected(old, 50, "old revision rejected"));
                assert_eq!(status.current().unwrap().project, current);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);

                status.canonical_revision(replacement);
                status.published(replacement, 52);
                assert!(!status.rejected(current, 51, "old project rejected"));
                assert_eq!(status.current().unwrap().project, replacement);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
            }

            #[test]
            fn older_rejection_does_not_replace_newer_published_candidate() {
                let current = project(3, 8);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(current);
                status.published(current, 10);
                status.published(current, 11);
                assert!(!status.rejected(current, 10, "coalesced stale rejection"));
                status.applied(current, 11);
                assert_eq!(status.current().unwrap().audio_revision, Some(11));
                assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
                assert_eq!(status.last_audible().unwrap().audio_revision, 11);
            }

            #[test]
            fn stale_applied_candidate_does_not_replace_current_or_latest_audibility() {
                let old = project(1, 9);
                let current = project(1, 10);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(old);
                status.published(old, 3);
                status.canonical_revision(current);
                status.published(current, 4);
                status.applied(old, 3);
                assert_eq!(status.current().unwrap().project, current);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
                assert_eq!(status.last_audible().unwrap().project, old);
                assert!(status.label().contains("rev 9 audible"));
                assert!(
                    status
                        .description()
                        .contains("project generation 1 revision 9")
                );
                status.applied(current, 4);
                status.applied(old, 3);
                assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
                assert_eq!(status.last_audible().unwrap().project, current);
            }

            #[test]
            fn audio_reset_keeps_canonical_identity_pending_and_clears_old_audibility() {
                let current = project(7, 40);
                let mut status = AudioAdmissionStatus::default();
                status.canonical_revision(current);
                status.published(current, 2);
                status.applied(current, 2);
                status.reset_audio();
                assert_eq!(status.current().unwrap().project, current);
                assert_eq!(status.current().unwrap().audio_revision, None);
                assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
                assert_eq!(status.last_audible(), None);
            }
        }
        mod comparison_tests {
            use super::*;
            #[test]
            fn comparison_correlation_keeps_pending_rejected_and_real_audio_revision() {
                let old = ProjectRevision {
                    generation: 3,
                    revision: 7,
                };
                let next = ProjectRevision {
                    generation: 3,
                    revision: 8,
                };
                for rejected in [false, true] {
                    let mut state = AudioAdmissionStatus::default();
                    state.canonical_revision(old);
                    state.published(old, 91);
                    if rejected {
                        state.rejected(old, 91, "refused");
                    }
                    state.comparison_revision(old, next, false);
                    assert_eq!(state.current().unwrap().project, next);
                    assert_eq!(state.current().unwrap().audio_revision, Some(91));
                    assert_eq!(
                        state.current().unwrap().state,
                        if rejected {
                            AdmissionState::Rejected("refused")
                        } else {
                            AdmissionState::Pending
                        }
                    );
                    assert!(state.last_audible().is_none());
                }
                let mut state = AudioAdmissionStatus::default();
                state.canonical_revision(old);
                state.applied(old, 91);
                state.comparison_revision(old, next, false);
                assert_eq!(
                    state.current().unwrap().project,
                    old,
                    "no actual candidate/output association means no applied promotion"
                );
                state.comparison_revision(old, next, true);
                state.comparison_revision(old, next, true);
                assert_eq!(state.current().unwrap().project, next);
                assert_eq!(state.last_audible().unwrap().project, next);
                assert_eq!(state.current().unwrap().audio_revision, Some(91));
            }
        }
    }
    mod clip_content_cache {
        mod profile {
            mod navigation {
                mod selection {
                    use crate::navigation_selection_state::*;
                    mod tests {
                        use super::*;

                        #[test]
                        fn comparison_retains_default_and_balances_reference_current_trials() {
                            assert_eq!(comparison_trials(None).unwrap(), &[false]);
                            assert_eq!(comparison_trials(Some("0")).unwrap(), &[false]);
                            assert_eq!(
                                comparison_trials(Some("1")).unwrap(),
                                &[true, false, false, true]
                            );
                            for value in ["", " ", "true", "2", "1 "] {
                                assert!(comparison_trials(Some(value)).is_err());
                            }
                            let (rounds, warmup) = iterations(false);
                            assert_eq!(
                                comparison_trials(Some("1")).unwrap().len() * 2 * rounds,
                                384
                            );
                            assert_eq!((rounds, warmup), (48, 8));
                        }

                        #[test]
                        fn default_keeps_complete_generated_and_project_case_inventories() {
                            let full = Selection::parse(None).unwrap();
                            assert_eq!(full, Selection::parse(Some("all")).unwrap());
                            let mut generated = Vec::new();
                            for content in ["mixed", "audio"] {
                                for (tracks, rows) in [(1, &[false][..]), (100, &[false, true][..])]
                                {
                                    for &row in rows {
                                        let cases = full.cases(content, tracks, row);
                                        assert_eq!(
                                            cases
                                                .iter()
                                                .map(|c| (c.density, c.action))
                                                .collect::<Vec<_>>(),
                                            [
                                                ("dense", "pan"),
                                                ("dense", "zoom"),
                                                ("zoomed", "pan"),
                                                ("zoomed", "zoom")
                                            ]
                                        );
                                        generated.extend(cases);
                                    }
                                }
                            }
                            assert_eq!(generated.len(), 24);
                            assert_eq!(
                                [false, true]
                                    .into_iter()
                                    .map(|row| full.cases("project", 71, row).len())
                                    .sum::<usize>(),
                                8
                            );
                            assert_eq!(iterations(false), (48, 8));
                            assert_eq!(iterations(false).0 - iterations(false).1, 40);
                            assert_eq!(iterations(true), (4, 1));
                        }

                        #[test]
                        fn bounded_scopes_select_only_their_normal_dense_pan_and_zoom() {
                            for (scope, content, tracks) in [
                                ("audio-100-dense", "audio", 100),
                                ("project-dense", "project", 71),
                            ] {
                                let selection = Selection::parse(Some(scope)).unwrap();
                                assert_eq!(selection.label(), scope);
                                assert_eq!(
                                    selection.cases(content, tracks, false),
                                    [
                                        Case {
                                            density: "dense",
                                            divisor: 1,
                                            action: "pan"
                                        },
                                        Case {
                                            density: "dense",
                                            divisor: 1,
                                            action: "zoom"
                                        },
                                    ]
                                );
                                assert!(selection.cases(content, tracks, true).is_empty());
                                assert!(selection.cases("mixed", tracks, false).is_empty());
                            }
                            assert!(Selection::Audio100Dense.cases("audio", 1, false).is_empty());
                            assert!(
                                Selection::Audio100Dense
                                    .cases("project", 100, false)
                                    .is_empty()
                            );
                            assert!(
                                Selection::ProjectDense
                                    .cases("audio", 100, false)
                                    .is_empty()
                            );
                        }

                        #[test]
                        fn invalid_or_incompatible_selections_refuse_instead_of_running_nothing() {
                            for value in ["", " ", "audio", "all,project-dense", "project-dense "] {
                                assert!(Selection::parse(Some(value)).is_err());
                            }
                            for project in [false, true] {
                                for mixed in [false, true] {
                                    for audio in [false, true] {
                                        assert_eq!(
                                            Selection::All
                                                .validate_inputs(project, mixed, audio)
                                                .is_ok(),
                                            project || mixed
                                        );
                                        assert_eq!(
                                            Selection::Audio100Dense
                                                .validate_inputs(project, mixed, audio)
                                                .is_ok(),
                                            audio
                                        );
                                        assert_eq!(
                                            Selection::ProjectDense
                                                .validate_inputs(project, mixed, audio)
                                                .is_ok(),
                                            project
                                        );
                                    }
                                }
                            }
                            assert!(Selection::All.cases("unknown", 1, false).is_empty());
                            assert!(
                                Selection::ProjectDense
                                    .cases("project", 0, false)
                                    .is_empty()
                            );
                        }
                    }
                }
            }
        }
    }
    mod recovery_state {
        use crate::recovery_state::*;
        mod tests {
            use super::RecoveryState;

            #[test]
            fn recovery_requires_exact_success_not_a_later_terminal() {
                use hex_audio::prepared::{ActionOutcome, ActionOutcomeReceipt};
                let mut state = RecoveryState::default();
                let generation = state.begin(64).unwrap();
                assert!(state.opened(generation));
                assert!(state.restored(generation));
                state.seeking(3);
                state.observe_receipt(ActionOutcomeReceipt {
                    ticket: 4,
                    outcome: ActionOutcome::Applied,
                });
                assert!(!state.complete_exact(true, 64, false));
                state.observe_receipt(ActionOutcomeReceipt {
                    ticket: 3,
                    outcome: ActionOutcome::Rejected(
                        hex_engine::PlaybackActionRejection::OutputFailed,
                    ),
                });
                assert!(state.seek_failed());
                assert!(!state.complete_exact(true, 64, false));
            }

            #[test]
            fn prepared_recovery_requires_exact_attempt_request_and_actual_stopped_ack() {
                let mut state = RecoveryState::default();
                let generation = state.begin(129).unwrap();
                assert!(!state.permits_seek(None));
                assert!(!state.permits_seek(Some(generation)));
                state.opened(generation);
                state.restored(generation);
                assert!(state.permits_seek(Some(generation)));
                assert!(!state.permits_seek(Some(generation + 1)));
                assert!(!state.prepared_seeking(generation + 1, 7));
                assert!(state.prepared_seeking(generation, 7));
                assert!(!state.prepared_seeking(generation, 8));
                assert_eq!(state.seek_needed(true), None);
                assert!(
                    !state.complete(true, u64::MAX, 129, false),
                    "command tickets cannot apply prepared recovery"
                );
                assert!(!state.complete_prepared(true, None, 9, 129, false));
                assert!(!state.complete_prepared(true, Some((8, 9)), 9, 129, false));
                assert!(!state.complete_prepared(true, Some((7, 9)), 8, 129, false));
                assert!(!state.complete_prepared(true, Some((7, 9)), 10, 129, false));
                assert!(!state.complete_prepared(true, Some((7, 9)), 9, 128, false));
                assert!(!state.complete_prepared(true, Some((7, 9)), 9, 129, true));
                assert!(!state.complete_prepared(false, Some((7, 9)), 9, 129, false));
                state.prepared_published(8, 10);
                assert_eq!(state.prepared_revision(), None);
                state.prepared_published(7, 9);
                assert_eq!(state.prepared_revision(), Some(9));
                assert!(state.complete_prepared(true, Some((7, 9)), 9, 129, false));
                assert!(state.permits_seek(None));
                assert!(!state.permits_seek(Some(generation)));

                let next = state.begin(129).unwrap();
                state.opened(next);
                state.restored(next);
                state.prepared_seeking(next, 11);
                state.prepared_published(11, 12);
                state.stop();
                assert_eq!(state.prepared_request(), None);
                assert_eq!(state.prepared_revision(), None);
                assert_eq!(state.seek_needed(true), Some(0));
                assert!(!state.complete_prepared(true, Some((11, 12)), 12, 129, false));
                state.seeking(13);
                assert!(state.complete(true, 13, 0, false));
            }

            #[test]
            fn obsolete_completion_and_old_acknowledgments_do_not_enable_play() {
                let mut state = RecoveryState::default();
                let first = state.begin(640).expect("first recovery");
                assert!(state.begin(640).is_err());
                state.fail();
                let current = state.begin(640).expect("retry");
                assert!(!state.opened(first));
                assert!(!state.restored(first));
                assert!(state.opened(current));
                assert!(state.restored(current));
                // A huge prior stream acknowledgment is insufficient without a new seek.
                assert!(!state.complete(true, 1000, 640, false));
                assert_eq!(state.seek_needed(true), Some(640));
                state.seeking(1);
                assert!(!state.complete(false, 1, 640, false));
                assert!(!state.complete(true, 0, 640, false));
                assert!(!state.complete(true, 1, 0, false));
                assert!(!state.complete(true, 1, 640, true));
                assert!(state.complete(true, 1, 640, false));
                assert!(state.generation().is_none());
            }

            #[test]
            fn stop_supersedes_saved_cursor_and_requires_a_fresh_seek() {
                let mut state = RecoveryState::default();
                let generation = state.begin(960).expect("recovery");
                state.opened(generation);
                state.restored(generation);
                state.seeking(1);
                state.stop();
                assert_eq!(state.position(), Some(0));
                assert!(!state.complete(true, 1, 960, false));
                assert_eq!(state.seek_needed(true), Some(0));
                state.seeking(2);
                assert!(!state.complete(true, 1, 0, false));
                assert!(state.complete(true, 2, 0, false));
            }
        }
    }
    mod play_intent {
        use crate::play_intent_state::*;
        mod tests {
            use super::{AdmissionError, TokenExhausted, Tracker, prepare_admission};

            #[test]
            fn launch_then_space_tracks_pause_then_play() {
                let mut tracker = Tracker::default();
                let launch = tracker.reserve_token().expect("launch token");
                tracker.admit(launch, true);
                assert!(tracker.desired_playing(false));

                let pause = tracker.reserve_token().expect("pause token");
                tracker.admit(pause, false);
                assert!(!tracker.desired_playing(false));

                let play = tracker.reserve_token().expect("play token");
                tracker.admit(play, true);
                assert!(tracker.desired_playing(false));
            }

            #[test]
            fn direct_play_pause_order_is_latest_intent_wins() {
                let mut tracker = Tracker::default();
                let play = tracker.reserve_token().expect("play token");
                tracker.admit(play, true);
                let pause = tracker.reserve_token().expect("pause token");
                tracker.admit(pause, false);
                assert!(!tracker.desired_playing(false));
            }

            #[test]
            fn stale_launch_clear_cannot_erase_a_newer_pause() {
                let mut tracker = Tracker::default();
                let launch = tracker.reserve_token().expect("launch token");
                tracker.admit(launch, true);
                let pause = tracker.reserve_token().expect("pause token");
                tracker.admit(pause, false);
                tracker.clear_if_current(launch);
                assert!(!tracker.desired_playing(true));
            }

            #[test]
            fn earlier_ticket_does_not_resolve_a_newer_unsent_intent() {
                let mut tracker = Tracker::default();
                let launch = tracker.reserve_token().expect("launch token");
                tracker.admit(launch, true);
                tracker.attach_ticket(launch, 8);
                let pause = tracker.reserve_token().expect("pause token");
                tracker.admit(pause, false);
                tracker.attach_ticket(launch, 8);
                tracker.reconcile(8, true, None);
                assert!(!tracker.desired_playing(true));
            }

            #[test]
            fn later_receipt_does_not_resolve_an_earlier_pending_intent() {
                let mut tracker = Tracker::default();
                let token = tracker.reserve_token().unwrap();
                tracker.admit(token, true);
                tracker.attach_ticket(token, 4);
                tracker.reconcile_receipt(5, false);
                assert!(tracker.desired_playing(false));
                tracker.reconcile_receipt(4, false);
                assert!(!tracker.desired_playing(false));
            }

            #[test]
            fn consumed_or_failed_latest_ticket_resolves_to_actual_state() {
                let mut tracker = Tracker::default();
                let launch = tracker.reserve_token().expect("launch token");
                tracker.admit(launch, true);
                tracker.attach_ticket(launch, 11);
                tracker.reconcile(0, false, Some(11));
                assert!(!tracker.desired_playing(false));

                let next = tracker.reserve_token().expect("next token");
                tracker.admit(next, true);
                assert!(tracker.desired_playing(false));
            }

            #[test]
            fn full_ui_queue_keeps_existing_intent_unchanged() {
                let mut tracker = Tracker::default();
                let play = prepare_admission(&mut tracker, Some(true), 0, 32)
                    .expect("play admission")
                    .expect("play token");
                tracker.admit(play, true);
                assert_eq!(
                    prepare_admission(&mut tracker, Some(false), 32, 32),
                    Err(AdmissionError::QueueFull)
                );
                assert!(tracker.desired_playing(false));
            }

            #[test]
            fn priority_stop_and_lifecycle_clear_do_not_depend_on_tokens() {
                let mut tracker = Tracker::default();
                let play = tracker.reserve_token().expect("play token");
                tracker.admit(play, true);
                tracker.priority_stop();
                assert!(!tracker.desired_playing(true));
                tracker.reconcile(0, false, None);
                assert!(!tracker.desired_playing(false));

                let next = tracker.reserve_token().expect("next token");
                tracker.admit(next, true);
                tracker.clear();
                assert!(!tracker.desired_playing(false));
            }

            #[test]
            fn token_exhaustion_is_checked_without_aliasing() {
                let mut tracker = Tracker::starting_at(u64::MAX);
                assert_eq!(tracker.reserve_token(), Err(TokenExhausted));
                assert_eq!(tracker.reserve_token(), Err(TokenExhausted));
                assert!(!tracker.desired_playing(false));
                assert!(tracker.desired_playing(true));
                tracker.priority_stop();
                assert!(!tracker.desired_playing(true));
                let first = Tracker::default().reserve_token().unwrap();
                let last = Tracker::starting_at(u64::MAX - 1).reserve_token().unwrap();
                assert_ne!(first, last);
            }
        }
    }
}
