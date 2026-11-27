//! Device routing, automation, modulation and instrument-asset edit dispatch.
use super::{
    Command, Created, InstrumentAssetOwner, InstrumentLoadDestination, InstrumentLoadPlacement,
    TrackDeviceSlot,
};
use crate::{Invalid, Project, converted_frame_count};

impl Project {
    pub(super) fn apply_device_command(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            Command::SetRouting(edges) => self.adopt_routing(edges),
            Command::SetAudioFeed {
                source,
                destination,
                gain,
            } => self.set_audio_feed(source, destination, gain),
            Command::SetSidechain { device, source } => self.set_sidechain(device, source),
            Command::SetMidiSource {
                destination,
                source,
            } => self.set_midi_source(destination, source),
            Command::AssignTrackAudioInput {
                destination,
                source,
            } => self.assign_track_audio_input(destination, source),
            Command::SetTrackOutput { track, destination } => {
                self.set_track_output(track, destination)
            }
            Command::SetTrackInputMonitoring { track, mode } => {
                self.set_track_input_monitoring(track, mode)
            }
            Command::SetReturnFeed {
                source,
                destination,
                gain,
            } => self.set_return_feed(source, destination, gain),
            Command::ManualDeviceEdit {
                device,
                state,
                targets,
            } => self.manual_device_edit(device, state, targets),
            Command::SetDeviceAutomationReadSelection {
                device,
                target,
                selection,
            } => self.set_device_automation_read_selection(device, target, selection),
            Command::ReenableDeviceAutomation => self.reenable_device_automation(),
            Command::SpliceDeviceAutomationTouch {
                device,
                target,
                vst3_class,
                spans,
            } => self.splice_device_automation_touch(device, target, vst3_class, spans),
            Command::SpliceVst3AutomationTouchGroup {
                device,
                class,
                lanes,
            } => self.splice_vst3_automation_touch_group(device, class, lanes),
            Command::ReplaceDeviceModulation { device, modulation } => {
                self.replace_device_modulation(device, modulation)
            }
            Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points,
            } => self.replace_device_automation_target(device, target, points),
            Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points,
            } => self.replace_device_automation(device, parameter, points),
            Command::InitializeDevice(initialization) => initialization.apply(self),
            Command::ImportDeviceAsset {
                path,
                sample_rate,
                frames,
            } => {
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                self.add_prepared_asset(path, sample_rate, frames, playback, conversion)
                    .map(Created::Asset)
            }
            Command::ImportDeviceAssetIntoNewInstrument {
                destination,
                path,
                sample_rate,
                frames,
                state,
                source_slot,
            } => {
                if !state.asset_references().is_empty() {
                    return Err(Invalid("device source template is already bound"));
                }
                if let InstrumentLoadDestination::Track { track, placement } = destination {
                    let target = self
                        .track(track)
                        .ok_or(Invalid("instrument track no longer exists"))?;
                    let replacing = placement.replacement();
                    if target.is_group()
                        || target.instrument().map(crate::Device::id) != replacing
                        || (replacing.is_some() && !target.receives_source_audio())
                    {
                        return Err(Invalid(
                            "device source import target changed or is ineligible",
                        ));
                    }
                }
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                let asset =
                    self.add_prepared_asset(path, sample_rate, frames, playback, conversion)?;
                let state = state
                    .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                        slot: source_slot,
                        asset: hex_lattice_contract::DeviceAssetKey::new(asset.value())
                            .map_err(|_| Invalid("invalid device source id"))?,
                    }])
                    .map_err(|_| Invalid("invalid device source binding"))?;
                // Asset, instrument and any new track belong to one private candidate.
                let (track, placement) = match destination {
                    InstrumentLoadDestination::Track { track, placement } => (track, placement),
                    InstrumentLoadDestination::NewTrack { before_track } => {
                        self.apply(Command::InsertDefaultTrackWithDevice {
                            before_track,
                            slot: TrackDeviceSlot::Instrument,
                            state,
                            device_name: "sampler".into(),
                        })?;
                        return Ok(Created::Asset(asset));
                    }
                };
                let replacing = placement.replacement();
                if replacing.is_some() {
                    self.apply(Command::LoadInstrument {
                        track,
                        replacing,
                        state,
                        device_name: "sampler".into(),
                    })?;
                } else {
                    let created = self.apply(Command::InsertInstrument { track, state })?;
                    if placement == InstrumentLoadPlacement::Append {
                        let Created::Device(device) = created else {
                            return Err(Invalid("imported instrument identity is unavailable"));
                        };
                        self.apply(Command::MoveTrackDevice {
                            device,
                            track,
                            before: None,
                        })?;
                    }
                }
                Ok(Created::Asset(asset))
            }
            Command::ImportDeviceAssetReplacingInstrument {
                owner,
                device,
                expected_state,
                path,
                sample_rate,
                frames,
                state,
                source_slot,
            } => {
                let instrument = match owner {
                    InstrumentAssetOwner::Track(track) => {
                        self.track(track).and_then(|track| track.instrument())
                    }
                    InstrumentAssetOwner::Source(source) => {
                        if self
                            .tracks()
                            .iter()
                            .any(|track| track.source_id() == Some(source))
                        {
                            return Err(Invalid("instrument source is already assigned"));
                        }
                        self.native_source(source).map(|source| source.instrument())
                    }
                };
                let target = instrument
                    .filter(|target| target.id() == device && target.state() == &expected_state)
                    .ok_or(Invalid("instrument source replacement target changed"))?;
                if state.type_id() != target.state().type_id()
                    || state.input_gain_millidb() != expected_state.input_gain_millidb()
                    || state.output_gain_millidb() != expected_state.output_gain_millidb()
                    || state.asset_references() != expected_state.asset_references()
                    || (!state.asset_references().is_empty()
                        && state
                            .asset_references()
                            .iter()
                            .filter(|r| r.slot == source_slot)
                            .count()
                            != 1)
                {
                    return Err(Invalid(
                        "instrument replacement must preserve provider and host metadata",
                    ));
                }
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                let asset =
                    self.add_prepared_asset(path, sample_rate, frames, playback, conversion)?;
                let key = hex_lattice_contract::DeviceAssetKey::new(asset.value())
                    .map_err(|_| Invalid("invalid device source id"))?;
                let references = if state.asset_references().is_empty() {
                    vec![hex_lattice_contract::DeviceAssetReference {
                        slot: source_slot,
                        asset: key,
                    }]
                } else {
                    state
                        .asset_references()
                        .iter()
                        .map(|reference| hex_lattice_contract::DeviceAssetReference {
                            slot: reference.slot,
                            asset: if reference.slot == source_slot {
                                key
                            } else {
                                reference.asset
                            },
                        })
                        .collect()
                };
                let state = state
                    .with_asset_references(references)
                    .map_err(|_| Invalid("invalid device source binding"))?;
                self.apply(Command::ReplaceDeviceState { device, state })?;
                Ok(Created::Asset(asset))
            }
            Command::UpdateDrumRackDevice {
                device,
                expected_state,
                state,
                import_asset,
            } => {
                const DRUM_RACK: hex_lattice_contract::DeviceTypeId =
                    hex_lattice_contract::DeviceTypeId::from_static("hex.rack.drum");
                let location = self
                    .device_location(device)
                    .ok_or(Invalid("drum rack target no longer exists"))?;
                if location.class != crate::DeviceUnitClass::Instrument
                    || location.device.state().type_id() != DRUM_RACK
                    || location.device.state() != &expected_state
                {
                    return Err(Invalid("drum rack target changed"));
                }
                if state.type_id() != DRUM_RACK
                    || state.input_gain_millidb() != expected_state.input_gain_millidb()
                    || state.output_gain_millidb() != expected_state.output_gain_millidb()
                {
                    return Err(Invalid(
                        "drum rack update must preserve provider and host metadata",
                    ));
                }
                if let Some(import) = import_asset {
                    if !state
                        .asset_references()
                        .iter()
                        .any(|r| r.asset.value() == import.expected_asset.value())
                    {
                        return Err(Invalid(
                            "drum rack update state does not reference imported sample",
                        ));
                    }
                    let playback =
                        converted_frame_count(import.frames, import.sample_rate, self.sample_rate)?;
                    let conversion =
                        crate::import_conversion_version(import.sample_rate, self.sample_rate);
                    let asset = self.add_prepared_asset(
                        import.path,
                        import.sample_rate,
                        import.frames,
                        playback,
                        conversion,
                    )?;
                    if asset != import.expected_asset {
                        return Err(Invalid("drum rack sample asset identity is stale"));
                    }
                    self.apply(Command::ReplaceDeviceState { device, state })?;
                    Ok(Created::Asset(asset))
                } else {
                    self.apply(Command::ReplaceDeviceState { device, state })?;
                    Ok(Created::Device(device))
                }
            }
            Command::RemoveDevices { owner, selected } => {
                crate::device_clipboard::remove(self, owner, &selected)
            }
            Command::PasteDevices {
                clipboard,
                owner,
                before,
            } => crate::device_clipboard::paste(self, &clipboard, owner, before),
            _ => Err(Invalid("not a device edit command")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_dispatch_rejects_non_device_commands_without_mutating_project() {
        let mut project = Project::new(48_000).expect("project");
        let before = project.clone();
        assert!(
            project
                .apply_device_command(Command::ToggleLoopRegion)
                .is_err()
        );
        assert_eq!(project, before);
    }

    #[test]
    fn update_drum_rack_device_handles_shared_source_import_undo_redo_and_stale_refusal() {
        use hex_lattice_contract::{
            DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
        };
        let rack_type = DeviceTypeId::from_static("hex.rack.drum");
        let initial_state = NativeDeviceState::new(rack_type, 1, vec![1, 2, 3])
            .unwrap()
            .with_input_gain_millidb(-100)
            .unwrap()
            .with_output_gain_millidb(200)
            .unwrap();

        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "drums".into(),
                devices: vec![initial_state.clone()],
                instrument_index: 0,
            })
            .unwrap()
        else {
            panic!("expected native source");
        };
        let Created::Track(track_a) = editor
            .execute(Command::AddMidiTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let Created::Track(track_b) = editor
            .execute(Command::AddMidiTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        editor
            .execute(Command::BindTrackSource {
                track: track_a,
                source: Some(source),
            })
            .unwrap();
        editor
            .execute(Command::BindTrackSource {
                track: track_b,
                source: Some(source),
            })
            .unwrap();
        let device = editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id();

        let next_asset = editor.project().peek_next_asset_id().unwrap();
        let with_sample = NativeDeviceState::new(rack_type, 1, vec![4, 5, 6])
            .unwrap()
            .with_input_gain_millidb(-100)
            .unwrap()
            .with_output_gain_millidb(200)
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(next_asset.value()).unwrap(),
            }])
            .unwrap();
        let created = editor
            .execute(Command::UpdateDrumRackDevice {
                device,
                expected_state: initial_state.clone(),
                state: with_sample.clone(),
                import_asset: Some(crate::DrumRackPadAssetImport {
                    expected_asset: next_asset,
                    path: std::path::PathBuf::from("/tmp/kick.wav"),
                    sample_rate: 48_000,
                    frames: 256,
                }),
            })
            .unwrap();
        assert_eq!(created, Created::Asset(next_asset));
        assert!(editor.project().asset(next_asset).is_some());
        assert_eq!(
            editor
                .project()
                .track(track_a)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &with_sample
        );
        assert_eq!(
            editor
                .project()
                .track(track_b)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &with_sample
        );

        let before_refusal = editor.project().clone();
        assert!(
            editor
                .execute(Command::UpdateDrumRackDevice {
                    device,
                    expected_state: initial_state.clone(),
                    state: with_sample.clone(),
                    import_asset: None,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before_refusal);

        let wrong_gain = NativeDeviceState::new(rack_type, 1, vec![7, 8, 9]).unwrap();
        assert!(
            editor
                .execute(Command::UpdateDrumRackDevice {
                    device,
                    expected_state: with_sample.clone(),
                    state: wrong_gain,
                    import_asset: None,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before_refusal);

        assert!(editor.undo());
        assert!(editor.project().asset(next_asset).is_none());
        assert_eq!(
            editor
                .project()
                .native_source(source)
                .unwrap()
                .instrument()
                .state(),
            &initial_state
        );
        assert!(editor.redo());
        assert!(editor.project().asset(next_asset).is_some());
        assert_eq!(
            editor
                .project()
                .native_source(source)
                .unwrap()
                .instrument()
                .state(),
            &with_sample
        );
    }
}
