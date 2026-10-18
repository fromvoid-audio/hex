//! Console placement is resolved on control-side candidate metadata, never by ID guesses.
use crate::{SessionError, import_batch::AudioBatchDestination};
use hex_project::{Command, Created, Editor, Project, SceneId, SessionCellContent, TrackId};

/// One browser audio payload targets one stable cell or inserts one scene row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleDestination {
    Cell {
        track: TrackId,
        scene: SceneId,
    },
    InsertScene {
        track: TrackId,
        before: Option<SceneId>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedCell {
    pub track: TrackId,
    pub scene: SceneId,
}

pub(crate) fn validate(
    project: &Project,
    target: ConsoleDestination,
    count: usize,
) -> Result<(), SessionError> {
    if count != 1 {
        return Err(SessionError::Invalid(
            "console drop requires one audio source",
        ));
    }
    let (track, scene) = match target {
        ConsoleDestination::Cell { track, scene } => (track, Some(scene)),
        ConsoleDestination::InsertScene { track, before } => {
            if project.scenes().len() >= hex_project::MAX_SCENES {
                return Err(SessionError::Invalid("scene limit reached"));
            }
            (track, before)
        }
    };
    project
        .track(track)
        .ok_or(SessionError::Invalid("console track no longer exists"))?;
    if scene.is_some_and(|id| !project.scenes().iter().any(|s| s.id() == id)) {
        return Err(SessionError::Invalid("console scene no longer exists"));
    }
    Ok(())
}

pub(crate) fn prefix(
    project: &Project,
    destination: AudioBatchDestination,
    count: usize,
) -> Result<(Vec<Command>, Option<ResolvedCell>), SessionError> {
    let AudioBatchDestination::Console(target) = destination else {
        return Ok((Vec::new(), None));
    };
    validate(project, target, count)?;
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(1)
        .map_err(|_| SessionError::Allocation)?;
    let cell = match target {
        ConsoleDestination::Cell { track, scene } => {
            if project.session_cell(track, scene).is_some() {
                commands.push(Command::ClearSessionCell { track, scene });
            }
            ResolvedCell { track, scene }
        }
        ConsoleDestination::InsertScene { track, before } => {
            let command = Command::InsertScene {
                name: format!("scene {}", project.scenes().len() + 1),
                before,
            };
            // The existing projected-Editor law allocates the actual ID on the
            // captured clone. The unchanged canonical batch repeats that command;
            // it never scans for a latest ID or commits this projected history.
            let mut projected = Editor::new(project.clone(), 1)?;
            let Created::Scene(scene) = projected.execute(command.clone())? else {
                return Err(SessionError::Invalid(
                    "console insertion did not create a scene",
                ));
            };
            commands.push(command);
            ResolvedCell { track, scene }
        }
    };
    Ok((commands, Some(cell)))
}

pub(crate) fn normalize(
    project: &Project,
    results: &[Created],
    cell: Option<ResolvedCell>,
    count: usize,
) -> Result<Vec<Created>, SessionError> {
    let created = if let Some(cell) = cell {
        let Some(SessionCellContent::Audio(audio)) = project.session_cell(cell.track, cell.scene)
        else {
            return Err(SessionError::Invalid(
                "console import candidate has no audio cell",
            ));
        };
        let mut clips = results.iter().filter_map(|result| {
            if let Created::SessionClip(id) = result {
                Some(*id)
            } else {
                None
            }
        });
        if clips.next() != Some(audio.id()) || clips.next().is_some() {
            return Err(SessionError::Invalid(
                "console import result differs from exact cell",
            ));
        }
        vec![Created::Imported {
            track: cell.track,
            asset: audio.asset(),
            clip: audio.id(),
        }]
    } else {
        results
            .iter()
            .filter(|result| matches!(result, Created::Imported { .. }))
            .cloned()
            .collect()
    };
    if created.len() != count {
        return Err(SessionError::Invalid("audio import result count differs"));
    }
    Ok(created)
}
