//! Shared compact title and toolbar rows for editor startup and loaded projects.
use crate::theme::{self, Palette};
use gpui::{Div, Stateful, div, prelude::*, px, rgb};
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

pub(super) fn title_bar(title: impl AsRef<str>) -> Stateful<Div> {
    div()
        .id("project-title-bar")
        .debug_selector(|| "project-title-bar".into())
        .h(px(TITLE_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        // Equal padding on both sides keeps the title centered while protecting
        // the first row beneath macOS traffic lights.
        .px(px(theme::TITLEBAR_LEADING_INSET))
        .border_b_1()
        .border_color(rgb(Palette::BORDER))
        .bg(rgb(Palette::SURFACE))
        .child(
            div()
                .id("project-title-label")
                .debug_selector(|| "project-title-label".into())
                .w_full()
                .min_w_0()
                .max_w(px(720.0))
                .text_center()
                .text_xs()
                .text_color(rgb(Palette::TEXT))
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
        .border_color(rgb(Palette::BORDER))
        .bg(rgb(Palette::SURFACE))
}

pub(super) fn loading_toolbar(message: &str) -> Stateful<Div> {
    toolbar_row().justify_center().child(
        div()
            .text_xs()
            .text_color(rgb(Palette::MUTED))
            .child(theme::text(message)),
    )
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
    use gpui::{TestAppContext, VisualTestContext, px, size};
    use hex_project::Project;
    use std::sync::Arc;

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
                project: Arc::clone(&current.project),
                waveforms: Default::default(),
                midi_preview: Arc::clone(&current.midi_preview),
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
        let window = visual.update(|window, _| window.bounds());
        let title_bottom = f32::from(title.origin.y + title.size.height);
        let toolbar_top = f32::from(toolbar.origin.y);
        let title_center = f32::from(title_label.origin.x + title_label.size.width / 2.0);
        let window_center = f32::from(window.origin.x + window.size.width / 2.0);
        assert!((f32::from(title.size.height) - super::TITLE_HEIGHT).abs() < 1.0);
        assert!((f32::from(toolbar.size.height) - super::TOOLBAR_HEIGHT).abs() < 1.0);
        assert!((toolbar_top - title_bottom).abs() < 1.0);
        assert!((title_center - window_center).abs() < 1.0);
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
    }
}
