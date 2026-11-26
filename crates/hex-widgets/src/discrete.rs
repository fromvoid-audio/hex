//! Discrete boolean toggles, choice selectors, and compact panel tabs.
mod selector;
mod toggle;

pub use selector::{
    Selector, SelectorAppearance, SelectorChanged, SelectorConfig, bind_selector_keys,
};
pub use toggle::{Toggle, ToggleAppearance, ToggleChanged, ToggleConfig};

use crate::ControlStyle;
use gpui::prelude::*;

/// Flat compact panel navigation. The caller owns lowercase text, identity,
/// focus and page-switch semantics; the shared library owns its appearance.
pub fn panel_tab(label: gpui::SharedString, active: bool, style: ControlStyle) -> gpui::Div {
    gpui::div()
        .h(gpui::px(16.0))
        .px(gpui::px(3.0))
        .flex()
        .items_center()
        .gap(gpui::px(3.0))
        .border_b_1()
        .border_color(gpui::rgb(if active { style.accent } else { style.border }))
        .focus(move |focus| focus.border_color(gpui::rgb(style.focus_marker)))
        .text_size(gpui::px(8.0))
        .text_color(gpui::rgb(style.text))
        .child(
            gpui::div()
                .size(gpui::px(6.0))
                .border_1()
                .border_color(gpui::rgb(style.accent))
                .when(active, |marker| marker.bg(gpui::rgb(style.accent))),
        )
        .child(label)
}

#[cfg(test)]
mod contrast_tests {
    use super::*;
    use crate::tests::STYLE;
    #[gpui::test]
    fn checked_compact_labels_and_icons_use_dark_ink_on_bright_fill(cx: &mut gpui::TestAppContext) {
        for appearance in [
            ToggleAppearance::CompactButton,
            ToggleAppearance::CompactIconButton,
        ] {
            let toggle = cx.new(|cx| {
                Toggle::from_config(
                    cx,
                    ToggleConfig::new("contrast", "on", true, STYLE)
                        .unwrap()
                        .with_appearance(appearance),
                )
            });
            toggle.read_with(cx, |toggle, _| assert_eq!(toggle.foreground(), 0x202020));
        }
    }
}
