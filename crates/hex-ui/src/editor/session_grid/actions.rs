//! Session grid target validation against the current project.
use super::super::EditorMode;
use hex_engine::SessionPlaybackCommand;
use hex_project::{ClipId, Project, SceneId, SessionCellContent, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum DeleteTarget {
    MidiNotes,
    SessionCell { track: TrackId, scene: SceneId },
    SessionScene(SceneId),
    ArrangementClip(ClipId),
}

pub(in crate::editor) fn resolve_delete_target(
    project: Option<&Project>,
    mode: EditorMode,
    piano_focused: bool,
    note_selection_nonempty: bool,
    selected_cell: Option<(TrackId, SceneId)>,
    selected_scene: Option<SceneId>,
    selected_clip: Option<ClipId>,
) -> Option<DeleteTarget> {
    if piano_focused {
        return note_selection_nonempty.then_some(DeleteTarget::MidiNotes);
    }
    if mode == EditorMode::Session {
        let project = project?;
        if let Some((track, scene)) = selected_cell {
            if project.track(track).is_none()
                || !project.scenes().iter().any(|item| item.id() == scene)
            {
                return None;
            }
            return matches!(
                project.session_cell(track, scene),
                Some(
                    SessionCellContent::Audio(_)
                        | SessionCellContent::Midi(_)
                        | SessionCellContent::Stop
                )
            )
            .then_some(DeleteTarget::SessionCell { track, scene });
        }
        return selected_scene
            .filter(|scene| project.scenes().iter().any(|item| item.id() == *scene))
            .map(DeleteTarget::SessionScene);
    }
    selected_clip.map(DeleteTarget::ArrangementClip)
}

pub(in crate::editor) fn action_is_current(
    project: Option<&Project>,
    command: SessionPlaybackCommand,
    expected_generation: u64,
    current_generation: u64,
    expected_revision: u64,
    current_revision: Option<u64>,
) -> bool {
    expected_generation == current_generation
        && current_revision == Some(expected_revision)
        && action_matches_project(project, command)
}

pub(in crate::editor) fn action_matches_project(
    project: Option<&Project>,
    command: SessionPlaybackCommand,
) -> bool {
    let Some(project) = project else {
        return false;
    };
    match command {
        SessionPlaybackCommand::LaunchClip { clip, .. } => project.session_cells().iter().any(
            |cell| matches!(cell.content(), SessionCellContent::Midi(item) if item.id() == clip),
        ),
        SessionPlaybackCommand::CueMidiClip { track, clip, .. } => {
            project.session_cells().iter().any(|cell| {
                cell.track() == track
                    && matches!(cell.content(), SessionCellContent::Midi(item) if item.id() == clip)
            })
        }
        SessionPlaybackCommand::LaunchScene { scene, .. } => {
            project.scenes().iter().any(|item| item.id() == scene)
        }
        SessionPlaybackCommand::StopTrack { track, .. } => project.track(track).is_some(),
        SessionPlaybackCommand::StopAllClips { .. }
        | SessionPlaybackCommand::BackToArrangement { .. }
        | SessionPlaybackCommand::Play
        | SessionPlaybackCommand::Pause
        | SessionPlaybackCommand::Stop
        | SessionPlaybackCommand::Seek(_)
        | SessionPlaybackCommand::CancelPending { .. } => true,
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{
        DeleteTarget, EditorMode, action_is_current, action_matches_project, resolve_delete_target,
    };
    use hex_engine::{LaunchQuantization, SessionPlaybackCommand};
    use hex_project::{Command, Created, Editor, Project, SessionCellContent};

    #[test]
    fn guarded_session_actions_require_current_stable_ids() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let launch = SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        };
        assert!(action_matches_project(Some(editor.project()), launch));
        assert!(action_is_current(
            Some(editor.project()),
            launch,
            8,
            8,
            12,
            Some(12)
        ));
        assert!(!action_is_current(
            Some(editor.project()),
            launch,
            7,
            8,
            12,
            Some(12)
        ));
        assert!(!action_is_current(
            Some(editor.project()),
            launch,
            8,
            8,
            12,
            Some(13)
        ));
        editor.execute(Command::RemoveScene(scene)).unwrap();
        assert!(!action_matches_project(Some(editor.project()), launch));
        let cancel = SessionPlaybackCommand::CancelPending { track: None };
        assert!(action_is_current(
            Some(editor.project()),
            cancel,
            8,
            8,
            13,
            Some(13)
        ));
        assert!(!action_is_current(
            Some(editor.project()),
            cancel,
            7,
            8,
            13,
            Some(13)
        ));
        let stop = SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        };
        assert!(action_matches_project(Some(editor.project()), stop));
        editor.execute(Command::RemoveTrack(track)).unwrap();
        assert!(!action_matches_project(Some(editor.project()), stop));
    }

    #[test]
    fn delete_target_routes_session_cells_scenes_and_piano_focus_safely() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 3).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Scene(midi_scene) = editor
            .execute(Command::AddScene {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("midi scene")
        };
        editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene: midi_scene,
                name: "pattern".into(),
                source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap();
        let Created::Scene(stop_scene) = editor
            .execute(Command::AddScene {
                name: "stop".into(),
            })
            .unwrap()
        else {
            panic!("stop scene")
        };
        editor
            .execute(Command::SetSessionStopCell {
                track,
                scene: stop_scene,
            })
            .unwrap();
        let Created::Scene(empty_scene) = editor
            .execute(Command::AddScene {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("empty scene")
        };

        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                Some((track, midi_scene)),
                Some(stop_scene),
                None,
            ),
            Some(DeleteTarget::SessionCell {
                track,
                scene: midi_scene
            })
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                Some((track, stop_scene)),
                None,
                None,
            ),
            Some(DeleteTarget::SessionCell {
                track,
                scene: stop_scene
            })
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                Some((track, empty_scene)),
                Some(midi_scene),
                None,
            ),
            None,
            "an empty selected cell is a no-op, not a fallback scene delete"
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                None,
                Some(midi_scene),
                None,
            ),
            Some(DeleteTarget::SessionScene(midi_scene))
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                true,
                true,
                Some((track, midi_scene)),
                Some(midi_scene),
                None,
            ),
            Some(DeleteTarget::MidiNotes)
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                true,
                false,
                Some((track, midi_scene)),
                Some(midi_scene),
                None,
            ),
            None,
            "empty piano selection cannot fall through to grid deletion"
        );

        editor.execute(Command::RemoveScene(empty_scene)).unwrap();
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                Some((track, empty_scene)),
                Some(midi_scene),
                None,
            ),
            None,
            "a stale cell must not fall through to scene deletion"
        );
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Session,
                false,
                false,
                None,
                Some(empty_scene),
                None,
            ),
            None
        );
        let Some(SessionCellContent::Midi(clip)) = editor.project().session_cell(track, midi_scene)
        else {
            panic!("session clip")
        };
        assert_eq!(
            resolve_delete_target(
                Some(editor.project()),
                EditorMode::Arrangement,
                false,
                false,
                None,
                None,
                Some(clip.id()),
            ),
            Some(DeleteTarget::ArrangementClip(clip.id()))
        );

        // Hybrid tracks hold audio Session cells; Delete clears them too.
        let Created::Scene(audio_scene) = editor
            .execute(Command::AddScene {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("audio scene")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 1_000,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene: audio_scene,
                name: "audio".into(),
                asset,
                source_start: 0,
                source_end: 1_000,
            })
            .unwrap();
        let with_audio = editor.project().snapshot();
        let target = resolve_delete_target(
            Some(editor.project()),
            EditorMode::Session,
            false,
            false,
            Some((track, audio_scene)),
            None,
            None,
        );
        assert_eq!(
            target,
            Some(DeleteTarget::SessionCell {
                track,
                scene: audio_scene
            })
        );
        editor
            .execute(Command::ClearSessionCell {
                track,
                scene: audio_scene,
            })
            .unwrap();
        assert!(editor.project().session_cell(track, audio_scene).is_none());
        assert!(editor.undo());
        assert!(matches!(
            editor.project().session_cell(track, audio_scene),
            Some(SessionCellContent::Audio(_))
        ));
        assert_eq!(editor.project().snapshot(), with_audio);
    }
}
