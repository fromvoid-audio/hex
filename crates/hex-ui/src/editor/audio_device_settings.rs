//! Application audio output device, preferred sample rate, and hardware buffer size settings.
//!
//! Filesystem persistence and CPAL output device probing run on `background_executor()`.
//! Applying validated changes saves `audio-device.json` atomically and requests an audio output
//! restart through the existing Session worker (`Request::RestartAudio`), never mutating project
//! documents or undo history.

use super::EditorView;
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_audio::{OutputDeviceInfo, OutputDevicesSnapshot, buffer_latency_ms};
use hex_preferences::{
    AudioDevicePreferences, SUPPORTED_BUFFER_SIZES, SUPPORTED_SAMPLE_RATES,
    load_audio_device_preferences, save_audio_device_preferences,
};
use std::path::PathBuf;

pub(super) struct State {
    pub path: Option<PathBuf>,
    pub preferences: AudioDevicePreferences,
    pub draft: AudioDevicePreferences,
    pub snapshot: OutputDevicesSnapshot,
    pub probed: bool,
    pub loading: bool,
    pub probing: bool,
    pub saving: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub status: Option<String>,
}

impl State {
    pub(super) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            preferences: AudioDevicePreferences::default(),
            draft: AudioDevicePreferences::default(),
            snapshot: OutputDevicesSnapshot::default(),
            probed: false,
            loading: true,
            probing: false,
            saving: false,
            writable: true,
            error: None,
            status: None,
        }
    }

    pub(super) fn can_apply(&self) -> bool {
        !self.loading && !self.saving && self.writable && self.draft != self.preferences
    }
}

/// Pure validation of a draft audio output preference against the last probed host snapshot.
/// Returns an optional lowercase advisory note (for example when CoreAudio sample-rate conversion
/// will be used), or an explicit lowercase refusal when the chosen device/buffer is unsupported.
pub(crate) fn validate_draft_against_snapshot(
    draft: &AudioDevicePreferences,
    snapshot: &OutputDevicesSnapshot,
    probed: bool,
) -> Result<Option<String>, String> {
    draft.validate().map_err(|error| {
        hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
    })?;

    let target: Option<&OutputDeviceInfo> = match draft.output_device_uid.as_deref() {
        Some(uid) => {
            if let Some(device) = snapshot.find_by_uid(uid) {
                Some(device)
            } else if probed {
                return Err(
                    "selected audio output is unavailable · choose another device or follow system default"
                        .into(),
                );
            } else {
                None
            }
        }
        None => snapshot.default_device(),
    };

    if let (Some(frames), Some(device)) = (draft.buffer_size_frames, target) {
        device.validate_buffer_size(frames)?;
    }

    let advisory = match (draft.sample_rate_hz, target) {
        (Some(rate), Some(device)) if !device.supports_native_sample_rate(rate) => Some(format!(
            "note · {rate} hz is not listed as native on this device · host rate conversion may apply"
        )),
        _ => None,
    };

    Ok(advisory)
}

/// Format the live hardware buffer and output latency readout for Settings.
pub(crate) fn format_latency_readout(
    active_rate_hz: Option<u32>,
    active_buffer_frames: Option<u32>,
    configured_buffer_frames: Option<u32>,
) -> String {
    match (active_buffer_frames, active_rate_hz) {
        (Some(frames), Some(rate)) => match buffer_latency_ms(frames, rate) {
            Some(ms) => format!("buffer · {frames} frames · {ms:.2} ms at {rate} hz"),
            None => format!("buffer · {frames} frames"),
        },
        (Some(frames), None) => format!("buffer · {frames} frames"),
        (None, Some(rate)) => match configured_buffer_frames {
            Some(frames) => match buffer_latency_ms(frames, rate) {
                Some(ms) => {
                    format!("buffer · {frames} frames requested · ~{ms:.2} ms at {rate} hz")
                }
                None => format!("buffer · {frames} frames requested"),
            },
            None => format!("buffer · device default at {rate} hz"),
        },
        (None, None) => match configured_buffer_frames {
            Some(frames) => format!("buffer · {frames} frames requested · output inactive"),
            None => "buffer · device default · output inactive".into(),
        },
    }
}

fn device_button_label(device: &OutputDeviceInfo, duplicate_name: bool) -> String {
    let base = if duplicate_name {
        format!("{} · {}", device.name, device.uid)
    } else {
        device.name.clone()
    };
    if device.is_default {
        format!(
            "{} · default ({} hz)",
            hex_session::service::text::verbatim(&base),
            device.nominal_sample_rate_hz
        )
    } else {
        format!(
            "{} ({} hz)",
            hex_session::service::text::verbatim(&base),
            device.nominal_sample_rate_hz
        )
    }
}

impl EditorView {
    pub(super) fn start_audio_device_preferences_load(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.audio_device_settings.path.clone() else {
            self.audio_device_settings.loading = false;
            self.audio_device_settings.writable = false;
            self.audio_device_settings.error =
                Some("audio output settings unavailable · home directory is unset".into());
            return;
        };
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    load_audio_device_preferences(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_device_settings;
                state.loading = false;
                match result {
                    Ok(preferences) => {
                        state.preferences = preferences.clone();
                        state.draft = preferences;
                    }
                    Err(error) => {
                        state.writable = false;
                        state.error = Some(format!(
                            "could not load audio output settings · {}",
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

    pub(super) fn prepare_audio_device_settings(&mut self, cx: &mut Context<Self>) {
        if !self.audio_device_settings.saving {
            self.audio_device_settings.draft = self.audio_device_settings.preferences.clone();
        }
        self.audio_device_settings.status = None;
        if self.audio_device_settings.writable {
            self.audio_device_settings.error = None;
        }
        self.refresh_audio_output_devices(cx);
    }

    pub(super) fn refresh_audio_output_devices(&mut self, cx: &mut Context<Self>) {
        if self.audio_device_settings.probing {
            return;
        }
        self.audio_device_settings.probing = true;
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    {
                        Ok::<OutputDevicesSnapshot, String>(OutputDevicesSnapshot::default())
                    }
                    #[cfg(not(test))]
                    {
                        hex_audio::probe_output_devices_on_control()
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let state = &mut view.audio_device_settings;
                state.probing = false;
                match result {
                    Ok(snapshot) => {
                        state.snapshot = snapshot;
                        state.probed = true;
                    }
                    Err(error) => {
                        state.error = Some(format!(
                            "could not scan audio outputs · {}",
                            hex_session::service::text::lowercase_outside_verbatim(&error)
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply_audio_device_settings(&mut self, cx: &mut Context<Self>) {
        if !self.audio_device_settings.can_apply() {
            return;
        }
        if self.audio_recorder.blocks_edits() {
            self.audio_device_settings.error =
                Some("stop recording before changing the audio output device".into());
            cx.notify();
            return;
        }
        let draft = self.audio_device_settings.draft.clone();
        let advisory = match validate_draft_against_snapshot(
            &draft,
            &self.audio_device_settings.snapshot,
            self.audio_device_settings.probed,
        ) {
            Ok(advisory) => advisory,
            Err(reason) => {
                self.audio_device_settings.error = Some(reason);
                cx.notify();
                return;
            }
        };
        let Some(path) = self.audio_device_settings.path.clone() else {
            self.audio_device_settings.writable = false;
            self.audio_device_settings.error =
                Some("audio output settings unavailable · home directory is unset".into());
            cx.notify();
            return;
        };

        self.audio_device_settings.saving = true;
        self.audio_device_settings.error = None;
        self.audio_device_settings.status = Some("saving audio output settings…".into());
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let save_draft = draft.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    save_audio_device_preferences(&path, &save_draft)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.audio_device_settings.saving = false;
                match result {
                    Ok(()) => {
                        view.audio_device_settings.preferences = draft;
                        let restarted = view.restart_audio_for_current_document();
                        view.audio_device_settings.status = Some(match (restarted, advisory) {
                            (true, Some(note)) => {
                                format!("audio output saved · restarting stream · {note}")
                            }
                            (true, None) => "audio output saved · restarting stream".into(),
                            (false, Some(note)) => {
                                format!("audio output saved · restart failed · {note}")
                            }
                            (false, None) => "audio output saved · restart failed".into(),
                        });
                    }
                    Err(error) => {
                        view.audio_device_settings.status = None;
                        view.audio_device_settings.error = Some(format!(
                            "could not save audio output settings · {}",
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
        cx.notify();
    }

    pub(super) fn audio_device_settings_row(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.audio_device_settings;
        let style = theme::control_style();
        let active_rate_hz = self
            .audio
            .as_ref()
            .and_then(|audio| audio.status())
            .and_then(|status| status.interface_sample_rate());
        let active_buffer_frames = self
            .audio
            .as_ref()
            .and_then(|audio| audio.status())
            .and_then(|status| status.buffer_size_frames());
        let latency_label = format_latency_readout(
            active_rate_hz,
            active_buffer_frames,
            state.preferences.buffer_size_frames,
        );
        let interactive = !state.loading && !state.saving && state.writable;

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(div().text_xs().child(theme::text("audio output")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("audio output latency readout")
                            .debug_selector(|| "audio output latency readout".into())
                            .text_xs()
                            .text_color(rgb(style.muted))
                            .child(theme::text(latency_label)),
                    )
                    .child(
                        hex_widgets::action(
                            theme::text(if state.probing {
                                "scanning…"
                            } else {
                                "refresh"
                            }),
                            theme::action_style(),
                        )
                        .id("refresh audio outputs")
                        .debug_selector(|| "refresh audio outputs".into())
                        .px(px(4.0))
                        .text_xs()
                        .opacity(if state.probing { 0.45 } else { 1.0 })
                        .on_click(
                            cx.listener(|view, _, _, cx| view.refresh_audio_output_devices(cx)),
                        ),
                    ),
            );

        let mut name_counts = std::collections::HashMap::<&str, usize>::new();
        for device in &state.snapshot.devices {
            *name_counts.entry(device.name.as_str()).or_default() += 1;
        }
        let follow_selected = state.draft.output_device_uid.is_none();
        let mut device_buttons = Vec::new();
        device_buttons.push(
            hex_widgets::action(theme::text("follow system default"), theme::action_style())
                .id("audio output follow default")
                .debug_selector(|| "audio output follow default".into())
                .border_1()
                .border_color(rgb(if follow_selected {
                    style.accent
                } else {
                    style.border
                }))
                .px(px(4.0))
                .text_xs()
                .opacity(if interactive { 1.0 } else { 0.45 })
                .on_click(cx.listener(|view, _, _, cx| {
                    if !view.audio_device_settings.loading && !view.audio_device_settings.saving {
                        view.audio_device_settings.draft.output_device_uid = None;
                        view.audio_device_settings.error = None;
                        view.audio_device_settings.status = None;
                        cx.notify();
                    }
                })),
        );

        for (index, device) in state.snapshot.devices.iter().enumerate() {
            let uid = device.uid.clone();
            let duplicate = name_counts.get(device.name.as_str()).copied().unwrap_or(0) > 1;
            let label = device_button_label(device, duplicate);
            let selected = state.draft.output_device_uid.as_deref() == Some(uid.as_str());
            let id = format!("audio output device {index}");
            device_buttons.push(
                hex_widgets::action(theme::text(label), theme::action_style())
                    .id(gpui::SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .border_1()
                    .border_color(rgb(if selected { style.accent } else { style.border }))
                    .px(px(4.0))
                    .text_xs()
                    .opacity(if interactive { 1.0 } else { 0.45 })
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.audio_device_settings.loading && !view.audio_device_settings.saving
                        {
                            view.audio_device_settings.draft.output_device_uid = Some(uid.clone());
                            view.audio_device_settings.error = None;
                            view.audio_device_settings.status = None;
                            cx.notify();
                        }
                    })),
            );
        }

        if let Some(pinned_uid) = state.draft.output_device_uid.as_deref()
            && state.probed
            && state.snapshot.find_by_uid(pinned_uid).is_none()
        {
            device_buttons.push(
                hex_widgets::action(
                    theme::text(format!(
                        "unavailable · {}",
                        hex_session::service::text::verbatim(pinned_uid)
                    )),
                    theme::action_style(),
                )
                .id("audio output unavailable pinned")
                .debug_selector(|| "audio output unavailable pinned".into())
                .border_1()
                .border_color(rgb(style.accent))
                .px(px(4.0))
                .text_xs()
                .opacity(0.65),
            );
        }

        let devices_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .children(device_buttons);

        let rate_choices: Vec<Option<u32>> = std::iter::once(None)
            .chain(SUPPORTED_SAMPLE_RATES.into_iter().map(Some))
            .collect();
        let rate_buttons = rate_choices.into_iter().map(|choice| {
            let label = choice.map_or_else(|| "auto".to_owned(), |rate| rate.to_string());
            let selected = state.draft.sample_rate_hz == choice;
            let id = format!("audio output rate {label}");
            hex_widgets::action(theme::text(label), theme::action_style())
                .id(gpui::SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .border_1()
                .border_color(rgb(if selected { style.accent } else { style.border }))
                .px(px(4.0))
                .text_xs()
                .opacity(if interactive { 1.0 } else { 0.45 })
                .on_click(cx.listener(move |view, _, _, cx| {
                    if !view.audio_device_settings.loading && !view.audio_device_settings.saving {
                        view.audio_device_settings.draft.sample_rate_hz = choice;
                        view.audio_device_settings.error = None;
                        view.audio_device_settings.status = None;
                        cx.notify();
                    }
                }))
        });
        let rate_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(style.muted))
                    .child(theme::text("output rate")),
            )
            .children(rate_buttons);

        let buffer_choices: Vec<Option<u32>> = std::iter::once(None)
            .chain(SUPPORTED_BUFFER_SIZES.into_iter().map(Some))
            .collect();
        let buffer_buttons = buffer_choices.into_iter().map(|choice| {
            let label = choice.map_or_else(|| "default".to_owned(), |frames| frames.to_string());
            let selected = state.draft.buffer_size_frames == choice;
            let id = format!("audio output buffer {label}");
            hex_widgets::action(theme::text(label), theme::action_style())
                .id(gpui::SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .border_1()
                .border_color(rgb(if selected { style.accent } else { style.border }))
                .px(px(4.0))
                .text_xs()
                .opacity(if interactive { 1.0 } else { 0.45 })
                .on_click(cx.listener(move |view, _, _, cx| {
                    if !view.audio_device_settings.loading && !view.audio_device_settings.saving {
                        view.audio_device_settings.draft.buffer_size_frames = choice;
                        view.audio_device_settings.error = None;
                        view.audio_device_settings.status = None;
                        cx.notify();
                    }
                }))
        });
        let buffer_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(style.muted))
                    .child(theme::text("buffer size")),
            )
            .children(buffer_buttons)
            .child(
                hex_widgets::action(
                    theme::text(if state.saving {
                        "applying…"
                    } else {
                        "apply audio output"
                    }),
                    theme::action_style(),
                )
                .id("apply audio output settings")
                .debug_selector(|| "apply audio output settings".into())
                .px(px(5.0))
                .text_xs()
                .opacity(if state.can_apply() { 1.0 } else { 0.45 })
                .on_click(cx.listener(|view, _, _, cx| view.apply_audio_device_settings(cx))),
            );

        div()
            .debug_selector(|| "settings audio output".into())
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children([header, devices_row, rate_row, buffer_row])
            .when_some(state.error.as_deref(), |row, reason| {
                row.child(
                    div()
                        .id("audio output settings error")
                        .debug_selector(|| "audio output settings error".into())
                        .text_xs()
                        .child(theme::text(reason)),
                )
            })
            .when_some(state.status.as_deref(), |row, note| {
                row.child(
                    div()
                        .id("audio output settings status")
                        .debug_selector(|| "audio output settings status".into())
                        .text_xs()
                        .text_color(rgb(style.muted))
                        .child(theme::text(note)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> OutputDevicesSnapshot {
        OutputDevicesSnapshot {
            default_uid: Some("built-in-uid".into()),
            devices: vec![
                OutputDeviceInfo {
                    uid: "built-in-uid".into(),
                    name: "Built-In Output".into(),
                    nominal_sample_rate_hz: 48_000,
                    channels: 2,
                    min_buffer_frames: Some(64),
                    max_buffer_frames: Some(512),
                    supported_sample_rates: vec![44_100, 48_000, 96_000],
                    is_default: true,
                },
                OutputDeviceInfo {
                    uid: "usb-dac-uid".into(),
                    name: "USB Interface".into(),
                    nominal_sample_rate_hz: 96_000,
                    channels: 2,
                    min_buffer_frames: Some(128),
                    max_buffer_frames: Some(1024),
                    supported_sample_rates: vec![48_000, 96_000, 192_000],
                    is_default: false,
                },
            ],
        }
    }

    #[test]
    fn refuses_unavailable_pinned_output_device() {
        let snapshot = sample_snapshot();
        let draft = AudioDevicePreferences {
            output_device_uid: Some("missing-usb-uid".into()),
            sample_rate_hz: None,
            buffer_size_frames: Some(256),
        };
        let error = validate_draft_against_snapshot(&draft, &snapshot, true)
            .expect_err("unavailable pinned UID must be refused");
        assert!(error.contains("unavailable"));
    }

    #[test]
    fn refuses_buffer_size_outside_device_range() {
        let snapshot = sample_snapshot();
        let draft = AudioDevicePreferences {
            output_device_uid: Some("built-in-uid".into()),
            sample_rate_hz: Some(48_000),
            buffer_size_frames: Some(1024),
        };
        let error = validate_draft_against_snapshot(&draft, &snapshot, true)
            .expect_err("1024 frames exceeds built-in 512 frame max");
        assert!(error.contains("outside device range"));
    }

    #[test]
    fn emits_advisory_when_sample_rate_is_not_natively_listed() {
        let snapshot = sample_snapshot();
        let draft = AudioDevicePreferences {
            output_device_uid: Some("usb-dac-uid".into()),
            sample_rate_hz: Some(44_100),
            buffer_size_frames: Some(256),
        };
        let advisory = validate_draft_against_snapshot(&draft, &snapshot, true)
            .expect("conversion advisory is allowed")
            .expect("expected host rate conversion advisory");
        assert!(advisory.contains("44100 hz"));
    }

    #[test]
    fn accepts_native_device_rate_and_buffer_without_advisory() {
        let snapshot = sample_snapshot();
        let draft = AudioDevicePreferences {
            output_device_uid: Some("usb-dac-uid".into()),
            sample_rate_hz: Some(96_000),
            buffer_size_frames: Some(256),
        };
        let advisory = validate_draft_against_snapshot(&draft, &snapshot, true)
            .expect("native configuration must pass");
        assert!(advisory.is_none());
    }

    #[test]
    fn formats_live_latency_readout_for_active_and_default_buffers() {
        assert_eq!(
            format_latency_readout(Some(48_000), Some(256), Some(256)),
            "buffer · 256 frames · 5.33 ms at 48000 hz"
        );
        assert_eq!(
            format_latency_readout(Some(48_000), None, None),
            "buffer · device default at 48000 hz"
        );
        assert_eq!(
            format_latency_readout(None, None, Some(128)),
            "buffer · 128 frames requested · output inactive"
        );
    }

    #[test]
    fn audio_device_state_tracks_dirty_apply_eligibility() {
        let mut state = State::new(None);
        state.loading = false;
        assert!(
            !state.can_apply(),
            "unchanged draft should not enable apply"
        );
        state.draft.buffer_size_frames = Some(256);
        assert!(state.can_apply(), "dirty draft enables apply");
        state.saving = true;
        assert!(!state.can_apply(), "saving disables apply");
    }
}
