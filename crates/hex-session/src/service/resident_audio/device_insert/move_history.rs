//! Prove that a history target is exactly one direct native device move.
//! Match a Delay or Reverb with an optional retained Utility on one side, or
//! an unchanged single-contributor instrument. Audio proves the actual cut
//! and retains authority over a Sampler's backing or the original VST3 worker.
use super::hosted_instrument_move::Intent;
use hex_project::{
    Command, Device, DeviceSelectionIds, Editor, Invalid, Project, SourceId, StagedProjectHistory,
    Track,
};

pub(super) struct StagedMove {
    pub(super) edge: StagedProjectHistory,
    pub(super) hosted_instrument_move: Option<Intent>,
}

pub(super) fn stage(editor: &mut Editor, redo: bool) -> Result<Option<StagedMove>, Invalid> {
    let Some(command) = editor.inspect_history(redo, move_command).flatten() else {
        return Ok(None);
    };
    let hosted_instrument_move = Intent::from_command(&command);
    let Ok(expected) = editor.stage_edit(command) else {
        return Ok(None);
    };
    let Some(history) = editor.stage_history(redo)? else {
        return Ok(None);
    };
    // Compare the whole normalized target. A batch or history restore can
    // move an identical processor while also changing sources or other state.
    Ok(
        (expected.project() == history.project()).then_some(StagedMove {
            edge: history,
            hosted_instrument_move,
        }),
    )
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
    let supported_worker = match type_id.as_str() {
        Ok("hex.instrument.operator" | "hex.vst3") => true,
        Ok("hex.instrument.sampler") => device.state().asset_references().len() == 1,
        _ => false,
    };
    let supported = type_id == hex_project::DEFAULT_INSTRUMENT_TYPE_ID
        || (supported_worker && device.automation().is_empty() && device.modulation().is_empty());
    (supported
        && source.receiver() == Some(track.id())
        && source.devices().count() == 1
        && project.source_contributors(id).count() == 1)
        .then_some((device, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{ResidentRuntimeLoop, ResidentSourceOrigin};
    use hex_project::Created;

    #[test]
    fn hosted_history_lends_only_the_normalized_direct_move() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut add_track = |name: &str| {
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack { name: name.into() })
                .unwrap()
            else {
                panic!("expected a MIDI track");
            };
            track
        };
        let donor = add_track("donor");
        let receiver = add_track("receiver");
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(b"original"),
            controller: None,
        })
        .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertInstrument {
                track: donor,
                state,
            })
            .unwrap()
        else {
            panic!("expected a hosted instrument");
        };
        let command = || Command::MoveTrackDevices {
            source: donor,
            destination: receiver,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        };
        let before = editor.project().clone();
        editor.execute(command()).unwrap();
        let moved = editor.project().clone();
        for (redo, expected, [from, to]) in [
            (false, &before, [receiver, donor]),
            (true, &moved, [donor, receiver]),
        ] {
            let staged = stage(&mut editor, redo).unwrap().unwrap();
            assert_eq!(staged.edge.project(), expected);
            let route = staged
                .hosted_instrument_move
                .unwrap()
                .preparation(
                    editor.project(),
                    staged.edge.project(),
                    ResidentRuntimeLoop {
                        output_identity: 7,
                        audio_revision: 8,
                        source_context: None,
                        source_origin: ResidentSourceOrigin::CanonicalCandidate,
                        section_loop: None,
                    },
                )
                .unwrap();
            assert_eq!(
                (route.donor, route.receiver, route.device),
                (from, to, device)
            );
            editor.commit_staged_history(staged.edge).unwrap();
        }

        let mut compound = Editor::new(before, 8).unwrap();
        compound
            .execute_many_prepared::<(), Invalid>(
                vec![
                    command(),
                    Command::SetTrackGains {
                        gains: vec![(donor, 0.5)],
                    },
                ],
                |_, _| Ok(()),
            )
            .unwrap();
        assert!(stage(&mut compound, false).unwrap().is_none());
        assert!(compound.can_undo());
    }
}
