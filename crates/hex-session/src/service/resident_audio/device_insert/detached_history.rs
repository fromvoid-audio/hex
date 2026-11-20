//! Exact history for a single retained hosted source attached to a direct track.
use super::GraphEditOperation;
use hex_project::{Command, Editor, Invalid, Project, StagedProjectHistory, Track};

pub(super) fn stage(
    editor: &mut Editor,
    redo: bool,
) -> Result<Option<(StagedProjectHistory, GraphEditOperation)>, Invalid> {
    let Some((command, operation)) = editor.inspect_history(redo, binding_change).flatten() else {
        return Ok(None);
    };
    let Ok(expected) = editor.stage_edit(command) else {
        return Ok(None);
    };
    let Some(edge) = editor.stage_history(redo)? else {
        return Ok(None);
    };
    Ok((edge.project() == expected.project()).then_some((edge, operation)))
}

fn binding_change(before: &Project, after: &Project) -> Option<(Command, GraphEditOperation)> {
    let mut changed = before
        .tracks()
        .iter()
        .zip(after.tracks())
        .filter(|(old, new)| old.source_id() != new.source_id());
    let (old, new) = changed.next()?;
    let direct = |track: &Track| {
        !track.is_group()
            && track.parent_group().is_none()
            && track.devices().is_empty()
            && track.sends().is_empty()
    };
    if changed.next().is_some() || old.id() != new.id() || !direct(old) || !direct(new) {
        return None;
    }
    let track = old.id();
    let (source, attached, detached, command, operation) = match (old.source_id(), new.source_id())
    {
        (None, Some(source)) => (
            source,
            after,
            before,
            Command::BindTrackSource {
                track,
                source: Some(source),
            },
            GraphEditOperation::InsertDevice,
        ),
        (Some(source), None) => (
            source,
            before,
            after,
            Command::RemoveInstrument { track },
            GraphEditOperation::RemoveDevice,
        ),
        _ => return None,
    };
    let routed = attached.native_source(source)?;
    let retained = detached.native_source(source)?;
    let instrument = routed.instrument();
    (routed.receiver() == Some(track)
        && attached.source_contributors(source).count() == 1
        && routed.post_instrument_effects().is_empty()
        && retained.receiver().is_none()
        && detached.source_contributors(source).next().is_none()
        && instrument == retained.instrument()
        && !instrument.bypassed()
        && instrument.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID)
        .then_some((command, operation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{NativeDeviceState, VST3_DEVICE_TYPE_ID};
    use hex_project::Created;

    #[test]
    fn only_the_exact_attachment_history_uses_the_graph_receipt() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("midi track");
        };
        editor
            .execute(Command::InsertInstrument {
                track,
                state: NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1]).unwrap(),
            })
            .unwrap();
        let attached = editor.project().clone();
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let detached = editor.project().clone();
        let (undo, operation) = stage(&mut editor, false).unwrap().unwrap();
        assert_eq!(operation, GraphEditOperation::InsertDevice);
        assert_eq!(undo.project(), &attached);
        assert_eq!(editor.project(), &detached);
        editor.commit_staged_history(undo).unwrap();
        let (redo, operation) = stage(&mut editor, true).unwrap().unwrap();
        assert_eq!(operation, GraphEditOperation::RemoveDevice);
        assert_eq!(redo.project(), &detached);
        assert_eq!(editor.project(), &attached);

        editor
            .execute_many_prepared(
                vec![
                    Command::RemoveInstrument { track },
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
}
