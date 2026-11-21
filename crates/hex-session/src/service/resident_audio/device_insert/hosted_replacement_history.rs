//! Compare exact, normalized hosted replacement history edges on control.
use hex_lattice_contract::VST3_DEVICE_TYPE_ID;
use hex_project::{Command, Device, DeviceId, Editor, Project};

/// Admit a same-slot leaf substitution on the same Return, with all other
/// authored values (including routing and send levels) preserved.
pub(super) fn replaces_one_return(before: &Project, after: &Project) -> bool {
    if before.return_tracks().len() != after.return_tracks().len() {
        return false;
    }
    before
        .return_tracks()
        .iter()
        .zip(after.return_tracks())
        .find(|(old, new)| {
            old.devices()
                .iter()
                .map(Device::id)
                .ne(new.devices().iter().map(Device::id))
        })
        .is_some_and(|(old, new)| {
            old.id() == new.id() && replaces_chain_leaf(before, after, old.devices(), new.devices())
        })
}

/// The two leaf IDs must be absent from the opposite complete project.
/// Removing each exact leaf must leave identical normalized projects.
pub(super) fn replaces_chain_leaf(
    before: &Project,
    after: &Project,
    before_chain: &[Device],
    after_chain: &[Device],
) -> bool {
    if before_chain.len() != after_chain.len() {
        return false;
    }
    let Some((old, new)) = before_chain
        .iter()
        .zip(after_chain)
        .find(|(old, new)| old.id() != new.id())
    else {
        return false;
    };
    if !is_hosted_leaf(before_chain, old)
        || !is_hosted_leaf(after_chain, new)
        || after.device(old.id()).is_some()
        || before.device(new.id()).is_some()
    {
        return false;
    }
    let Some(without_old) = without_effect(before, old.id()) else {
        return false;
    };
    without_effect(after, new.id())
        .is_some_and(|without| without.project() == without_old.project())
}

fn is_hosted_leaf(chain: &[Device], device: &Device) -> bool {
    device.parent().is_none()
        && device.state().type_id() == VST3_DEVICE_TYPE_ID
        && !chain
            .iter()
            .any(|child| child.parent() == Some(device.id()))
}

pub(super) fn without_effect(project: &Project, device: DeviceId) -> Option<Editor> {
    let mut stripped = Editor::new(project.clone(), 1).ok()?;
    stripped.execute(Command::RemoveDevice(device)).ok()?;
    Some(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{Created, ReturnId};

    fn add_return(editor: &mut Editor, name: &str) -> ReturnId {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: name.into() })
            .unwrap()
        else {
            panic!("return");
        };
        id
    }

    #[test]
    fn exact_return_swap_requires_the_same_bus_slot_and_sends_in_both_directions() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("source track");
        };
        let bus = add_return(&mut editor, "wet");
        let other = add_return(&mut editor, "other return");
        let send = |gain| Command::SetReturnSend {
            track,
            return_track: bus,
            gain,
        };
        editor.execute(send(0.5)).unwrap();
        let Created::Device(kept) = editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                    "hex.utility",
                ))
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("adjacent effect");
        };
        let state = |byte| NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![byte]).unwrap();
        let Created::Device(original) = editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state(1),
            })
            .unwrap()
        else {
            panic!("original effect");
        };
        let Created::Device(fresh) = editor
            .execute(Command::ReplaceReturnDevice {
                return_track: bus,
                replacing: original,
                state: state(2),
            })
            .unwrap()
        else {
            panic!("replacement");
        };

        let replaced = editor.project().clone();
        let undo = editor.stage_history(false).unwrap().unwrap();
        assert!(replaces_one_return(&replaced, undo.project()));

        for commands in [
            vec![Command::MoveReturnDevice {
                device: fresh,
                before: Some(kept),
            }],
            vec![send(0.25)],
            vec![
                Command::RemoveDevice(fresh),
                Command::InsertReturnDevice {
                    return_track: other,
                    before: None,
                    state: state(2),
                },
            ],
        ] {
            let mut changed = Editor::new(replaced.clone(), 1).unwrap();
            for command in commands {
                changed.execute(command).unwrap();
            }
            assert!(!replaces_one_return(undo.project(), changed.project()));
        }

        editor.commit_staged_history(undo).unwrap();
        let redo = editor.stage_history(true).unwrap().unwrap();
        assert!(replaces_one_return(editor.project(), redo.project()));
    }
}
