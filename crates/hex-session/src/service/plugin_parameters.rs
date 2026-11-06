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
}

pub struct PluginParameterResult {
    pub query: PluginParameterQuery,
    pub rows: Result<Vec<PluginParameterRow>, String>,
}
