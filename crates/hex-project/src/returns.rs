use crate::{Command, Device, DeviceId, Invalid, Project, ReturnId};

pub const MAX_RETURN_TRACKS: usize = 16;
pub const MAX_RETURN_SENDS_PER_TRACK: usize = MAX_RETURN_TRACKS;

/// A sparse, nonzero send from one regular track to one return track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SendLevel {
    pub(crate) return_track: ReturnId,
    pub(crate) gain: f32,
}
impl SendLevel {
    pub fn return_track(&self) -> ReturnId {
        self.return_track
    }
    pub fn gain(&self) -> f32 {
        self.gain
    }
}

/// Canonical return bus. Returns have effects and mix controls, but no clips,
/// instrument, Session cells, or route to another return.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnTrack {
    pub(crate) id: ReturnId,
    pub(crate) name: String,
    pub(crate) gain: f32,
    pub(crate) muted: bool,
    pub(crate) devices: Vec<Device>,
}
impl ReturnTrack {
    pub fn id(&self) -> ReturnId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn gain(&self) -> f32 {
        self.gain
    }
    pub fn muted(&self) -> bool {
        self.muted
    }
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<crate::Created, Invalid> {
    match command {
        Command::AddReturnTrack { name } => add_return(project, name, 1.0),
        Command::AddReturnTrackWithGain { name, gain } => add_return(project, name, gain),
        Command::RemoveReturnTrack(id) => {
            let index = project
                .returns
                .iter()
                .position(|track| track.id == id)
                .ok_or(Invalid("return track no longer exists"))?;
            project.returns.remove(index);
            project
                .audio_feeds
                .retain(|r| r.source() != crate::RoutingOwner::Return(id));
            project
                .routing
                .retain(|feed| feed.source != id && feed.destination != id);
            for track in &mut project.tracks {
                track.sends.retain(|send| send.return_track != id);
            }
            Ok(crate::Created::None)
        }
        Command::RenameReturnTrack { return_track, name } => {
            crate::edit::name_valid(&name)?;
            find_mut(project, return_track)?.name = name;
            Ok(crate::Created::None)
        }
        Command::ReturnGain { return_track, gain } => {
            validate_gain(gain)?;
            find_mut(project, return_track)?.gain = gain;
            Ok(crate::Created::None)
        }
        Command::ReturnMute {
            return_track,
            muted,
        } => {
            find_mut(project, return_track)?.muted = muted;
            Ok(crate::Created::None)
        }
        Command::SetReturnSend {
            track,
            return_track,
            gain,
        } => {
            validate_gain(gain)?;
            if !project
                .returns
                .iter()
                .any(|candidate| candidate.id == return_track)
            {
                return Err(Invalid("return track no longer exists"));
            }
            let source = project.track_mut(track)?;
            if let Some(index) = source
                .sends
                .iter()
                .position(|send| send.return_track == return_track)
            {
                if gain == 0.0 {
                    source.sends.remove(index);
                } else {
                    source.sends[index].gain = gain;
                }
            } else if gain != 0.0 {
                if source.sends.len() >= MAX_RETURN_SENDS_PER_TRACK {
                    return Err(Invalid("track return send limit reached"));
                }
                source.sends.push(SendLevel { return_track, gain });
            }
            Ok(crate::Created::None)
        }
        Command::InsertReturnDevice {
            return_track,
            before,
            state,
        } => {
            if device_count(project)? >= crate::MAX_DEVICES {
                return Err(Invalid("project device limit reached"));
            }
            let target = find(project, return_track)?;
            if target.devices.len() >= crate::MAX_DEVICES_PER_TRACK {
                return Err(Invalid("return device limit reached"));
            }
            let index = match before {
                Some(id) => target
                    .devices
                    .iter()
                    .position(|device| device.id == id)
                    .ok_or(Invalid("return device insertion anchor no longer exists"))?,
                None => target.devices.len(),
            };
            let id = DeviceId(project.allocate_id()?);
            find_mut(project, return_track)?
                .devices
                .insert(index, Device::fresh_serial_leaf(id, state));
            Ok(crate::Created::Device(id))
        }
        Command::ReplaceReturnDevice {
            return_track,
            replacing,
            state,
        } => {
            let target = find(project, return_track)?;
            let index = target
                .devices
                .iter()
                .position(|device| device.id == replacing && device.parent.is_none())
                .ok_or(Invalid("return device replacement target changed"))?;
            if crate::device_inventory::serial_unit_span(&target.devices, index)?.len() != 1 {
                return Err(Invalid(
                    "return device replacement requires a single effect",
                ));
            }
            let id = DeviceId(project.allocate_id()?);
            find_mut(project, return_track)?.devices[index] = Device::fresh_serial_leaf(id, state);
            Ok(crate::Created::Device(id))
        }
        Command::MoveReturnDevice { device, before } => {
            let (source, source_index) = project
                .returns
                .iter()
                .enumerate()
                .find_map(|(track_index, track)| {
                    track
                        .devices
                        .iter()
                        .position(|candidate| candidate.id == device)
                        .map(|index| (track_index, index))
                })
                .ok_or(Invalid("return device no longer exists"))?;
            let destination = match before {
                Some(anchor) => {
                    let (track_index, index) = project
                        .returns
                        .iter()
                        .enumerate()
                        .find_map(|(track_index, track)| {
                            track
                                .devices
                                .iter()
                                .position(|candidate| candidate.id == anchor)
                                .map(|index| (track_index, index))
                        })
                        .ok_or(Invalid("return device insertion anchor no longer exists"))?;
                    if track_index != source {
                        return Err(Invalid("return device anchor belongs to another return"));
                    }
                    if index == source_index {
                        return Err(Invalid("return device cannot be moved before itself"));
                    }
                    index
                }
                None => project.returns[source].devices.len(),
            };
            let span = crate::device_inventory::serial_unit_span(
                &project.returns[source].devices,
                source_index,
            )?;
            if span.contains(&destination) {
                return Err(Invalid("return device cannot be moved inside its unit"));
            }
            crate::device::move_serial_unit(
                &mut project.returns[source].devices,
                source_index,
                destination,
            )?;
            Ok(crate::Created::None)
        }
        _ => Err(Invalid("command is not a return-track edit")),
    }
}

fn add_return(project: &mut Project, name: String, gain: f32) -> Result<crate::Created, Invalid> {
    validate_gain(gain)?;
    crate::edit::name_valid(&name)?;
    if project.returns.len() >= MAX_RETURN_TRACKS {
        return Err(Invalid("return track limit reached"));
    }
    let id = ReturnId(project.allocate_id()?);
    project.returns.push(ReturnTrack {
        id,
        name,
        gain,
        muted: false,
        devices: Vec::new(),
    });
    Ok(crate::Created::ReturnTrack(id))
}

fn validate_gain(gain: f32) -> Result<(), Invalid> {
    if gain.is_finite() && (0.0..=2.0).contains(&gain) {
        Ok(())
    } else {
        Err(Invalid("gain must be finite and within 0..2"))
    }
}

fn find(project: &Project, id: ReturnId) -> Result<&ReturnTrack, Invalid> {
    project
        .returns
        .iter()
        .find(|track| track.id == id)
        .ok_or(Invalid("return track no longer exists"))
}
fn find_mut(project: &mut Project, id: ReturnId) -> Result<&mut ReturnTrack, Invalid> {
    project
        .returns
        .iter_mut()
        .find(|track| track.id == id)
        .ok_or(Invalid("return track no longer exists"))
}

pub(crate) fn device_count(project: &Project) -> Result<usize, Invalid> {
    project.devices().try_fold(0usize, |count, _| {
        count.checked_add(1).ok_or(Invalid("device count overflow"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Created, Editor};
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};

    #[test]
    fn return_replacement_keeps_exact_owner_neighbors_and_sends() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 12).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("source track");
        };
        let mut add_return = |name: &str| {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: name.into() })
                .unwrap()
            else {
                panic!("return track");
            };
            id
        };
        let bus = add_return("wet");
        let other = add_return("another return");
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let authored =
            NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, vec![1]).unwrap();
        for _ in 0..3 {
            editor
                .execute(Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: authored.clone(),
                })
                .unwrap();
        }
        let old = find(editor.project(), bus).unwrap().devices[1].id();
        editor
            .execute(Command::SetDeviceBypass {
                device: old,
                bypassed: true,
            })
            .unwrap();
        let original = editor.project().clone();
        let expected = original.next_identity();
        let replace = |return_track| Command::ReplaceReturnDevice {
            return_track,
            replacing: old,
            state: authored.clone(),
        };
        assert!(editor.execute(replace(other)).is_err());
        assert_eq!(editor.project(), &original);
        let Created::Device(new) = editor.execute(replace(bus)).unwrap() else {
            panic!("replacement device");
        };
        assert_eq!(new.value(), expected);
        let replaced = editor.project().clone();
        let before = &find(&original, bus).unwrap().devices;
        let after = &find(&replaced, bus).unwrap().devices;
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0], before[0]);
        assert_eq!(after[2], before[2]);
        assert_eq!(after[1].id(), new);
        assert!(!after[1].bypassed());
        assert_eq!(replaced.tracks(), original.tracks());
        assert_eq!(
            find(&replaced, other).unwrap(),
            find(&original, other).unwrap()
        );
        assert!(editor.undo());
        assert_eq!(editor.project().return_tracks(), original.return_tracks());
        assert!(editor.redo());
        assert_eq!(editor.project().return_tracks(), replaced.return_tracks());
    }
}
