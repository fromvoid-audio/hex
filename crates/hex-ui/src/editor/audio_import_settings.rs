//! Application import preference and admission-time intent capture. No project
//! mutation or decode lives here; the worker owns the one canonical transaction.
//! Loading blocks the toggle, so a late load cannot overwrite user intent. A
//! toggle becomes active only after its serialized background save succeeds;
//! queued imports retain their previous immutable policy while a save is pending.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_preferences::AudioImportPreferences;
use hex_session::{
    import_batch::{AudioBatchDestination, ConsoleDestination},
    import_batch_decode::AudioBatchContext,
    service::Request,
};

mod admission;
pub(super) use admission::State;

impl EditorView {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_auto_warp_imports_for_test(&mut self, enabled: bool) {
        self.audio_import_settings.loading = false;
        self.audio_import_settings.preferences.auto_warp_beats = enabled;
    }

    pub(super) fn capture_audio_import_policy(&self, request: Request) -> Result<Request, String> {
        let mut request = self.audio_import_settings.capture(
            request,
            AudioBatchContext {
                project_generation: self.project_generation,
                revision: self.state.as_ref().map_or(0, |state| state.revision),
                active_gesture: self.device_gesture.snapshot().is_some(),
            },
        )?;
        if let Request::ImportAudioBatch {
            destination:
                AudioBatchDestination::NewTracks { .. }
                | AudioBatchDestination::Console(ConsoleDestination::NewTrack { .. }),
            options,
            ..
        } = &mut request
        {
            options.new_track_gain =
                Some(self.track_defaults.captured_gain().map_err(str::to_owned)?);
        }
        match request {
            Request::Import(path) => Ok(Request::ImportToNewTrackWithGain {
                path,
                before_track: None,
                start: 0,
                gain: self.track_defaults.captured_gain().map_err(str::to_owned)?,
            }),
            Request::ImportToNewTrack {
                path,
                before_track,
                start,
            } => Ok(Request::ImportToNewTrackWithGain {
                path,
                before_track,
                start,
                gain: self.track_defaults.captured_gain().map_err(str::to_owned)?,
            }),
            request => Ok(request),
        }
    }

    pub(super) fn start_audio_import_preferences_load(&mut self, cx: &mut Context<Self>) {
        let path = self.audio_import_settings.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load_audio_import_preferences(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_import_settings;
                state.loading = false;
                match result {
                    Ok(preferences) => state.preferences = preferences,
                    Err(error) => {
                        state.writable = false;
                        state.error = Some(format!(
                            "could not load audio import settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_audio_import_warp(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.audio_import_settings;
        if state.loading || state.saving || !state.writable {
            return;
        }
        let preferences = AudioImportPreferences {
            auto_warp_beats: !state.preferences.auto_warp_beats,
        };
        state.saving = true;
        state.error = None;
        let path = state.path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save_audio_import_preferences(&path, preferences)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_import_settings;
                state.saving = false;
                match result {
                    Ok(()) => state.preferences = preferences,
                    Err(error) => {
                        state.error = Some(format!(
                            "could not save audio import settings · {}",
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string()
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn audio_import_settings_row(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.audio_import_settings;
        let style = super::device_panel::control_style();
        let label = if state.loading {
            "auto-warp imported audio to beats · loading…"
        } else if state.saving {
            "auto-warp imported audio to beats · saving…"
        } else if state.preferences.auto_warp_beats {
            "auto-warp imported audio to beats · on"
        } else {
            "auto-warp imported audio to beats · off"
        };
        let mut row = div().flex().flex_col().gap(px(3.0)).child(
            div()
                .id("auto warp imports")
                .debug_selector(|| "auto warp imports".into())
                .text_xs()
                .px(px(4.0))
                .py(px(3.0))
                .border_1()
                .border_color(rgb(style.border))
                .cursor_pointer()
                .on_click(cx.listener(|view, _, _, cx| view.toggle_audio_import_warp(cx)))
                .child(theme::text(label)),
        );
        if let Some(error) = &state.error {
            row = row.child(
                div()
                    .id("audio import settings status")
                    .debug_selector(|| "audio import settings status".into())
                    .text_xs()
                    .child(theme::text(error)),
            );
        }
        row
    }
}
