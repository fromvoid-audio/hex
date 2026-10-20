//! Shared compact title and toolbar rows for editor startup and loaded projects.
use crate::theme;
use gpui::{App, ClickEvent, Div, Stateful, Window, div, prelude::*, px, rgb};
use std::path::Path;

pub(super) const TITLE_HEIGHT: f32 = 22.0;
pub(super) const TOOLBAR_HEIGHT: f32 = 26.0;
pub(super) const CONTROL_SIZE: f32 = 20.0;

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

pub(super) fn title_bar(
    title: impl AsRef<str>,
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
        .h_full()
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

    div()
        .id("project-title-bar")
        .debug_selector(|| "project-title-bar".into())
        .relative()
        .h(px(TITLE_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        // Equal padding on both sides keeps the title centered while protecting
        // the first row beneath macOS traffic lights.
        .px(px(
            crate::build_info::HEADER_INSET.max(theme::TITLEBAR_LEADING_INSET)
        ))
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .on_click(|event, window, cx| {
            if event.standard_click() && event.click_count() == 2 {
                cx.stop_propagation();
                window.toggle_fullscreen();
            }
        })
        .child(lights)
        .child(
            div()
                .absolute()
                .right_2()
                .top_0()
                .h_full()
                .flex()
                .items_center()
                .child(crate::build_info::editor_label()),
        )
        .child(
            div()
                .id("project-title-label")
                .debug_selector(|| "project-title-label".into())
                .w_full()
                .min_w_0()
                .max_w(px(720.0))
                .text_center()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                .line_clamp(1)
                .text_ellipsis()
                .child(theme::text(title)),
        )
}

pub(super) fn toolbar_row() -> Stateful<Div> {
    div()
        .id("editor-toolbar")
        .debug_selector(|| "editor-toolbar".into())
        .h(px(TOOLBAR_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(1.0))
        // The toolbar is below the title row and does not need the traffic-light inset.
        .pr_2()
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
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
    fn title_double_click_enters_and_exits_fullscreen(cx: &mut TestAppContext) {
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
        for (button, click_count, expected) in [
            (MouseButton::Left, 1, false),
            (MouseButton::Right, 2, false),
            (MouseButton::Left, 2, true),
            (MouseButton::Left, 3, true),
            (MouseButton::Left, 1, true),
            (MouseButton::Left, 2, false),
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
            assert_eq!(visual.update(|window, _| window.is_fullscreen()), expected);
        }
    }

    #[gpui::test]
    fn compact_header_centers_project_title_and_keeps_toolbar_controls_clear_of_traffic_lights(
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
                project: current.project.clone(),
                waveforms: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: current.midi_preview.clone(),
                gain_preview: Ok(hex_engine::SharedGainAutomationPreview::new_on_control(
                    hex_engine::GainAutomationPreview::default(),
                )),
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
        // Exercise common rendered geometry on every platform, including the
        // actual minimum width. Only the OS-specific lights are conditional.
        for width in [880.0, 960.0, 1440.0] {
            visual.simulate_resize(size(px(width), px(540.0)));
            visual.run_until_parked();

            let title = visual
                .debug_bounds("project-title-bar")
                .expect("project title row");
            let title_label = visual
                .debug_bounds("project-title-label")
                .expect("centered title label");
            let version = visual
                .debug_bounds("application-build-version")
                .expect("build version remains visible at minimum width");
            let debug = visual
                .debug_bounds("open-session-log")
                .expect("debug button remains visible beside the version");
            let changelog = visual
                .debug_bounds("open-changelog")
                .expect("changelog button");
            let toolbar = visual
                .debug_bounds("editor-toolbar")
                .expect("compact toolbar row");
            let tempo = visual
                .debug_bounds("project-tempo-field")
                .expect("first toolbar field");
            let import = visual
                .debug_bounds("import audio · cmd+i")
                .expect("toolbar action remains visible");
            let restart = visual
                .debug_bounds("restart default audio output · stays stopped")
                .expect("right-side audio control remains visible");
            let output_status = visual
                .debug_bounds("audio output status")
                .expect("output status remains visible");
            let window = visual.update(|window, _| window.bounds());
            eprintln!(
                "header bounds window={window:?} title={title_label:?} version={version:?} log={debug:?} changelog={changelog:?} tempo={tempo:?} import={import:?}"
            );
            let title_bottom = f32::from(title.origin.y + title.size.height);
            let toolbar_top = f32::from(toolbar.origin.y);
            let title_center = f32::from(title_label.origin.x + title_label.size.width / 2.0);
            let window_center = f32::from(window.origin.x + window.size.width / 2.0);
            assert!((f32::from(title.size.height) - super::TITLE_HEIGHT).abs() < 1.0);
            assert!((f32::from(toolbar.size.height) - super::TOOLBAR_HEIGHT).abs() < 1.0);
            assert!((toolbar_top - title_bottom).abs() < 1.0);
            assert!((title_center - window_center).abs() < 1.0);
            assert!(title_label.origin.x + title_label.size.width <= version.origin.x);
            assert!(version.size.width > px(0.0));
            assert!(
                (f32::from(
                    window.origin.x + window.size.width - changelog.origin.x - changelog.size.width
                ) - 8.0)
                    .abs()
                    < 1.0
            );
            assert!(version.origin.y >= title.origin.y);
            assert!(version.origin.y + version.size.height <= title.origin.y + title.size.height);
            // Changelog is the rightmost control; Log remains its compact neighbor.
            assert_eq!(
                debug.origin.x - (version.origin.x + version.size.width),
                px(4.0)
            );
            assert_eq!(
                changelog.origin.x - (debug.origin.x + debug.size.width),
                px(4.0)
            );
            assert!(changelog.size.width > px(0.0));
            assert_eq!(changelog.size.height, px(super::CONTROL_SIZE));
            assert!(changelog.origin.y >= title.origin.y);
            assert!(
                changelog.origin.y + changelog.size.height <= title.origin.y + title.size.height
            );
            assert_eq!(debug.size.width, px(20.0));
            assert!(debug.origin.y >= title.origin.y);
            assert!(debug.origin.y + debug.size.height <= title.origin.y + title.size.height);
            assert!((f32::from(import.size.width) - super::CONTROL_SIZE).abs() < 1.0);
            assert!((f32::from(import.size.height) - super::CONTROL_SIZE).abs() < 1.0);
            // Tempo now leads the toolbar; Import follows Tempo and Tuning.
            // The first field still starts at the row edge, without the title inset.
            assert_eq!(tempo.origin.x, toolbar.origin.x);
            assert!(f32::from(tempo.origin.x) < crate::theme::TITLEBAR_LEADING_INSET);
            assert!(tempo.origin.x + tempo.size.width <= import.origin.x);
            assert!(import.origin.x + import.size.width <= toolbar.origin.x + toolbar.size.width);
            assert!(f32::from(title_label.size.width) <= 720.0);
            assert!(
                f32::from(restart.origin.x + restart.size.width)
                    <= f32::from(toolbar.origin.x + toolbar.size.width)
            );
            assert!(
                f32::from(output_status.origin.x + output_status.size.width)
                    <= f32::from(toolbar.origin.x + toolbar.size.width)
            );
            assert!(f32::from(output_status.size.width) > 0.0);
            if cfg!(target_os = "macos") {
                let close = visual
                    .debug_bounds("window-control-close")
                    .expect("custom close control");
                let minimize = visual
                    .debug_bounds("window-control-minimize")
                    .expect("custom minimize control");
                let fullscreen = visual
                    .debug_bounds("window-control-fullscreen")
                    .expect("custom fullscreen control");
                assert!((f32::from(close.size.width) - 16.0).abs() < 1.0);
                assert!((f32::from(minimize.size.width) - 16.0).abs() < 1.0);
                assert!((f32::from(fullscreen.size.width) - 16.0).abs() < 1.0);
                assert!(f32::from(close.origin.x) < f32::from(minimize.origin.x));
                assert!(f32::from(minimize.origin.x) < f32::from(fullscreen.origin.x));
                assert!(f32::from(fullscreen.origin.x + fullscreen.size.width) < window_center);
                assert!(fullscreen.origin.x + fullscreen.size.width <= title_label.origin.x);
            }
            assert!(!gpui::TitlebarOptions::default().hide_native_window_controls);
        }
    }

    #[gpui::test]
    fn compact_toolbar_keeps_names_status_and_controls_visible_in_both_workspaces(
        cx: &mut TestAppContext,
    ) {
        use super::super::EditorMode;
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            Project::new(48_000).unwrap(),
            Arc::default(),
        );
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let mut visual = VisualTestContext::from_window(handle, cx);
        // The existing headless control carrier supplies labels without OS audio.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
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
                for width in [880.0, 881.0, 960.0, 1440.0, 880.0] {
                    visual.simulate_resize(size(px(width), px(540.0)));
                    visual.run_until_parked();
                    let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
                    let name = visual
                        .debug_bounds(match mode {
                            EditorMode::Arrangement => "arrange view",
                            EditorMode::Session => "console view",
                        })
                        .unwrap();
                    let status = visual.debug_bounds("audio output status").unwrap();
                    let restart = visual
                        .debug_bounds("restart default audio output · stays stopped")
                        .unwrap();
                    // Ioskeley Mono: enough actual shaped width for all seven
                    // characters of 'offline', not an invisible sliver.
                    assert!(
                        status.size.width >= name.size.width * (7.0 / 12.0),
                        "{width} {mode:?}: status={status:?} name={name:?}"
                    );
                    assert!(name.size.width >= px(80.0), "full canonical view name");
                    assert!(name.right() <= restart.left());
                    assert!(restart.right() <= status.left());
                    assert!(status.right() <= toolbar.right() - px(8.0));
                    assert!(status.size.height <= toolbar.size.height);
                    for selector in [
                        "import audio · cmd+i",
                        "show arrange view · tab",
                        "show console view · tab",
                        "restart default audio output · stays stopped",
                    ] {
                        let control = visual.debug_bounds(selector).unwrap();
                        assert_eq!(
                            control.size,
                            size(px(super::CONTROL_SIZE), px(super::CONTROL_SIZE))
                        );
                        assert!(
                            control.left() >= toolbar.left() && control.right() <= toolbar.right()
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
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
                for width in [880.0, 881.0, 960.0, 1440.0, 880.0] {
                    visual.simulate_resize(size(px(width), px(540.0)));
                    visual.run_until_parked();
                    let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
                    let name = visual
                        .debug_bounds(match mode {
                            EditorMode::Arrangement => "arrange view",
                            EditorMode::Session => "console view",
                        })
                        .unwrap();
                    let status = visual.debug_bounds("audio output status").unwrap();
                    let restart = visual
                        .debug_bounds("restart default audio output · stays stopped")
                        .unwrap();
                    editor.read_with(&visual, |v, _| assert_eq!(v.state.is_none(), loading));
                    eprintln!(
                        "actual startup loading={loading} width={width} mode={mode:?} status={status:?} name={name:?} restart={restart:?}"
                    );
                    let seven_chars = name.size.width * (7.0 / 12.0);
                    assert!(
                        status.size.width >= seven_chars,
                        "both project loading and full offline status must remain visible: {width} {mode:?} loading={loading} status={status:?}"
                    );
                    let readout = visual.debug_bounds("project position").unwrap();
                    assert!(readout.size.width >= seven_chars);
                    assert!(readout.right() <= restart.left());
                    assert!(name.right() <= readout.left());
                    assert!(restart.right() <= status.left());
                    assert!(status.right() <= toolbar.right() - px(8.0));
                    assert!(status.size.height <= toolbar.size.height);
                    assert!(name.size.width >= px(80.0));
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
                        "show arrange view · tab",
                        "show console view · tab",
                        "restart default audio output · stays stopped",
                    ] {
                        let control = visual.debug_bounds(selector).unwrap();
                        assert_eq!(
                            control.size,
                            size(px(super::CONTROL_SIZE), px(super::CONTROL_SIZE))
                        );
                        assert!(
                            control.left() >= toolbar.left() && control.right() <= toolbar.right()
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
