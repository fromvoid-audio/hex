//! Read-only compile-time identity shared by canonical, loading and demo headers.
use crate::theme;
use gpui::{Div, Stateful, div, prelude::*, px, rgb};

pub(crate) const LABEL: &str =
    concat!(env!("CARGO_PKG_VERSION"), " · ", env!("HEX_BUILD_IDENTITY"));
pub(crate) const HEADER_INSET: f32 = 208.0;

pub(crate) fn label() -> Stateful<Div> {
    div()
        .id("application-build-version")
        .debug_selector(|| "application-build-version".into())
        .flex_shrink_0()
        .text_size(px(10.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(LABEL))
}
