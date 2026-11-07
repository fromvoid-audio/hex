//! Application styling and domain limits for the shared native text input.
//! Search filtering and canonical name edits remain with their current owners.
use crate::theme;
use gpui::{App, Context, Entity};
use hex_widgets::{TextInput, TextInputConfig, TextInputKind};
fn clear_action_style() -> hex_widgets::ActionStyle {
    let style = theme::control_style();
    hex_widgets::ActionStyle {
        background: style.muted,
        text: style.raised,
        hover_background: style.text,
        hover_text: Some(style.raised),
    }
}
fn config(kind: TextInputKind) -> TextInputConfig {
    TextInputConfig {
        id: if kind == TextInputKind::Search {
            "browser-search"
        } else {
            "clip-name-input"
        }
        .into(),
        kind,
        style: theme::control_style(),
        action_style: clear_action_style(),
        font_family: theme::FONT_FAMILY.into(),
        placeholder: theme::text("search · cmd+f"),
        clear_name: theme::text("clear search"),
        clear_glyph: theme::text("×"),
        max_bytes: if kind == TextInputKind::Value {
            hex_project::MAX_NAME_BYTES
        } else {
            65_536
        },
    }
}
pub(super) fn search(cx: &mut Context<TextInput>) -> TextInput {
    TextInput::new(cx, config(TextInputKind::Search))
}
pub(super) fn plugin_parameter_search(cx: &mut Context<TextInput>) -> TextInput {
    let mut settings = config(TextInputKind::Search);
    settings.id = "vst3-parameter-search".into();
    settings.placeholder = theme::text("search parameters");
    settings.max_bytes = 256;
    TextInput::new(cx, settings)
}
pub(super) fn name(content: String, cx: &mut Context<TextInput>) -> TextInput {
    let mut input = TextInput::new(cx, config(TextInputKind::Value));
    input.set_text(&content, cx);
    input.select_all_text(cx);
    input
}
/// Bounded comma-separated labels use the shared native value input.
pub(super) fn tags(content: String, cx: &mut Context<TextInput>) -> TextInput {
    let mut settings = config(TextInputKind::Value);
    settings.max_bytes = hex_project::MAX_TRACK_TAGS * (hex_project::MAX_TRACK_TAG_BYTES + 2);
    let mut input = TextInput::new(cx, settings);
    input.set_text(&content, cx);
    input.select_all_text(cx);
    input
}
pub(super) fn restyle(input: &Entity<TextInput>, cx: &mut App) {
    input.update(cx, |input, cx| {
        let _ = input.set_external_style(theme::control_style(), clear_action_style(), cx);
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "browser_input_caret_ui_test.rs"]
mod caret_idle_tests;
