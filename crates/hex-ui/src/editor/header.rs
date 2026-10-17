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
                .child(crate::build_info::label()),
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
        .gap(px(2.0))
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
        if !cfg!(target_os = "macos") {
            return;
        }
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
                project: Arc::clone(&current.project),
                waveforms: Default::default(),
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
        visual.simulate_resize(size(px(880.0), px(540.0)));
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
        let toolbar = visual
            .debug_bounds("editor-toolbar")
            .expect("compact toolbar row");
        let import = visual
            .debug_bounds("import audio · cmd+i")
            .expect("toolbar action remains visible");
        let restart = visual
            .debug_bounds("restart default audio output · stays stopped")
            .expect("right-side audio control remains visible");
        let output_status = visual
            .debug_bounds("audio output status")
            .expect("output status remains visible");
        let close = visual
            .debug_bounds("window-control-close")
            .expect("custom close control");
        let minimize = visual
            .debug_bounds("window-control-minimize")
            .expect("custom minimize control");
        let fullscreen = visual
            .debug_bounds("window-control-fullscreen")
            .expect("custom fullscreen control");
        let window = visual.update(|window, _| window.bounds());
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
            (f32::from(window.origin.x + window.size.width - debug.origin.x - debug.size.width)
                - 8.0)
                .abs()
                < 1.0
        );
        assert!(version.origin.y >= title.origin.y);
        assert!(version.origin.y + version.size.height <= title.origin.y + title.size.height);
        assert!(version.origin.x + version.size.width <= debug.origin.x);
        assert_eq!(debug.size.width, px(20.0));
        assert!(debug.origin.y >= title.origin.y);
        assert!(debug.origin.y + debug.size.height <= title.origin.y + title.size.height);
        assert!((f32::from(import.size.width) - super::CONTROL_SIZE).abs() < 1.0);
        assert!((f32::from(import.size.height) - super::CONTROL_SIZE).abs() < 1.0);
        assert!(f32::from(import.origin.x) < crate::theme::TITLEBAR_LEADING_INSET);
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
        assert!((f32::from(close.size.width) - 16.0).abs() < 1.0);
        assert!((f32::from(minimize.size.width) - 16.0).abs() < 1.0);
        assert!((f32::from(fullscreen.size.width) - 16.0).abs() < 1.0);
        assert!(f32::from(close.origin.x) < f32::from(minimize.origin.x));
        assert!(f32::from(minimize.origin.x) < f32::from(fullscreen.origin.x));
        assert!(f32::from(fullscreen.origin.x + fullscreen.size.width) < window_center);
        assert!(!gpui::TitlebarOptions::default().hide_native_window_controls);
    }
}
