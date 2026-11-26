//! Compact build identity and diagnostic control shared by editor, loading and demo headers.
use crate::theme;
use gpui::{Div, Stateful, div, prelude::*, px, rgb};
use std::sync::OnceLock;

/// Compile-time build and source provenance injected once by the desktop
/// entrypoint so unrelated commits do not rebuild `hex-ui`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub display_version: &'static str,
    pub build_identity: &'static str,
    pub diagnostic_source: &'static str,
}

impl BuildIdentity {
    pub const fn new(
        display_version: &'static str,
        build_identity: &'static str,
        diagnostic_source: &'static str,
    ) -> Self {
        Self {
            display_version,
            build_identity,
            diagnostic_source,
        }
    }

    /// Fallback metadata used when `hex-ui` runs in tests or examples without
    /// desktop startup injection.
    pub const fn fallback() -> Self {
        Self {
            display_version: env!("HEX_DISPLAY_VERSION"),
            build_identity: "unknown",
            diagnostic_source: "unknown",
        }
    }

    pub fn display_label(self) -> String {
        format!("{} · {}", self.display_version, self.build_identity)
    }

    pub fn diagnostic_string(self) -> String {
        format!("{} source={}", self.display_version, self.diagnostic_source)
    }
}

struct ResolvedBuildInfo {
    identity: BuildIdentity,
    label: String,
    diagnostic: String,
}

impl ResolvedBuildInfo {
    fn from_identity(identity: BuildIdentity) -> Self {
        Self {
            identity,
            label: identity.display_label(),
            diagnostic: identity.diagnostic_string(),
        }
    }
}

static BUILD_INFO: OnceLock<ResolvedBuildInfo> = OnceLock::new();

/// Install desktop-captured build provenance once at startup. Subsequent calls
/// are ignored so display labels and diagnostic headers remain immutable for
/// the lifetime of the process.
pub fn install_build_identity(identity: BuildIdentity) {
    let _ = BUILD_INFO.set(ResolvedBuildInfo::from_identity(identity));
}

fn resolved() -> &'static ResolvedBuildInfo {
    BUILD_INFO.get_or_init(|| ResolvedBuildInfo::from_identity(BuildIdentity::fallback()))
}

pub(crate) fn current_identity() -> BuildIdentity {
    resolved().identity
}

pub(crate) fn label_text() -> &'static str {
    &resolved().label
}

pub fn diagnostic_build_identity() -> &'static str {
    &resolved().diagnostic
}

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
        .child(theme::text(label_text()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_formats_display_label_and_diagnostic_headers_identically() {
        let clean = BuildIdentity::new(
            "0.2.0",
            "0bf6b787",
            "0bf6b787974bc9bf2441b8af88a88a016faf88dd:deadbeef:clean",
        );
        assert_eq!(clean.display_label(), "0.2.0 · 0bf6b787");
        assert_eq!(
            clean.diagnostic_string(),
            "0.2.0 source=0bf6b787974bc9bf2441b8af88a88a016faf88dd:deadbeef:clean"
        );

        let dirty = BuildIdentity::new(
            "0.2.0-beta.1",
            "0bf6b787 dirty",
            "0bf6b787974bc9bf2441b8af88a88a016faf88dd:tree_digest_unavailable:dirty",
        );
        assert_eq!(dirty.display_label(), "0.2.0-beta.1 · 0bf6b787 dirty");
        assert_eq!(
            dirty.diagnostic_string(),
            "0.2.0-beta.1 source=0bf6b787974bc9bf2441b8af88a88a016faf88dd:tree_digest_unavailable:dirty"
        );

        let fallback = BuildIdentity::fallback();
        assert_eq!(fallback.build_identity, "unknown");
        assert_eq!(fallback.diagnostic_source, "unknown");
        assert_eq!(
            fallback.display_label(),
            format!("{} · unknown", env!("HEX_DISPLAY_VERSION"))
        );
        assert_eq!(
            fallback.diagnostic_string(),
            format!("{} source=unknown", env!("HEX_DISPLAY_VERSION"))
        );
    }
}
