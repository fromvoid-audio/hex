//! GPUI production-event regressions for project replacement and publication policy.
use super::*;
use gpui::TestAppContext;
use hex_engine::{PreparedRevision, SineTable};
use hex_project::{Command, Editor, Project};
use hex_session::service::{Service, ViewState};
use std::collections::BTreeMap;

fn event(project: Arc<Project>, revision: u64, replaced: bool) -> Event {
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("test revision"),
    );
    Event {
        state: Arc::new(ViewState {
            project,
            waveforms: Default::default(),
            midi_preview: Arc::new(Default::default()),
            timeline_frames: 384_000,
            can_undo: false,
            can_redo: false,
            revision,
            unsaved: !replaced,
            document_path: None,
        }),
        playback: Some(Playback::try_from_prepared_with_session(prepared).expect("playback")),
        status: None,
        error: None,
        request_complete: true,
        save_complete: false,
        open_complete: replaced,
        project_replaced: replaced,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
    }
}

#[gpui::test]
fn receive_preserves_fresh_installation_through_edits_and_failed_open(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let mut project = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let hex_project::Created::Track(track) = project
        .execute(Command::AddTrack {
            name: "new file".into(),
        })
        .expect("track")
    else {
        panic!("track expected")
    };
    let original = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("first edit");
    let first_edit = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.25 })
        .expect("second edit");
    let second_edit = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.75 })
        .expect("later edit");
    let later_edit = Arc::new(project.project().clone());
    window
        .update(cx, |view, _, _| {
            view.receive(event(Arc::clone(&original), 0, true));
            let generation = view.project_generation;
            assert!(
                view.pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );
            // Multiple committed edits can arrive before audio publication has
            // space. They replace the candidate, not its project identity.
            view.receive(event(first_edit, 1, false));
            view.receive(event(Arc::clone(&second_edit), 2, false));
            let pending = view.pending_playback.as_ref().unwrap();
            assert!(pending.install_fresh_runtime);
            assert_eq!(pending.project_revision, 2);
            assert_eq!(pending.project_generation, generation);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .gain(),
                0.25
            );

            let mut refused_open = event(second_edit, 2, false);
            refused_open.playback = None;
            refused_open.open_complete = true;
            refused_open.error = Some("missing project".into());
            view.receive(refused_open);
            assert_eq!(view.project_generation, generation);
            assert!(
                view.pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );

            // The audio queue owns an admitted candidate. Later same-project
            // edits inherit, while another successful Open installs fresh again.
            let admitted = view.pending_playback.take().unwrap();
            let (mut control, _callback) = hex_audio::prepared::channel(48_000).unwrap();
            assert!(
                control
                    .publish_with_policy(admitted.playback, PublicationPolicy::InstallFreshRuntime)
                    .is_ok()
            );
            view.receive(event(later_edit, 3, false));
            assert!(
                !view
                    .pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );
            view.receive(event(original, 0, true));
            assert_ne!(view.project_generation, generation);
            let pending = view.pending_playback.as_ref().unwrap();
            assert!(pending.install_fresh_runtime);
            assert_eq!(pending.project_revision, 0);
            assert_eq!(pending.project_generation, view.project_generation);
        })
        .expect("test window");
}
