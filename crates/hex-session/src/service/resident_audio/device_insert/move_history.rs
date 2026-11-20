//! Prove that a history target is exactly one direct native device move.
//! Match a Delay or Reverb with an optional retained Utility on one side, or
//! an unchanged, single-contributor Sine or Vector. Audio proves the cut.
use hex_project::{
    Command, Device, DeviceSelectionIds, Editor, Invalid, Project, SourceId, StagedProjectHistory,
    Track,
};

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
        if old.devices() == new.devices()
            && old.instrument() == new.instrument()
            && old.source_id() == new.source_id()
        {
            continue;
        }
        if old.id() != new.id() || old.is_group() || old.parent_group().is_some() {
            return None;
        }
        if old.instrument() != new.instrument() {
            if !old.devices().is_empty() || !new.devices().is_empty() {
                return None;
            }
            let (slot, attached, detached, project) = match (old.instrument(), new.instrument()) {
                (Some(_), None) => (&mut source, old, new, before),
                (None, Some(_)) => (&mut destination, new, old, after),
                _ => return None,
            };
            let (device, native_source) = solitary_move_instrument(project, attached)?;
            if detached.source_id().is_some()
                || slot
                    .replace((old.id(), device, Some(native_source)))
                    .is_some()
            {
                return None;
            }
            continue;
        }
        if old.instrument().is_some() || old.source_id().is_some() {
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
        if slot.replace((old.id(), device, None)).is_some() {
            return None;
        }
    }
    let (source, device, source_id) = source?;
    let (destination, moved, destination_source_id) = destination?;
    if device != moved
        || source_id != destination_source_id
        || device.bypassed()
        || (source_id.is_none()
            && !matches!(
                device.state().type_id().as_str(),
                Ok("hex.delay" | "hex.reverb")
            ))
    {
        return None;
    }
    Some(Command::MoveTrackDevices {
        source,
        destination,
        selected: DeviceSelectionIds::new(&[device.id()]).ok()?,
    })
}

fn solitary_move_instrument<'a>(
    project: &Project,
    track: &'a Track,
) -> Option<(&'a Device, SourceId)> {
    let device = track.instrument()?;
    let id = track.source_id()?;
    let source = project.native_source(id)?;
    let type_id = device.state().type_id();
    let supported = type_id == hex_project::DEFAULT_INSTRUMENT_TYPE_ID
        || (matches!(type_id.as_str(), Ok("hex.instrument.operator"))
            && device.automation().is_empty()
            && device.modulation().is_empty());
    (supported
        && source.receiver() == Some(track.id())
        && source.devices().count() == 1
        && project.source_contributors(id).count() == 1)
        .then_some((device, id))
}
