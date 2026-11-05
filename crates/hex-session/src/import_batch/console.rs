//! Console placement is resolved on control-side candidate metadata, never by ID guesses.
use crate::{SessionError, import_batch::AudioBatchDestination};
use hex_project::{
    Command, Created, Editor, Project, SceneId, SessionCellContent, SessionSceneTarget, TrackId,
};

/// One source targets a stable cell/row, or creates one normal track and an explicit scene target.
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
    NewTrack {
        before_track: Option<TrackId>,
        scene: SessionSceneTarget,
    },
}

impl ConsoleDestination {
    /// The same read-only policy used by UI preview and worker admission.
    pub fn validate(self, project: &Project) -> Result<(), SessionError> {
        validate(project, self, 1)
    }
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
            SessionSceneTarget::Insert { before }.validate(project)?;
            (track, before)
        }
        ConsoleDestination::NewTrack {
            before_track,
            scene,
        } => {
            if project.tracks().len() >= hex_project::MAX_TRACKS {
                return Err(SessionError::Invalid("track limit reached"));
            }
            if before_track.is_some_and(|id| project.track(id).is_none()) {
                return Err(SessionError::Invalid(
                    "track insertion anchor no longer exists",
                ));
            }
            scene.validate(project)?;
            return Ok(());
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
    track_name: &str,
    new_track_gain: Option<f32>,
) -> Result<(Vec<Command>, Option<ResolvedCell>), SessionError> {
    let AudioBatchDestination::Console(target) = destination else {
        return Ok((Vec::new(), None));
    };
    validate(project, target, count)?;
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(3)
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
        ConsoleDestination::NewTrack {
            before_track,
            scene,
        } => {
            let mut projected = Editor::new(project.clone(), 1)?;
            let command = Command::InsertTrack {
                name: track_name.into(),
                before_track,
            };
            let Created::Track(track) = projected.execute(command.clone())? else {
                return Err(SessionError::Invalid(
                    "console insertion did not create a track",
                ));
            };
            commands.push(command);
            if let Some(gain) = new_track_gain {
                let gain_command = Command::Gain { track, gain };
                projected.execute(gain_command.clone())?;
                commands.push(gain_command);
            }
            let scene = match scene {
                SessionSceneTarget::Existing(scene) => scene,
                SessionSceneTarget::Insert { before } => {
                    let command = Command::InsertScene {
                        name: format!("scene {}", project.scenes().len() + 1),
                        before,
                    };
                    let Created::Scene(scene) = projected.execute(command.clone())? else {
                        return Err(SessionError::Invalid(
                            "console insertion did not create a scene",
                        ));
                    };
                    commands.push(command);
                    scene
                }
            };
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
    let mut created = Vec::new();
    created
        .try_reserve_exact(count)
        .map_err(|_| SessionError::Allocation)?;
    if let Some(cell) = cell {
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
        created.push(Created::Imported {
            track: cell.track,
            asset: audio.asset(),
            clip: audio.id(),
        });
    } else {
        for result in results
            .iter()
            .filter(|result| matches!(result, Created::Imported { .. }))
        {
            if created.len() == count {
                return Err(SessionError::Invalid("audio import has extra results"));
            }
            created.push(*result);
        }
    }
    if created.len() != count {
        return Err(SessionError::Invalid("audio import result count differs"));
    }
    Ok(created)
}
