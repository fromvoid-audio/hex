//! Project projections of captured native state for Save, bounce and track duplication.
//! Session validates complete project state and provider/binary identity without
//! owning SDK instances. These snapshots never change canonical state or history.
use super::*;
use hex_engine::ProcessorChainOwner;
use std::collections::BTreeMap;
#[cfg(test)]
mod tests;

/// One exact active device's original authored input and fresh component/
/// controller bytes. The native adapter must obtain these from that device's
/// actual matching Root source, drain every admitted request to its terminal,
/// and preserve source/request cancellation. This is data, not source authority.
pub struct PluginStateCapture {
    pub owner: ProcessorChainOwner,
    pub device_id: u64,
    pub expected: NativeDeviceState,
    pub captured: NativeDeviceState,
}

pub struct PluginStateCaptureInput<'a> {
    pub(super) project: &'a Project,
    pub(super) context: hex_engine::AudioSourceContext,
    pub(super) cancellation: &'a RequestCancellation,
    /// Optional set of target device IDs. The canonical project still
    /// resolves the actual hosted owner, even when a selected track only
    /// contributes MIDI to a shared instrument.
    pub(super) device_filter: Option<&'a BTreeMap<u64, ProcessorChainOwner>>,
}
impl PluginStateCaptureInput<'_> {
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn cancellation(&self) -> &RequestCancellation {
        self.cancellation
    }
    /// Reading raw targets retained in the canonical saved project. Native
    /// source metadata must independently validate their exact class and bases.
    pub fn authored_plugin_targets(
        &self,
        device: u64,
    ) -> impl Iterator<Item = (u32, Option<[u8; 16]>)> + '_ {
        self.project
            .automation_devices()
            .find(|candidate| candidate.id().value() == device)
            .into_iter()
            .flat_map(hex_project::Device::reading_automation)
            .filter_map(|lane| match lane.target {
                hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter } => {
                    Some((parameter, lane.vst3_class))
                }
                _ => None,
            })
    }

    pub fn visit_plugin_devices(
        &self,
        mut visit: impl FnMut(ProcessorChainOwner, u64, &NativeDeviceState) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut device = |owner, device: &hex_project::Device| -> Result<(), String> {
            if device.state().type_id() == hex_plugin_state::TYPE_ID
                && self
                    .device_filter
                    .is_none_or(|devices| devices.contains_key(&device.id().value()))
            {
                visit(owner, device.id().value(), device.state())?;
            }
            Ok(())
        };
        for track in self.project.tracks() {
            let owner = ProcessorChainOwner::Track(track.id().value());
            for effect in track.devices() {
                device(owner, effect)?;
            }
            if let Some(instrument) = track.instrument().filter(|_| track.receives_source_audio()) {
                device(owner, instrument)?;
            }
        }
        for track in self.project.return_tracks() {
            for effect in track.devices() {
                device(ProcessorChainOwner::Return(track.id().value()), effect)?;
            }
        }
        for effect in self.project.main_devices() {
            device(ProcessorChainOwner::Main, effect)?;
        }
        Ok(())
    }
}

pub(super) fn project_for_save_with_capture(
    session: &Session,
    media_base: Option<&Path>,
    context: hex_engine::AudioSourceContext,
    cancellation: &RequestCancellation,
    audio: Option<&mut (dyn WorkerAudioSink + 'static)>,
) -> Result<Project, String> {
    let project = super::recovery::project_for_save(session, media_base)?;
    let captures = match audio {
        Some(audio) => audio.capture_plugin_states(PluginStateCaptureInput {
            project: session.project(),
            context,
            cancellation,
            device_filter: None,
        })?,
        None => Vec::new(),
    };
    if !captures.is_empty() && cancellation.is_cancelled() {
        return Err("project save cancelled".into());
    }
    project_with_captures(project, captures)
}

/// Snapshot current hosted state into an isolated export projection without
/// editing the canonical project or transferring live processor custody.
pub(super) fn project_for_export_with_capture(
    project: &Project,
    context: hex_engine::AudioSourceContext,
    cancellation: &RequestCancellation,
    audio: &mut dyn WorkerAudioSink,
) -> Result<Project, String> {
    if cancellation.is_cancelled() {
        return Err("export cancelled".into());
    }
    let captures = audio.capture_plugin_states(PluginStateCaptureInput {
        project,
        context,
        cancellation,
        device_filter: None,
    })?;
    if cancellation.is_cancelled() {
        return Err("export cancelled".into());
    }
    project_with_captures(project.clone(), captures)
}

/// Freeze the selected tracks from a temporary projection of their live
/// plug-in state. The original canonical track and its runtime stay owned by
/// the existing edit transaction, including when capture or paste is refused.
pub(super) fn capture_tracks_for_duplicate(
    project: &Project,
    tracks: &hex_project::TrackSelectionIds,
    without_clips: bool,
    context: hex_engine::AudioSourceContext,
    cancellation: &RequestCancellation,
    audio: Option<&mut (dyn WorkerAudioSink + 'static)>,
) -> Result<hex_project::TrackClipboard, String> {
    let capture_tracks = |project: &Project| {
        project
            .capture_tracks(tracks, without_clips)
            .map_err(|error| error.to_string())
    };
    let Some(audio) = audio else {
        return capture_tracks(project);
    };
    let expanded = project
        .expand_track_selection(tracks)
        .map_err(|error| error.to_string())?;
    let mut devices = BTreeMap::new();
    for id in expanded.ids() {
        let track = project
            .track(*id)
            .ok_or("selected track no longer exists")?;
        for device in track.devices().iter().chain(track.instrument()) {
            if device.state().type_id() == hex_plugin_state::TYPE_ID {
                devices.insert(device.id().value(), ProcessorChainOwner::Track(id.value()));
            }
        }
    }
    if devices.is_empty() {
        return capture_tracks(project);
    }
    if cancellation.is_cancelled() {
        return Err("track duplication cancelled".into());
    }
    let captures = audio.capture_plugin_states(PluginStateCaptureInput {
        project,
        context,
        cancellation,
        device_filter: Some(&devices),
    })?;
    if cancellation.is_cancelled() {
        return Err("track duplication cancelled".into());
    }
    if captures.is_empty() {
        return capture_tracks(project);
    }
    if captures
        .iter()
        .any(|capture| !devices.contains_key(&capture.device_id))
    {
        return Err("captured vst3 device is outside the track selection".into());
    }
    let projected = project_with_captures(project.clone(), captures)?
        .with_offline_assets(project.offline_assets())
        .map_err(|error| error.to_string())?;
    capture_tracks(&projected)
}

/// Refresh only the isolated graph's processors, using the actual live
/// owner to capture and the isolated owner to compile. A shared instrument
/// can have a different receiver in the bounce than in the live graph.
pub(super) fn project_for_bounce_with_capture(
    canonical: &Project,
    isolated: &ResidentEffectPreparation<'_>,
    cancellation: &RequestCancellation,
    sink: &mut dyn WorkerAudioSink,
) -> Result<Option<Project>, String> {
    let mut devices = BTreeMap::new();
    isolated.visit_plugin_devices(|destination, id, _| {
        if devices.insert(id, destination.owner()).is_some() {
            return Err("duplicate offline vst3 processor".into());
        }
        Ok(())
    })?;
    if devices.is_empty() {
        return Ok(None);
    }
    if cancellation.is_cancelled() {
        return Err("bounce cancelled".into());
    }
    let mut captures = sink.capture_plugin_states(PluginStateCaptureInput {
        project: canonical,
        context: hex_engine::AudioSourceContext {
            editor_revision: isolated.context.editor_revision.saturating_sub(1),
            ..isolated.context
        },
        cancellation,
        device_filter: Some(&devices),
    })?;
    if cancellation.is_cancelled() {
        return Err("bounce cancelled".into());
    }
    if captures.is_empty() {
        return Ok(None);
    }
    for capture in &mut captures {
        capture.owner = *devices
            .get(&capture.device_id)
            .ok_or("captured vst3 device is outside the bounce")?;
    }
    let project = isolated.project;
    let projected = project_with_captures(project.clone(), captures)?
        .with_offline_assets(project.offline_assets())
        .map_err(|error| error.to_string())?;
    Ok(Some(projected))
}

pub(super) fn project_with_captures(
    project: hex_project::Project,
    captures: Vec<PluginStateCapture>,
) -> Result<hex_project::Project, String> {
    if captures.is_empty() {
        return Ok(project);
    }
    let mut snapshot = project.snapshot();
    let count = snapshot
        .tracks
        .iter()
        .map(|track| track.devices.len() + usize::from(track.instrument.is_some()))
        .chain(
            snapshot
                .return_tracks
                .iter()
                .map(|track| track.devices.len()),
        )
        .chain(std::iter::once(snapshot.main_devices.len()))
        .sum::<usize>();
    if captures.len() > count {
        return Err("captured device inventory exceeds the project snapshot".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for capture in captures {
        if !seen.insert(capture.device_id) {
            return Err("duplicate captured device state".into());
        }
        let target = match capture.owner {
            ProcessorChainOwner::Source(_) => {
                return Err("captured source effects are not supported".into());
            }
            ProcessorChainOwner::Track(id) => {
                let track = snapshot
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == id)
                    .ok_or("captured track differs from the project snapshot")?;
                if track
                    .instrument
                    .as_ref()
                    .is_some_and(|device| device.id == capture.device_id)
                {
                    track.instrument.as_mut()
                } else {
                    track
                        .devices
                        .iter_mut()
                        .find(|device| device.id == capture.device_id)
                }
            }
            ProcessorChainOwner::Return(id) => snapshot
                .return_tracks
                .iter_mut()
                .find(|track| track.id == id)
                .ok_or("captured return differs from the project snapshot")?
                .devices
                .iter_mut()
                .find(|device| device.id == capture.device_id),
            ProcessorChainOwner::Main => snapshot
                .main_devices
                .iter_mut()
                .find(|device| device.id == capture.device_id),
        };
        let target = target.ok_or("captured device differs from the project snapshot")?;
        if !target.state.processing_eq(&capture.expected) {
            return Err("captured device source changed before projection".into());
        }
        validate_capture(&capture.expected, &capture.captured)?;
        target.state = capture
            .captured
            .with_input_gain_millidb(target.state.input_gain_millidb())
            .and_then(|state| state.with_output_gain_millidb(target.state.output_gain_millidb()))
            .map_err(|_| "captured state host gains are invalid")?;
        // Bound instruments are resolved views of the retained registry owner.
        // Project's strict current snapshot requires both representations to
        // agree. Propagate the capture to the registry and every track
        // projection of the same instrument. Inactive comparison state stays intact.
        let saved_state = target.state.clone();
        if let Some(source_device) = snapshot
            .native_sources
            .iter_mut()
            .flat_map(|source| &mut source.devices)
            .find(|device| device.id == capture.device_id)
        {
            source_device.state = saved_state.clone();
            for instrument in snapshot
                .tracks
                .iter_mut()
                .filter_map(|track| track.instrument.as_mut())
                .filter(|device| device.id == capture.device_id)
            {
                instrument.state = saved_state.clone();
            }
        }
    }
    hex_project::Project::from_snapshot(snapshot)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))
}

/// A control capture may replace opaque bytes only for the same retained source.
pub(super) fn validate_capture(
    expected_state: &NativeDeviceState,
    captured: &NativeDeviceState,
) -> Result<(), &'static str> {
    let expected = hex_plugin_state::decode(expected_state)
        .map_err(|_| "captured source envelope is unavailable")?;
    expected_state
        .require_asset_free_processing()
        .map_err(|_| "captured source has unsupported asset bindings")?;
    let current =
        hex_plugin_state::decode(captured).map_err(|_| "captured state envelope is invalid")?;
    captured
        .require_asset_free_processing()
        .map_err(|_| "captured state has unsupported asset bindings")?;
    if current.identity != expected.identity
        || expected.component.is_none()
        || current.component.is_none()
    {
        return Err("captured state does not match original plugin source");
    }
    Ok(())
}
