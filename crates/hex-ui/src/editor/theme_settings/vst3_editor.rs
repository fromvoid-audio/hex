//! Application keyboard settings. The persisted choice is read when Hex starts
//! its VST3 host so focused helpers keep one policy for their lifetime.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, Entity, Subscription, div, prelude::*, px, rgb};
use hex_preferences::Vst3EditorPreferences;
use hex_widgets::{Toggle, ToggleChanged, ToggleConfig};
use std::path::PathBuf;

pub(super) struct State {
    path: PathBuf,
    load_started: bool,
    preferences: Option<Vst3EditorPreferences>,
    saving: bool,
    error: Option<String>,
    toggle: Option<(Entity<Toggle>, Subscription)>,
}

impl State {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            load_started: false,
            preferences: None,
            saving: false,
            error: None,
            toggle: None,
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn visible_choice(&self, cx: &gpui::App) -> Option<bool> {
        let saved = self.preferences?.spacebar_controls_transport;
        let displayed = self.toggle.as_ref()?.0.read(cx).checked();
        (!self.saving && saved == displayed).then_some(displayed)
    }
}

impl EditorView {
    pub(super) fn load_vst3_editor_settings(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.theme_settings.vst3_editor;
        if state.load_started {
            return;
        }
        state.load_started = true;
        let path = state.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let load = cx.background_executor().spawn(async move {
            #[cfg(test)]
            let _test_environment = test_environment;
            hex_preferences::load_vst3_editor_preferences(&path)
        });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.theme_settings.vst3_editor;
                match result {
                    Ok(preferences) => state.preferences = Some(preferences),
                    Err(error) => state.error = Some(setting_error("load", &error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_vst3_space_setting(&mut self, checked: bool, cx: &mut Context<Self>) {
        let state = &mut self.theme_settings.vst3_editor;
        let preferences = Vst3EditorPreferences {
            spacebar_controls_transport: checked,
        };
        if state.saving || state.preferences.is_none() || state.preferences == Some(preferences) {
            return;
        }
        state.saving = true;
        state.error = None;
        let path = state.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let save = cx.background_executor().spawn(async move {
            #[cfg(test)]
            let _test_environment = test_environment;
            hex_preferences::save_vst3_editor_preferences(&path, preferences)
        });
        cx.spawn(async move |this, cx| {
            let result = save.await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.theme_settings.vst3_editor;
                state.saving = false;
                match result {
                    Ok(()) => state.preferences = Some(preferences),
                    Err(error) => state.error = Some(setting_error("save", &error)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn vst3_editor_settings_row(&mut self, cx: &mut Context<Self>) -> Div {
        let style = theme::control_style();
        let state = &mut self.theme_settings.vst3_editor;
        let checked = state
            .preferences
            .unwrap_or_default()
            .spacebar_controls_transport;
        if state.toggle.is_none() {
            let config = match ToggleConfig::new(
                "vst3 space transport",
                theme::text("space controls transport in vst3 editors"),
                checked,
                style,
            ) {
                Ok(config) => config,
                Err(error) => return div().child(theme::text(error)),
            };
            let toggle = cx.new(|cx| {
                let mut toggle = Toggle::from_config(cx, config);
                toggle.set_tab_navigation(true, cx);
                toggle
            });
            let subscription = cx.subscribe(&toggle, |view, _, event: &ToggleChanged, cx| {
                view.save_vst3_space_setting(event.checked, cx);
            });
            state.toggle = Some((toggle, subscription));
        }

        let status = if let Some(error) = state.error.as_deref() {
            error
        } else if state.saving {
            "saving…"
        } else if state.preferences.is_none() {
            "loading…"
        } else {
            "restart hex to apply changes"
        };
        let mut row = div().flex().items_center().gap(px(8.0));
        if let Some((entity, _)) = &state.toggle {
            entity.update(cx, |toggle, cx| {
                let _ = toggle.set_external_style(style, cx);
                toggle.set_checked(checked, cx);
                toggle.set_enabled(state.preferences.is_some() && !state.saving, cx);
            });
            row = row.child(entity.clone());
        }
        row.child(
            div()
                .id("vst3 editor settings status")
                .debug_selector(|| "vst3 editor settings status".into())
                .text_xs()
                .text_color(rgb(style.muted))
                .child(theme::text(status)),
        )
    }
}

fn setting_error(operation: &str, error: &hex_preferences::PreferencesError) -> String {
    let error = hex_session::service::text::lowercase_outside_verbatim(&error.to_string());
    format!("could not {operation} vst3 editor settings · {error}")
}
