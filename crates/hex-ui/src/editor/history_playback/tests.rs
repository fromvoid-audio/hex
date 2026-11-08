use super::*;
use crate::editor::{
    arrangement_seek::{SeekKind, SeekStage},
    edit_cursor::EditCursor,
};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use std::{sync::Arc, sync::atomic::Ordering::Acquire, time::Instant};

#[gpui::test]
fn play_menu_preserves_history_through_delayed_ack_and_completed_repaints(cx: &mut TestAppContext) {
    let mut fixture = crate::editor::worker_ui_fixture::open(cx, feedback_commands());
    let editor = fixture.editor.clone();
    let project = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let track = project.tracks()[0].id();
    cx.update(crate::editor::bind_editor_keys);
    editor.update(cx, |view, cx| {
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
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
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
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            project.as_ref()
        );
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
        assert!(
            Instant::now() < deadline,
            "{:?}",
            editor.read_with(&visual, |view, _| (
                view.prepared_seek.as_ref().map(|p| (p.stage, p.cancelled)),
                view.error.as_ref().map(|e| e.to_string()),
                view.status.to_string(),
                view.pending_playback.is_some(),
                view.pending_resident_publication.is_some(),
                view.playback_candidates.len(),
                view.worker_requests_pending,
                view.state.as_ref().map(|s| s.revision),
                view.applied_project_revision,
                view.audible_playback.as_ref().map(|a| a.audio_revision),
            ))
        );
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
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
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
    // Compare after the existing 256-frame transport declick. This tests the
    // actual UI-to-native history choice; a mislabeled Fresh request diverges.
    for _ in 0..4 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let frame = editor.read_with(&visual, |view, _| {
        view.audio_status().unwrap().performance_frame.load(Acquire)
    });
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let resources =
        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
            &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            &project,
            1 << 20,
            &cancelled,
        )
        .unwrap();
    let graph = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancelled,
        )
        .unwrap(),
    );
    let mut oracle =
        hex_engine::SessionPlayback::new_with_device_resources(graph, &resources).unwrap();
    oracle
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    for chunk in vec![[0.0; 2]; frame as usize].chunks_mut(127) {
        oracle.render(chunk).unwrap();
    }
    let mut expected = [[0.0; 2]; 64];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.001));
    let mut actual = [[0.0; 2]; 64];
    fixture.backend.render(&mut actual).unwrap();
    assert_eq!(actual, expected, "menu history continuation at {frame}");
    fixture.finish(cx);
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

/// Execute the same finite setup commands in an independent canonical editor to
/// retain its generated IDs, then let the real Service prepare its own resources.
fn feedback_commands() -> Vec<hex_project::Command> {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::device_gesture::{ParameterId, ParameterValue};
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::new(78).unwrap(),
        ParameterValue::Float(0.3),
    )
    .unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let mut commands = Vec::new();
    let mut edit = |command: Command| {
        let result = editor.execute(command.clone()).unwrap();
        commands.push(command);
        result
    };
    let Created::Track(track) = edit(Command::InsertTrackWithDevice {
        name: "feedback".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state,
    }) else {
        panic!("instrument track")
    };
    let Created::Clip(clip) = edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }) else {
        panic!("midi clip")
    };
    edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    commands
}
