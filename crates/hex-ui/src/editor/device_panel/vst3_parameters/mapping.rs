//! Hosted VST3 parameter modulation mapping targets.
use super::*;

/// Fresh loaded-row facts retained across a mapping pointer gesture.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::editor) struct MappingTarget {
    pub(in crate::editor) parameter: u32,
    pub(in crate::editor) class: [u8; 16],
    source: hex_lattice_contract::ForeignProcessorBinding,
    metadata_revision: u64,
}

pub(in crate::editor) fn mapping_target(
    view: &EditorView,
    device: DeviceId,
    row: &PluginParameterRow,
) -> Option<MappingTarget> {
    let panel = view.device_panel.vst3_parameters.panels.get(&device)?;
    let source = panel.source?;
    let class = row.automation_class?;
    if !row.editable
        || row.steps != 0
        || row.metadata_revision == 0
        || view.audio_pub.stopped_playback_restore.is_some()
        || view.device_panel.degraded_plugins.contains(device.value())
        || current_context(view) != Some(panel.query.context)
        || view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            != Some(panel.query.output_identity)
    {
        return None;
    }
    let state = view.state.as_ref()?;
    super::super::modulators::vst3_destination(&state.project, device, row.id, class).ok()?;
    Some(MappingTarget {
        parameter: row.id,
        class,
        source,
        metadata_revision: row.metadata_revision,
    })
}

pub(in crate::editor) fn mapping_target_is_current(
    view: &EditorView,
    device: DeviceId,
    target: MappingTarget,
) -> bool {
    view.device_panel
        .vst3_parameters
        .panels
        .get(&device)
        .and_then(Panel::rows)
        .and_then(|rows| rows.iter().find(|row| row.id == target.parameter))
        .and_then(|row| mapping_target(view, device, row))
        == Some(target)
}

pub(in crate::editor) fn has_mapping_target(view: &EditorView, device: DeviceId) -> bool {
    view.device_panel
        .vst3_parameters
        .panels
        .get(&device)
        .and_then(Panel::rows)
        .is_some_and(|rows| {
            rows.iter()
                .any(|row| mapping_target(view, device, row).is_some())
        })
}

pub(in crate::editor) fn mapping_label(
    view: &EditorView,
    device: DeviceId,
    id: u32,
) -> Option<&str> {
    view.device_panel
        .vst3_parameters
        .panels
        .get(&device)?
        .rows()?
        .iter()
        .find(|row| row.id == id)
        .map(|row| row.name.as_str())
}

pub(in crate::editor) fn dense_target_owns_parameter(
    view: &EditorView,
    device: DeviceId,
    id: u32,
) -> bool {
    view.state
        .as_ref()
        .and_then(|state| state.project.device(device))
        .and_then(|device| {
            hex_engine::project_vst3_modulation_target(device)
                .ok()
                .flatten()
        })
        .is_some_and(|(raw, _)| raw == id)
}
