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
            for (index, track) in project.tracks().iter().enumerate() {
                assert_eq!(
                    layers[index].color,
                    theme::element_colors(track.color()).accent
                );
                assert!(
                    (layers[index].points[0][1] - (96.0 - 6.0 - (index + 1) as f32 * 0.25) / 96.0)
                        .abs()
                        < 1e-6
                );
            }
            assert!(
                (layers.last().unwrap().points[0][1] - 90.0 / 96.0).abs() < 1e-6,
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
            px(22.0)
        );
        assert!(visual.debug_bounds("project-spectrum-plot").is_none());
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
fn reclaimed_top_chrome_keeps_full_spectrum_and_device_details_at_minimum_size(
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
    assert!(spectrum.size.height >= px(72.0) && spectrum.size.height <= px(120.0));
    assert!(
        visual.debug_bounds("project-spectrum-plot").is_some(),
        "full expanded mode retains live master and every selected layer"
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
            assert_eq!(plot.top(), spectrum.top(), "no reserved title row");
            let controls = visual.debug_bounds("project-spectrum-controls").unwrap();
            assert_eq!(controls.top(), spectrum.top());
            assert_eq!(controls.right(), spectrum.right());
            assert!(controls.size.width < spectrum.size.width / 2.0);
            for selector in ["project-spectrum-toggle", "sample-recorder-mode"] {
                let control = visual.debug_bounds(selector).unwrap();
                assert!(spectrum.contains(&control.center()));
            }
            let status = visual.debug_bounds("editor-status-bar").unwrap();
            assert_eq!(browser.top(), toolbar.bottom());
            assert_eq!(spectrum.top(), browser.top());
            assert_eq!(spectrum.left(), browser.right());
            assert_eq!(browser.bottom(), status.top());
            assert_eq!(spectrum.right(), status.right());
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
