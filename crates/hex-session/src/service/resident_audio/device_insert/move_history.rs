//! Prove that a history target is exactly one direct native effect move.
//! Recognize a sole Delay or Reverb with an optional retained Utility on one
//! side. The audio owner still validates the live route and runtime transfer.
use hex_project::{Command, DeviceSelectionIds, Editor, Invalid, Project, StagedProjectHistory};

pub(super) fn stage(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<StagedProjectHistory>, Invalid> {
    let Some(command) = editor.inspect_history(redo, move_command).flatten() else {
        return Ok(None);
    };
    let Ok(expected) = editor.stage_edit(command) else {
        return Ok(None);
    };
    let Some(history) = editor.stage_history(redo)? else {
        return Ok(None);
    };
    // Compare the whole normalized target. A batch or history restore can
    // move an identical processor while also changing sources or other state.
    Ok((expected.project() == history.project()).then_some(history))
}

fn move_command(before: &Project, after: &Project) -> Option<Command> {
    if before.tracks().len() != after.tracks().len() {
        return None;
    }
    let mut source = None;
    let mut destination = None;
    let mut has_utility_prefix = false;
    for (old, new) in before.tracks().iter().zip(after.tracks()) {
        if old.devices() == new.devices() {
            continue;
        }
        if old.id() != new.id()
            || old.is_group()
            || old.parent_group().is_some()
            || old.instrument().is_some()
            || old.source_id().is_some()
        {
            return None;
        }
        let (slot, longer, shorter) = if old.devices().len() > new.devices().len() {
            (&mut source, old.devices(), new.devices())
        } else {
            (&mut destination, new.devices(), old.devices())
        };
        let device = match (longer, shorter) {
            ([device], []) => device,
            ([prefix, device], [retained])
                if !has_utility_prefix
                    && prefix == retained
                    && !prefix.bypassed()
                    && prefix.automation().is_empty()
                    && prefix.modulation().is_empty()
                    && matches!(prefix.state().type_id().as_str(), Ok("hex.utility")) =>
            {
                has_utility_prefix = true;
                device
            }
            _ => return None,
        };
        if slot.replace((old.id(), device)).is_some() {
            return None;
        }
    }
    let (source, device) = source?;
    let (destination, moved) = destination?;
    if device != moved
        || device.bypassed()
        || !matches!(
            device.state().type_id().as_str(),
            Ok("hex.delay" | "hex.reverb")
        )
    {
        return None;
    }
    Some(Command::MoveTrackDevices {
        source,
        destination,
        selected: DeviceSelectionIds::new(&[device.id()]).ok()?,
    })
}
