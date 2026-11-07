//! Stable-ID keyboard navigation and Enter targets for the Session grid.
use super::EditorMode;
use hex_project::{ClipId, Project, SceneId, SessionCellContent, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    Cell { track: TrackId, scene: SceneId },
    Scene(SceneId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EnterTarget {
    LaunchClip { track: TrackId, clip: ClipId },
    StopTrack(TrackId),
    LaunchScene(SceneId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArrowTarget {
    Piano,
    Session,
    Arrangement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SceneMoveTarget {
    pub scene: SceneId,
    pub before: Option<SceneId>,
}

pub(super) fn resolve_scene_move(
    project: &Project,
    scene: SceneId,
    direction: Direction,
) -> Option<SceneMoveTarget> {
    let scenes = project.scenes();
    let index = scenes.iter().position(|item| item.id() == scene)?;
    let before = match direction {
        Direction::Up if index > 0 => Some(scenes.get(index - 1)?.id()),
        Direction::Down if index + 1 < scenes.len() => scenes.get(index + 2).map(|item| item.id()),
        Direction::Left | Direction::Right | Direction::Up | Direction::Down => return None,
    };
    Some(SceneMoveTarget { scene, before })
}

pub(super) fn arrow_target(mode: EditorMode, piano_focused: bool) -> ArrowTarget {
    if piano_focused {
        ArrowTarget::Piano
    } else if mode == EditorMode::Session {
        ArrowTarget::Session
    } else {
        ArrowTarget::Arrangement
    }
}

pub(super) fn session_editor_has_focus(mode: EditorMode, piano_focused: bool) -> bool {
    arrow_target(mode, piano_focused) == ArrowTarget::Session
}

#[cfg(test)]
pub(super) fn resolve_navigation(
    project: &Project,
    selected_cell: Option<(TrackId, SceneId)>,
    selected_scene: Option<SceneId>,
    direction: Direction,
) -> Option<Selection> {
    let indices: Vec<usize> = (0..project.tracks().len()).collect();
    resolve_visible_navigation(project, &indices, selected_cell, selected_scene, direction)
}

/// Navigation uses the same canonical-index projection as painted columns.
pub(super) fn resolve_visible_navigation(
    project: &Project,
    indices: &[usize],
    selected_cell: Option<(TrackId, SceneId)>,
    selected_scene: Option<SceneId>,
    direction: Direction,
) -> Option<Selection> {
    let tracks = project.tracks();
    let visible_track =
        |position: usize| indices.get(position).and_then(|index| tracks.get(*index));
    let scenes = project.scenes();
    if let Some((track, scene)) = selected_cell {
        let scene_index = scenes.iter().position(|item| item.id() == scene)?;
        let Some(track_index) = indices
            .iter()
            .position(|index| tracks.get(*index).is_some_and(|item| item.id() == track))
        else {
            let canonical = project
                .tracks()
                .iter()
                .position(|item| item.id() == track)?;
            let next = indices.partition_point(|index| *index < canonical);
            return match direction {
                Direction::Right if next >= indices.len() => Some(Selection::Scene(scene)),
                Direction::Right => Some(Selection::Cell {
                    track: visible_track(next)?.id(),
                    scene,
                }),
                Direction::Left | Direction::Up | Direction::Down => Some(Selection::Cell {
                    track: visible_track(next.saturating_sub(1))?.id(),
                    scene,
                }),
            };
        };
        if direction == Direction::Right && track_index + 1 == indices.len() {
            return Some(Selection::Scene(scene));
        }
        let next_track = match direction {
            Direction::Left => track_index.saturating_sub(1),
            Direction::Right => track_index.saturating_add(1),
            Direction::Up | Direction::Down => track_index,
        };
        let next_scene = match direction {
            Direction::Up => scene_index.saturating_sub(1),
            Direction::Down => scene_index
                .saturating_add(1)
                .min(scenes.len().saturating_sub(1)),
            Direction::Left | Direction::Right => scene_index,
        };
        return Some(Selection::Cell {
            track: visible_track(next_track)?.id(),
            scene: scenes.get(next_scene)?.id(),
        });
    }
    if let Some(scene) = selected_scene {
        let scene_index = scenes.iter().position(|item| item.id() == scene)?;
        return match direction {
            Direction::Up => Some(Selection::Scene(
                scenes.get(scene_index.saturating_sub(1))?.id(),
            )),
            Direction::Down => Some(Selection::Scene(
                scenes
                    .get(
                        scene_index
                            .saturating_add(1)
                            .min(scenes.len().saturating_sub(1)),
                    )?
                    .id(),
            )),
            Direction::Left if !indices.is_empty() => Some(Selection::Cell {
                track: tracks.get(*indices.last()?)?.id(),
                scene,
            }),
            Direction::Left | Direction::Right => Some(Selection::Scene(scene)),
        };
    }
    if indices.is_empty() {
        return Some(Selection::Scene(scenes.first()?.id()));
    }
    Some(Selection::Cell {
        track: visible_track(0)?.id(),
        scene: scenes.first()?.id(),
    })
}

pub(super) fn resolve_enter_target(
    project: &Project,
    selected_cell: Option<(TrackId, SceneId)>,
    selected_scene: Option<SceneId>,
) -> Option<EnterTarget> {
    if let Some((track, scene)) = selected_cell {
        if project.track(track).is_none() || !project.scenes().iter().any(|item| item.id() == scene)
        {
            return None;
        }
        return match project.session_cell(track, scene)? {
            SessionCellContent::Midi(clip) => Some(EnterTarget::LaunchClip {
                track,
                clip: clip.id(),
            }),
            SessionCellContent::Audio(clip) => Some(EnterTarget::LaunchClip {
                track,
                clip: clip.id(),
            }),
            SessionCellContent::Stop => Some(EnterTarget::StopTrack(track)),
        };
    }
    selected_scene
        .filter(|scene| project.scenes().iter().any(|item| item.id() == *scene))
        .map(EnterTarget::LaunchScene)
}

#[cfg(test)]
mod tests {
    use super::{
        ArrowTarget, Direction, EnterTarget, SceneMoveTarget, Selection, arrow_target,
        resolve_enter_target, resolve_navigation, resolve_scene_move,
    };
    use crate::editor::EditorMode;
    use crate::editor::session_navigation::session_editor_has_focus;
    use hex_project::{BeatTime, Command, Created, Editor, Project};

    fn fixture() -> (
        Project,
        hex_project::TrackId,
        hex_project::TrackId,
        hex_project::SceneId,
        hex_project::SceneId,
        hex_project::SceneId,
        hex_project::ClipId,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(first_track) = editor
            .execute(Command::AddMidiTrack { name: "one".into() })
            .unwrap()
        else {
            panic!("first track")
        };
        let Created::Track(second_track) = editor
            .execute(Command::AddMidiTrack { name: "two".into() })
            .unwrap()
        else {
            panic!("second track")
        };
        let Created::Scene(first_scene) = editor
            .execute(Command::AddScene { name: "one".into() })
            .unwrap()
        else {
            panic!("first scene")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track: first_track,
                scene: first_scene,
                name: "pattern".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        let Created::Scene(second_scene) = editor
            .execute(Command::AddScene { name: "two".into() })
            .unwrap()
        else {
            panic!("second scene")
        };
        editor
            .execute(Command::SetSessionStopCell {
                track: second_track,
                scene: second_scene,
            })
            .unwrap();
        let Created::Scene(third_scene) = editor
            .execute(Command::AddScene {
                name: "three".into(),
            })
            .unwrap()
        else {
            panic!("third scene")
        };
        (
            editor.project().clone(),
            first_track,
            second_track,
            first_scene,
            second_scene,
            third_scene,
            clip,
        )
    }

    #[test]
    fn cell_navigation_clamps_each_direction_and_no_selection_starts_top_left() {
        let (project, first_track, second_track, first_scene, second_scene, _, _) = fixture();
        let selected = Some((first_track, first_scene));
        assert_eq!(
            resolve_navigation(&project, selected, Some(second_scene), Direction::Left),
            Some(Selection::Cell {
                track: first_track,
                scene: first_scene
            })
        );
        assert_eq!(
            resolve_navigation(&project, selected, None, Direction::Right),
            Some(Selection::Cell {
                track: second_track,
                scene: first_scene
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((second_track, first_scene)),
                None,
                Direction::Left
            ),
            Some(Selection::Cell {
                track: first_track,
                scene: first_scene
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((second_track, first_scene)),
                None,
                Direction::Right
            ),
            Some(Selection::Scene(first_scene))
        );
        assert_eq!(
            resolve_navigation(&project, selected, None, Direction::Down),
            Some(Selection::Cell {
                track: first_track,
                scene: second_scene
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((first_track, second_scene)),
                None,
                Direction::Up
            ),
            Some(Selection::Cell {
                track: first_track,
                scene: first_scene
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((second_track, second_scene)),
                None,
                Direction::Down
            ),
            Some(Selection::Cell {
                track: second_track,
                scene: project.scenes().last().unwrap().id()
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((first_track, first_scene)),
                None,
                Direction::Up
            ),
            Some(Selection::Cell {
                track: first_track,
                scene: first_scene
            })
        );
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(
                resolve_navigation(&project, None, None, direction),
                Some(Selection::Cell {
                    track: first_track,
                    scene: first_scene
                })
            );
        }
    }

    #[test]
    fn scene_only_navigation_stays_scene_only_vertically_and_enters_from_edges() {
        let (project, first_track, second_track, first_scene, second_scene, third_scene, _) =
            fixture();
        assert_eq!(
            resolve_navigation(&project, None, Some(second_scene), Direction::Up),
            Some(Selection::Scene(first_scene))
        );
        assert_eq!(
            resolve_navigation(&project, None, Some(second_scene), Direction::Down),
            Some(Selection::Scene(third_scene))
        );
        assert_eq!(
            resolve_navigation(&project, None, Some(second_scene), Direction::Right),
            Some(Selection::Scene(second_scene))
        );
        assert_eq!(
            resolve_navigation(&project, None, Some(second_scene), Direction::Left),
            Some(Selection::Cell {
                track: second_track,
                scene: second_scene
            })
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((second_track, second_scene)),
                None,
                Direction::Right
            ),
            Some(Selection::Scene(second_scene))
        );
        assert_eq!(
            resolve_navigation(
                &project,
                Some((first_track, second_scene)),
                None,
                Direction::Left
            ),
            Some(Selection::Cell {
                track: first_track,
                scene: second_scene
            })
        );
    }

    #[test]
    fn scene_reorder_targets_stable_neighbors_and_clamps_edges() {
        let (project, _, _, first, second, third, _) = fixture();
        assert_eq!(resolve_scene_move(&project, first, Direction::Up), None);
        assert_eq!(
            resolve_scene_move(&project, first, Direction::Down),
            Some(SceneMoveTarget {
                scene: first,
                before: Some(third),
            })
        );
        assert_eq!(
            resolve_scene_move(&project, second, Direction::Up),
            Some(SceneMoveTarget {
                scene: second,
                before: Some(first),
            })
        );
        assert_eq!(
            resolve_scene_move(&project, second, Direction::Down),
            Some(SceneMoveTarget {
                scene: second,
                before: None,
            })
        );
        assert_eq!(resolve_scene_move(&project, third, Direction::Down), None);
        assert_eq!(resolve_scene_move(&project, third, Direction::Left), None);
        let mut editor = Editor::new(project, 8).unwrap();
        editor.execute(Command::RemoveScene(third)).unwrap();
        assert_eq!(
            resolve_scene_move(editor.project(), third, Direction::Up),
            None
        );
    }

    #[test]
    fn empty_and_stale_grid_selections_are_safe_no_ops() {
        let empty = Project::new(48_000).unwrap();
        assert_eq!(
            resolve_navigation(&empty, None, None, Direction::Down),
            None
        );
        let mut scene_only = Editor::new(empty, 4).unwrap();
        let Created::Scene(only_scene) = scene_only
            .execute(Command::AddScene {
                name: "only".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        assert_eq!(
            resolve_navigation(scene_only.project(), None, None, Direction::Right),
            Some(Selection::Scene(only_scene))
        );
        let (mut project, first_track, _, first_scene, _, _, _) = fixture();
        let stale_scene = first_scene;
        let stale_track = first_track;
        let mut editor = Editor::new(project.clone(), 9).unwrap();
        editor.execute(Command::RemoveTrack(stale_track)).unwrap();
        editor.execute(Command::RemoveScene(stale_scene)).unwrap();
        project = editor.project().clone();
        assert_eq!(
            resolve_navigation(
                &project,
                Some((stale_track, stale_scene)),
                None,
                Direction::Right,
            ),
            None
        );
        assert_eq!(
            resolve_navigation(&project, None, Some(stale_scene), Direction::Left),
            None
        );
        assert_eq!(
            resolve_enter_target(&project, Some((stale_track, stale_scene)), None),
            None
        );
        assert_eq!(
            resolve_enter_target(&project, None, Some(stale_scene)),
            None
        );
    }

    #[test]
    fn enter_resolves_only_current_canonical_session_content() {
        let (project, first_track, second_track, first_scene, second_scene, _third_scene, clip) =
            fixture();
        assert_eq!(
            resolve_enter_target(
                &project,
                Some((first_track, first_scene)),
                Some(second_scene)
            ),
            Some(EnterTarget::LaunchClip {
                track: first_track,
                clip
            })
        );
        assert_eq!(
            resolve_enter_target(&project, Some((second_track, second_scene)), None),
            Some(EnterTarget::StopTrack(second_track))
        );
        assert_eq!(
            resolve_enter_target(&project, Some((first_track, second_scene)), None),
            None
        );
        assert_eq!(
            resolve_enter_target(&project, None, Some(first_scene)),
            Some(EnterTarget::LaunchScene(first_scene))
        );
        assert_eq!(resolve_enter_target(&project, None, None), None);
    }

    #[test]
    fn session_arrows_and_enter_do_not_take_over_piano_focus_or_arrangement_mode() {
        assert_eq!(arrow_target(EditorMode::Session, true), ArrowTarget::Piano);
        assert_eq!(
            arrow_target(EditorMode::Session, false),
            ArrowTarget::Session
        );
        assert_eq!(
            arrow_target(EditorMode::Arrangement, false),
            ArrowTarget::Arrangement
        );
        assert!(session_editor_has_focus(EditorMode::Session, false));
        assert!(!session_editor_has_focus(EditorMode::Session, true));
        assert!(!session_editor_has_focus(EditorMode::Arrangement, false));
    }
}
