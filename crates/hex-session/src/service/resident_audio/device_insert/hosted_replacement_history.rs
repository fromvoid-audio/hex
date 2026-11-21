//! Compare exact, normalized hosted replacement history edges on control.
use hex_lattice_contract::VST3_DEVICE_TYPE_ID;
use hex_project::{
    Command, Device, DeviceId, Editor, Invalid, Project, ReturnId, ReturnTrack,
    StagedProjectHistory, Track, TrackId,
};

/// Stage one exact substitution between two enabled workers on the same
/// Return. Authenticating the leaving worker and playback interval remains
/// the caller's job.
pub(super) fn stage_enabled_return_replacement(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<(StagedProjectHistory, ReturnId, DeviceId)>, Invalid> {
    let Some((return_track, device)) = editor
        .inspect_history(redo, |before, after| {
            let (old_bus, new_bus) = return_replacement_chains(before, after)?;
            let (old, new) = replacement_leaves(old_bus.devices(), new_bus.devices())?;
            (!old.bypassed() && !new.bypassed()).then_some((old_bus.id(), old.id()))
        })
        .flatten()
    else {
        return Ok(None);
    };
    Ok(editor
        .stage_history(redo)?
        .filter(|edge| replaces_one_return(editor.project(), edge.project()))
        .map(|edge| (edge, return_track, device)))
}

/// Admit a same-slot leaf substitution on the same Return, with all other
/// authored values (including routing and send levels) preserved.
pub(super) fn replaces_one_return(before: &Project, after: &Project) -> bool {
    return_replacement_chains(before, after)
        .is_some_and(|(old, new)| replaces_chain_leaf(before, after, old.devices(), new.devices()))
}

fn return_replacement_chains<'a>(
    before: &'a Project,
    after: &'a Project,
) -> Option<(&'a ReturnTrack, &'a ReturnTrack)> {
    if before.return_tracks().len() != after.return_tracks().len() {
        return None;
    }
    before
        .return_tracks()
        .iter()
        .zip(after.return_tracks())
        .find(|(old, new)| effect_ids_differ(old.devices(), new.devices()))
        .filter(|(old, new)| old.id() == new.id())
}

/// Stage one exact substitution between enabled workers on the same direct
/// Track. Authenticating the leaving worker and playback interval remains
/// the caller's job.
pub(super) fn stage_enabled_track_replacement(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<(StagedProjectHistory, TrackId, DeviceId)>, Invalid> {
    let Some((track, device)) = editor
        .inspect_history(redo, |before, after| {
            let (old_track, new_track) = track_replacement_chains(before, after)?;
            let (old, new) = replacement_leaves(old_track.devices(), new_track.devices())?;
            (!old.bypassed() && !new.bypassed()).then_some((old_track.id(), old.id()))
        })
        .flatten()
    else {
        return Ok(None);
    };
    Ok(editor
        .stage_history(redo)?
        .filter(|edge| replaces_one_track(editor.project(), edge.project()))
        .map(|edge| (edge, track, device)))
}

pub(super) fn replaces_one_track(before: &Project, after: &Project) -> bool {
    track_replacement_chains(before, after)
        .is_some_and(|(old, new)| replaces_chain_leaf(before, after, old.devices(), new.devices()))
}

/// A direct Track effect swap keeps the separately authored instrument gap.
/// Removing the swapped leaf alone can collapse two different gaps.
fn track_replacement_chains<'a>(
    before: &'a Project,
    after: &'a Project,
) -> Option<(&'a Track, &'a Track)> {
    if before.tracks().len() != after.tracks().len() {
        return None;
    }
    before
        .tracks()
        .iter()
        .zip(after.tracks())
        .find(|(old, new)| effect_ids_differ(old.devices(), new.devices()))
        .filter(|(old, new)| {
            old.id() == new.id()
                && !old.is_group()
                && !new.is_group()
                && old.parent_group().is_none()
                && new.parent_group().is_none()
                && old.instrument_position() == new.instrument_position()
        })
}

fn effect_ids_differ(before: &[Device], after: &[Device]) -> bool {
    before
        .iter()
        .map(Device::id)
        .ne(after.iter().map(Device::id))
}

/// The two leaf IDs must be absent from the opposite complete project.
/// Removing each exact leaf must leave identical normalized projects.
pub(super) fn replaces_chain_leaf(
    before: &Project,
    after: &Project,
    before_chain: &[Device],
    after_chain: &[Device],
) -> bool {
    let Some((old, new)) = replacement_leaves(before_chain, after_chain) else {
        return false;
    };
    if after.device(old.id()).is_some() || before.device(new.id()).is_some() {
        return false;
    }
    let Some(without_old) = without_effect(before, old.id()) else {
        return false;
    };
    without_effect(after, new.id())
        .is_some_and(|without| without.project() == without_old.project())
}

/// Find a candidate at the first changed ordinal without cloning either project.
/// Callers must still prove whole-project equivalence before admitting history.
pub(super) fn replacement_leaves<'a>(
    before: &'a [Device],
    after: &'a [Device],
) -> Option<(&'a Device, &'a Device)> {
    if before.len() != after.len() {
        return None;
    }
    let (old, new) = before
        .iter()
        .zip(after)
        .find(|(old, new)| old.id() != new.id())?;
    (is_hosted_leaf(before, old) && is_hosted_leaf(after, new)).then_some((old, new))
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
    use hex_project::{Created, TrackDeviceSlot, TrackSelectionIds};

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
        let (undo, bus_id, leaving) = stage_enabled_return_replacement(&mut editor, false)
            .unwrap()
            .unwrap();
        assert_eq!((bus_id, leaving), (bus, fresh));
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
        let (redo, bus_id, leaving) = stage_enabled_return_replacement(&mut editor, true)
            .unwrap()
            .unwrap();
        assert_eq!((bus_id, leaving), (bus, original));
        assert!(replaces_one_return(editor.project(), redo.project()));
    }

    #[test]
    fn exact_track_swap_requires_the_same_direct_slot_and_instrument_gap() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "instrument track".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let utility =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap();
        let insert = |state| Command::InsertDevice {
            track,
            before: None,
            state,
        };
        let Created::Device(neighbor) = editor.execute(insert(utility)).unwrap() else {
            panic!("neighbor");
        };
        let hosted = |byte| NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![byte]).unwrap();
        let Created::Device(original) = editor.execute(insert(hosted(1))).unwrap() else {
            panic!("original effect");
        };
        let move_device = |device, before| Command::MoveTrackDevice {
            device,
            track,
            before,
        };
        editor
            .execute(move_device(instrument, Some(original)))
            .unwrap();
        let replace = || Command::ReplaceTrackDevice {
            track,
            replacing: original,
            state: hosted(2),
        };
        let Created::Device(fresh) = editor.execute(replace()).unwrap() else {
            panic!("replacement");
        };
        let replaced = editor.project().clone();
        let (undo, track_id, leaving) = stage_enabled_track_replacement(&mut editor, false)
            .unwrap()
            .unwrap();
        assert_eq!((track_id, leaving), (track, fresh));
        assert!(replaces_one_track(&replaced, undo.project()));

        let mut wrong_slot = Editor::new(replaced.clone(), 1).unwrap();
        wrong_slot
            .execute(move_device(fresh, Some(neighbor)))
            .unwrap();
        assert!(!replaces_one_track(undo.project(), wrong_slot.project()));

        let mut wrong_gap = Editor::new(replaced, 1).unwrap();
        wrong_gap.execute(move_device(instrument, None)).unwrap();
        let original_track = undo.project().track(track).unwrap();
        let wrong_track = wrong_gap.project().track(track).unwrap();
        assert_ne!(
            original_track.instrument_position(),
            wrong_track.instrument_position()
        );
        assert!(replaces_chain_leaf(
            undo.project(),
            wrong_gap.project(),
            original_track.devices(),
            wrong_track.devices(),
        ));
        assert!(!replaces_one_track(undo.project(), wrong_gap.project()));

        editor.commit_staged_history(undo).unwrap();
        let (redo, track_id, leaving) = stage_enabled_track_replacement(&mut editor, true)
            .unwrap()
            .unwrap();
        assert_eq!((track_id, leaving), (track, original));
        assert!(replaces_one_track(editor.project(), redo.project()));

        editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .parent_group()
                .is_some()
        );
        let grouped = editor.project().clone();
        editor.execute(replace()).unwrap();
        assert!(!replaces_one_track(&grouped, editor.project()));
    }
}
