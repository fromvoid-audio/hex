//! Browser intent and cancellation only. Session retains the admitted request;
//! the audio owner retains native resources through its exact terminal.
use super::*;
use gpui::{Div, div, prelude::*, rgb};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};

#[cfg(feature = "ui-test-support")]
pub(crate) fn fixture_view(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<crate::PluginEffectTestView, String> {
    window
        .read(cx, |view: gpui::Entity<EditorView>, cx| {
            let view = view.read(cx);
            let (parameter_controls, parameter_query_pending) =
                device_panel::vst3_parameters::test_state(view);
            let devices: std::collections::BTreeSet<_> = parameter_controls
                .iter()
                .map(|(device, _, _)| *device)
                .collect();
            let parameter_touches = devices
                .into_iter()
                .filter_map(|device| {
                    let (row, control, lane) =
                        device_panel::vst3_parameters::test_touch_state(view, device, cx);
                    row.map(|(parameter, normalized)| {
                        (device, parameter, normalized, control, lane)
                    })
                })
                .collect();
            crate::PluginEffectTestView {
                state: view.state.clone(),
                pending: view.pending_plugin_effect.is_some(),
                queued_requests: view.worker_requests_pending
                    + view.pending_requests.len()
                    + view.device_gesture_barriers.len(),
                parameter_controls,
                parameter_query_pending,
                parameter_touches,
                parameter_gesture: view
                    .device_gesture
                    .snapshot()
                    .and_then(|snapshot| snapshot.plugin_value)
                    .map(|value| value.normalized()),
                operation: view
                    .pending_plugin_effect
                    .as_ref()
                    .map(|pending| Arc::downgrade(&pending.control)),
                error: view.error.as_ref().map(ToString::to_string),
                selected_owner: view
                    .state
                    .as_ref()
                    .and_then(|state| device_panel::selected_owner(view, state))
                    .and_then(|owner| match owner {
                        device_panel::DevicePanelOwner::Track(id) => {
                            Some(PluginEffectDestination::Track(id))
                        }
                        device_panel::DevicePanelOwner::Return(id) => {
                            Some(PluginEffectDestination::Return(id))
                        }
                        device_panel::DevicePanelOwner::Main => Some(PluginEffectDestination::Main),
                        device_panel::DevicePanelOwner::Source(_) => None,
                    }),
                status: view.status.clone(),
                applied: view
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .and_then(|status| status.native_effect_source()),
                transport: view
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .map(|status| {
                        use std::sync::atomic::Ordering::Acquire;
                        (status.position.load(Acquire), status.playing.load(Acquire))
                    }),
            }
        })
        .map_err(|error| error.to_string())
}

pub(super) struct PendingPluginEffect {
    pub(super) status: String,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) destination: PluginEffectDestination,
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) before: Option<hex_project::DeviceId>,
    track: Option<ProposedPluginTrack>,
    control: Arc<PluginEffectControl>,
    trace: Option<crate::diagnostics::PluginLoad>,
}
impl Drop for PendingPluginEffect {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

pub(super) struct ProposedPluginTrack {
    before: Option<TrackId>,
    name: String,
}

pub(super) struct FailedPluginTrack {
    track: ProposedPluginTrack,
    control: Arc<PluginEffectControl>,
    error: String,
}

impl EditorView {
    fn plugin_track_proposal(
        &self,
    ) -> Option<(
        &ProposedPluginTrack,
        &Arc<PluginEffectControl>,
        Option<&str>,
    )> {
        if let Some((pending, track)) = self
            .pending_plugin_effect
            .as_ref()
            .filter(|pending| !pending.control.is_cancelled())
            .and_then(|pending| pending.track.as_ref().map(|track| (pending, track)))
        {
            return Some((track, &pending.control, None));
        }
        self.failed_plugin_track
            .as_ref()
            .map(|failed| (&failed.track, &failed.control, Some(failed.error.as_str())))
    }

    pub(super) fn proposed_plugin_track_anchor(&self) -> Option<Option<TrackId>> {
        self.plugin_track_proposal()
            .map(|(track, _, _)| track.before)
    }

    pub(super) fn restore_proposed_plugin_focus(&self, window: &mut Window) {
        if self
            .proposed_plugin_action_focus
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window))
            && (self.mode != EditorMode::Arrangement || self.plugin_track_proposal().is_none())
        {
            self.focus.focus(window);
        }
    }

    pub(super) fn proposed_plugin_track_row(&mut self, cx: &mut Context<Self>) -> Div {
        let Some((track, control, error)) = self.plugin_track_proposal() else {
            return div();
        };
        let title = hex_session::service::text::verbatim(&track.name);
        let failed = error.is_some();
        let detail = error.unwrap_or("loading vst3").to_owned();
        let tooltip = super::display_error::DisplayError::from(detail.clone());
        let control = Arc::clone(control);
        let focus = self
            .proposed_plugin_action_focus
            .get_or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
            .clone();
        let action = move |view: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            if failed {
                if view
                    .failed_plugin_track
                    .as_ref()
                    .is_some_and(|track| Arc::ptr_eq(&track.control, &control))
                {
                    view.failed_plugin_track = None;
                }
            } else if let Some(pending) = &view.pending_plugin_effect
                && Arc::ptr_eq(&pending.control, &control)
            {
                pending.control.cancel();
            }
            view.restore_proposed_plugin_focus(window);
            cx.stop_propagation();
            cx.notify();
        };
        let key_action = action.clone();
        let action_id = if failed {
            "arrangement-pending-device-dismiss"
        } else {
            "arrangement-pending-device-cancel"
        };
        let button = hex_widgets::action(
            theme::text(if failed { "dismiss" } else { "cancel" }),
            theme::action_style(),
        )
        .id(action_id)
        .debug_selector(move || action_id.into())
        .h(px(22.0))
        .px_2()
        .text_xs()
        .track_focus(&focus)
        .focus(|style| {
            style
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Text)))
        })
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |view, _, window, cx| action(view, window, cx)))
        .on_key_down(
            cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.modifiers == gpui::Modifiers::default()
                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                {
                    cx.stop_propagation();
                    if !event.is_held {
                        key_action(view, window, cx);
                    }
                }
            }),
        );
        div()
            .debug_selector(|| "arrangement-pending-device-track".into())
            .w_full()
            .h(px(super::tracks::TRACK_ROW_HEIGHT))
            .flex()
            .justify_end()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .child(
                div()
                    .w(px(self.track_header_width()))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_xs()
                            .child(div().truncate().child(theme::text(title)))
                            .child(
                                div()
                                    .text_color(rgb(theme::color(if failed {
                                        theme::PaletteRole::Error
                                    } else {
                                        theme::PaletteRole::Muted
                                    })))
                                    .truncate()
                                    .id("arrangement-pending-device-status")
                                    .debug_selector(|| "arrangement-pending-device-status".into())
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| {
                                            super::display_error::ErrorTooltip(tooltip.clone())
                                        })
                                        .into()
                                    })
                                    .child(theme::text(detail)),
                            ),
                    )
                    .children([button]),
            )
    }

    pub(super) fn reconcile_device_insert_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project || self.close_prompt_open || self.save_dialog_open {
            return;
        }
        let Some(offer) = self.device_insert_confirmation.take() else {
            return;
        };
        cx.defer_in(window, move |_, window, cx| {
            if !offer.is_pending() {
                return;
            }
            use hex_session::service::GraphEditOperation;
            let title = match offer.operation() {
                GraphEditOperation::InsertDevice => "interrupt playback to insert this device?",
                GraphEditOperation::RemoveDevice => "interrupt playback to remove this device?",
                GraphEditOperation::MoveDevice => "interrupt playback to move devices?",
                GraphEditOperation::RemoveTrack => "interrupt playback to remove this track?",
            };
            let answer = window.prompt(
                gpui::PromptLevel::Warning,
                &theme::text(title),
                Some(&theme::text(
                    "playback will fade out briefly and resume from the same position. \
                     held notes and effect tails may reset.",
                )),
                &[
                    gpui::PromptButton::cancel(theme::text("cancel")),
                    gpui::PromptButton::new(theme::text("proceed")),
                ],
                cx,
            );
            cx.spawn(async move |_, _| {
                if answer.await.ok() == Some(1) {
                    offer.proceed();
                } else {
                    offer.cancel();
                }
            })
            .detach();
        });
    }

    pub(super) fn cancel_plugin_effect(&self) {
        if let Some(pending) = &self.pending_plugin_effect {
            pending.control.cancel();
        }
    }

    pub(super) fn plugin_effect_complete(&mut self, event: &hex_session::service::Event) {
        let Some(terminal) = self.pending_plugin_effect.as_ref().and_then(|pending| {
            event
                .plugin_effect_complete
                .iter()
                .find(|terminal| Arc::ptr_eq(&pending.control, &terminal.control))
        }) else {
            return;
        };
        self.retire_plugin_effect(&terminal.control, terminal.accepted, event.error.as_deref());
    }

    pub(super) fn retire_plugin_effect(
        &mut self,
        control: &Arc<PluginEffectControl>,
        installed: bool,
        error: Option<&str>,
    ) -> bool {
        if self
            .pending_plugin_effect
            .as_ref()
            .is_none_or(|pending| !Arc::ptr_eq(&pending.control, control))
        {
            return false;
        }
        let Some(mut pending) = self.pending_plugin_effect.take() else {
            return false;
        };
        if let Some(trace) = &pending.trace {
            trace.complete(installed);
        }
        if !installed
            && !pending.control.is_cancelled()
            && let Some(track) = pending.track.take()
        {
            self.failed_plugin_track = Some(FailedPluginTrack {
                track,
                control: Arc::clone(control),
                error: error.unwrap_or("vst3 load failed").into(),
            });
        }
        true
    }

    pub(super) fn insert_browser_plugin_effect(
        &mut self,
        selection: hex_plugin_catalog::PluginSelection,
        cx: &App,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let destination = match device_panel::selected_owner(self, state) {
            Some(device_panel::DevicePanelOwner::Track(id)) => PluginEffectDestination::Track(id),
            Some(device_panel::DevicePanelOwner::Return(id)) => PluginEffectDestination::Return(id),
            Some(device_panel::DevicePanelOwner::Main) => PluginEffectDestination::Main,
            None | Some(device_panel::DevicePanelOwner::Source(_)) => {
                self.error =
                    Some("select a track, return or main before inserting an effect".into());
                return;
            }
        };
        self.insert_plugin_effect(&selection, destination, None, cx);
    }

    pub(super) fn insert_plugin_effect(
        &mut self,
        selection: &hex_plugin_catalog::PluginSelection,
        destination: PluginEffectDestination,
        before: Option<hex_project::DeviceId>,
        cx: &App,
    ) {
        let (destination, before) = match (selection.role, destination) {
            (hex_plugin_catalog::PluginRole::Instrument, PluginEffectDestination::Track(track)) => {
                (PluginEffectDestination::Instrument(track), None)
            }
            (
                hex_plugin_catalog::PluginRole::Instrument,
                PluginEffectDestination::ReplaceInstrument { .. }
                | PluginEffectDestination::NewTrack {
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    ..
                },
            ) => (destination, None),
            (hex_plugin_catalog::PluginRole::Instrument, _) => {
                self.error = Some("select a track for the vst3 instrument".into());
                return;
            }
            _ => (destination, before),
        };
        if self.pending_plugin_effect.is_some() {
            self.error = Some("wait for the pending vst3 insertion".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let control = Arc::new(PluginEffectControl::opening_editor());
        let request = PluginEffectInsertion {
            context: hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision: state.revision,
            },
            selection: PluginEffectSelection {
                module: selection.module.clone(),
                executable: selection.executable_relative_path.clone(),
                cid: selection.cid,
                executable_sha256: selection.executable_sha256.clone(),
                info_sha256: selection.info_sha256.clone(),
            },
            destination,
            before,
            track_device_name: matches!(destination, PluginEffectDestination::NewTrack { .. })
                .then(|| selection.class_name.clone()),
            control: Arc::clone(&control),
        };
        let trace = crate::diagnostics::PluginLoad::new(
            cx,
            &selection.class_name,
            request.context,
            destination,
            state.project.sample_rate(),
        );
        if self.enqueue(Request::InsertPluginEffect(request)) {
            if let Some(trace) = &trace {
                trace.submitted();
            }
            let status = format!("loading vst3 · {}", selection.class_name);
            self.status.clone_from(&status);
            self.pending_plugin_effect = Some(PendingPluginEffect {
                control,
                trace,
                status,
                track: match destination {
                    PluginEffectDestination::NewTrack { before_track, .. } => {
                        Some(ProposedPluginTrack {
                            before: before_track,
                            name: selection.class_name.clone(),
                        })
                    }
                    _ => None,
                },
                #[cfg(all(test, feature = "ui-test-support"))]
                destination,
                #[cfg(all(test, feature = "ui-test-support"))]
                before,
            });
            self.failed_plugin_track = None;
            self.error = None;
        }
    }
}

// Catalog transport receipts below are synthetic fixture metadata only. Actual
// source revalidation and all native preparation/ACK remain with the worker.
#[cfg(feature = "ui-test-support")]
pub(crate) fn fixture_catalog(
    root: &std::path::Path,
    selected: &PluginEffectSelection,
    subcategories: &str,
    factory_flags: u32,
) -> Result<(), String> {
    selected.validate()?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let temporary = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !root.starts_with(&temporary) {
        return Err("plugin fixture requires an owned temporary root".into());
    }
    let report = serde_json::from_value(serde_json::json!({
        "profile":"explicit-user-trusted-macos-factory-only","request":1,"generation":1,
        "receipt_sha256":"a".repeat(64),"helper_sha256":"b".repeat(64),
        "bundle":{"supplied_path":selected.module,"canonical_path":selected.module.canonicalize().map_err(|error| error.to_string())?,"info_sha256":selected.info_sha256},
        "executable":{"relative_path":selected.executable,"sha256":selected.executable_sha256,"image":"thin_arm64"},
        "bundle_identifier":"org.hex.original","interruption":null,"error":null,"stdout":"","stderr":"",
        "cleanup":{"direct_reaped":true,"direct_exit_code":0,"direct_exit_signal":null,"custody_lost":false,"cleanup_unconfirmed":false,
          "exit_observed_before_signals":true,"original_group_term":{"status":"absent"},"original_group_kill":{"status":"absent"},"direct_kill":null,"descendants":"unknown","pending":false},
        "discovery":{"request":1,"sdk":"3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96","public_sdk":"586dc5e6c8012c3e4b01c79389375cbe96bdb1da","platform":"darwin-arm64",
          "factory_flags":factory_flags,"vendor":"original fixture","url":"","email":"","classes":[{"cid":selected.cid,"cardinality":1,"category":"Audio Module Class","name":"original processor","class_flags":0,"subcategories":subcategories,"class_vendor":"original fixture","class_version":"1","class_sdk_version":"3.8"}]}
    })).map_err(|error| error.to_string())?;
    hex_plugin_catalog::storage::update(&root.join("plugin-catalog.json"), |catalog| {
        catalog.record(selected.module.clone(), Ok((report, "e".repeat(40))))
    })
    .map(|_| ())
}
