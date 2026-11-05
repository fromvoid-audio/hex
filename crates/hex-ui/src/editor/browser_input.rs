//! Application styling and domain limits for the shared native text input.
//! Search filtering and canonical name edits remain with their current owners.
use crate::theme;
use gpui::{App, Context, Entity};
use hex_widgets::{TextInput, TextInputConfig, TextInputKind};
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
        action_style: theme::action_style(),
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
pub(super) fn name(content: String, cx: &mut Context<TextInput>) -> TextInput {
    let mut input = TextInput::new(cx, config(TextInputKind::Value));
    input.set_text(&content, cx);
    input.select_all_text(cx);
    input
}
pub(super) fn restyle(input: &Entity<TextInput>, cx: &mut App) {
    input.update(cx, |input, cx| {
        let _ = input.set_external_style(theme::control_style(), theme::action_style(), cx);
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "browser_input_caret_ui_test.rs"]
mod caret_idle_tests;
