//! Fixed-row Group shapes and their exact normalized history targets.
use hex_project::{
    Command, Editor, Invalid, Project, StagedProjectHistory, TrackDestination, TrackId,
    TrackSelectionIds,
};
use std::ops::Range;

/// The root audio leaf immediately follows an already populated root Group.
/// Appending the leaf to that Group preserves every canonical row.
pub(super) fn fixed_entry(project: &Project, leaf: TrackId, group: TrackId) -> bool {
    fixed_rows(project, leaf, group, None)
        .is_some_and(|(leaf, group)| group.len() > 1 && group.end == leaf.start)
}

/// Moving the final direct leaf immediately outside its still-populated Group
/// preserves its row only when the anchor is the following root, if present.
pub(super) fn fixed_exit(project: &Project, leaf: TrackId, before: Option<TrackId>) -> bool {
    let Some(group) = project
        .track(leaf)
        .and_then(hex_project::Track::parent_group)
    else {
        return false;
    };
    fixed_rows(project, leaf, group, Some(group)).is_some_and(|(leaf, group)| {
        leaf.end == group.end
            && group.len() > 2
            && before == project.tracks().get(group.end).map(hex_project::Track::id)
    })
}

fn fixed_rows(
    project: &Project,
    leaf: TrackId,
    group: TrackId,
    parent: Option<TrackId>,
) -> Option<(Range<usize>, Range<usize>)> {
    let (leaf, group) = (project.track(leaf)?, project.track(group)?);
    if leaf.is_group()
        || leaf.parent_group() != parent
        || leaf.clips().is_empty()
        || !group.is_group()
        || group.parent_group().is_some()
    {
        return None;
    }
    let hierarchy = project.track_hierarchy().ok()?;
    Some((
        hierarchy.row(leaf.id())?.subtree_range(),
        hierarchy.row(group.id())?.subtree_range(),
    ))
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

    #[test]
    fn explicit_exit_keeps_the_following_root_anchor_and_a_surviving_child() {
        let (mut editor, leaf, group) = fixture();
        editor.execute(entry(leaf, group)).unwrap();
        let following = editor.project().tracks().last().unwrap().id();
        assert!(fixed_exit(editor.project(), leaf, Some(following)));
        assert!(!fixed_exit(editor.project(), leaf, Some(leaf)));
        assert!(!fixed_exit(editor.project(), leaf, None));
        editor
            .execute(Command::MoveTracksBefore {
                tracks: TrackSelectionIds::new(&[following]).unwrap(),
                before: Some(group),
            })
            .unwrap();
        assert!(fixed_exit(editor.project(), leaf, None));

        let child = editor.project().tracks()[2].id();
        assert_eq!(
            editor.project().track(child).unwrap().parent_group(),
            Some(group)
        );
        editor
            .execute(Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[child]).unwrap(),
                destination: TrackDestination {
                    parent_group: None,
                    before: Some(group),
                },
            })
            .unwrap();
        assert!(!fixed_exit(editor.project(), leaf, None));
    }
}
