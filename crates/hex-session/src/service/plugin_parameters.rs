//! Copied VST3 controller inventory. The native sink owns every worker query.
use hex_engine::AudioSourceContext;
use hex_project::DeviceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginParameterQuery {
    pub context: AudioSourceContext,
    pub device: DeviceId,
    pub output_identity: u64,
    pub request_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginParameterRow {
    pub id: u32,
    pub name: String,
    pub value: String,
    pub normalized: f64,
    pub default_normalized: f64,
    pub steps: u32,
    pub editable: bool,
    /// Class of the exact source that reported automation eligibility.
    pub automation_class: Option<[u8; 16]>,
    pub page_first: u16,
    pub metadata_revision: u64,
}

pub struct PluginParameterResult {
    pub query: PluginParameterQuery,
    pub rows: Result<Vec<PluginParameterRow>, String>,
}

/// Saved class identity only. Live automation eligibility belongs to the
/// loaded SDK worker and is checked again during candidate preparation.
pub fn vst3_automation_class(state: &hex_lattice_contract::NativeDeviceState) -> Option<[u8; 16]> {
    hex_plugin_state::decode(state)
        .ok()
        .map(|plugin| plugin.identity.class.bytes())
}
