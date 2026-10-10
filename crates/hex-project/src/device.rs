use crate::{Command, DeviceId, Invalid, Project, TrackId};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};

pub const MAX_DEVICES_PER_TRACK: usize = 64;
pub const MAX_DEVICES: usize = crate::MAX_TRACKS * (MAX_DEVICES_PER_TRACK + 1);
pub const DEFAULT_INSTRUMENT_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.instrument.sine");

/// Neutral default envelope shared by new tracks and legacy-schema migration.
/// Project/storage do not depend on the DSP definition or its implementation.
pub fn default_instrument_state() -> Result<NativeDeviceState, Invalid> {
    NativeDeviceState::new(DEFAULT_INSTRUMENT_TYPE_ID, 1, Vec::new())
        .map_err(|_| Invalid("could not construct default instrument state"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub(crate) id: DeviceId,
    pub(crate) state: NativeDeviceState,
    pub(crate) bypassed: bool,
}
impl Device {
    pub fn id(&self) -> DeviceId {
        self.id
    }
    pub fn state(&self) -> &NativeDeviceState {
        &self.state
    }
    pub fn bypassed(&self) -> bool {
        self.bypassed
    }
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<crate::Created, Invalid> {
    match command {
        Command::InsertDevice {
            track,
            before,
            state,
        } => {
            let id = insert(project, track, before, state)?;
            Ok(crate::Created::Device(id))
        }
        Command::InsertInstrument { track, state } => {
            let id = insert_instrument(project, track, state)?;
            Ok(crate::Created::Device(id))
        }
        Command::RemoveInstrument { track } => {
            let target = project.track_mut(track)?;
            if target.instrument.take().is_none() {
                return Err(Invalid("instrument slot is already empty"));
            }
            Ok(crate::Created::None)
        }
        Command::RemoveDevice(id) => {
            let (track, index) = locate_device(project, id)?;
            project.tracks[track].devices.remove(index);
            Ok(crate::Created::None)
        }
        Command::MoveDevice {
            device,
            track,
            before,
        } => {
            move_device(project, device, track, before)?;
            Ok(crate::Created::None)
        }
        Command::ReplaceDeviceState { device, state } => {
            project.device_mut(device)?.state = state;
            Ok(crate::Created::None)
        }
        Command::SetDeviceBypass { device, bypassed } => {
            project.device_mut(device)?.bypassed = bypassed;
            Ok(crate::Created::None)
        }
        _ => Err(Invalid("command is not a device edit")),
    }
}

fn insert_instrument(
    project: &mut Project,
    track: TrackId,
    state: NativeDeviceState,
) -> Result<DeviceId, Invalid> {
    let target = project
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?;
    if target.instrument.is_some() {
        return Err(Invalid("instrument slot is already occupied"));
    }
    let total_devices = project
        .tracks
        .iter()
        .try_fold(0_usize, |count, candidate| {
            count
                .checked_add(candidate.devices.len())
                .and_then(|sum| sum.checked_add(usize::from(candidate.instrument.is_some())))
                .ok_or(Invalid("device count overflow"))
        })?;
    if total_devices >= MAX_DEVICES {
        return Err(Invalid("project device limit reached"));
    }
    let id = DeviceId(project.allocate_id()?);
    project.track_mut(track)?.instrument = Some(Device {
        id,
        state,
        bypassed: false,
    });
    Ok(id)
}

fn insert(
    project: &mut Project,
    track: TrackId,
    before: Option<DeviceId>,
    state: NativeDeviceState,
) -> Result<DeviceId, Invalid> {
    let devices = &project
        .tracks
        .iter()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?
        .devices;
    if devices.len() >= MAX_DEVICES_PER_TRACK {
        return Err(Invalid("track device limit reached"));
    }
    let total_devices = project
        .tracks
        .iter()
        .try_fold(0_usize, |count, candidate| {
            count
                .checked_add(candidate.devices.len())
                .and_then(|sum| sum.checked_add(usize::from(candidate.instrument.is_some())))
                .ok_or(Invalid("device count overflow"))
        })?;
    if total_devices >= MAX_DEVICES {
        return Err(Invalid("project device limit reached"));
    }
    let index = anchor_index(devices, before)?;
    let id = DeviceId(project.allocate_id()?);
    project
        .tracks
        .iter_mut()
        .find(|candidate| candidate.id == track)
        .ok_or(Invalid("track no longer exists"))?
        .devices
        .insert(
            index,
            Device {
                id,
                state,
                bypassed: false,
            },
        );
    Ok(id)
}

fn locate_device(project: &Project, id: DeviceId) -> Result<(usize, usize), Invalid> {
    project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| {
            track
                .devices
                .iter()
                .position(|device| device.id == id)
                .map(|index| (track_index, index))
        })
        .ok_or(Invalid("device no longer exists"))
}
fn anchor_index(devices: &[Device], before: Option<DeviceId>) -> Result<usize, Invalid> {
    match before {
        Some(id) => devices
            .iter()
            .position(|device| device.id == id)
            .ok_or(Invalid("device insertion anchor no longer exists")),
        None => Ok(devices.len()),
    }
}
fn move_device(
    project: &mut Project,
    id: DeviceId,
    target_track: TrackId,
    before: Option<DeviceId>,
) -> Result<(), Invalid> {
    let (source_index, source_position) = locate_device(project, id)?;
    let target_index = project
        .tracks
        .iter()
        .position(|track| track.id == target_track)
        .ok_or(Invalid("target track no longer exists"))?;
    if before == Some(id) && source_index == target_index {
        return Ok(());
    }
    let target_len_after = project.tracks[target_index]
        .devices
        .len()
        .saturating_sub(usize::from(source_index == target_index));
    if source_index != target_index && target_len_after >= MAX_DEVICES_PER_TRACK {
        return Err(Invalid("track device limit reached"));
    }
    if source_index == target_index {
        let devices = &mut project.tracks[source_index].devices;
        let moving = devices.remove(source_position);
        let destination = anchor_index(devices, before)?;
        devices.insert(destination, moving);
    } else {
        let destination = anchor_index(&project.tracks[target_index].devices, before)?;
        let moving = project.tracks[source_index].devices.remove(source_position);
        project.tracks[target_index]
            .devices
            .insert(destination, moving);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::DeviceTypeId;

    fn state(payload: Vec<u8>) -> NativeDeviceState {
        NativeDeviceState::new(DeviceTypeId::new("test.device").unwrap(), 1, payload).unwrap()
    }
    fn track_id(editor: &mut crate::Editor, name: &str) -> TrackId {
        match editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        {
            crate::Created::Track(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        }
    }
    fn device_id(editor: &mut crate::Editor, track: TrackId) -> DeviceId {
        match editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state(vec![7]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        }
    }

    #[test]
    fn generic_device_commands_allocate_stable_ids_and_order() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "track");
        let first = device_id(&mut editor, track);
        let second = match editor
            .execute(Command::InsertDevice {
                track,
                before: Some(first),
                state: state(vec![8]),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            _ => panic!("expected device"),
        };
        editor
            .execute(Command::ReplaceDeviceState {
                device: first,
                state: state(vec![9]),
            })
            .unwrap();
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .devices()
                .iter()
                .map(Device::id)
                .collect::<Vec<_>>(),
            [second, first]
        );
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [9]
        );
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [7]
        );
        assert!(editor.redo());
        assert_eq!(
            editor.project().device(first).unwrap().state().payload(),
            [9]
        );
    }

    #[test]
    fn device_move_bypass_removal_and_failed_edits_are_atomic() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let first_track = track_id(&mut editor, "first");
        let second_track = track_id(&mut editor, "second");
        let first = device_id(&mut editor, first_track);
        let second = device_id(&mut editor, first_track);
        editor
            .execute(Command::MoveDevice {
                device: first,
                track: second_track,
                before: None,
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device: first,
                bypassed: true,
            })
            .unwrap();
        assert!(editor.project().device(first).unwrap().bypassed());
        assert_eq!(
            editor.project().track(first_track).unwrap().devices()[0].id(),
            second
        );
        assert_eq!(
            editor.project().track(second_track).unwrap().devices()[0].id(),
            first
        );
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::MoveDevice {
                    device: first,
                    track: first_track,
                    before: Some(DeviceId(u64::MAX))
                })
                .is_err()
        );
        assert!(
            editor
                .execute(Command::InsertDevice {
                    track: first_track,
                    before: Some(DeviceId(u64::MAX)),
                    state: state(vec![10])
                })
                .is_err()
        );
        assert_eq!(*editor.project(), before);
        editor.execute(Command::RemoveDevice(first)).unwrap();
        assert!(editor.project().device(first).is_none());
        assert!(editor.undo());
        assert!(editor.project().device(first).is_some());
    }

    #[test]
    fn device_limit_and_allocator_high_water_are_enforced() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
        let track = track_id(&mut editor, "track");
        for _ in 0..MAX_DEVICES_PER_TRACK {
            device_id(&mut editor, track);
        }
        let before = editor.project().clone();
        assert!(
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state(vec![])
                })
                .is_err()
        );
        assert_eq!(*editor.project(), before);
        let mut snapshot = editor.project().snapshot();
        snapshot.next_id = u64::MAX;
        let mut exhausted =
            crate::Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let before = exhausted.project().clone();
        assert!(
            exhausted
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state(vec![])
                })
                .is_err()
        );
        assert_eq!(*exhausted.project(), before);
    }

    #[test]
    fn instrument_slot_state_bypass_remove_insert_and_undo_are_atomic() {
        let mut editor = crate::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track_id(&mut editor, "keys");
        let original = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        assert_eq!(
            editor.project().device(original).unwrap().state(),
            &crate::default_instrument_state().unwrap()
        );

        let custom = state(vec![3, 9, 1]);
        editor
            .execute(Command::ReplaceDeviceState {
                device: original,
                state: custom.clone(),
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device: original,
                bypassed: true,
            })
            .unwrap();
        assert!(editor.project().device(original).unwrap().bypassed());
        assert_eq!(editor.project().device(original).unwrap().state(), &custom);
        assert!(editor.undo());
        assert!(!editor.project().device(original).unwrap().bypassed());
        assert!(editor.undo());
        assert_eq!(
            editor.project().device(original).unwrap().state(),
            &crate::default_instrument_state().unwrap()
        );

        editor.execute(Command::RemoveInstrument { track }).unwrap();
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        assert!(editor.undo());
        assert_eq!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            original
        );
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let replacement = match editor
            .execute(Command::InsertInstrument {
                track,
                state: custom.clone(),
            })
            .unwrap()
        {
            crate::Created::Device(id) => id,
            other => panic!("unexpected edit result: {other:?}"),
        };
        assert!(replacement > original);
        assert_eq!(
            editor.project().device(replacement).unwrap().state(),
            &custom
        );
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );

        let before = editor.project().clone();
        let mut snapshot = before.snapshot();
        snapshot.next_id = u64::MAX;
        let mut exhausted =
            crate::Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let before_exhausted = exhausted.project().clone();
        assert!(
            exhausted
                .execute(Command::InsertInstrument {
                    track,
                    state: custom,
                })
                .is_err()
        );
        assert_eq!(*exhausted.project(), before_exhausted);
        assert!(!exhausted.can_undo());
    }
}
