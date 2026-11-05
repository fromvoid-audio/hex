//! Real pointer frames around controlled typed worker terminals; Session/provider
//! transaction and PCM behavior keep their own owning regression fixtures.
use super::super::tests::{copy_fixture_state, fixture};
use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext, point};
use hex_project::{Command, Editor};
use hex_session::service::SamplerParameterEdit;

fn rendered_start(visual: &mut VisualTestContext) -> f32 {
    let waveform = visual.debug_bounds("sampler-source-waveform").unwrap();
    let marker = visual.debug_bounds("sampler-source-start").unwrap();
    f32::from(marker.left() - waveform.left()) / f32::from(waveform.size.width)
}

fn submitted_request(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> SamplerParameterEdit {
    editor.read_with(visual, |view, _| {
        assert_eq!(
            view.worker_requests_pending, 1,
            "one gesture submits one worker request"
        );
        crate::editor::sampler_parameter::tests::pending_request(view)
            .unwrap_or_else(|| panic!("missing sampler receipt: {:?}", view.error))
    })
}

fn terminal(
    view: &EditorView,
    request: &SamplerParameterEdit,
    success: bool,
) -> hex_session::service::Event {
    let old = view.state.as_ref().unwrap();
    let mut event = crate::editor::arrangement_main_track_ui_test::completed_event(old);
    let mut state = copy_fixture_state(old);
    if success {
        let device = state
            .project
            .track(request.track)
            .unwrap()
            .instrument()
            .unwrap();
        let next = hex_engine::patch_sampler_parameter_on_control(
            request.parameter,
            device.state(),
            request.value,
        )
        .unwrap();
        let mut model = Editor::new((*state.project).clone(), 16).unwrap();
        model
            .execute(Command::ReplaceDeviceState {
                device: request.device,
                state: next,
            })
            .unwrap();
        state.project = Arc::new(model.project().clone());
        state.revision += 1;
        state.can_undo = true;
        let source = SourceTarget::resolve(view, request.track, request.device).unwrap();
        state.source_overviews = hex_session::source_overview_test_support::Fixture::new(
            source.asset,
            &vec![[0.5, -0.25]; source.frames as usize],
            source.rate,
            source.content,
            None,
        )
        .unwrap()
        .view(hex_engine::AudioSourceContext {
            project_generation: source.generation,
            editor_revision: state.revision,
        });
    } else {
        event.error = Some("sampler boundary refused by fixture".into());
    }
    event.state = Arc::new(state);
    event.sampler_edit_complete = Some(request.request_id);
    event
}

#[gpui::test]
fn sampler_boundary_live_drag_release_and_exact_terminal_keep_one_display_value(
    cx: &mut TestAppContext,
) {
    for success in [true, false] {
        let (handle, editor, track, device) = fixture(cx, 1024, true);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
        let down = point(bounds.left() + px(1.0), bounds.top() + px(12.0));
        let moved = point(down.x + bounds.size.width / 4.0, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!((rendered_start(visual) - 0.25).abs() < 0.002);
        editor.read_with(visual, |view, cx| {
            let device = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap();
            assert_eq!(
                view.sampler_display_value(track, device, SamplerParameter::StartFrame, cx)
                    .unwrap(),
                256.0
            );
            assert_eq!(
                hex_engine::sampler_parameter_value_on_control(
                    SamplerParameter::StartFrame,
                    device.state()
                )
                .unwrap(),
                0.0
            );
            assert_eq!(
                crate::editor::sampler_parameter::tests::rendered_control_value(
                    view,
                    SamplerParameter::StartFrame,
                    cx
                ),
                Some(256.0)
            );
            assert!(view.pending_requests.is_empty());
        });
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let request = submitted_request(&editor, visual);
        assert_eq!(
            (request.device, request.parameter, request.value),
            (device, SamplerParameter::StartFrame, 256.0)
        );
        for _ in 0..3 {
            editor.update(visual, |_, cx| cx.notify());
            visual.run_until_parked();
            editor.read_with(visual, |view, cx| {
                assert_eq!(
                    crate::editor::sampler_parameter::tests::rendered_control_value(
                        view,
                        SamplerParameter::StartFrame,
                        cx
                    ),
                    Some(256.0)
                )
            });
            assert!(
                (rendered_start(visual) - 0.25).abs() < 0.002,
                "release cannot expose old canonical bounds before terminal"
            );
        }
        editor.update(visual, |view, cx| {
            assert!(
                view.device_panel.sampler_cursor.boundary.is_none(),
                "input capture ends on release"
            );
            assert!(
                !view.cancel_sampler_drafts(cx),
                "input cancellation cannot retire admitted custody"
            );
            let mut wrong = terminal(view, &request, false);
            wrong.sampler_edit_complete = Some(request.request_id + 1);
            view.receive(wrong);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            (rendered_start(visual) - 0.25).abs() < 0.002,
            "wrong terminal cannot retire frozen display"
        );
        editor.update(visual, |view, cx| {
            view.receive(terminal(view, &request, success));
            cx.notify();
        });
        visual.run_until_parked();
        assert!((rendered_start(visual) - if success { 0.25 } else { 0.0 }).abs() < 0.002);
        editor.read_with(visual, |view, cx| {
            assert!(!view.sampler_parameter_pending());
            let device = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap();
            assert_eq!(
                view.sampler_display_value(track, device, SamplerParameter::StartFrame, cx)
                    .unwrap(),
                if success { 256.0 } else { 0.0 }
            );
        });
    }
}

#[gpui::test]
fn sampler_boundary_cursor_actions_and_invalid_endpoints_are_explicit(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = fixture(cx, 1024, false);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let hit = visual
        .debug_bounds("sampler-start-at-cursor")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    editor.read_with(visual, |view, _| assert!(view.pending_requests.is_empty()));
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    let target = point(
        bounds.left() + bounds.size.width / 4.0,
        bounds.top() + px(10.0),
    );
    visual.simulate_click(target, Modifiers::default());
    visual.run_until_parked();
    let hit = visual
        .debug_bounds("sampler-start-at-cursor")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    let request = submitted_request(&editor, visual);
    assert_eq!(
        (request.parameter, request.value),
        (SamplerParameter::StartFrame, 256.0)
    );
    editor.update(visual, |view, cx| {
        view.receive(terminal(view, &request, true));
        cx.notify();
    });
    visual.run_until_parked();
    // End at the same cursor would make an empty region. The provider refuses;
    // neither another request nor a silently shifted endpoint is allowed.
    let hit = visual
        .debug_bounds("sampler-end-at-cursor")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert!(view.pending_requests.is_empty());
        assert!(view.error.is_some());
    });
    assert!((rendered_start(visual) - 0.25).abs() < 0.002);
}

#[gpui::test]
fn sampler_boundary_cancel_stale_geometry_and_invalid_drag_preserve_canonical_state(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    for case in 0..6 {
        let (handle, editor, _, _) = fixture(cx, 1024, true);
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
        let down = point(bounds.left() + px(1.0), bounds.top() + px(12.0));
        let moved = point(down.x + bounds.size.width / 4.0, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!((rendered_start(visual) - 0.25).abs() < 0.002);
        let release_at = match case {
            0 => {
                visual.simulate_keystrokes("escape");
                moved
            }
            1 => {
                visual.simulate_keystrokes("cmd-z");
                moved
            }
            2 => {
                visual.simulate_resize(gpui::size(px(1100.0), px(750.0)));
                moved
            }
            3 => {
                editor.update(visual, |view, cx| {
                    let mut state = copy_fixture_state(view.state.as_ref().unwrap());
                    state.source_overviews = Default::default();
                    view.state = Some(Arc::new(state));
                    cx.notify();
                });
                moved
            }
            4 => {
                visual
                    .update_window(handle, |_, window, cx| {
                        editor.read_with(cx, |view, cx| {
                            view.browser.search.read(cx).focus_handle().focus(window)
                        })
                    })
                    .unwrap();
                moved
            }
            _ => {
                let invalid = point(bounds.right() + px(100.0), down.y);
                visual.simulate_mouse_move(invalid, MouseButton::Left, Modifiers::default());
                invalid
            }
        };
        visual.run_until_parked();
        visual.simulate_mouse_up(release_at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(visual, |view, _| {
            assert!(view.pending_requests.is_empty(), "case {case}");
            assert!(
                view.device_panel.sampler_cursor.boundary.is_none(),
                "case {case}"
            );
            assert!(!view.sampler_parameter_pending());
            let track = view.selected_track.unwrap();
            let device = view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap();
            assert_eq!(
                hex_engine::sampler_parameter_value_on_control(
                    SamplerParameter::StartFrame,
                    device.state()
                )
                .unwrap(),
                0.0
            );
        });
    }
}

#[gpui::test]
fn sampler_boundary_end_handle_keeps_grab_offset_and_submits_original_end_frame(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _) = fixture(cx, 1024, true);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1600.0), px(700.0)));
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    let down = point(bounds.right() - px(2.0), bounds.top() + px(12.0));
    let moved = point(down.x - bounds.size.width / 4.0, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let marker = visual.debug_bounds("sampler-source-end").unwrap();
    assert!(
        (f32::from(marker.left() - bounds.left()) / f32::from(bounds.size.width) - 0.75).abs()
            < 0.002
    );
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let request = submitted_request(&editor, visual);
    assert_eq!(
        (request.parameter, request.value),
        (SamplerParameter::EndFrame, 768.0)
    );
    editor.update(visual, |view, cx| {
        view.receive(terminal(view, &request, true));
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("sampler-source-end").is_some());
}

#[gpui::test]
fn sampler_boundary_rejects_a_cursor_edit_conflicting_with_the_authored_loop(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, device) = fixture(cx, 1024, true);
    editor.update(cx, |view, cx| {
        let source = SourceTarget::resolve(view, track, device).unwrap();
        let mut state = copy_fixture_state(view.state.as_ref().unwrap());
        let mut model = Editor::new((*state.project).clone(), 16).unwrap();
        model.execute(Command::ReplaceDeviceState { device,
            state: hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(1024, source.asset, 0.01).unwrap(),
        }).unwrap();
        state.project = Arc::new(model.project().clone());
        view.state = Some(Arc::new(state));
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    visual.simulate_click(
        point(
            bounds.left() + bounds.size.width / 4.0,
            bounds.top() + px(10.0),
        ),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let hit = visual
        .debug_bounds("sampler-start-at-cursor")
        .unwrap()
        .center();
    visual.simulate_click(hit, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.worker_requests_pending, 0);
        assert!(!view.sampler_parameter_pending());
        assert!(view.error.is_some());
        let device = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(
            hex_engine::sampler_parameter_value_on_control(
                SamplerParameter::StartFrame,
                device.state()
            )
            .unwrap(),
            0.0
        );
        assert_eq!(
            hex_engine::sampler_parameter_value_on_control(
                SamplerParameter::LoopStartFrame,
                device.state()
            )
            .unwrap(),
            0.0
        );
    });
}

#[gpui::test]
fn unrelated_pending_edit_keeps_sampler_paint_and_blocks_new_controls(cx: &mut TestAppContext) {
    let (handle, editor, _, _) = fixture(cx, 1024, true);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("sampler-source-waveform").unwrap();
    let at = point(bounds.left() + bounds.size.width * 0.4, bounds.center().y);
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = visual.rendered_filled_quads("device-panel");
    let project = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 1;
        cx.notify();
    });
    for _ in 0..3 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert_eq!(
            visual.rendered_filled_quads("device-panel"),
            before,
            "unrelated pending edit flashes sampler controls"
        );
    }
    let at = visual
        .debug_bounds("sampler-start-at-cursor")
        .unwrap()
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.worker_requests_pending, 1);
        assert!(view.pending_requests.is_empty());
        assert!(!view.sampler_parameter_pending());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), project);
    });
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 0;
        cx.notify();
    });
    visual.simulate_mouse_move(
        point(bounds.left() + bounds.size.width * 0.4, bounds.center().y),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(visual.rendered_filled_quads("device-panel"), before);
}

#[path = "sampler_boundary_capture_tests.rs"]
mod capture_tests;
