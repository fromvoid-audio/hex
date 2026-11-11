use super::{
    PendingPlaybackAction, allows_non_open_request, clear_rejected_scene_duplicate,
    intent_desired_playing, queued_action_is_current,
};
use crate::editor::PendingSceneDuplicate;
use hex_engine::{PlaybackAction, PlaybackCommand, SessionPlaybackCommand};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::Request;

#[test]
fn rejected_scene_duplicate_clears_only_its_completion_selection_intent() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "source".into(),
        })
        .expect("scene")
    else {
        panic!("scene creation returns a scene id")
    };
    let intent = || {
        Some(PendingSceneDuplicate {
            source_scene: scene,
            project_generation: 3,
            select_on_completion: true,
        })
    };
    let mut pending = intent();

    clear_rejected_scene_duplicate(
        &mut pending,
        &Request::Edit(Command::DuplicateScene(scene)),
    );
    assert!(pending.is_none());

    let mut pending = intent();
    clear_rejected_scene_duplicate(
        &mut pending,
        &Request::Edit(Command::MoveScene {
            scene,
            before: None,
        }),
    );
    assert!(pending.is_some());
}

#[test]
fn project_edit_backpressure_preserves_transport_but_invalidates_session_actions() {
    let project = Project::new(48_000).expect("project");
    let transport = PendingPlaybackAction {
        action: PlaybackAction::Transport(PlaybackCommand::Pause),
        generation: 8,
        revision: None,
        intent_token: None,
    };
    let session = PendingPlaybackAction {
        action: PlaybackAction::Session(SessionPlaybackCommand::StopAllClips {
            quantization: hex_engine::LaunchQuantization::Immediate,
        }),
        generation: 8,
        revision: Some(12),
        intent_token: None,
    };

    assert!(queued_action_is_current(
        transport,
        true,
        8,
        Some(&project),
        Some(12),
    ));
    assert!(!queued_action_is_current(
        session,
        true,
        8,
        Some(&project),
        Some(12),
    ));
    assert!(!queued_action_is_current(
        transport,
        false,
        9,
        Some(&project),
        Some(12),
    ));
}

#[test]
fn launch_and_transport_actions_have_explicit_play_intents() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 1).expect("editor");
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .expect("scene")
    else {
        panic!("scene")
    };
    assert_eq!(
        intent_desired_playing(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: hex_engine::LaunchQuantization::Immediate,
            }
        )),
        Some(true)
    );
    assert_eq!(
        intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Play)),
        Some(true)
    );
    assert_eq!(
        intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Pause)),
        Some(false)
    );
    assert_eq!(
        intent_desired_playing(PlaybackAction::Transport(PlaybackCommand::Seek(1))),
        None
    );
}

#[test]
fn close_prompt_allows_only_owned_duplicate_continuations() {
    assert!(allows_non_open_request(true, false, true));
    assert!(!allows_non_open_request(true, false, false));
    assert!(allows_non_open_request(false, false, false));
    assert!(!allows_non_open_request(true, true, true));
}
