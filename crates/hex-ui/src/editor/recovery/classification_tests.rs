use super::*;
use hex_project::{Created, Editor, Project, TrackId, TrackSelectionIds};
use std::path::PathBuf;

fn sample_track_id() -> TrackId {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack { name: "t".into() })
        .unwrap()
    else {
        panic!("track expected")
    };
    track
}

#[test]
fn graph_republishing_classifier_covers_solo_mute_history_and_excludes_control_only_requests() {
    let track = sample_track_id();
    let tracks = TrackSelectionIds::new(&[track]).unwrap();

    // Solo, mute, history, and general project/mixer/clip edits republish the graph.
    assert!(is_graph_republishing_edit(&Request::Edit(
        Command::ToggleSolo {
            track,
            exclusive: false,
        }
    )));
    assert!(is_graph_republishing_edit(&Request::Edit(
        Command::ToggleSoloTracks {
            tracks: tracks.clone(),
            exclusive: true,
        }
    )));
    assert!(is_graph_republishing_edit(&Request::Edit(
        Command::ToggleMute(track)
    )));
    assert!(is_graph_republishing_edit(&Request::Edit(
        Command::ToggleTracksMute(tracks.clone())
    )));
    assert!(is_graph_republishing_edit(&Request::Edit(Command::Mute {
        track,
        muted: true,
    })));
    assert!(is_graph_republishing_edit(&Request::Undo));
    assert!(is_graph_republishing_edit(&Request::Redo));
    assert!(is_graph_republishing_edit(&Request::CutClips(Vec::new())));
    assert!(is_graph_republishing_edit(&Request::TrackClipboard {
        project_generation: 0,
        revision: 0,
        action: TrackClipboardAction::Cut(tracks.clone()),
    }));
    assert!(is_graph_republishing_edit(&Request::TuningLibrary {
        project_generation: 0,
        revision: 0,
        operation_id: 1,
        operation: TuningLibraryOperation::Load { slot: 0 },
    }));

    // Read-only copies, saves, opens, audio lifecycle, queries, and exports stay unintercepted.
    assert!(!is_graph_republishing_edit(&Request::TrackClipboard {
        project_generation: 0,
        revision: 0,
        action: TrackClipboardAction::Copy(tracks),
    }));
    assert!(!is_graph_republishing_edit(&Request::TuningLibrary {
        project_generation: 0,
        revision: 0,
        operation_id: 2,
        operation: TuningLibraryOperation::List,
    }));
    assert!(!is_graph_republishing_edit(&Request::SaveCurrent));
    assert!(!is_graph_republishing_edit(&Request::Open(PathBuf::from(
        "fixture.hex"
    ))));
    assert!(!is_graph_republishing_edit(&Request::RestartAudio {
        project_generation: 0,
        request_id: 1,
    }));
    assert!(!is_graph_republishing_edit(
        &Request::RestoreStoppedPlayback
    ));
    assert!(!is_graph_republishing_edit(
        &Request::QueryFailedPluginAutomation { request_id: 1 }
    ));
    assert!(!is_graph_republishing_edit(&Request::Export(
        PathBuf::from("out.wav")
    )));
    assert!(!is_graph_republishing_edit(&Request::CopyClips(Vec::new())));
    assert!(!is_graph_republishing_edit(&Request::ReloadPlugin {
        project_generation: 0,
        device: {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
            let Created::Device(device) = editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: hex_engine::default_native_device_state(
                        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                    )
                    .unwrap(),
                })
                .unwrap()
            else {
                panic!("device expected")
            };
            device
        },
    }));
}

#[test]
fn deferred_graph_edit_messages_are_lowercase_and_actionable() {
    let track = sample_track_id();
    for (request, expected_wait, expected_apply) in [
        (
            Request::Edit(Command::ToggleSolo {
                track,
                exclusive: false,
            }),
            "restarting audio · solo will update when ready",
            "updating solo",
        ),
        (
            Request::Edit(Command::ToggleMute(track)),
            "restarting audio · mute will update when ready",
            "updating mute",
        ),
        (
            Request::Undo,
            "restarting audio · undo will apply when ready",
            "undoing",
        ),
        (
            Request::Redo,
            "restarting audio · redo will apply when ready",
            "redoing",
        ),
        (
            Request::Edit(Command::AddTrack {
                name: "audio".into(),
            }),
            "restarting audio · edit will apply when ready",
            "applying edit",
        ),
    ] {
        let wait = deferred_graph_edit_waiting_status(&request);
        let apply = deferred_graph_edit_applying_status(&request);
        assert_eq!(wait, expected_wait);
        assert_eq!(apply, expected_apply);
        assert_eq!(wait, wait.to_lowercase());
        assert_eq!(apply, apply.to_lowercase());
        assert!(!wait.contains("native graph source is unavailable"));
        assert!(!apply.contains("native graph source is unavailable"));
    }
}
