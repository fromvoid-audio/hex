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
            crate::PluginEffectTestView {
                state: view.state.clone(),
                pending: view.pending_plugin_effect.is_some(),
                queued_requests: view.worker_requests_pending
                    + view.pending_requests.len()
                    + view.device_gesture_barriers.len(),
                operation: view
                    .pending_plugin_effect
                    .as_ref()
                    .map(|pending| Arc::downgrade(&pending.0)),
                error: view.error.as_ref().map(ToString::to_string),
                selected_owner: view
                    .state
                    .as_ref()
                    .and_then(|state| device_panel::selected_owner(view, state))
                    .map(|owner| match owner {
                        device_panel::DevicePanelOwner::Track(id) => {
                            PluginEffectDestination::Track(id)
                        }
                        device_panel::DevicePanelOwner::Return(id) => {
                            PluginEffectDestination::Return(id)
                        }
                        device_panel::DevicePanelOwner::Main => PluginEffectDestination::Main,
                    }),
                status: view.status.clone(),
                applied: view
                    .audio
                    .as_ref()
                    .and_then(|audio| audio.status())
                    .and_then(|status| status.native_effect_source()),
            }
        })
        .map_err(|error| error.to_string())
}

pub(super) struct PendingPluginEffect(Arc<PluginEffectControl>);
impl Drop for PendingPluginEffect {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl EditorView {
    pub(super) fn cancel_plugin_effect(&self) {
        if let Some(pending) = &self.pending_plugin_effect {
            pending.0.cancel();
        }
    }

    pub(super) fn plugin_effect_complete(&mut self, event: &hex_session::service::Event) {
        if self.pending_plugin_effect.as_ref().is_some_and(|pending| {
            event
                .plugin_effect_complete
                .iter()
                .any(|terminal| Arc::ptr_eq(&pending.0, &terminal.control))
        }) {
            self.pending_plugin_effect.take();
        }
    }

    pub(super) fn insert_browser_plugin_effect(
        &mut self,
        selection: hex_plugin_catalog::EffectSelection,
    ) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let destination = match device_panel::selected_owner(self, state) {
            Some(device_panel::DevicePanelOwner::Track(id)) => PluginEffectDestination::Track(id),
            Some(device_panel::DevicePanelOwner::Return(id)) => PluginEffectDestination::Return(id),
            Some(device_panel::DevicePanelOwner::Main) => PluginEffectDestination::Main,
            None => {
                self.error =
                    Some("select a track, return or main before inserting an effect".into());
                return;
            }
        };
        self.insert_plugin_effect(&selection, destination, None);
    }

    pub(super) fn insert_plugin_effect(
        &mut self,
        selection: &hex_plugin_catalog::EffectSelection,
        destination: PluginEffectDestination,
        before: Option<hex_project::DeviceId>,
    ) {
        if self.pending_plugin_effect.is_some() {
            self.error = Some("wait for the pending vst3 effect insertion".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let control = Arc::new(PluginEffectControl::default());
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
            control: Arc::clone(&control),
        };
        if self.enqueue(Request::InsertPluginEffect(request)) {
            self.pending_plugin_effect = Some(PendingPluginEffect(control));
            self.status = "loading vst3 effect".into();
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
