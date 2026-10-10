use hex_engine::compile;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn compile_project(project: &Project) -> Result<hex_engine::Project, hex_engine::Invalid> {
    compile::project(project, &BTreeMap::new())
}

#[test]
fn playback_refuses_return_tracks_and_nonzero_sends_until_routing_is_compiled() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    assert!(compile_project(editor.project()).is_ok());
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(compile_project(editor.project()).is_ok());

    let before = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let rejected = editor.execute_validated(
        Command::AddReturnTrack {
            name: "return".into(),
        },
        |candidate| {
            compile_project(candidate)
                .map(|_| ())
                .map_err(|_| hex_project::Invalid("compile rejected return"))
        },
    );
    assert!(rejected.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.can_undo(), can_undo);

    let return_track = match editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    {
        Created::ReturnTrack(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(compile_project(editor.project()).is_err());
    let before_send = editor.project().snapshot();
    let can_undo = editor.can_undo();
    let rejected_send = editor.execute_validated(
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        },
        |candidate| {
            compile_project(candidate)
                .map(|_| ())
                .map_err(|_| hex_project::Invalid("compile rejected send"))
        },
    );
    assert!(rejected_send.is_err());
    assert_eq!(editor.project().snapshot(), before_send);
    assert_eq!(editor.can_undo(), can_undo);

    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .unwrap();
    assert!(compile_project(editor.project()).is_err());
}
