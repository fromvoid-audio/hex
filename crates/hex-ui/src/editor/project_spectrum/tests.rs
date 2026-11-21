use super::*;
use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, size};
use hex_project::{Command, Created, Editor, ElementColor, Project};
use hex_session::service::ViewState;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn publish(view: &mut EditorView, project: Project) {
    let old = view.state.as_ref().unwrap();
    view.state = Some(Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(project),
        waveforms: old.waveforms.clone(),
        source_overviews: old.source_overviews.clone(),
        audio_clip_waveforms: old.audio_clip_waveforms.clone(),
        midi_preview: Arc::clone(&old.midi_preview),
        gain_preview: old.gain_preview.clone(),
        timeline_frames: old.timeline_frames,
        can_undo: old.can_undo,
        can_redo: old.can_redo,
        revision: old.revision + 1,
        unsaved: old.unsaved,
        document_path: old.document_path.clone(),
    }));
}
fn result(view: &EditorView, now: Instant) -> PublishedProjectSpectrum {
    let ids = view.project_spectrum_ids();
    let mut selection =
        hex_audio::spectrum_worker::ProjectSelection::new(&ids, view.project_generation, 1, 48_000)
            .unwrap();
    selection.token = 2;
    PublishedProjectSpectrum {
        selection,
        applied_revision: 1,
        captured_at: now,
        processed_at: now,
        frames: (0..hex_audio::spectrum_worker::MAX_PROJECT_LAYERS)
            .map(|i| hex_audio::spectrum::SpectrumFrame {
                sample_rate_hz: 48_000,
                bins_dbfs: [-6.0 - i as f32 * 0.25; hex_audio::spectrum::SPECTRUM_BANDS],
                peak_dbfs: [-6.0 - i as f32 * 0.25; hex_audio::spectrum::SPECTRUM_BANDS],
            })
            .collect(),
    }
}
#[gpui::test]
fn project_spectrum_layers_preserve_all_members_colors_and_clear_replaced_context(
    cx: &mut TestAppContext,
) {
    for count in [0, 1, 2, 256] {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for i in 0..count {
            assert!(matches!(
                model
                    .execute(Command::AddTrack {
                        name: format!("track {i}")
                    })
                    .unwrap(),
                Created::Track(_)
            ));
        }
        if let Some(track) = model.project().tracks().first().map(|track| track.id()) {
            model
                .execute(Command::SetTrackColor {
                    track,
                    color: ElementColor::Orange,
                })
                .unwrap();
        }
        let project = model.project().clone();
        let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
        let _visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |view, _| {
            let ids: Vec<_> = project.tracks().iter().map(|t| t.id()).collect();
            if let Some(first) = ids.first() {
                view.track_selection.select(&project, *first, false, false);
            }
            if let Some(last) = ids.last() {
                view.track_selection.select(&project, *last, true, false);
            }
            let now = Instant::now();
            view.project_spectrum.result = Some(result(view, now));
            let layers = view.project_spectrum_layers(now);
            assert_eq!(
                layers.len(),
                count + 1,
                "no selection cap or selected-track sum"
            );
            let repeated = view.project_spectrum_layers(now);
            assert!(
                std::ptr::eq(layers.as_ptr(), repeated.as_ptr()),
                "steady repaint shares every layer and point allocation"
            );
            view.project_spectrum.result.as_mut().unwrap().processed_at += Duration::from_nanos(1);
            let updated = view.project_spectrum_layers(now);
            assert!(
                !std::ptr::eq(layers.as_ptr(), updated.as_ptr()),
                "a new result invalidates shared render data"
            );
            let layers = updated;
            publish(view, project.clone());
            let unrelated = view.project_spectrum_layers(now);
            assert!(
                std::ptr::eq(layers.as_ptr(), unrelated.as_ptr()),
                "unchanged colors preserve points across canonical revisions"
            );
            let (min_db, max_db) = view.project_spectrum.options.range.bounds_dbfs();
            let span_db = max_db - min_db;
            for (index, track) in project.tracks().iter().enumerate() {
                assert_eq!(
                    layers[index].color,
                    theme::element_colors(track.color()).accent
                );
                let expected =
                    ((-6.0 - (index + 1) as f32 * 0.25 - min_db) / span_db).clamp(0.0, 1.0);
                assert!((layers[index].points[0][1] - expected).abs() < 1e-6);
            }
            assert!(
                (layers.last().unwrap().points[0][1] - ((-6.0 - min_db) / span_db).clamp(0.0, 1.0))
                    .abs()
                    < 1e-6,
                "master paints last"
            );
            if let Some(track) = ids.first() {
                let mut changed = Editor::new(project.clone(), 8).unwrap();
                changed
                    .execute(Command::SetTrackColor {
                        track: *track,
                        color: ElementColor::Orange,
                    })
                    .unwrap();
                assert_eq!(changed.project().tracks()[0].color(), ElementColor::Orange);
                publish(view, changed.project().clone());
                let unchanged = view.project_spectrum_layers(now);
                assert!(
                    std::ptr::eq(layers.as_ptr(), unchanged.as_ptr()),
                    "setting the authored color again preserves cached layers"
                );
                let mut previous = unchanged;
                for color in [ElementColor::Blue, ElementColor::Orange] {
                    assert_ne!(changed.project().tracks()[0].color(), color);
                    changed
                        .execute(Command::SetTrackColor {
                            track: *track,
                            color,
                        })
                        .unwrap();
                    assert_eq!(changed.project().tracks()[0].color(), color);
                    publish(view, changed.project().clone());
                    let recolored = view.project_spectrum_layers(now);
                    assert_eq!(recolored.len(), count + 1);
                    assert!(
                        !std::ptr::eq(previous.as_ptr(), recolored.as_ptr()),
                        "color change invalidates cache"
                    );
                    assert_eq!(recolored[0].color, theme::element_colors(color).accent);
                    assert_eq!(
                        recolored[0].points, layers[0].points,
                        "color change does not reset or change signal data"
                    );
                    previous = recolored;
                }
                changed.execute(Command::RemoveTrack(*track)).unwrap();
                publish(view, changed.project().clone());
                assert!(
                    view.project_spectrum_layers(now).is_empty(),
                    "removed IDs clear synchronously"
                );
            }
            view.project_spectrum.result = Some(result(view, now));
            assert!(
                view.project_spectrum_layers(
                    now + view
                        .project_spectrum
                        .result
                        .as_ref()
                        .unwrap()
                        .selection
                        .stale_after()
                        + Duration::from_millis(1)
                )
                .is_empty()
            );
            view.project_spectrum.output_identity = Some(123);
            assert!(
                view.project_spectrum_layers(now).is_empty(),
                "closed/replaced output clears even with reused project IDs"
            );
            view.project_spectrum.output_identity = None;
            view.project_generation += 1;
            assert!(
                view.project_spectrum_layers(now).is_empty(),
                "project replacement clears reused IDs"
            );
        });
    }
}

#[gpui::test]
fn project_spectrum_cancel_narrow_resize_restores_wide_preferred_height(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        view.project_spectrum.height = 140.0;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1920.0), px(540.0)));
    visual.run_until_parked();
    let original = visual.debug_bounds("project-spectrum").unwrap().size.height;
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("project-spectrum").unwrap().size.height < original);
    let grip = visual
        .debug_bounds("project-spectrum-resize")
        .unwrap()
        .center();
    visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(grip.x, grip.y - px(10.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_resize(size(px(1920.0), px(540.0)));
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("project-spectrum").unwrap().size.height,
        original
    );
}

#[gpui::test]
fn project_spectrum_collapse_resize_and_cancel_preserve_layout_and_history(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (width, height) in [
        (880.0, 540.0),
        (999.0, 540.0),
        (1000.0, 540.0),
        (super::super::header::WIDE_TOOLBAR_MIN_WIDTH - 1.0, 540.0),
        (super::super::header::WIDE_TOOLBAR_MIN_WIDTH, 540.0),
        (1500.0, 650.0),
    ] {
        visual.simulate_resize(size(px(width), px(height)));
        visual.run_until_parked();
        let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
        let panel = visual.debug_bounds("project-spectrum").unwrap();
        assert!(panel.origin.y >= toolbar.bottom());
        assert!(
            panel.size.height >= px(72.0)
                && panel.size.height <= px(bounded_height(5000.0, size(px(width), px(height))))
        );
        let content = visual.debug_bounds("arrangement-content").unwrap();
        assert!(content.origin.y >= panel.bottom());
        assert!(
            content.size.height >= px(60.0),
            "workspace remains usable: {content:?}"
        );
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
        let grip = visual
            .debug_bounds("project-spectrum-resize")
            .unwrap()
            .center();
        visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(grip.x, grip.y - px(5000.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(px(-20.0), grip.y - px(5000.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("project-spectrum").unwrap().size.height,
            px(72.0)
        );
        editor.read_with(cx, |view, _| {
            assert!(view.project_spectrum.resize.is_none())
        });
        let grip = visual
            .debug_bounds("project-spectrum-resize")
            .unwrap()
            .center();
        visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(grip.x, grip.y + px(5000.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("project-spectrum").unwrap().size.height,
            px(72.0),
            "escape restores prior view height"
        );
        let grip = visual
            .debug_bounds("project-spectrum-resize")
            .unwrap()
            .center();
        visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(grip.x, grip.y + px(5000.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(px(-20.0), grip.y + px(5000.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("project-spectrum").unwrap().size.height,
            px(bounded_height(5000.0, size(px(width), px(height))))
        );
        let toggle = visual
            .debug_bounds("project-spectrum-toggle")
            .unwrap()
            .center();
        visual.simulate_click(toggle, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("project-spectrum").unwrap().size.height,
            px(30.0)
        );
        assert!(visual.debug_bounds("project-spectrum-plot").is_none());
        let collapsed = visual.debug_bounds("project-spectrum").unwrap();
        let controls = visual.debug_bounds("project-spectrum-controls").unwrap();
        assert_eq!(controls.size.height, px(22.0));
        assert!(controls.top() >= collapsed.top() + px(4.0));
        assert!(controls.bottom() <= collapsed.bottom() - px(4.0));
        let toggle = visual
            .debug_bounds("project-spectrum-toggle")
            .unwrap()
            .center();
        visual.simulate_click(toggle, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, before)
        });
    }
}

#[gpui::test]
fn project_spectrum_keeps_workspace_with_populated_details_and_returns(cx: &mut TestAppContext) {
    let (project, _, _, _) = audio_project();
    let mut canonical = Editor::new(project, 8).unwrap();
    for index in 0..4 {
        canonical
            .execute(Command::AddReturnTrack {
                name: format!("return {index}"),
            })
            .unwrap();
    }
    let (handle, editor) = editor_window(cx, canonical.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    editor.update(cx, |view, cx| {
        view.clip_view_open = true;
        view.device_panel_active = true;
        cx.notify();
    });
    visual.run_until_parked();
    let viewport = visual.debug_bounds("clip-detail-viewport").unwrap();
    let content = visual.debug_bounds("arrangement-content").unwrap();
    assert!(viewport.size.height >= px(48.0));
    assert!(
        viewport.size.height < px(213.0),
        "real device details remain in a scroll viewport"
    );
    assert!(
        content.size.height >= px(44.0),
        "timeline survives populated detail and routing chrome: {content:?}"
    );
}

#[gpui::test]
fn framed_workspace_keeps_compact_spectrum_and_complete_devices_at_minimum_size(
    cx: &mut TestAppContext,
) {
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, cx| {
        view.clip_view_open = true;
        view.device_panel_active = true;
        cx.notify();
    });
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let spectrum = visual.debug_bounds("project-spectrum").unwrap();
    assert_eq!(spectrum.size.height, px(30.0));
    assert!(
        visual.debug_bounds("project-spectrum-plot").is_some(),
        "compact mode retains live master and every selected layer"
    );
    assert!(
        visual.debug_bounds("clip-detail-viewport").is_none(),
        "full device controls fit without clipping"
    );
    assert!(
        visual
            .debug_bounds("arrangement-content")
            .unwrap()
            .size
            .height
            >= px(44.0)
    );
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("project-spectrum").unwrap().size.height,
        px(120.0)
    );
    editor.read_with(cx, |view, _| assert!(view.project_spectrum.expanded));
}

#[gpui::test]
fn workspace_three_returns_spectrum_resize_preserves_capture(cx: &mut TestAppContext) {
    for cancel in [false, true] {
        let cx = &mut cx.new_app();
        cx.update(crate::editor::bind_editor_keys);
        let (project, _, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1280.0), px(800.0)));
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let grip = visual
            .debug_bounds("project-spectrum-resize")
            .unwrap()
            .center();
        let original = editor.read_with(&visual, |v, _| v.project_spectrum.height);
        let end = grip + point(px(0.0), px(12.0));
        visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        let (before, height) = editor.read_with(&visual, |v, _| {
            assert!(v.project_spectrum.resize.is_some());
            assert_ne!(v.project_spectrum.height, original);
            (v.state.as_ref().unwrap().clone(), v.project_spectrum.height)
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Arrangement);
            assert!(v.project_spectrum.resize.is_some());
            assert_eq!(v.project_spectrum.height, height);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.project_spectrum.resize.is_none());
            assert_eq!(
                v.project_spectrum.height,
                if cancel { original } else { height }
            );
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Session)
        });
    }
}

#[gpui::test]
fn browser_and_spectrum_share_columns_in_both_workspaces(cx: &mut TestAppContext) {
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    for mode in [
        crate::editor::EditorMode::Arrangement,
        crate::editor::EditorMode::Session,
    ] {
        for (width, height) in [(880.0, 540.0), (1440.0, 800.0)] {
            visual.simulate_resize(size(px(width), px(height)));
            editor.update(&mut visual, |view, cx| {
                view.set_mode(mode);
                view.browser.open = true;
                cx.notify();
            });
            visual.run_until_parked();
            let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
            let browser = visual.debug_bounds("resize-browser-hit-region").unwrap();
            let reported_browser_width =
                visual.update(|window, cx| editor.read(cx).browser_content_width(window));
            assert_eq!(browser.size.width, px(reported_browser_width));
            let spectrum = visual.debug_bounds("project-spectrum").unwrap();
            let plot = visual.debug_bounds("project-spectrum-plot").unwrap();
            assert_eq!(
                plot.top(),
                spectrum.top() + px(4.0),
                "only the structural inset"
            );
            assert_eq!(plot.left(), spectrum.left() + px(4.0));
            assert_eq!(plot.right(), spectrum.right() - px(4.0));
            let scale = visual.update(|window, _| window.scale_factor());
            let (corners, _) = visual
                .rendered_surface_geometry("project-spectrum")
                .unwrap();
            assert_eq!(corners.top_left, px(4.0).scale(scale));
            assert_eq!(corners.bottom_right, px(4.0).scale(scale));
            let controls = visual.debug_bounds("project-spectrum-controls").unwrap();
            assert_eq!(controls.top(), spectrum.top() + px(4.0));
            assert_eq!(controls.right(), spectrum.right() - px(4.0));
            assert!(controls.size.width < spectrum.size.width / 2.0);
            for selector in ["project-spectrum-toggle", "sample-recorder-mode"] {
                let control = visual.debug_bounds(selector).unwrap();
                assert!(spectrum.contains(&control.center()));
            }
            let status = visual.debug_bounds("editor-status-bar").unwrap();
            assert_eq!(browser.top(), toolbar.bottom());
            assert_eq!(spectrum.top(), browser.top());
            assert_eq!(spectrum.left(), browser.right() + px(3.0));
            assert_eq!(browser.bottom(), status.top());
            assert_eq!(spectrum.right(), status.right());
            let main = visual.debug_bounds("workspace-main-panel").unwrap();
            assert_eq!(main.top(), spectrum.bottom() + px(3.0));
            assert_eq!(main.left(), spectrum.left());
            assert_eq!(main.right(), spectrum.right());
            let body_width =
                visual.update(|window, cx| editor.read(cx).workspace_body_width(window));
            assert_eq!(main.size.width, px(body_width + 8.0));
            let (corners, _) = visual
                .rendered_surface_geometry("workspace-main-panel")
                .unwrap();
            assert_eq!(corners.top_left, px(4.0).scale(scale));
            assert_eq!(corners.bottom_right, px(4.0).scale(scale));
            let toggle = visual
                .debug_bounds("toggle browser · cmd+alt+b")
                .unwrap()
                .center();
            visual.simulate_click(toggle, Modifiers::default());
            assert_eq!(
                visual
                    .debug_bounds("resize-browser-hit-region")
                    .unwrap()
                    .size
                    .width,
                px(36.0),
                "collapsed rail includes its rounded surround"
            );
            let expanded = visual.debug_bounds("project-spectrum").unwrap();
            assert!(expanded.size.width > spectrum.size.width);
            assert_eq!(expanded.top(), spectrum.top());
            assert_eq!(expanded.right(), spectrum.right());
            visual.simulate_click(toggle, Modifiers::default());
            assert_eq!(visual.debug_bounds("project-spectrum").unwrap(), spectrum);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, before.project);
                assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
                assert!(view.pending_requests.is_empty());
                assert!(view.last_enqueued_edit.is_none());
            });
        }
    }
}

#[gpui::test]
fn project_spectrum_display_controls_freeze_max_hold_and_crosshair_work_without_project_edits(
    cx: &mut TestAppContext,
) {
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();

    let now = Instant::now();
    let revision_before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);

    // Seed a master spectrum frame at -12 dBFS across all bands and verify
    // slope, smoothing, range, max-hold peak, and freeze behavior.
    editor.update(&mut visual, |view, cx| {
        let mut published = result(view, now);
        let mut frame = hex_audio::spectrum::SpectrumFrame::silent(48_000);
        frame.bins_dbfs = [-12.0; hex_audio::spectrum::SPECTRUM_BANDS];
        frame.peak_dbfs = [-10.0; hex_audio::spectrum::SPECTRUM_BANDS];
        // Keep band 0 at silence floor to verify positive tilt never lifts silence.
        frame.bins_dbfs[0] = hex_audio::spectrum::FLOOR_DBFS;
        frame.peak_dbfs[0] = hex_audio::spectrum::FLOOR_DBFS;
        published.frames = Arc::from([frame]);
        published.selection = hex_audio::spectrum_worker::ProjectSelection::new(
            &[],
            view.project_generation,
            1,
            48_000,
        )
        .unwrap();
        view.track_selection = Default::default();
        view.project_spectrum.result = Some(published);
        view.sync_project_spectrum_canvas(now, cx);

        // Default smoothing is ERB (Human); constant -12 dB maps cleanly to 60/78.
        assert_eq!(
            view.project_spectrum.options.smoothing,
            SmoothingPreset::Human
        );
        let base = view.project_spectrum_layers(now);
        assert_eq!(base.len(), 1);
        assert!((base[0].points[96][1] - 60.0 / 78.0).abs() < 1e-5);

        // Switching to Wide96 (-96..0) maps -12 dB to 84/96.
        view.cycle_project_spectrum_range(cx);
        assert_eq!(view.project_spectrum.options.range, RangePreset::Wide96);
        let wide = view.project_spectrum_layers(now);
        assert!((wide[0].points[96][1] - 84.0 / 96.0).abs() < 1e-5);

        // Cycling slope to Steep (6 dB/oct) lifts high frequencies relative to 1 kHz.
        let prism_hi = wide[0].points.last().unwrap()[1];
        view.cycle_project_spectrum_slope(cx);
        assert_eq!(view.project_spectrum.options.slope, SlopePreset::Steep);
        let steep = view.project_spectrum_layers(now);
        assert!(steep[0].points.last().unwrap()[1] > prism_hi);

        // Cycling smoothing through Fine (1/12), 1/6, 1/3, and back to ERB stays
        // finite & bounded, and Fine keeps band 0 floor at 0 even under Steep slope.
        for expected in [
            SmoothingPreset::Fine,
            SmoothingPreset::Sixth,
            SmoothingPreset::Third,
            SmoothingPreset::Human,
        ] {
            view.cycle_project_spectrum_smoothing(cx);
            assert_eq!(view.project_spectrum.options.smoothing, expected);
            let smoothed = view.project_spectrum_layers(now);
            if expected == SmoothingPreset::Fine {
                assert_eq!(
                    smoothed[0].points[0][1], 0.0,
                    "slope never lifts silence floor"
                );
            }
            assert!(
                smoothed[0]
                    .points
                    .iter()
                    .all(|[x, y]| x.is_finite() && y.is_finite() && (0.0..=1.0).contains(y))
            );
        }

        // Gaussian ERB smoothing rounds an isolated spike into a smooth bell
        // with strictly decreasing flanks (no flat-topped boxcar plateau).
        let mut spike = [hex_audio::spectrum::FLOOR_DBFS; hex_audio::spectrum::SPECTRUM_BANDS];
        spike.fill(-48.0);
        spike[96] = -12.0;
        let bell =
            process_display_bands(&spike, 48_000, SlopePreset::Prism, SmoothingPreset::Human);
        assert!(bell[96] > bell[95] && bell[95] > bell[94] && bell[94] > bell[93]);
        assert!(bell[96] > bell[97] && bell[97] > bell[98] && bell[98] > bell[99]);
        // All-silent frame stays exact silence floor under ERB + Steep slope.
        let silent = [hex_audio::spectrum::FLOOR_DBFS; hex_audio::spectrum::SPECTRUM_BANDS];
        let silent_out =
            process_display_bands(&silent, 48_000, SlopePreset::Steep, SmoothingPreset::Human);
        assert!(
            silent_out
                .iter()
                .all(|&v| v == hex_audio::spectrum::FLOOR_DBFS)
        );

        // Peak hold mode retains high peaks across quieter frames and resets cleanly.
        view.cycle_project_spectrum_peak_mode(cx);
        assert_eq!(view.project_spectrum.options.peak_mode, PeakMode::Hold);
        let held_peak_y = view.project_spectrum_layers(now)[0].peak.as_ref().unwrap()[96][1];
        let mut quieter = hex_audio::spectrum::SpectrumFrame::silent(48_000);
        quieter.bins_dbfs = [-48.0; hex_audio::spectrum::SPECTRUM_BANDS];
        quieter.peak_dbfs = [-48.0; hex_audio::spectrum::SPECTRUM_BANDS];
        view.update_max_peak_from(&quieter);
        if let Some(res) = view.project_spectrum.result.as_mut() {
            res.frames = Arc::from([quieter]);
            res.processed_at += Duration::from_millis(1);
        }
        view.project_spectrum.plot_cache = None;
        let still_held_y = view.project_spectrum_layers(now)[0].peak.as_ref().unwrap()[96][1];
        assert!(
            (still_held_y - held_peak_y).abs() < 1e-5,
            "max-hold peak survives quieter frames"
        );
        view.reset_project_spectrum_peaks(cx);
        let reset_y = view.project_spectrum_layers(now)[0].peak.as_ref().unwrap()[96][1];
        assert!(
            reset_y < still_held_y,
            "reset clears retained max-hold peak"
        );

        // Peak Off removes peak curve cleanly.
        view.cycle_project_spectrum_peak_mode(cx);
        assert_eq!(view.project_spectrum.options.peak_mode, PeakMode::Off);
        assert!(view.project_spectrum_layers(now)[0].peak.is_none());

        // Freeze preserves snapshot past stale_after().
        view.toggle_project_spectrum_freeze(cx);
        assert!(view.project_spectrum.options.frozen);
        let stale_time = now + Duration::from_secs(5);
        assert!(
            !view.project_spectrum_layers(stale_time).is_empty(),
            "frozen snapshot stays visible after live freshness window expires"
        );
        view.toggle_project_spectrum_freeze(cx);
        assert!(view.project_spectrum_layers(stale_time).is_empty());
    });

    // Hovering inside the plot paints a live crosshair readout on the isolated canvas.
    visual.run_until_parked();
    let plot_bounds = visual.debug_bounds("project-spectrum-plot").unwrap();
    visual.simulate_mouse_move(plot_bounds.center(), None, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, cx| {
        let canvas = view.project_spectrum.canvas.as_ref().unwrap().read(cx);
        let crosshair = canvas
            .build_crosshair()
            .expect("hover inside plot builds crosshair");
        assert!(crosshair.primary_label.contains("hz"));
        assert!(crosshair.secondary_label.as_ref().unwrap().contains("db"));
        assert_eq!(view.state.as_ref().unwrap().revision, revision_before);
        assert!(view.pending_requests.is_empty());
    });
}

#[gpui::test]
fn project_spectrum_control_clicks_do_not_bleed_into_plot_and_freeze_toggles_cleanly(
    cx: &mut TestAppContext,
) {
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();

    // Clicking smoothing, slope, range, and peak pills must change only their
    // own setting and never bleed through to the plot's freeze handler.
    let smooth_pos = visual
        .debug_bounds("project-spectrum-smoothing")
        .unwrap()
        .center();
    visual.simulate_click(smooth_pos, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.project_spectrum.options.smoothing,
            SmoothingPreset::Fine
        );
        assert!(
            !view.project_spectrum.options.frozen,
            "clicking smoothing pill must not toggle freeze"
        );
    });

    for (selector, check) in [
        (
            "project-spectrum-slope",
            Box::new(|o: DisplayOptions| o.slope == SlopePreset::Steep)
                as Box<dyn Fn(DisplayOptions) -> bool>,
        ),
        (
            "project-spectrum-range",
            Box::new(|o: DisplayOptions| o.range == RangePreset::Wide96),
        ),
        (
            "project-spectrum-peak",
            Box::new(|o: DisplayOptions| o.peak_mode == PeakMode::Hold),
        ),
    ] {
        let pos = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(pos, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(check(view.project_spectrum.options), "{selector} updated");
            assert!(
                !view.project_spectrum.options.frozen,
                "clicking {selector} must not toggle freeze"
            );
        });
    }

    // Clicking the freeze pill toggles freeze on and off cleanly in one click each.
    let freeze_pos = visual
        .debug_bounds("project-spectrum-freeze")
        .unwrap()
        .center();
    visual.simulate_click(freeze_pos, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.project_spectrum.options.frozen,
            "first freeze pill click freezes"
        );
    });
    visual.simulate_click(freeze_pos, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            !view.project_spectrum.options.frozen,
            "second freeze pill click unfreezes cleanly"
        );
    });

    // Consecutive clicks on the plot canvas itself toggle freeze on and off
    // reliably even within the OS double-click interval.
    let plot_pos = visual
        .debug_bounds("project-spectrum-plot")
        .unwrap()
        .center();
    visual.simulate_click(plot_pos, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.project_spectrum.options.frozen, "plot click freezes");
    });
    visual.simulate_click(plot_pos, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            !view.project_spectrum.options.frozen,
            "immediate second plot click unfreezes cleanly"
        );
    });
}
