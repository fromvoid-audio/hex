//! Browser intent and cancellation only. Session retains the admitted request;
//! the audio owner retains native resources through its exact terminal.
use super::*;
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};

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
        if self.pending_plugin_effect.is_some() {
            self.error = Some("wait for the pending vst3 effect insertion".into());
            return;
        }
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
        let control = Arc::new(PluginEffectControl::default());
        let request = PluginEffectInsertion {
            context: hex_engine::AudioSourceContext {
                project_generation: self.project_generation,
                editor_revision: state.revision,
            },
            selection: PluginEffectSelection {
                module: selection.module,
                executable: selection.executable_relative_path,
                cid: selection.cid,
                executable_sha256: selection.executable_sha256,
                info_sha256: selection.info_sha256,
            },
            destination,
            before: None,
            control: Arc::clone(&control),
        };
        if self.enqueue(Request::InsertPluginEffect(request)) {
            self.pending_plugin_effect = Some(PendingPluginEffect(control));
            self.status = "loading vst3 effect".into();
            self.error = None;
        }
    }
}
