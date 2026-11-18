//! Browser intent and cancellation only. Session retains the admitted request;
//! the audio owner retains native resources through its exact terminal.
use super::*;
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
    control: Arc<PluginEffectControl>,
    trace: Option<crate::diagnostics::PluginLoad>,
}
impl Drop for PendingPluginEffect {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

impl EditorView {
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
            let answer = window.prompt(
                gpui::PromptLevel::Warning,
                &theme::text("interrupt playback to insert this device?"),
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
        if let Some(pending) = self.pending_plugin_effect.take()
            && let Some(trace) = &pending.trace
        {
            trace.complete(terminal.accepted);
        }
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
                #[cfg(all(test, feature = "ui-test-support"))]
                destination,
                #[cfg(all(test, feature = "ui-test-support"))]
                before,
            });
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
