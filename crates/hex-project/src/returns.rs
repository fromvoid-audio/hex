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
            find_mut(project, return_track)?.devices.insert(
                index,
                Device {
                    automation: Vec::new(),
                    comparison: None,
                    id,
                    state,
                    bypassed: false,
                },
            );
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
                    index - usize::from(index > source_index)
                }
                None => project.returns[source].devices.len().saturating_sub(1),
            };
            let moving = project.returns[source].devices.remove(source_index);
            project.returns[source].devices.insert(destination, moving);
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
    let regular = project
        .tracks
        .iter()
        .try_fold(project.main_devices.len(), |count, track| {
            count
                .checked_add(track.devices.len())
                .ok_or(Invalid("device count overflow"))
        })?;
    let regular = project
        .native_sources
        .iter()
        .try_fold(regular, |count, source| {
            count
                .checked_add(source.devices().count())
                .ok_or(Invalid("device count overflow"))
        })?;
    project.returns.iter().try_fold(regular, |count, track| {
        count
            .checked_add(track.devices.len())
            .ok_or(Invalid("device count overflow"))
    })
}
