//! Recoverable application input selection. Loading blocks selection so a late
//! disk result cannot replace user intent. Live changes apply immediately; saves
//! are serialized and failures remain visible without destroying source bytes.
use super::*;
use hex_preferences::MidiInputPreferences;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct Preferences {
    pub(super) path: Option<PathBuf>,
    pub(super) value: MidiInputPreferences,
    pub(super) loading: bool,
    pub(super) saving: bool,
    pub(super) unwritable: bool,
    pub(super) error: Option<String>,
}

impl ControllerInput {
    pub(in crate::editor) fn new(path: PathBuf) -> Self {
        let mut state = Self::default();
        state.preferences.path = Some(path);
        state.preferences.loading = true;
        state
    }
}

impl EditorView {
    pub(in crate::editor) fn start_controller_preferences_load(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.controller_input.preferences.path.clone() else {
            return;
        };
        let this = cx.entity().downgrade();
        #[cfg(test)]
        let environment = Arc::clone(&self.test_environment);
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    hex_preferences::load_midi_input_preferences(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.controller_input.preferences.loading = false;
                match result {
                    Ok(value) => {
                        view.controller_input.channel = value.channel;
                        let restore = value.port.is_some();
                        view.controller_input.preferences.value = value;
                        if restore {
                            view.refresh_controller_inputs(cx);
                        }
                    }
                    Err(error) => {
                        view.controller_input.preferences.unwritable = true;
                        view.controller_input.preferences.error = Some(format!(
                            "could not load midi input settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ));
                    }
                }
                view.sync_controller_selectors(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn controller_preferences_editable(&self) -> bool {
        let state = &self.controller_input.preferences;
        !state.loading && !state.saving && !state.unwritable
    }

    pub(super) fn save_controller_preferences(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.controller_input.preferences;
        let Some(path) = state.path.clone() else {
            return;
        };
        let value = state.value.clone();
        state.saving = true;
        state.error = None;
        let this = cx.entity().downgrade();
        #[cfg(test)]
        let environment = Arc::clone(&self.test_environment);
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _environment = environment;
                    hex_preferences::save_midi_input_preferences(&path, &value)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.controller_input.preferences.saving = false;
                view.controller_input.preferences.error = result.err().map(|error| {
                    format!(
                        "midi input changes are not saved · {}",
                        hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                    )
                });
                cx.notify();
            });
        })
        .detach();
    }
}
