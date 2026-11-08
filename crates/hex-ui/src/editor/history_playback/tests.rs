use super::*;
use crate::editor::{
    PlaybackCandidate,
    arrangement_seek::{SeekKind, SeekStage},
    edit_cursor::EditCursor,
};
use crate::editor_backend::EditorAudio;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use std::{collections::BTreeMap, sync::Arc, sync::atomic::Ordering::Acquire, time::Instant};

#[gpui::test]
fn play_menu_preserves_history_through_delayed_ack_and_completed_repaints(cx: &mut TestAppContext) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let mut project =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = project
        .execute(hex_project::Command::AddTrack {
            name: "arrangement".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let project = project.project().clone();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let audio_revision = output
        .publication
        .control()
        .unwrap()
        .publish(
            hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
                .unwrap(),
        )
        .unwrap();
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let client = output.take_client().unwrap();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::default());
    cx.update(crate::editor::bind_editor_keys);
    editor.update(cx, |view, cx| {
        // Geometry helper installs an explicit isolated view snapshot. Consume
        // its startup event so the normal publication tick cannot replace it.
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if view.service.try_event().is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fixture startup event unavailable"
            );
            std::thread::yield_now();
        }
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "history fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.applied_project_revision = Some(1);
        view.audible_playback = Some(PlaybackCandidate {
            worker_requires_preroll: None,
            observation: None,
            audio_revision,
            project_generation: view.project_generation,
            state: view.state.as_ref().unwrap().clone(),
            prepared_revision: Some(prepared),
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        view.edit_cursor = Some(EditCursor {
            track,
            frame: 256,
            generation: view.project_generation,
            pulse_color: 0xffffff,
            pulse_started: Instant::now(),
        });
        view.error = None;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let play = visual.debug_bounds("transport-play-icon").unwrap().center();
    visual.simulate_mouse_down(play, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(play, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.context_menu.as_ref().unwrap().items[0].label,
            "play with history"
        );
        assert!(
            view.prepared_seek.is_none(),
            "opening the menu never starts playback"
        );
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
        let pending = view.prepared_seek.as_ref().expect("history request queued");
        assert!(matches!(pending.kind, SeekKind::History));
        assert_eq!(pending.destination_frame, 256);
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &project);
    });
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    loop {
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(&visual, |view, _| {
            matches!(
                view.prepared_seek.as_ref().map(|p| p.stage),
                Some(SeekStage::Published { .. })
            )
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "{:?}", editor.read_with(&visual, |view, _| (
            view.prepared_seek.as_ref().map(|p| (p.stage, p.cancelled)),
            view.error.as_ref().map(|e| e.to_string()), view.status.to_string(),
            view.pending_playback.is_some(), view.pending_resident_publication.is_some(),
            view.playback_candidates.len(), view.worker_requests_pending,
            view.state.as_ref().map(|s| s.revision), view.applied_project_revision,
            view.audible_playback.as_ref().map(|a| a.audio_revision),
        )));
        std::thread::yield_now();
    }
    for _ in 0..2 {
        editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                view.prepared_seek.as_ref().unwrap().kind,
                SeekKind::History
            ));
            assert_eq!(view.playback_start_frame(), 256);
            assert_eq!(view.audio_status().unwrap().position.load(Acquire), 0);
        });
    }
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.prepared_seek.is_none(),
            "exact callback acknowledgement retires the intent"
        );
        assert_eq!(view.playback_start_frame(), 256);
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 320);
    });
    drop(backend);
    output.publication.close();
}

#[gpui::test]
fn history_menu_refuses_stale_loop_and_work_limit_without_a_request(cx: &mut TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let action = MenuAction {
            generation: view.project_generation + 1,
            revision: 1,
            frame: 0,
        };
        view.history_playback_menu_action(action, cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("target changed")
        );
        let action = MenuAction {
            generation: view.project_generation,
            revision: 1,
            frame: hex_session::arrangement_seek::MAX_SEEK_WORK_FRAMES + 1,
        };
        view.history_playback_menu_action(action, cx);
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("limit exceeded")
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
    let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
    snapshot.loop_region = Some(hex_project::LoopRegion::new(0, 512, true).unwrap());
    let (_, loop_editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::from_snapshot(snapshot).unwrap(),
        Arc::default(),
    );
    loop_editor.update(cx, |view, cx| {
        view.history_playback_menu_action(
            MenuAction {
                generation: view.project_generation,
                revision: 1,
                frame: 0,
            },
            cx,
        );
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("loop off")
        );
        assert!(view.prepared_seek.is_none());
        assert!(view.latched_seek_intent.is_none());
    });
}
