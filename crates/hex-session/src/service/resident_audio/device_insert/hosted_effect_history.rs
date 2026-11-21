//! Confirm one top-level hosted effect history edge through the Resident callback.
use super::GraphEditOperation;
use hex_project::{Command, Device, DeviceId, Editor, Invalid, Project, StagedProjectHistory};

pub(super) fn stage(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<(StagedProjectHistory, GraphEditOperation)>, Invalid> {
    let Some((id, operation)) = editor.inspect_history(redo, effect_membership).flatten() else {
        return Ok(None);
    };
    let Some(history) = editor.stage_history(redo)? else {
        return Ok(None);
    };
    let (without, with) = if operation == GraphEditOperation::InsertDevice {
        (editor.project(), history.project())
    } else {
        (history.project(), editor.project())
    };
    // An apparent membership change can share a history edge with other edits.
    // Removing the exact leaf must reproduce the complete normalized target.
    let mut stripped = Editor::new(with.clone(), 1)?;
    let exact =
        stripped.execute(Command::RemoveDevice(id)).is_ok() && stripped.project() == without;
    Ok(exact.then_some((history, operation)))
}

fn effect_membership(before: &Project, after: &Project) -> Option<(DeviceId, GraphEditOperation)> {
    added_effect(before, after)
        .map(|id| (id, GraphEditOperation::InsertDevice))
        .or_else(|| added_effect(after, before).map(|id| (id, GraphEditOperation::RemoveDevice)))
}

fn added_effect(before: &Project, after: &Project) -> Option<DeviceId> {
    added_track_effect(before, after)
        .or_else(|| added_return_effect(before, after))
        .or_else(|| {
            let devices = after.main_devices();
            if before.main_devices().len().checked_add(1) != Some(devices.len()) {
                return None;
            }
            added_leaf(before, devices)
        })
}

fn added_track_effect(before: &Project, after: &Project) -> Option<DeviceId> {
    if before.tracks().len() != after.tracks().len() {
        return None;
    }
    let mut tracks = before
        .tracks()
        .iter()
        .zip(after.tracks())
        .filter(|(old, new)| {
            old.id() == new.id()
                && !old.is_group()
                && !new.is_group()
                && old.parent_group().is_none()
                && new.parent_group().is_none()
                && old.devices().len().checked_add(1) == Some(new.devices().len())
        });
    let (_, track) = tracks.next()?;
    if tracks.next().is_some() {
        return None;
    }
    added_leaf(before, track.devices())
}

fn added_return_effect(before: &Project, after: &Project) -> Option<DeviceId> {
    if before.return_tracks().len() != after.return_tracks().len() {
        return None;
    }
    let mut returns = before
        .return_tracks()
        .iter()
        .zip(after.return_tracks())
        .filter(|(old, new)| {
            old.id() == new.id() && old.devices().len().checked_add(1) == Some(new.devices().len())
        });
    let (_, bus) = returns.next()?;
    if returns.next().is_some() {
        return None;
    }
    added_leaf(before, bus.devices())
}

fn added_leaf(before: &Project, devices: &[Device]) -> Option<DeviceId> {
    let mut added = devices
        .iter()
        .filter(|device| before.device(device.id()).is_none());
    let device = added.next()?;
    (added.next().is_none()
        && device.parent().is_none()
        && device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID
        && !devices
            .iter()
            .any(|child| child.parent() == Some(device.id())))
    .then_some(device.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{NativeDeviceState, VST3_DEVICE_TYPE_ID};
    use hex_project::Created;

    fn assert_exact_removal_history(editor: &mut Editor, id: DeviceId) {
        let original = editor.project().clone();
        editor.execute(Command::RemoveDevice(id)).unwrap();
        let removed = editor.project().clone();
        let (undo, operation) = stage(editor, false).unwrap().unwrap();
        assert_eq!(operation, GraphEditOperation::InsertDevice);
        assert_eq!(undo.project(), &original);
        assert_eq!(editor.project(), &removed);
        editor.commit_staged_history(undo).unwrap();
        let (redo, operation) = stage(editor, true).unwrap().unwrap();
        assert_eq!(operation, GraphEditOperation::RemoveDevice);
        assert_eq!(redo.project(), &removed);
        assert_eq!(editor.project(), &original);
    }

    #[test]
    fn one_exact_track_leaf_history_installs_and_compound_edits_refuse() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Device(id) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        assert_exact_removal_history(&mut editor, id);

        editor
            .execute_many_prepared(
                vec![
                    Command::RemoveDevice(id),
                    Command::RenameTrack {
                        track,
                        name: "edited".into(),
                    },
                ],
                |_, _| Ok::<_, Invalid>(()),
            )
            .unwrap();
        assert!(stage(&mut editor, false).unwrap().is_none());
    }

    #[test]
    fn one_exact_return_leaf_history_keeps_bus_settings() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::ReturnTrack(return_track) = editor
            .execute(Command::AddReturnTrack {
                name: "hosted return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        let Created::Device(id) = editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state: NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        assert_exact_removal_history(&mut editor, id);

        editor
            .execute_many_prepared(
                vec![
                    Command::RemoveDevice(id),
                    Command::ReturnGain {
                        return_track,
                        gain: 0.25,
                    },
                ],
                |_, _| Ok::<_, Invalid>(()),
            )
            .unwrap();
        assert!(stage(&mut editor, false).unwrap().is_none());
    }

    #[test]
    fn one_exact_main_leaf_history_keeps_project_controls() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        assert!(stage(&mut editor, false).unwrap().is_none());

        let Created::Device(id) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        assert_exact_removal_history(&mut editor, id);

        editor
            .execute_many_prepared(
                vec![
                    Command::RemoveDevice(id),
                    Command::SetProjectTempo {
                        tempo: hex_project::Tempo::new(131_000_000).unwrap(),
                    },
                ],
                |_, _| Ok::<_, Invalid>(()),
            )
            .unwrap();
        assert!(stage(&mut editor, false).unwrap().is_none());
    }
}
