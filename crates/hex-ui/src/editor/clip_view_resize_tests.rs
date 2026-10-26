use super::*;
use crate::editor::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
use std::sync::Arc;

#[gpui::test]
fn shared_splitters_cancel_button_loss_and_replaced_source_without_edits(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, track, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    let original = visual
        .debug_bounds("arrangement-audio-clip-view")
        .unwrap()
        .size
        .height;
    let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        down + point(px(0.0), original + px(40.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(visual.debug_bounds("clip-detail-resize").is_none());
    assert!(editor.read_with(&visual, |view, _| view.clip_detail_resize.is_active()));
    // Capture stays mounted outside the hidden handle and can reopen the preview.
    visual.simulate_mouse_move(
        down - point(px(0.0), px(20.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(visual.debug_bounds("clip-detail-resize").is_some());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    assert_eq!(
        visual
            .debug_bounds("arrangement-audio-clip-view")
            .unwrap()
            .size
            .height,
        original
    );
    for (grip, panel, direction) in [
        ("clip-detail-resize", "arrangement-audio-clip-view", -1.0),
        ("project-spectrum-resize", "project-spectrum", 1.0),
    ] {
        for replace_source in [false, true] {
            let original = visual.debug_bounds(panel).unwrap().size.height;
            let down = visual.debug_bounds(grip).unwrap().center();
            let moved = down + point(px(0.0), px(30.0 * direction));
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert!(
                    view.clip_detail_resize.is_active() || view.project_spectrum.resize_is_active()
                );
            });
            if replace_source {
                editor.update(&mut visual, |view, cx| {
                    view.project_generation += 1;
                    cx.notify();
                });
                visual.run_until_parked();
            } else {
                visual.simulate_mouse_move(moved, None, Modifiers::default());
            }
            visual.simulate_mouse_up(
                moved + point(px(0.0), px(20.0 * direction)),
                MouseButton::Left,
                Modifiers::default(),
            );
            assert_eq!(visual.debug_bounds(panel).unwrap().size.height, original);
            editor.read_with(&visual, |view, _| {
                assert!(!view.clip_detail_resize.is_active());
                assert!(!view.project_spectrum.resize_is_active());
                assert_eq!(view.state.as_ref().unwrap().project, before.project);
                assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
                assert!(view.pending_requests.is_empty());
                assert!(view.pending_audio_commands.is_empty());
            });
        }
    }
}

#[gpui::test]
fn held_splitter_isolates_second_press_until_cancellation(cx: &mut TestAppContext) {
    let (project, track, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let original = visual
        .debug_bounds("arrangement-audio-clip-view")
        .unwrap()
        .size
        .height;
    let detail = visual.debug_bounds("clip-detail-resize").unwrap().center();
    visual.simulate_mouse_down(detail, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        detail - point(px(0.0), px(30.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    let spectrum = visual
        .debug_bounds("project-spectrum-resize")
        .unwrap()
        .center();
    visual.simulate_mouse_down(spectrum, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.clip_detail_resize.is_active());
        assert!(!view.project_spectrum.resize_is_active());
    });
    // A second press cannot steal a held gesture. After actual button loss,
    // the next independent press starts the other sizing owner normally.
    visual.simulate_mouse_move(spectrum, None, Modifiers::default());
    visual.simulate_mouse_up(spectrum, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_down(spectrum, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(!view.clip_detail_resize.is_active());
        assert!(view.project_spectrum.resize_is_active());
    });
    assert_eq!(
        visual
            .debug_bounds("arrangement-audio-clip-view")
            .unwrap()
            .size
            .height,
        original
    );
    visual.simulate_mouse_up(spectrum, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(!view.project_spectrum.resize_is_active())
    });
}

#[gpui::test]
fn clip_detail_pointer_resize_collapse_restore_and_fixed_devices(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    for (midi, session) in [(false, false), (true, false), (false, true), (true, true)] {
        let (project, track, _, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let (project, clip, scene) = if session {
            let mut model = hex_project::Editor::new(project, 8).unwrap();
            let hex_project::Created::Scene(scene) = model
                .execute(hex_project::Command::AddScene {
                    name: "scene".into(),
                })
                .unwrap()
            else {
                panic!("scene fixture")
            };
            model
                .execute(if midi {
                    hex_project::Command::CopyArrangementMidiToSession {
                        source: clip,
                        scene,
                    }
                } else {
                    hex_project::Command::CopyArrangementAudioClipToSession {
                        source: clip,
                        scene,
                    }
                })
                .unwrap();
            let clip = match model.project().session_cell(track, scene).unwrap() {
                hex_project::SessionCellContent::Audio(clip) => clip.id(),
                hex_project::SessionCellContent::Midi(clip) => clip.id(),
                _ => panic!("clip fixture"),
            };
            (model.project().clone(), clip, Some(scene))
        } else {
            (project, clip, None)
        };
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let visual = &mut VisualTestContext::from_window(handle, cx);
        editor.update(visual, |view, cx| {
            view.mode = if session {
                crate::editor::EditorMode::Session
            } else {
                crate::editor::EditorMode::Arrangement
            };
            view.selected_cell = scene.map(|scene| (track, scene));
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.clip_view_open = true;
            view.device_panel_active = false;
            cx.notify();
        });
        visual.run_until_parked();
        let body_selector = if midi {
            "midi-note-roll"
        } else if session {
            "session-audio-source-panel"
        } else {
            "arrangement-audio-clip-view"
        };
        let before = visual
            .debug_bounds(body_selector)
            .expect("rendered clip canvas");
        let divider = visual
            .debug_bounds("clip-detail-resize")
            .expect("clip resize divider");
        let down = divider.center();
        visual.simulate_click(down, Modifiers::default());
        assert_eq!(
            visual.debug_bounds(body_selector).unwrap().size.height,
            before.size.height,
            "a stationary divider click preserves the size"
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let up = point(down.x, down.y - px(60.0));
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        let larger = visual.debug_bounds(body_selector).unwrap();
        assert!(
            larger.size.height > before.size.height,
            "canvas grows during capture"
        );
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        let remembered = editor.read_with(visual, |view, _| view.clip_detail_body_height);
        let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let closed = point(down.x, down.y + px(remembered + 40.0));
        visual.simulate_mouse_up(closed, MouseButton::Left, Modifiers::default());
        assert!(visual.debug_bounds(body_selector).is_none());
        let toggle = visual.debug_bounds("clip-view-toggle").unwrap().center();
        visual.simulate_click(toggle, Modifiers::default());
        assert_eq!(
            editor.read_with(visual, |view, _| view.clip_detail_body_height),
            remembered
        );
        let restored = visual.debug_bounds(body_selector).unwrap();
        assert_eq!(restored.size.height, larger.size.height);
        let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x, down.y - px(30.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_keystrokes("escape");
        assert_eq!(
            visual.debug_bounds(body_selector).unwrap().size.height,
            restored.size.height
        );
        let device_tab = visual.debug_bounds("device-detail-tab").unwrap().center();
        visual.simulate_click(device_tab, Modifiers::default());
        assert!(
            visual.debug_bounds("clip-detail-resize").is_none(),
            "device pane has no resize affordance"
        );
        assert_eq!(
            editor.read_with(visual, |view, _| view.project_spectrum.detail_height),
            Some(crate::editor::device_panel::DEVICE_PANEL_HEIGHT)
        );
        let device_pane = visual.debug_bounds("clip-view").unwrap();
        let down = point(device_pane.center().x, device_pane.origin.y + px(1.0));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let up = point(down.x, down.y - px(60.0));
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        assert_eq!(
            visual.debug_bounds("clip-view").unwrap().size.height,
            device_pane.size.height
        );
        editor.read_with(visual, |view, _| {
            assert!(view.pending_requests.is_empty());
            assert!(view.pending_audio_commands.is_empty());
        });
    }
}

#[gpui::test]
fn clip_detail_resize_minimum_window_budget_blur_and_resize_cancel(cx: &mut TestAppContext) {
    let (project, track, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1100.0), px(800.0)));
    editor.update(visual, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let original = visual
        .debug_bounds("arrangement-audio-clip-view")
        .unwrap()
        .size
        .height;
    let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    // Between the collapse threshold and the useful minimum, keep a useful canvas.
    let minimum = point(down.x, down.y + original - px(60.0));
    visual.simulate_mouse_move(minimum, MouseButton::Left, Modifiers::default());
    assert_eq!(
        visual
            .debug_bounds("arrangement-audio-clip-view")
            .unwrap()
            .size
            .height,
        px(128.0)
    );
    // Passing through minimum before collapse still restores the gesture's original size.
    let closed = point(down.x, down.y + original);
    visual.simulate_mouse_move(closed, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(closed, MouseButton::Left, Modifiers::default());
    let toggle = visual.debug_bounds("clip-view-toggle").unwrap().center();
    visual.simulate_click(toggle, Modifiers::default());
    assert_eq!(
        visual
            .debug_bounds("arrangement-audio-clip-view")
            .unwrap()
            .size
            .height,
        original
    );

    for blur in [true, false] {
        let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x, down.y - px(40.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        if blur {
            visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window));
            visual.run_until_parked();
        } else {
            visual.simulate_resize(gpui::size(px(1100.0), px(780.0)));
        }
        assert!(!editor.read_with(visual, |view, _| view.clip_detail_resize.is_active()));
        assert_eq!(
            visual
                .debug_bounds("arrangement-audio-clip-view")
                .unwrap()
                .size
                .height,
            original
        );
    }
    // An extreme upward release is clamped, preserving Arrangement space.
    let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        point(down.x, px(-5000.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(
        visual
            .debug_bounds("arrangement-content")
            .unwrap()
            .size
            .height
            >= px(44.0)
    );
    let full_height = visual
        .debug_bounds("arrangement-audio-clip-view")
        .unwrap()
        .size
        .height;
    visual.simulate_resize(gpui::size(px(880.0), px(540.0)));
    assert!(
        visual
            .debug_bounds("arrangement-content")
            .unwrap()
            .size
            .height
            >= px(44.0)
    );
    let pane = visual.debug_bounds("clip-view").unwrap();
    assert!(pane.bottom() <= px(540.0));
    let down = visual.debug_bounds("clip-detail-resize").unwrap().center();
    let height = editor.read_with(visual, |view, _| view.clip_detail_body_height);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        point(down.x, down.y + px(height + 40.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_resize(gpui::size(px(1100.0), px(780.0)));
    let toggle = visual.debug_bounds("clip-view-toggle").unwrap().center();
    visual.simulate_click(toggle, Modifiers::default());
    assert_eq!(
        visual
            .debug_bounds("arrangement-audio-clip-view")
            .unwrap()
            .size
            .height,
        full_height,
        "temporary window clamping must not overwrite remembered size"
    );
}

#[gpui::test]
fn clip_detail_small_window_returns_scroll_useful_piano_instead_of_crushing_tracks(
    cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = midi_project();
    let mut model = hex_project::Editor::new(project, 8).unwrap();
    for index in 0..4 {
        model
            .execute(hex_project::Command::AddReturnTrack {
                name: format!("return {index}"),
            })
            .unwrap();
    }
    let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(880.0), px(540.0)));
    editor.update(visual, |view, cx| {
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        view.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let viewport = visual
        .debug_bounds("clip-detail-viewport")
        .expect("scroll viewport below useful piano minimum");
    assert!(viewport.size.height >= px(48.0));
    assert!(visual.debug_bounds("midi-note-roll").unwrap().size.height > px(48.0));
    assert!(
        visual
            .debug_bounds("arrangement-content")
            .unwrap()
            .size
            .height
            >= px(44.0)
    );
}
