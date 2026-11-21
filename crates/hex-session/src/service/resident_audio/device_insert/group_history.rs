//! Identify a fixed-row Group edit and verify its complete normalized history target.
use hex_project::{
    Command, Editor, Invalid, Project, StagedProjectHistory, TrackDestination, TrackId,
    TrackSelectionIds,
};

/// The root audio leaf immediately follows an already populated root Group.
/// Appending the leaf to that Group preserves every canonical row.
pub(super) fn fixed_entry(project: &Project, leaf: TrackId, group: TrackId) -> bool {
    let (Some(leaf), Some(group)) = (project.track(leaf), project.track(group)) else {
        return false;
    };
    if leaf.is_group()
        || leaf.parent_group().is_some()
        || leaf.clips().is_empty()
        || !group.is_group()
        || group.parent_group().is_some()
    {
        return false;
    }
    let Ok(hierarchy) = project.track_hierarchy() else {
        return false;
    };
    let (Some(leaf_row), Some(group_row)) = (hierarchy.row(leaf.id()), hierarchy.row(group.id()))
    else {
        return false;
    };
    let subtree = group_row.subtree_range();
    subtree.len() > 1 && subtree.end == leaf_row.subtree_range().start
}

pub(super) fn command(current: &Project, target: &Project) -> Option<Command> {
    if current.tracks().len() != target.tracks().len() {
        return None;
    }
    let mut changed = None;
    for (row, (old, new)) in current.tracks().iter().zip(target.tracks()).enumerate() {
        if old.id() != new.id() {
            return None;
        }
        if old.parent_group() != new.parent_group() && changed.replace((row, old, new)).is_some() {
            return None;
        }
    }
    let (row, old, new) = changed?;
    let (direct, group, before) = match (old.parent_group(), new.parent_group()) {
        (None, Some(group)) => (current, group, None),
        (Some(group), None) => (
            target,
            group,
            target.tracks().get(row + 1).map(hex_project::Track::id),
        ),
        _ => return None,
    };
    if !fixed_entry(direct, old.id(), group) {
        return None;
    }
    Some(Command::ReparentTracks {
        tracks: TrackSelectionIds::new(&[old.id()]).ok()?,
        destination: TrackDestination {
            parent_group: new.parent_group(),
            before,
        },
    })
}

pub(super) fn stage(
    editor: &mut Editor,
    redo: bool,
    command: Command,
) -> Result<Option<StagedProjectHistory>, Invalid> {
    let Ok(expected) = editor.stage_edit(command) else {
        return Ok(None);
    };
    let Some(history) = editor.stage_history(redo)? else {
        return Ok(None);
    };
    // A compound edit may look like a reparent until the complete target is compared.
    Ok((expected.project() == history.project()).then_some(history))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::Created;

    fn fixture() -> (Editor, TrackId, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(child) = editor
            .execute(Command::AddTrack {
                name: "existing child".into(),
            })
            .unwrap()
        else {
            panic!("child");
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[child]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };
        let Created::Imported { track: leaf, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "leaf".into(),
                path: "leaf.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("audio leaf");
        };
        editor
            .execute(Command::AddTrack {
                name: "following root".into(),
            })
            .unwrap();
        (editor, leaf, group)
    }

    fn entry(leaf: TrackId, group: TrackId) -> Command {
        Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        }
    }

    #[test]
    fn fixed_row_history_stages_both_normalized_directions_before_the_following_root() {
        let (mut editor, leaf, group) = fixture();
        let rows: Vec<_> = editor
            .project()
            .tracks()
            .iter()
            .map(hex_project::Track::id)
            .collect();
        let direct = editor.project().clone();
        editor.execute(entry(leaf, group)).unwrap();
        let grouped = editor.project().clone();
        editor
            .execute(Command::AddTrack {
                name: "later allocation".into(),
            })
            .unwrap();
        assert!(editor.undo());
        for (redo, parent, raw) in [(false, None, &direct), (true, Some(group), &grouped)] {
            let move_track = editor.inspect_history(redo, command).flatten().unwrap();
            let original = editor.project().clone();
            let history = stage(&mut editor, redo, move_track).unwrap().unwrap();
            assert_eq!(editor.project(), &original);
            assert_eq!(
                history.project().track(leaf).unwrap().parent_group(),
                parent
            );
            assert!(
                history
                    .project()
                    .tracks()
                    .iter()
                    .map(hex_project::Track::id)
                    .eq(rows.iter().copied())
            );
            // The route predates a later allocation. History must keep the current allocator.
            assert_ne!(history.project(), raw);
            editor.commit_staged_history(history).unwrap();
        }
    }

    #[test]
    fn reparent_with_gain_change_and_ordinary_reorder_stay_out_of_staged_route_history() {
        let (mut editor, leaf, group) = fixture();
        let direct = editor.project().clone();
        editor
            .execute_many_prepared(
                vec![
                    entry(leaf, group),
                    Command::Gain {
                        track: leaf,
                        gain: 0.5,
                    },
                ],
                |_, _| Ok::<_, Invalid>(()),
            )
            .unwrap();
        let compound = editor.project().clone();
        let move_track = editor.inspect_history(false, command).flatten().unwrap();
        assert!(stage(&mut editor, false, move_track).unwrap().is_none());
        assert_eq!(editor.project(), &compound);
        assert!(editor.undo());
        assert_eq!(editor.project(), &direct);
        assert!(command(&direct, &direct).is_none());
        editor
            .execute(Command::MoveTracksBefore {
                tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
                before: Some(group),
            })
            .unwrap();
        assert!(editor.inspect_history(false, command).flatten().is_none());
    }
}
