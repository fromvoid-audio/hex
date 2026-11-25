//! Hosted VST3 parameter touch tracking and refresh queuing.
use super::*;

pub(in crate::editor) fn current_context(view: &EditorView) -> Option<AudioSourceContext> {
    Some(AudioSourceContext {
        project_generation: view.project_generation,
        editor_revision: view.state.as_ref()?.revision,
    })
}

pub(in crate::editor) fn current_applied_source(view: &EditorView) -> Option<AppliedAudioSource> {
    view.audio
        .as_ref()
        .and_then(|audio| audio.status())
        .and_then(|status| status.native_effect_source())
}

/// A panel can outlive the original hosted endpoint while Stop or an output
/// restart prepares its replacement. Only the installed output can supply
/// inventory, and its exact revision fences late background terminals.
pub(in crate::editor) fn refresh_source(view: &EditorView) -> Option<AppliedAudioSource> {
    let source = current_applied_source(view)?;
    let output = view.audio.as_ref()?.control.output_identity()?;
    (source.output_identity == output
        && source.context.project_generation == view.project_generation
        && view.stopped_playback_restore.is_none()
        && view.recovery.generation().is_none())
    .then_some(source)
}

pub(in crate::editor) fn refreshable_device(
    view: &EditorView,
    device: DeviceId,
    source: AppliedAudioSource,
) -> bool {
    !view.device_panel.degraded_plugins.contains(device.value())
        && view.audible_playback.as_ref().is_some_and(|audible| {
            audible.audio_revision == source.audio_revision
                && audible.project_generation == source.context.project_generation
                && super::super::audible_vst3_processor(view, device)
                    == Ok(hex_engine::PreparedDeviceProcessing::Foreign)
        })
}

pub(in crate::editor) fn touch_is_current(
    view: &EditorView,
    device: DeviceId,
    touch: TouchObservation,
) -> bool {
    if !touch.binding.is_valid() {
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
        return false;
    };
    let Some(context) = current_context(view) else {
        return false;
    };
    touch.source.context.project_generation == context.project_generation
        && touch.source.context.editor_revision >= context.editor_revision
        && view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            == Some(touch.source.output_identity)
        && current_applied_source(view) == Some(touch.source)
        && panel.query.context.project_generation == touch.source.context.project_generation
        && panel.query.output_identity == touch.source.output_identity
        && view.state.as_ref().is_some_and(|state| {
            super::super::chain_ids(&state.project, panel.owner).contains(&device)
        })
}

pub(in crate::editor) fn touched(
    view: &mut EditorView,
    source: AppliedAudioSource,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    device_id: u64,
    parameter: u32,
    sequence: u64,
    value: Option<PluginParameterValue>,
) {
    let Some(device) = view
        .device_panel
        .vst3_parameters
        .panels
        .keys()
        .copied()
        .find(|device| device.value() == device_id)
    else {
        return;
    };
    let touch = TouchObservation {
        source,
        binding,
        parameter,
    };
    if !touch_is_current(view, device, touch) {
        return;
    }
    view.device_panel.vst3_parameters.last_touched_device = Some(device);
    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
        panel.observe_value(touch, sequence, value);
        queue_native_touch(panel, touch);
    }
}

// Called only after the current applied source and binding have been checked.
// Repeated Perform notifications are refresh hints, not new touch identities.
pub(in crate::editor) fn queue_native_touch(panel: &mut Panel, touch: TouchObservation) {
    if panel.queued_touch == Some(touch)
        || panel.queued_touch.is_none() && panel.pending_touch == Some(touch)
    {
        // An earlier request can return a useful intermediate value while the
        // pointer is still moving. Do not orphan that request on every Perform.
        return;
    }
    let page = (panel.query.context == touch.source.context
        && panel.source == Some(touch.binding)
        && panel.last_touched == Some(touch.parameter))
    .then(|| {
        panel.rows()?.iter().find_map(|row| {
            (row.id == touch.parameter && row.metadata_revision != 0).then_some(row.page_first)
        })
    })
    .flatten();
    if let Some(page) = page {
        panel.queued_touch = None;
        panel.pending_touch = None;
        panel.pending_touch_request = None;
        // Prioritize the known target at the next scheduled read. Repeated
        // Perform callbacks must not bypass the refresh cadence.
        panel.priority_page = Some(page);
    } else {
        panel.queued_touch = Some(touch);
        panel.pending_touch_request = None;
    }
}

pub(in crate::editor) fn queue_host_touch(
    view: &mut EditorView,
    device: DeviceId,
    parameter: u32,
    cx: &mut Context<EditorView>,
) {
    let Some(source) = current_applied_source(view) else {
        return;
    };
    let Some(binding) = view
        .device_panel
        .vst3_parameters
        .panels
        .get(&device)
        .and_then(|panel| panel.source)
    else {
        return;
    };
    let touch = TouchObservation {
        source,
        binding,
        parameter,
    };
    if !touch_is_current(view, device, touch) {
        return;
    }
    let preview = view
        .device_panel
        .vst3_parameters
        .panels
        .get_mut(&device)
        .and_then(|panel| {
            let row = panel
                .rows()
                .and_then(|rows| rows.iter().find(|row| row.id == parameter))?
                .clone();
            if row.metadata_revision == 0 {
                return None;
            }
            panel.last_touched = Some(parameter);
            panel.touch_unavailable = !row.editable;
            panel.pending_touch = None;
            panel.queued_touch = None;
            panel.pending_touch_request = None;
            Some((panel.owner, row))
        });
    if let Some((owner, row)) = preview {
        view.device_panel.vst3_parameters.last_touched_device = Some(device);
        view.remember_touched_vst3(owner, device, &row, cx);
    }
}

pub(in crate::editor) fn apply_touch(
    view: &mut EditorView,
    device: DeviceId,
    touch: TouchObservation,
    cx: &mut Context<EditorView>,
) -> bool {
    if !touch_is_current(view, device, touch) {
        if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
            if panel.queued_touch == Some(touch) {
                panel.queued_touch = None;
            }
            if panel.pending_touch == Some(touch) {
                panel.pending_touch = None;
            }
        }
        return false;
    }
    if current_context(view) != Some(touch.source.context) {
        if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
            panel.queued_touch = None;
            panel.pending_touch = Some(touch);
            panel.pending_touch_request = None;
        }
        return false;
    }
    let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
        return false;
    };
    let row = panel
        .rows()
        .and_then(|rows| rows.iter().find(|row| row.id == touch.parameter))
        .cloned();
    queue_touch_refresh(panel, touch, row.as_ref());
    cx.notify();
    false
}

pub(in crate::editor) fn queue_touch_refresh(
    panel: &mut Panel,
    touch: TouchObservation,
    row: Option<&PluginParameterRow>,
) {
    let page = row.map(|row| row.page_first);
    // Keep the last validated presentation until the new target resolves.
    // A pending touch suspends follow-control input without erasing its label.
    panel.touch_unavailable = false;
    panel.queued_touch = None;
    panel.pending_touch = Some(touch);
    panel.pending_touch_request = None;
    if panel.source != Some(touch.binding) {
        panel.mark_values_stale();
        panel.pending_touch = Some(touch);
        panel.priority_page = page;
    } else if let Some(page) = page {
        panel.priority_page = Some(page);
    } else {
        // Unknown IDs require a full inventory query, not removal of the
        // current inventory. A plug-in can report non-exposed edit targets.
        panel.priority_page = None;
    }
    panel.next_query_at = Instant::now();
}

pub(in crate::editor) fn resolve_pending_touch(
    panel: &mut Panel,
    source: hex_lattice_contract::ForeignProcessorBinding,
    fresh_rows: &[PluginParameterRow],
    page_first: Option<u16>,
    is_touch_query: bool,
) -> Option<(super::super::DevicePanelOwner, PluginParameterRow)> {
    if !is_touch_query {
        return None;
    }
    let touch = panel.pending_touch?;
    panel.pending_touch_request = None;
    if touch.binding != source {
        panel.pending_touch = None;
        panel.last_touched = None;
        panel.touch_unavailable = false;
        return None;
    }
    if let Some(row) = fresh_touch_row(touch, source, fresh_rows, page_first) {
        panel.last_touched = Some(touch.parameter);
        panel.touch_unavailable = !row.editable;
        panel.pending_touch = None;
        return Some((panel.owner, row));
    }
    panel.last_touched = Some(touch.parameter);
    panel.touch_unavailable = true;
    panel.pending_touch = None;
    None
}

pub(in crate::editor) fn parameter_origin(
    query: PluginParameterQuery,
    row: &PluginParameterRow,
) -> Option<PluginParameterOrigin> {
    Some(PluginParameterOrigin {
        context: query.context,
        device: query.device,
        parameter: row.id,
        page_first: row.page_first,
        metadata_revision: row.metadata_revision,
        initial: PluginParameterValue::new(row.normalized).ok()?,
    })
}

pub(in crate::editor) fn cache_location(
    view: &EditorView,
    device: DeviceId,
) -> Option<(String, PathBuf)> {
    let state = view.state.as_ref()?.project.device(device)?.state();
    let key = hex_session::service::vst3_parameter_cache_key(state)?;
    let home = super::super::super::user_home_directory()?;
    let root = hex_preferences::browser_places_path(&home).with_file_name("vst3-parameters");
    Some((key.clone(), root.join(format!("{key}.json"))))
}
