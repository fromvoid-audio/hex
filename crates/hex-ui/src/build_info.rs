//! Compact build identity and diagnostic control shared by editor, loading and demo headers.
use crate::theme;
use gpui::{Div, Stateful, div, prelude::*, px, rgb};

pub(crate) const LABEL: &str = concat!(
    env!("HEX_DISPLAY_VERSION"),
    " · ",
    env!("HEX_BUILD_IDENTITY")
);
// Reserve the right edge (8), three control gaps (12), log/help buttons (40),
// changelog button including padding (62), and an eight-pixel title gap.
// Ten-pixel Ioskeley Mono advances six pixels per character; UTF-8 byte length
// conservatively bounds that width. Keep the existing compact minimum.
pub(crate) const HEADER_INSET: f32 = {
    let width = 130.0 + LABEL.len() as f32 * 6.0;
    if width > 232.0 { width } else { 232.0 }
};

pub(crate) fn label() -> Stateful<Div> {
    div()
        .id("application-build-controls")
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(version_label())
        .child(debug_button())
}

pub(crate) fn editor_label() -> Stateful<Div> {
    label()
        .child(
            hex_widgets::action(theme::text("changelog"), theme::action_style())
                .id("open-changelog")
                .tab_index(0)
                .key_context("hex_changelog_button")
                .debug_selector(|| "open-changelog".into())
                .h(px(20.0))
                .px_1()
                .text_size(px(10.0))
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .focus(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(|event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "tab"
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.alt
                        && !event.keystroke.modifiers.platform
                    {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                        cx.stop_propagation();
                    }
                })
                .on_click(|_, window, cx| {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(crate::editor::changelog::OpenChangelog), cx);
                }),
        )
        .child(crate::tooltips::HelpToggle)
}

fn version_label() -> Stateful<Div> {
    div()
        .id("application-build-version")
        .debug_selector(|| "application-build-version".into())
        .flex_shrink_0()
        .text_size(px(10.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(LABEL))
}

fn debug_button() -> Stateful<Div> {
    hex_widgets::action(theme::text("≡"), theme::action_style())
        .id("open-session-log")
        .debug_selector(|| "open-session-log".into())
        .size(px(20.0))
        .text_size(px(12.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .tooltip(|_, cx| cx.new(|_| LogTooltip).into())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, window, cx| {
            cx.stop_propagation();
            crate::diagnostics::open_session_log(window, cx);
        })
}

struct LogTooltip;

impl gpui::Render for LogTooltip {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        crate::tooltips::view(
            &crate::tooltips::Kind::control("debug log"),
            "debug · open session log",
            _cx,
        )
    }
}
