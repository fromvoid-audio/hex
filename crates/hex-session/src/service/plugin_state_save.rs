//! Save-only projection of captured native state. Session validates complete
//! project state and provider/binary identity without owning SDK instances.
//! Canonical state/history and inactive comparison data remain untouched.
use super::*;
use hex_engine::ProcessorChainOwner;
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

pub struct PluginStateSaveInput<'a> {
    pub(super) project: &'a Project,
    pub(super) context: hex_engine::AudioSourceContext,
    pub(super) cancellation: &'a RequestCancellation,
}
impl PluginStateSaveInput<'_> {
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn cancellation(&self) -> &RequestCancellation {
        self.cancellation
    }
    pub fn visit_plugin_devices(
        &self,
        mut visit: impl FnMut(ProcessorChainOwner, u64, &NativeDeviceState) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut device = |owner, device: &hex_project::Device| -> Result<(), String> {
            if device.state().type_id() == hex_plugin_state::TYPE_ID {
                visit(owner, device.id().value(), device.state())?;
            }
            Ok(())
        };
        for track in self.project.tracks() {
            let owner = ProcessorChainOwner::Track(track.id().value());
            for effect in track.devices() {
                device(owner, effect)?;
            }
            if let Some(instrument) = track.instrument() {
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
        Some(audio) => audio.capture_plugin_states_for_save(PluginStateSaveInput {
            project: session.project(),
            context,
            cancellation,
        })?,
        None => Vec::new(),
    };
    if !captures.is_empty() && cancellation.is_cancelled() {
        return Err("project save cancelled".into());
    }
    project_with_captures(project, captures)
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
        return Err("captured device inventory exceeds saved project".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for capture in captures {
        if !seen.insert(capture.device_id) {
            return Err("duplicate captured device state".into());
        }
        let target = match capture.owner {
            ProcessorChainOwner::Track(id) => {
                let track = snapshot
                    .tracks
                    .iter_mut()
                    .find(|track| track.id == id)
                    .ok_or("captured track differs from saved project")?;
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
                .ok_or("captured return differs from saved project")?
                .devices
                .iter_mut()
                .find(|device| device.id == capture.device_id),
            ProcessorChainOwner::Main => snapshot
                .main_devices
                .iter_mut()
                .find(|device| device.id == capture.device_id),
        };
        let target = target.ok_or("captured device differs from saved project")?;
        if !target.state.processing_eq(&capture.expected) {
            return Err("captured device source changed before saving".into());
        }
        validate_capture(&capture.expected, &capture.captured)?;
        target.state = capture
            .captured
            .with_input_gain_millidb(target.state.input_gain_millidb())
            .and_then(|state| state.with_output_gain_millidb(target.state.output_gain_millidb()))
            .map_err(|_| "captured state host gains are invalid")?;
        // Bound instruments are resolved views of the retained registry owner.
        // Project's strict current snapshot requires both representations to
        // agree. This save-only clone updates that same globally unique device;
        // inactive comparison state and every unrelated source remain intact.
        let saved_state = target.state.clone();
        if let Some(source_device) = snapshot
            .native_sources
            .iter_mut()
            .flat_map(|source| &mut source.devices)
            .find(|device| device.id == capture.device_id)
        {
            source_device.state = saved_state;
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
