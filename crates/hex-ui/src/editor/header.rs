//! Shared compact title and toolbar rows for editor startup and loaded projects.
use crate::theme;
use gpui::{App, ClickEvent, Div, Stateful, Window, div, prelude::*, px, rgb};
use std::path::Path;

pub(super) const TOOLBAR_HEIGHT: f32 = 22.0;
/// Overlays leave the first toolbar row clear for native window controls.
pub(super) const WINDOW_CONTROL_HEIGHT: f32 = TOOLBAR_HEIGHT;
// Keep every transport action, full view name and useful output status at the
// minimum window width. Reserve the wide line for the longest input policy,
// position readout and output/loading indicators as well as fixed controls.
pub(super) const WIDE_TOOLBAR_MIN_WIDTH: f32 = 1_600.0;
pub(super) fn toolbar_height(width: f32) -> f32 {
    if width < WIDE_TOOLBAR_MIN_WIDTH {
        // Two full rows plus the existing bottom divider.
        TOOLBAR_HEIGHT * 2.0 + 1.0
    } else {
        TOOLBAR_HEIGHT
    }
}
pub(super) const CONTROL_SIZE: f32 = 18.0;

pub(super) fn project_title(document_path: Option<&Path>) -> String {
    document_path
        .and_then(Path::file_stem)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "hex".into())
}

#[derive(Clone, Copy)]
pub(super) enum TitlebarControl {
    Close,
    Minimize,
    Fullscreen,
}

pub(super) fn window_controls(
    on_control: impl Fn(TitlebarControl) -> Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
) -> Stateful<Div> {
    let controls = [
        (TitlebarControl::Close, "close window", 0xe65c4f),
        (TitlebarControl::Minimize, "minimize window", 0xd8a63b),
        (TitlebarControl::Fullscreen, "toggle fullscreen", 0x57a86b),
    ];
    let mut lights = div()
        .id("window-controls")
        .absolute()
        .left_2()
        .top_0()
        .h(px(TOOLBAR_HEIGHT))
        .flex()
        .items_center()
        .gap(px(4.0));
    for (control, tooltip, hover_color) in
        controls.into_iter().filter(|_| cfg!(target_os = "macos"))
    {
        let on_control = on_control(control);
        lights = lights.child(
            div()
                .id(match control {
                    TitlebarControl::Close => "window-control-close",
                    TitlebarControl::Minimize => "window-control-minimize",
                    TitlebarControl::Fullscreen => "window-control-fullscreen",
                })
                .debug_selector(move || match control {
                    TitlebarControl::Close => "window-control-close".into(),
                    TitlebarControl::Minimize => "window-control-minimize".into(),
                    TitlebarControl::Fullscreen => "window-control-fullscreen".into(),
                })
                .size(px(16.0))
                .group("window-control")
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .tooltip(move |_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        kind: crate::tooltips::Kind::Identification,
                        description: tooltip.into(),
                    })
                    .into()
                })
                .child(
                    div()
                        .size(px(10.0))
                        .rounded_full()
                        .bg(rgb(theme::color(theme::PaletteRole::Muted)))
                        .group_hover("window-control", move |style| style.bg(rgb(hover_color))),
                )
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_control(event, window, cx);
                }),
        );
    }

    lights
}

pub(super) fn toolbar_row() -> Stateful<Div> {
    div()
        .id("editor-toolbar")
        .debug_selector(|| "editor-toolbar".into())
        .h(px(TOOLBAR_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        // Every first-row control stays clear of native window chrome.
        .pl(px(theme::TITLEBAR_LEADING_INSET))
        .pr_2()
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .on_click(|event, window, cx| {
            if event.standard_click() && event.click_count() == 2 {
                cx.stop_propagation();
                window.titlebar_double_click();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::project_title;
    use crate::theme;
    use std::path::Path;

    #[test]
    fn project_title_uses_filename_without_mutating_original_path_and_falls_back_to_hex() {
        let path = Path::new("/Volumes/Library/My Session.HEX");
        assert_eq!(project_title(Some(path)), "My Session");
        assert_eq!(path, Path::new("/Volumes/Library/My Session.HEX"));
        assert_eq!(theme::text(project_title(Some(path))), "my session");
        assert_eq!(project_title(None), "hex");
        assert_eq!(
            project_title(Some(Path::new("/Volumes/Library/.hex"))),
            ".hex"
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use gpui::{
        MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext, point, px,
        size,
    };
    use hex_project::Project;
    use std::sync::Arc;

    #[gpui::test]
    fn cpu_meter_click_resets_the_visible_overload(cx: &mut TestAppContext) {
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            Project::new(48_000).expect("project"),
            Arc::default(),
        );
        editor.update(cx, |view, cx| {
            let now = std::time::Instant::now();
            let sample = hex_audio::cpu_meter::CpuSample {
                elapsed: 10,
                budget: 100,
                peak: 10_000,
                frames: 256,
                rate: 48_000,
            };
            view.cpu_meter.observe(Some((1, sample, 0)), now);
            view.cpu_meter.observe(Some((1, sample, 1)), now);
            assert!(view.cpu_meter.warning());
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("editor-cpu-meter").expect("cpu meter");
        let position = point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_event(MouseDownEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(MouseUpEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        assert!(!editor.read_with(cx, |view, _| view.cpu_meter.warning()));
    }

    #[gpui::test]
    fn title_double_click_never_toggles_fullscreen(cx: &mut TestAppContext) {
        let (handle, _editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            Project::new(48_000).expect("project"),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let title = visual.debug_bounds("project-title-label").expect("title");
        let position = point(
            title.origin.x + title.size.width / 2.0,
            title.origin.y + title.size.height / 2.0,
        );
        // The test platform's titlebar action is a no-op; the macOS action
        // follows the system zoom/minimize setting and never enters fullscreen.
        for (button, click_count) in [
            (MouseButton::Left, 1),
            (MouseButton::Right, 2),
            (MouseButton::Left, 2),
            (MouseButton::Left, 3),
        ] {
            visual.simulate_event(MouseDownEvent {
                position,
                modifiers: Default::default(),
                button,
                click_count,
                first_mouse: false,
            });
            visual.simulate_event(MouseUpEvent {
                position,
                modifiers: Default::default(),
                button,
                click_count,
            });
            assert!(!visual.update(|window, _| window.is_fullscreen()));
        }
    }

    #[gpui::test]
    fn compact_top_toolbar_and_bottom_project_title_preserve_chrome_and_controls(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            Project::new(48_000).expect("project"),
            Arc::default(),
        );
        editor.update(cx, |view, cx| {
            let current = view.state.as_ref().expect("fixture state");
            let filename = format!("{}.HEX", "Long Project Name ".repeat(30));
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::clone(&current.project),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview: Arc::clone(&current.midi_preview),
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                timeline_frames: current.timeline_frames,
                can_undo: false,
                can_redo: false,
                revision: current.revision,
                unsaved: current.unsaved,
                document_path: Some(std::path::Path::new("/Library/Projects").join(filename)),
            }));
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        for width in [
            880.0,
            960.0,
            1440.0,
            super::WIDE_TOOLBAR_MIN_WIDTH - 1.0,
            super::WIDE_TOOLBAR_MIN_WIDTH,
            1920.0,
        ] {
            visual.simulate_resize(size(px(width), px(540.0)));
            visual.run_until_parked();
            assert!(visual.debug_bounds("project-title-bar").is_none());
            let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
            let status = visual.debug_bounds("editor-status-bar").unwrap();
            let title = visual.debug_bounds("project-title-label").unwrap();
            let tempo = visual.debug_bounds("project-tempo-field").unwrap();
            let mode = visual.debug_bounds("workspace-view-toggle").unwrap();
            let window = visual.update(|window, _| window.bounds());
            assert_eq!(toolbar.top(), window.top());
            assert_eq!(toolbar.size.height, px(super::toolbar_height(width)));
            let old_chrome = 22.0 + if width < 1200.0 { 53.0 } else { 26.0 };
            assert!(toolbar.size.height < px(old_chrome));
            assert!(title.top() >= status.top() && title.bottom() <= status.bottom());
            assert!(title.left() >= status.left() && title.right() <= status.right());
            assert!(title.size.width <= px(120.0));
            assert!(tempo.left() >= toolbar.left() + px(crate::theme::TITLEBAR_LEADING_INSET));
            assert!(mode.right() <= toolbar.right());
            let mut previous_right = None;
            for selector in [
                "editor-cpu-meter",
                "application-build-version",
                "open-session-log",
                "open-changelog",
                "help-tooltip-toggle",
            ] {
                let control = visual.debug_bounds(selector).unwrap();
                assert!(control.size.width > px(0.0));
                assert!(
                    control.top() >= toolbar.top() && control.bottom() <= toolbar.bottom(),
                    "{width}: {selector}={control:?}, toolbar={toolbar:?}"
                );
                assert!(control.right() <= mode.left());
                if let Some(previous) = previous_right {
                    assert!(control.left() >= previous);
                }
                previous_right = Some(control.right());
            }
            let import = visual.debug_bounds("import audio · cmd+i").unwrap();
            assert_eq!(
                import.size,
                gpui::size(px(super::CONTROL_SIZE), px(super::CONTROL_SIZE))
            );
            assert!(tempo.right() <= import.left());
            for selector in [
                "restart default audio output · stays stopped",
                "audio output status",
            ] {
                let control = visual.debug_bounds(selector).unwrap();
                assert!(control.size.width > px(0.0));
                assert!(control.left() >= toolbar.left() && control.right() <= toolbar.right());
            }
            if cfg!(target_os = "macos") {
                let mut right = toolbar.left();
                for selector in [
                    "window-control-close",
                    "window-control-minimize",
                    "window-control-fullscreen",
                ] {
                    let control = visual.debug_bounds(selector).unwrap();
                    assert_eq!(control.size, gpui::size(px(16.0), px(16.0)));
                    assert!(control.left() >= right && control.right() <= tempo.left());
                    assert!(control.bottom() <= toolbar.top() + px(super::WINDOW_CONTROL_HEIGHT));
                    right = control.right();
                }
            }
        }
    }

    #[gpui::test]
    fn compact_toolbar_keeps_view_toggle_status_and_controls_visible_in_both_workspaces(
        cx: &mut TestAppContext,
    ) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        use super::super::EditorMode;
        use hex_project::InputQuantization::{Adaptive, Off, Sixteenth};
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            Project::new(48_000).unwrap(),
            Arc::default(),
        );
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let mut visual = VisualTestContext::from_window(handle, cx);
        // The existing headless control carrier supplies labels without OS audio.
        let deadline = crate::test_wait::hang_deadline();
        let (mut output, _backend) = loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok(output) => break output,
                Err(error)
                    if error == "audio output is open or still shutting down"
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(error) => panic!("headless toolbar fixture: {error}"),
            }
        };
        let mut client = output.take_client();
        for label in [
            None,
            Some("audio ready"),
            Some(
                "output unavailable: device disconnected; a very long original device name that must remain available in its tooltip",
            ),
        ] {
            if let Some(label) = label {
                editor.update(&mut visual, |view, cx| {
                    if let Some(audio) = view.audio.as_mut() {
                        audio.label = label.into();
                    } else {
                        view.audio = Some(crate::EditorAudio {
                            control: client.take().unwrap(),
                            sample_rate: 48_000,
                            label: label.into(),
                            spectrum_error: None,
                            mode: hex_session::DocumentAudioMode::Resident,
                        });
                    }
                    cx.notify();
                });
            }
            for mode in [EditorMode::Arrangement, EditorMode::Session] {
                visual.update(|window, cx| {
                    editor.update(cx, |view, cx| {
                        view.set_mode(mode);
                        view.focus.focus(window);
                        cx.notify();
                    })
                });
                for (width, input) in [
                    (880.0, Off),
                    (881.0, Adaptive),
                    (960.0, Sixteenth),
                    (999.0, Adaptive),
                    (1_000.0, Off),
                    (super::WIDE_TOOLBAR_MIN_WIDTH - 1.0, Adaptive),
                    (super::WIDE_TOOLBAR_MIN_WIDTH, Adaptive),
                    (super::WIDE_TOOLBAR_MIN_WIDTH, Off),
                    (super::WIDE_TOOLBAR_MIN_WIDTH, Sixteenth),
                    (1440.0, Adaptive),
                    (880.0, Off),
                ] {
                    editor.update(&mut visual, |view, cx| {
                        view.set_input_quantization(input);
                        cx.notify();
                    });
                    visual.simulate_resize(size(px(width), px(540.0)));
                    visual.run_until_parked();
                    let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
                    assert_eq!(toolbar.size.height, px(super::toolbar_height(width)));
                    let name = visual.debug_bounds("workspace-view-toggle").unwrap();
                    let status = visual.debug_bounds("audio output status").unwrap();
                    let restart = visual
                        .debug_bounds("restart default audio output · stays stopped")
                        .unwrap();
                    // Ioskeley Mono: enough actual shaped width for all seven
                    // characters of 'offline', not an invisible sliver.
                    assert!(
                        status.size.width >= px(80.0) * (7.0 / 12.0),
                        "{width} {mode:?}: status={status:?} name={name:?}"
                    );
                    assert_eq!(name.size.width, px(super::CONTROL_SIZE));
                    assert!(status.right() <= name.left());
                    assert!(restart.right() <= status.left());
                    assert!(status.right() <= toolbar.right() - px(8.0));
                    assert!(status.size.height <= toolbar.size.height);
                    let policy = visual.debug_bounds("input-quantization").unwrap();
                    let label_chars = format!("in {} ▾", input.label()).chars().count();
                    assert!(
                        policy.size.width >= px(80.0) * (label_chars as f32 / 12.0),
                        "the complete input policy label remains visible"
                    );
                    for selector in [
                        "arrange-zoom-fit",
                        "arrange-zoom-back",
                        "section-autoplay-toggle",
                        "global-launch-quantization",
                        "input-quantization",
                        "section-focus-control",
                    ] {
                        let control = visual.debug_bounds(selector).unwrap();
                        assert!(
                            control.size.width >= px(super::CONTROL_SIZE - 1.0)
                                && control.size.height >= px(super::CONTROL_SIZE)
                        );
                        assert!(
                            control.left() >= toolbar.left()
                                && control.right() <= toolbar.right()
                                && control.top() >= toolbar.top()
                                && control.bottom() <= toolbar.bottom()
                                && (control.bottom() <= name.top()
                                    || control.right() <= name.left()),
                            "{width} {mode:?} {selector}: control={control:?} toolbar={toolbar:?} view={name:?}"
                        );
                    }
                    for selector in [
                        "project tuning · cmd+alt+t",
                        "import audio · cmd+i",
                        "workspace-view-toggle",
                        "restart default audio output · stays stopped",
                    ] {
                        let control = visual.debug_bounds(selector).unwrap();
                        assert_eq!(
                            control.size,
                            size(px(super::CONTROL_SIZE), px(super::CONTROL_SIZE))
                        );
                        assert!(
                            control.left() >= toolbar.left()
                                && control.right() <= toolbar.right()
                                && control.top() >= toolbar.top()
                                && control.bottom() <= toolbar.bottom()
                        );
                    }
                    editor.read_with(&visual, |view, _| {
                        assert_eq!(view.state.as_ref().unwrap().project, before);
                        assert_eq!(view.audio_label(), label);
                        assert_eq!(view.worker_requests_pending, 0);
                    });
                }
            }
        }
        editor.update(&mut visual, |view, _| view.audio = None);
    }
    #[gpui::test]
    fn failed_session_startup_replaces_loading_with_a_visible_error(cx: &mut TestAppContext) {
        use super::super::EditorView;
        use gpui::AppContext;
        struct OfflineSink;
        impl hex_session::service::WorkerAudioSink for OfflineSink {
            fn initialize(&mut self, _: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
                Err("fixture device unavailable".into())
            }
            fn canonical_changed(
                &mut self,
                _: std::sync::Arc<hex_session::service::CanonicalAudioSourceSnapshot>,
            ) {
            }
            fn poll(&mut self) {}
            fn close(&mut self) {}
        }
        // History capacity 0 makes the worker's session construction fail.
        let service =
            hex_session::service::Service::with_audio_sink(0, 0, None, Box::new(OfflineSink))
                .unwrap();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline startup test".into()))
                })
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let error = editor.update(&mut visual, |v, cx| {
                if v.tick(cx) {
                    cx.notify();
                }
                v.error.as_deref().map(str::to_owned)
            });
            if let Some(error) = error {
                assert_eq!(
                    error,
                    "project session could not start: project edit failed: \
                     history capacity must be 1..128 · restart hex to try again"
                );
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "startup failure never surfaced"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert!(v.state.is_none()));
        assert!(visual.debug_bounds("project loading").is_none());
        assert!(visual.debug_bounds("editor-status-message").is_some());
    }
    #[gpui::test]
    fn compact_toolbar_startup_loading_and_real_service_ready_keep_both_indicators(
        cx: &mut TestAppContext,
    ) {
        use super::super::{EditorMode, EditorView};
        use gpui::AppContext;
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new(
                        window,
                        cx,
                        hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap(),
                        Err("offline startup test".into()),
                    )
                })
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        // This is the real constructor's initial phase. The virtual 16ms poll
        // timer is not advanced; no synthetic None ViewState is installed.
        editor.read_with(&visual, |v, _| {
            assert!(v.state.is_none());
            assert!(v.browser.directory.starts_with(v.test_environment.root()));
        });
        let mut ready: Option<Arc<hex_session::service::ViewState>> = None;
        for loading in [true, false] {
            if !loading {
                let deadline = crate::test_wait::hang_deadline();
                loop {
                    let installed = editor.update(&mut visual, |v, cx| {
                        if v.tick(cx) {
                            cx.notify();
                        }
                        v.state.clone()
                    });
                    if installed.is_some() {
                        ready = installed;
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "actual Service init timed out"
                    );
                    std::thread::yield_now();
                }
            }
            for mode in [EditorMode::Arrangement, EditorMode::Session] {
                editor.update(&mut visual, |v, cx| {
                    v.set_mode(mode);
                    cx.notify();
                });
                for width in [
                    880.0,
                    881.0,
                    960.0,
                    999.0,
                    1_000.0,
                    super::WIDE_TOOLBAR_MIN_WIDTH - 1.0,
                    super::WIDE_TOOLBAR_MIN_WIDTH,
                    1440.0,
                    880.0,
                ] {
                    visual.simulate_resize(size(px(width), px(540.0)));
                    visual.run_until_parked();
                    let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
                    let name = visual.debug_bounds("workspace-view-toggle").unwrap();
                    let status = visual.debug_bounds("audio output status").unwrap();
                    let restart = visual
                        .debug_bounds("restart default audio output · stays stopped")
                        .unwrap();
                    editor.read_with(&visual, |v, _| assert_eq!(v.state.is_none(), loading));
                    eprintln!(
                        "actual startup loading={loading} width={width} mode={mode:?} status={status:?} name={name:?} restart={restart:?}"
                    );
                    let seven_chars = px(80.0) * (7.0 / 12.0);
                    assert!(
                        status.size.width >= seven_chars,
                        "both project loading and full offline status must remain visible: {width} {mode:?} loading={loading} status={status:?}"
                    );
                    let readout = visual.debug_bounds("project position").unwrap();
                    assert!(readout.size.width >= seven_chars);
                    assert!(readout.right() <= restart.left());
                    assert!(readout.right() <= restart.left());
                    assert!(restart.right() <= status.left());
                    assert!(status.right() <= toolbar.right() - px(8.0));
                    assert!(status.size.height <= toolbar.size.height);
                    assert_eq!(name.size.width, px(super::CONTROL_SIZE));
                    // The real None branch omits the bound tempo field; the
                    // appended loading label does not compete with a loaded row.
                    let tempo = visual.debug_bounds("project-tempo-field");
                    if loading {
                        assert!(tempo.is_none());
                        let indicator = visual.debug_bounds("project loading").unwrap();
                        eprintln!("actual loading indicator={indicator:?} position={readout:?}");
                        assert!(indicator.size.width >= seven_chars + px(16.0));
                        assert!(indicator.size.height <= toolbar.size.height);
                        assert!(indicator.left() >= status.right());
                        assert!(indicator.right() <= toolbar.right() - px(8.0));
                    } else {
                        assert!(tempo.is_some_and(|bounds| bounds.size.width > px(0.0)));
                        assert!(visual.debug_bounds("project loading").is_none());
                    }
                    for selector in [
                        "import audio · cmd+i",
                        "workspace-view-toggle",
                        "restart default audio output · stays stopped",
                    ] {
                        let control = visual.debug_bounds(selector).unwrap();
                        assert_eq!(
                            control.size,
                            size(px(super::CONTROL_SIZE), px(super::CONTROL_SIZE))
                        );
                        assert!(
                            control.left() >= toolbar.left()
                                && control.right() <= toolbar.right()
                                && control.top() >= toolbar.top()
                                && control.bottom() <= toolbar.bottom()
                        );
                    }
                    editor.read_with(&visual, |v, _| {
                        assert_eq!(v.audio_label(), None);
                        assert_eq!(v.worker_requests_pending, 0);
                        assert!(v.pending_requests.is_empty());
                        if let Some(ready) = &ready {
                            assert_eq!(v.state.as_ref().unwrap().project, ready.project);
                            assert_eq!(v.state.as_ref().unwrap().revision, ready.revision);
                        }
                    });
                }
            }
        }
    }
}
