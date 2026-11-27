//! Device, plugin, sampler, automation, preset and source-utility worker operations.
use super::*;
use std::ops::ControlFlow;

/// Registered presentation name, offered as the automatic track name on device
/// loads; `hex_project::track_name` owns whether the track adopts it.
pub(crate) fn native_device_name(
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<String, String> {
    hex_engine::native_device_catalog_for_class(class)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?
        .into_iter()
        .find(|entry| entry.type_id == type_id)
        .map_or_else(
            // Unlisted definitions present their type identifier, as device views do.
            || {
                type_id
                    .as_str()
                    .map(str::to_owned)
                    .map_err(|_| "native device type identifier is invalid".to_owned())
            },
            |entry| Ok(entry.name),
        )
}

pub(crate) fn device_track_command(
    before_track: Option<hex_project::TrackId>,
    slot: hex_project::TrackDeviceSlot,
    state: hex_lattice_contract::NativeDeviceState,
    device_name: String,
    gain: Option<f32>,
) -> Command {
    match gain {
        Some(gain) => Command::InsertDefaultTrackWithDeviceAndGain {
            before_track,
            slot,
            state,
            device_name,
            gain,
        },
        None => Command::InsertDefaultTrackWithDevice {
            before_track,
            slot,
            state,
            device_name,
        },
    }
}

pub(crate) fn arrangement_device_command(
    project: &hex_project::Project,
    class: hex_engine::NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
    destination: ArrangementDeviceDestination,
    new_track_gain: Option<f32>,
) -> Result<Command, String> {
    use hex_engine::NativeDeviceClass;
    let state = hex_engine::default_native_device_state_for_class(class, type_id)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    match destination {
        ArrangementDeviceDestination::NewTrack { before_track } => {
            let slot = match class {
                NativeDeviceClass::Instrument => hex_project::TrackDeviceSlot::Instrument,
                NativeDeviceClass::Effect => hex_project::TrackDeviceSlot::Effect,
                NativeDeviceClass::MidiProcessor => {
                    return Err("drop a midi processor onto an existing ordinary track".into());
                }
            };
            Ok(device_track_command(
                before_track,
                slot,
                state,
                native_device_name(class, type_id)?,
                new_track_gain,
            ))
        }
        ArrangementDeviceDestination::ExistingTrack { track, instrument } => {
            let target = project
                .track(track)
                .ok_or("device drop track no longer exists")?;
            if target.instrument().map(hex_project::Device::id) != instrument {
                return Err("instrument replacement target changed".into());
            }
            Ok(match class {
                NativeDeviceClass::MidiProcessor => {
                    if let Some(existing) = target.midi_processor() {
                        Command::ReplaceTrackMidiProcessor {
                            track,
                            replacing: existing.id(),
                            state,
                        }
                    } else {
                        Command::InsertMidiProcessor { track, state }
                    }
                }
                NativeDeviceClass::Effect => Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
                NativeDeviceClass::Instrument => Command::LoadInstrument {
                    track,
                    replacing: instrument,
                    state,
                    device_name: native_device_name(class, type_id)?,
                },
            })
        }
    }
}

mod automation_ops;
mod insert_ops;
mod plugin_ops;
mod sampler_preset_ops;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::DeviceGesture(_)
        | Request::ReplaceDeviceModulation { .. }
        | Request::DeviceAutomation { .. }
        | Request::DeviceAutomationControl { .. }
        | Request::SetDeviceParameter { .. }
        | Request::DeviceAutomationTake { .. }
        | Request::StartPluginEditorRecording { .. }
        | Request::StopPluginEditorRecording { .. }
        | Request::ContinuePluginEditorRecording { .. }
        | Request::QueryFailedPluginAutomation { .. }
        | Request::InspectSealedAutomationRecovery { .. }
        | Request::QueryDeviceAutomationCloseSafety { .. }
        | Request::PreparePluginEditorClose { .. }
        | Request::RecoverDeviceAutomationTake { .. }
        | Request::ArmNativeParameterRecording { .. } => automation_ops::handle(ctx, request),

        Request::ReloadPlugin { .. }
        | Request::TogglePluginEditor { .. }
        | Request::ContinuePluginEditor
        | Request::QueryPluginParameters(_)
        | Request::ContinueRetiringEditorReplay
        | Request::ContinuePluginParameters
        | Request::QueryPluginParameterConversion(_)
        | Request::ContinuePluginParameterConversion
        | Request::InsertPluginEffect(_)
        | Request::ContinuePluginEffect
        | Request::ContinuePluginStateRestore => plugin_ops::handle(ctx, request),

        Request::LoadFirstSampler { .. }
        | Request::DropDrumRackPad { .. }
        | Request::AssignSampler { .. }
        | Request::ImportDeviceAsset { .. }
        | Request::SamplerEdit(_)
        | Request::ContinueSamplerEdit
        | Request::SaveDevicePreset { .. }
        | Request::SaveDeviceDefault { .. }
        | Request::SaveDeviceUnitPreset { .. }
        | Request::SaveDeviceUnitDefault { .. }
        | Request::LoadDevicePreset { .. } => sampler_preset_ops::handle(ctx, request),

        Request::MoveTrackDevices { .. }
        | Request::ReorderDevices { .. }
        | Request::AppendSourceUtility { .. }
        | Request::InsertBrowserInstrumentTrack { .. }
        | Request::ContinueDeviceInsert
        | Request::InsertEffectBusDevice { .. }
        | Request::InsertNestedCatalogDevice { .. }
        | Request::LoadTrackCatalogDevice { .. }
        | Request::DropBrowserDeviceOnArrangement { .. }
        | Request::DropNativeDevice { .. }
        | Request::DeviceClipboard { .. }
        | Request::AddNativeDevice { .. }
        | Request::ReplaceNativeDevice { .. }
        | Request::CompareDevice { .. } => insert_ops::handle(ctx, request),

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
            ControlFlow::Continue(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_device_name_falls_back_to_type_id_for_unlisted_device() {
        let unknown = hex_lattice_contract::DeviceTypeId::new("hex.test.unknown").unwrap();
        assert_eq!(
            native_device_name(hex_engine::NativeDeviceClass::Effect, unknown),
            Ok("hex.test.unknown".to_owned())
        );
    }
}
