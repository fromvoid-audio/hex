//! Compact build identity and diagnostic control shared by editor, loading and demo headers.
use crate::theme;
use gpui::{Div, Stateful, div, prelude::*, px, rgb};

pub(crate) const LABEL: &str =
    concat!(env!("CARGO_PKG_VERSION"), " · ", env!("HEX_BUILD_IDENTITY"));
pub(crate) const HEADER_INSET: f32 = 232.0;

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
    div()
        .id("open-session-log")
        .debug_selector(|| "open-session-log".into())
        .size(px(20.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .cursor_pointer()
        .tooltip(|_, cx| cx.new(|_| LogTooltip).into())
        .child(theme::text("≡"))
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
        div()
            .px_2()
            .py_1()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .child(theme::text("debug · open latest log in text editor"))
    }
}
