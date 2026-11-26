//! Arrangement locator UI integration tests.
use super::*;
use crate::editor::{
    clip_drag_ui_test::{editor_window, pump_move},
    external_audio_drop_ui_test::BatchFiles,
    musical_time::SnapGrid,
};
use gpui::{Modifiers, TestAppContext, VisualTestContext, point};

fn bounds(
    visual: &mut VisualTestContext,
    prefix: &str,
    id: LocatorId,
) -> gpui::Bounds<gpui::Pixels> {
    let selector = Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
    visual
        .debug_bounds(selector)
        .expect("visible locator element")
}

fn wait_for_locator_checkpoint(path: &std::path::Path, project: &hex_project::Project) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match hex_storage::load_journal(path) {
            Ok(published) if &published == project => return,
            Ok(_) => {}
            Err(hex_storage::StorageError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("invalid locator recovery journal: {error}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "committed locator checkpoint"
        );
        std::thread::yield_now();
    }
}

fn next_terminal(view: &EditorView) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = view.service.try_event() {
            assert!(event.request_complete);
            return event;
        }
        assert!(std::time::Instant::now() < deadline, "locator terminal");
        std::thread::yield_now();
    }
}

#[gpui::test]
fn applied_locator_actions_survive_held_recovery(cx: &mut TestAppContext) {
    use hex_engine::{LaunchQuantization, PlaybackCommand};
    use std::time::{Duration, Instant};

    cx.update(super::super::bind_editor_keys);
    let environment = super::super::test_environment::Environment::new().unwrap();
    let recovery = environment.root().join("held-locator-action.hex-journal");
    let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
        cx,
        vec![
            Command::AddTrack {
                name: "track".into(),
            },
            Command::AddArrangementLocator { frame: 48_000 },
        ],
        false,
        true,
        Some(recovery.clone()),
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.session_grid.quantization = LaunchQuantization::Immediate;
        view.computer_keyboard.enabled = false;
        view.place_locator_play_start(24_000);
        cx.notify();
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    let mut gate = hex_session::service::hold_next_checkpoint(recovery).unwrap();
    visual.simulate_keystrokes("i");
    editor.read_with(&visual, |view, _| assert_eq!(view.locators.adds.len(), 1));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut entered = false;
    let mut observation = None;
    while !entered || observation.is_none() {
        if !entered {
            match gate.try_entered() {
                Ok(()) => entered = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("checkpoint gate: {error}"),
            }
        }
        if observation.is_none() {
            observation = editor.read_with(&visual, |view, _| {
                view.service.try_arrangement_edit_view().ok()
            });
        }
        assert!(Instant::now() < deadline, "held locator observation");
        std::thread::yield_now();
    }
    let observation = observation.unwrap();
    let committed = observation.state.upgrade().unwrap();
    let id = committed
        .project
        .arrangement_locators()
        .first()
        .unwrap()
        .id();
    let revision = committed.revision;
    // Hold delivery of the real completion to exercise actions on the
    // committed view before the UI installs its canonical revision.
    let terminal = editor.read_with(&visual, |view, _| next_terminal(view));
    assert!(
        matches!(terminal.created, Some(hex_project::Created::ArrangementLocator(created)) if created == id)
    );
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_locator_add_view(observation));
        assert!(view.service.try_event().is_err());
        cx.notify();
    });
    visual.run_until_parked();
    let marker = bounds(&mut visual, "locator-play", id).center();
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(
            view.test_probes.last_transport_command.is_none(),
            "unapplied graph must not play"
        );
    });
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.audio_status()
                .and_then(|status| status.recording_source())
                .is_some_and(|source| source.context.editor_revision == revision)
        });
        if applied {
            break;
        }
        assert!(Instant::now() < deadline, "applied locator source");
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual, "locator-play", id).center(), marker);
    editor.update(&mut visual, |view, _| {
        assert!(view.locator_applied_source().is_some());
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity += 1;
        assert!(view.locator_applied_source().is_none(), "wrong output");
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity -= 1;
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .editor_revision += 1;
        assert!(view.locator_applied_source().is_none(), "wrong revision");
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .editor_revision -= 1;
        let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } = &mut view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .state
        else {
            panic!("resident metadata");
        };
        metadata.requires_preroll = true;
        assert!(
            view.locator_applied_source().is_some(),
            "stopped interactive start"
        );
        view.audio_status()
            .unwrap()
            .playing
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(
            view.locator_applied_source().is_none(),
            "running stateful seek"
        );
        view.audio_status()
            .unwrap()
            .playing
            .store(false, std::sync::atomic::Ordering::Release);
        let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } = &mut view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .state
        else {
            panic!("resident metadata");
        };
        metadata.requires_preroll = false;
        metadata.tuning_preview = Some(hex_session::service::TuningPreviewIdentity {
            project_generation: 0,
            key: hex_session::tuning_gesture::TuningGestureKey {
                generation: 0,
                token: 1,
            },
            sequence: 1,
            canonical_revision: revision,
        });
        assert!(view.locator_applied_source().is_none(), "preview source");
        let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } = &mut view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .state
        else {
            panic!("resident metadata");
        };
        metadata.tuning_preview = None;
        view.audio_status()
            .unwrap()
            .device_fault
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(view.locator_applied_source().is_none(), "device fault");
        view.audio_status()
            .unwrap()
            .device_fault
            .store(false, std::sync::atomic::Ordering::Release);
        view.pending_requests.push_back(Request::Undo);
        assert!(view.locator_applied_source().is_none(), "unrelated edit");
        view.pending_requests.pop_back();
        view.audio_pub
            .pending_audio_commands
            .push_back(super::super::PendingPlaybackAction {
                action: hex_engine::PlaybackAction::SetMetronome(true),
                generation: view.project_generation,
                revision: None,
                intent_token: None,
            });
        assert!(
            view.locator_applied_source().is_none(),
            "queued audio command"
        );
        view.audio_pub.pending_audio_commands.pop_back();
    });
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 24_000,
                note_chase: hex_engine::NoteChase::On
            })
        );
        assert!(view.service.try_event().is_err());
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(Acquire));
        assert_eq!(status.position.load(Acquire), 24_064);
        assert!(!view.seek_pending(), "click acknowledgment remains pending");
        assert!(
            view.locator_applied_source().is_some(),
            "source unavailable after click"
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::Seek(48_000))
        );
    });
    fixture.backend.render(&mut [[0.0; 2]; 256]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(Acquire));
        let position = status.position.load(Acquire);
        assert!(
            (48_000..49_000).contains(&position),
            "running locator seek: {position}"
        );
        assert!(view.service.try_event().is_err());
    });
    gate.release().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual, "locator-play", id).center(), marker);
    fixture.finish(cx);
}

#[gpui::test]
fn stateful_locator_actions_survive_held_checkpoint(cx: &mut TestAppContext) {
    use hex_engine::PlaybackCommand;
    use std::time::{Duration, Instant};

    cx.update(super::super::bind_editor_keys);
    let environment = super::super::test_environment::Environment::new().unwrap();
    let recovery = environment.root().join("held-stateful-locator.hex-journal");
    let reverb = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.reverb",
    ))
    .unwrap();
    let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
        cx,
        vec![Command::InsertMainDevice {
            before: None,
            state: reverb,
        }],
        false,
        true,
        Some(recovery.clone()),
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.computer_keyboard.enabled = false;
        view.place_locator_play_start(24_000);
        cx.notify();
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    let mut gate = hex_session::service::hold_next_checkpoint(recovery.clone()).unwrap();
    visual.simulate_keystrokes("i");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut entered = false;
    let mut observation = None;
    while !entered || observation.is_none() {
        if !entered {
            match gate.try_entered() {
                Ok(()) => entered = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("checkpoint gate: {error}"),
            }
        }
        if observation.is_none() {
            observation = editor.read_with(&visual, |view, _| {
                view.service.try_arrangement_edit_view().ok()
            });
        }
        assert!(Instant::now() < deadline, "held locator observation");
        std::thread::yield_now();
    }
    let observation = observation.unwrap();
    let committed = observation.state.upgrade().unwrap();
    let locator = committed.project.arrangement_locators().first().unwrap();
    let id = locator.id();
    let revision = committed.revision;
    // Hold delivery of the real completion to exercise actions on the
    // committed view before the UI installs its canonical revision.
    let terminal = editor.read_with(&visual, |view, _| next_terminal(view));
    assert!(
        matches!(terminal.created, Some(hex_project::Created::ArrangementLocator(created)) if created == id)
    );
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_locator_add_view(observation));
        assert!(view.service.try_event().is_err());
        cx.notify();
    });
    visual.run_until_parked();
    let marker = bounds(&mut visual, "locator-play", id).center();
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(
            view.test_probes.last_transport_command.is_none(),
            "unacknowledged graph must not play"
        )
    });
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            let Some(observation) = view.audio_pub.pending_publication_observation.as_ref() else {
                return false;
            };
            let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } =
                &observation.state
            else {
                return false;
            };
            metadata.requires_preroll
                && view
                    .audio_status()
                    .and_then(|status| status.recording_source())
                    .is_some_and(|source| source.context.editor_revision == revision)
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stateful locator source not applied"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 24_000,
                note_chase: hex_engine::NoteChase::On
            })
        );
        assert!(view.service.try_event().is_err());
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.read_with(&visual, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!((24_000..25_000).contains(&status.position.load(Acquire)));
        assert!(
            view.locator_applied_source().is_none(),
            "running stateful seek cannot use the direct transport path"
        );
        assert!(view.service.try_event().is_err());
    });
    // An immediate click keeps the ordinary PlayFrom law. Keyboard
    // navigation prepares a fresh seek below.
    for _ in 0..20 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        let source = view.locator_play_source().unwrap();
        assert!(view.committed_locator_seek_source_is_current(source));
        let mut wrong = source;
        wrong.output_identity += 1;
        assert!(!view.committed_locator_seek_source_is_current(wrong));
        wrong = source;
        wrong.context.editor_revision += 1;
        assert!(!view.committed_locator_seek_source_is_current(wrong));
        view.pending_requests.push_back(Request::Undo);
        assert!(!view.committed_locator_seek_context_is_current(source));
        view.pending_requests.pop_back();
        view.audio_status()
            .unwrap()
            .playing
            .store(false, std::sync::atomic::Ordering::Release);
        assert!(!view.committed_locator_seek_context_is_current(source));
        view.audio_status()
            .unwrap()
            .playing
            .store(true, std::sync::atomic::Ordering::Release);
        let previous_intent = std::mem::take(&mut view.play_intent);
        view.play_intent.priority_stop();
        assert!(!view.committed_locator_seek_context_is_current(source));
        view.play_intent = previous_intent;
        view.session_grid.quantization = hex_engine::LaunchQuantization::Immediate;
        view.test_probes.last_transport_command = None;
    });
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 24_000,
                note_chase: hex_engine::NoteChase::On
            })
        );
        assert!(view.service.try_event().is_err());
    });
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let position = editor.read_with(&visual, |view, _| {
            view.audio_status().unwrap().position.load(Acquire)
        });
        if (24_000..25_000).contains(&position) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "running locator PlayFrom not applied"
        );
        std::thread::yield_now();
    }
    assert!(editor.read_with(&visual, |view, _| view.service.try_event().is_err()));
    for _ in 0..20 {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let previous = editor.read_with(&visual, |view, _| {
        view.audio_status().unwrap().applied_revision.load(Acquire)
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-alt-left");
    editor.read_with(&visual, |view, _| {
        assert!(
            view.audio_pub.prepared_seek.is_some(),
            "running stateful locator seek must be admitted while the checkpoint is held"
        );
    });
    // Keep ordered worker completions in delivery order while polling the separate audio receipt.
    let seek_terminal = editor.read_with(&visual, |view, _| next_terminal(view));
    // Hold editor polling while the real callback installs the worker's
    // candidate. Its scalar result has not yet been consumed by the UI.
    let seek_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            status.applied_revision.load(Acquire) > previous
                && (24_000..25_000).contains(&status.position.load(Acquire))
        });
        if applied {
            break;
        }
        assert!(
            Instant::now() < seek_deadline,
            "stateful locator seek not applied"
        );
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        let pending = view.audio_pub.prepared_seek.as_ref().unwrap();
        assert!(matches!(
            pending.stage,
            super::super::arrangement_seek::SeekStage::Preparing
        ));
        assert!(
            view.pending_seek_source_is_current(pending),
            "the applied callback must not invalidate its pending worker result"
        );
        view.pending_requests.push_back(Request::Undo);
        assert!(
            !view.pending_seek_source_is_current(view.audio_pub.prepared_seek.as_ref().unwrap())
        );
        let applied = view.audio_status().unwrap().applied_revision.load(Acquire);
        view.audio_pub.prepared_seek.as_mut().unwrap().stage =
            super::super::arrangement_seek::SeekStage::Published {
                audio_revision: applied,
            };
        assert!(
            view.native_seek_applied_source(view.audio_pub.prepared_seek.as_ref().unwrap())
                .is_some(),
            "an unrelated queued edit must not hide an already applied seek acknowledgement"
        );
        assert!(view.seek_publication_is_current(
            view.audio_pub.prepared_seek.as_ref().unwrap().request_id,
            None
        ));
        view.audio_pub.prepared_seek.as_mut().unwrap().stage =
            super::super::arrangement_seek::SeekStage::Preparing;
        view.pending_requests.pop_back();
        view.tick(cx);
    });
    assert!(editor.read_with(&visual, |view, _| view.service.try_event().is_err()));
    gate.release().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    assert!(seek_terminal.error.is_none());
    editor.update(&mut visual, |view, cx| {
        view.receive(seek_terminal);
        cx.notify();
    });
    let retirement_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.audio_pub.prepared_seek.is_none()
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == revision)
        }) {
            break;
        }
        assert!(
            Instant::now() < retirement_deadline,
            "seek acknowledgement did not retire"
        );
        std::thread::yield_now();
    }
    wait_for_locator_checkpoint(&recovery, &committed.project);
    fixture.finish(cx);
}

#[gpui::test]
fn quantized_committed_locator_applies_at_captured_boundary_while_checkpoint_held(
    cx: &mut TestAppContext,
) {
    use hex_engine::LaunchQuantization;
    use std::time::{Duration, Instant};

    cx.update(super::super::bind_editor_keys);
    let environment = super::super::test_environment::Environment::new().unwrap();
    let recovery = environment
        .root()
        .join("held-quantized-locator.hex-journal");
    let reverb = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.reverb",
    ))
    .unwrap();
    let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
        cx,
        vec![Command::InsertMainDevice {
            before: None,
            state: reverb,
        }],
        false,
        true,
        Some(recovery.clone()),
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.computer_keyboard.enabled = false;
        view.place_locator_play_start(24_000);
        cx.notify();
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    let mut gate = hex_session::service::hold_next_checkpoint(recovery.clone()).unwrap();
    visual.simulate_keystrokes("i");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut entered = false;
    let mut observation = None;
    while !entered || observation.is_none() {
        if !entered {
            match gate.try_entered() {
                Ok(()) => entered = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("checkpoint gate: {error}"),
            }
        }
        if observation.is_none() {
            observation = editor.read_with(&visual, |view, _| {
                view.service.try_arrangement_edit_view().ok()
            });
        }
        assert!(Instant::now() < deadline, "held locator observation");
        std::thread::yield_now();
    }
    let observation = observation.unwrap();
    let committed = observation.state.upgrade().unwrap();
    let locator = committed.project.arrangement_locators().first().unwrap();
    let id = locator.id();
    let revision = committed.revision;
    // Hold delivery of the real completion to exercise actions on the
    // committed view before the UI installs its canonical revision.
    let terminal = editor.read_with(&visual, |view, _| next_terminal(view));
    assert!(
        matches!(terminal.created, Some(hex_project::Created::ArrangementLocator(created)) if created == id)
    );
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_locator_add_view(observation));
        cx.notify();
    });
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            let stateful = view
                .audio_pub
                .pending_publication_observation
                .as_ref()
                .is_some_and(|observation| {
                    matches!(
                        &observation.state,
                        crate::EditorPublicationState::ResidentSubmitted { metadata, .. }
                            if metadata.requires_preroll
                    )
                });
            stateful
                && view
                    .audio_status()
                    .and_then(|status| status.recording_source())
                    .is_some_and(|source| source.context.editor_revision == revision)
                && view.locator_play_source().is_some()
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "locator source not applied");
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, _| {
        let source = view.locator_play_source().unwrap();
        view.queue_applied_source_play(47_960, source, true);
    });
    fixture.backend.render(&mut [[0.0; 2]; 16]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 47_976);
        view.session_grid.quantization = LaunchQuantization::Quarter;
        cx.notify();
    });
    visual.run_until_parked();
    let marker = bounds(&mut visual, "locator-play", id).center();
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, _| {
        let seek = view
            .audio_pub
            .prepared_seek
            .as_ref()
            .expect("quantized committed locator must be admitted");
        assert_eq!(seek.destination_frame, 24_000);
        assert_eq!(seek.kind.quantization(), Some(LaunchQuantization::Quarter));
        view.session_grid.quantization = LaunchQuantization::OneBar;
    });
    // Keep ordered worker completions in delivery order while polling the separate audio receipt.
    let seek_terminal = editor.read_with(&visual, |view, _| next_terminal(view));
    let published = loop {
        let revision = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            match view.audio_pub.prepared_seek.as_ref().map(|seek| seek.stage) {
                Some(super::super::arrangement_seek::SeekStage::Published { audio_revision }) => {
                    Some(audio_revision)
                }
                _ => None,
            }
        });
        if let Some(revision) = revision {
            break revision;
        }
        assert!(Instant::now() < deadline, "quantized locator publication");
        std::thread::yield_now();
    };
    fixture.backend.render(&mut [[0.0; 2]; 7]).unwrap();
    editor.read_with(&visual, |view, _| {
        let status = view.audio_status().unwrap();
        assert_ne!(status.applied_revision.load(Acquire), published);
        assert_eq!(status.position.load(Acquire), 47_983);
    });
    fixture.backend.render(&mut [[0.0; 2]; 18]).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        let status = view.audio_status().unwrap();
        assert_eq!(status.applied_revision.load(Acquire), published);
        assert_eq!(status.position.load(Acquire), 24_001);
        assert!(view.service.try_event().is_err());
    });
    gate.release().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    assert!(seek_terminal.error.is_none());
    editor.update(&mut visual, |view, cx| {
        view.receive(seek_terminal);
        cx.notify();
    });
    wait_for_locator_checkpoint(&recovery, &committed.project);
    fixture.finish(cx);
}

#[gpui::test]
fn stopped_navigation_uses_only_complete_committed_locator_views(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::fixture;

    let (_files, window, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    for frame in [48_000, 72_000] {
        editor.update(&mut visual, |view, _| view.place_locator_play_start(frame));
        visual.simulate_keystrokes("i");
    }
    visual.run_until_parked();
    let deadline = crate::test_wait::hang_deadline();
    let mut observations = Vec::new();
    let mut terminals = Vec::new();
    while observations.len() < 2 || terminals.len() < 2 {
        editor.read_with(&visual, |view, _| {
            if observations.len() < 2
                && let Ok(observation) = view.service.try_arrangement_edit_view()
            {
                observations.push(observation);
            }
            if terminals.len() < 2
                && let Ok(event) = view.service.try_event()
            {
                assert!(event.request_complete && event.error.is_none());
                terminals.push(event);
            }
        });
        assert!(
            std::time::Instant::now() < deadline,
            "locator observations and terminals"
        );
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, _| view.place_locator_play_start(0));
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 0)
    });
    let mut observations = observations.into_iter();
    editor.update(&mut visual, |view, _| {
        assert!(view.observe_locator_add_view(observations.next().unwrap()));
    });
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 0)
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.observe_locator_add_view(observations.next().unwrap()));
        view.pending_requests.push_back(Request::Undo);
    });
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 0)
    });
    editor.update(&mut visual, |view, _| {
        assert!(matches!(
            view.pending_requests.pop_back(),
            Some(Request::Undo)
        ));
        let token = view.play_intent.reserve_token().unwrap();
        view.play_intent.admit(token, true);
    });
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 0);
        assert!(view.audio_pub.pending_seek_ticket.is_none());
    });
    editor.update(&mut visual, |view, _| view.play_intent = Default::default());
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 48_000)
    });
    visual.simulate_keystrokes("cmd-alt-right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 72_000)
    });
    visual.simulate_keystrokes("cmd-alt-left");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 48_000)
    });
    editor.update(&mut visual, |view, _| view.place_locator_play_start(90_000));
    for terminal in terminals {
        editor.update(&mut visual, |view, _| view.receive(terminal));
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 90_000)
    });
}

#[gpui::test]
fn refused_locator_rename_retains_draft_for_retry(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::fixture;
    let (_files, window, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddArrangementLocator {
            frame: 48_000
        })));
        let event = next_terminal(view);
        assert!(event.error.is_none());
        view.receive(event);
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let source = Source {
                generation: view.project_generation,
                revision: state.revision,
                id: state.project.arrangement_locators()[0].id(),
            };
            view.begin_locator_name(source, window, cx);
            let input = view.locators.name.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.set_text("chorus", cx));
            for _ in 0..super::super::UI_REQUEST_CAPACITY {
                view.pending_requests.push_back(Request::Undo);
            }
            assert!(view.finish_locator_name(true, window, cx));
            assert_eq!(
                view.locators.name.as_ref().unwrap().input.read(cx).text(),
                "chorus"
            );
            assert!(view.error.is_some());
            assert_eq!(
                view.state.as_ref().unwrap().project.arrangement_locators()[0].name(),
                None
            );
            view.pending_requests.clear();
        });
    });
    visual.simulate_keystrokes("enter");
    editor.update(&mut visual, |view, _| {
        let event = next_terminal(view);
        assert!(event.error.is_none());
        view.receive(event);
        assert_eq!(
            view.state.as_ref().unwrap().project.arrangement_locators()[0].name(),
            Some("chorus")
        );
        assert!(view.locators.name.is_none());
    });
}

#[gpui::test]
fn committed_locators_paint_and_accept_naming_before_terminals(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::fixture;
    use hex_session::service::ArrangementEditViewKind;
    let (_files, window, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    for frame in [48_000, 72_000] {
        editor.update(&mut visual, |view, _| view.place_locator_play_start(frame));
        visual.simulate_keystrokes("i");
    }
    visual.run_until_parked();
    let provisional = ["locator-play-pending-48000", "locator-play-pending-72000"]
        .map(|selector| visual.debug_bounds(selector).expect("provisional locator"));
    let deadline = crate::test_wait::hang_deadline();
    let mut views = Vec::new();
    let mut terminals = Vec::new();
    while views.len() < 2 || terminals.len() < 2 {
        editor.read_with(&visual, |view, _| {
            if views.len() < 2
                && let Ok(observation) = view.service.try_arrangement_edit_view()
            {
                views.push(observation);
            }
            if terminals.len() < 2
                && let Ok(event) = view.service.try_event()
            {
                assert!(event.request_complete && event.error.is_none());
                terminals.push(event);
            }
        });
        assert!(
            std::time::Instant::now() < deadline,
            "committed locator views and terminals"
        );
        std::thread::yield_now();
    }
    let ids: Vec<_> = views
        .iter()
        .map(|observation| match observation.kind {
            ArrangementEditViewKind::LocatorAdd(id) => id,
            _ => panic!("expected locator view"),
        })
        .collect();
    let mut observations = views.into_iter();
    let first_view = observations.next().unwrap();
    let second_view = observations.next().unwrap();
    editor.update(&mut visual, |view, cx| {
        let gap = hex_session::service::ArrangementEditViewObservation {
            project_generation: second_view.project_generation,
            first_previous_revision: second_view.first_previous_revision,
            previous_revision: second_view.previous_revision,
            kind: second_view.kind,
            state: Weak::clone(&second_view.state),
        };
        assert!(
            !view.observe_locator_add_view(gap),
            "a missing predecessor cannot install a view"
        );
        let stale = hex_session::service::ArrangementEditViewObservation {
            project_generation: first_view.project_generation.wrapping_add(1),
            first_previous_revision: first_view.first_previous_revision,
            previous_revision: first_view.previous_revision,
            kind: first_view.kind,
            state: Weak::clone(&first_view.state),
        };
        assert!(
            !view.observe_locator_add_view(stale),
            "wrong generation cannot install a view"
        );
        assert!(view.observe_locator_add_view(first_view));
        assert_eq!(view.worker_requests_pending, 2);
        cx.notify();
    });
    visual.run_until_parked();
    let first_name = bounds(&mut visual, "locator-name", ids[0]).center();
    visual.simulate_mouse_down(first_name, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(first_name, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        let selected = view.locators.selected;
        assert_eq!(selected, Some((view.project_generation, ids[0])));
        let wrong_identity = hex_session::service::ArrangementEditViewObservation {
            project_generation: second_view.project_generation,
            first_previous_revision: second_view.first_previous_revision,
            previous_revision: second_view.previous_revision,
            kind: ArrangementEditViewKind::LocatorAdd(ids[0]),
            state: Weak::clone(&second_view.state),
        };
        assert!(
            !view.observe_locator_add_view(wrong_identity),
            "an existing locator cannot impersonate the new identity"
        );
        assert!(view.observe_locator_add_view(second_view));
        assert_eq!(
            view.locators.selected, selected,
            "a later view cannot steal selection"
        );
        cx.notify();
    });
    visual.run_until_parked();
    let mut read_only = terminals.remove(0);
    read_only.plugin_parameter_observation = true;
    editor.update(&mut visual, |view, _| {
        view.locators.observe(view.project_generation, &read_only);
        assert_eq!(
            view.locators.adds.len(),
            2,
            "a read-only observation cannot complete an add receipt"
        );
    });
    read_only.plugin_parameter_observation = false;
    terminals.insert(0, read_only);
    for (index, id) in ids.iter().copied().enumerate() {
        assert_eq!(bounds(&mut visual, "locator-play", id), provisional[index]);
        assert!(
            visual
                .debug_bounds(["locator-play-pending-48000", "locator-play-pending-72000"][index])
                .is_none()
        );
    }
    let marker = bounds(&mut visual, "locator-play", ids[1]).center();
    let previous_transport =
        editor.read_with(&visual, |view, _| view.test_probes.last_transport_command);
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.test_probes.last_transport_command, previous_transport);
        assert_eq!(
            view.status,
            "locator playback is waiting for the current edit"
        );
    });
    let name = bounds(&mut visual, "locator-name", ids[1]).center();
    visual.simulate_event(MouseDownEvent {
        position: name,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_mouse_up(name, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.locators.name.is_some(),
            "name edit opens before terminal"
        )
    });
    visual.simulate_input("second locator");
    for terminal in terminals {
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.locators.name.is_some(),
                "name edit survives each earlier terminal"
            )
        });
        assert_eq!(bounds(&mut visual, "locator-play", ids[1]), provisional[1]);
    }
    visual.simulate_keystrokes("enter");
    editor.update(&mut visual, |view, cx| {
        let terminal = next_terminal(view);
        assert!(terminal.error.is_none());
        view.receive(terminal);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locator(ids[1])
                .unwrap()
                .name(),
            Some("second locator")
        );
        cx.notify();
    });
}

#[gpui::test]
fn committed_locator_view_retires_on_replacement_or_disconnect(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::fixture;
    for replace in [true, false] {
        let (_files, window, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(window, cx);
        editor.update(&mut visual, |view, _| view.place_locator_play_start(48_000));
        visual.simulate_keystrokes("i");
        let deadline = crate::test_wait::hang_deadline();
        let observation = loop {
            if let Ok(view) =
                editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
            {
                break view;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "committed locator view"
            );
            std::thread::yield_now();
        };
        let stale = hex_session::service::ArrangementEditViewObservation {
            project_generation: observation.project_generation,
            first_previous_revision: observation.first_previous_revision,
            previous_revision: observation.previous_revision,
            kind: observation.kind,
            state: Weak::clone(&observation.state),
        };
        let held = editor.read_with(&visual, |view, _| next_terminal(view));
        editor.update(&mut visual, |view, cx| {
            assert!(view.observe_locator_add_view(observation));
            if replace {
                view.service
                    .submit(Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None,
                    })
                    .unwrap();
                let replacement = next_terminal(view);
                assert!(replacement.project_replaced);
                view.receive(replacement);
                assert!(!view.observe_locator_add_view(stale));
            } else {
                view.service
                    .shutdown(std::time::Duration::from_secs(2))
                    .unwrap();
                view.tick(cx);
            }
            assert!(view.locators.committed.is_none());
            assert!(view.locators.adds.is_empty());
            cx.notify();
        });
        drop(held);
        visual.run_until_parked();
        assert!(visual.debug_bounds("locator-play-pending-48000").is_none());
    }
}

#[gpui::test]
fn locator_adds_paint_in_order_while_worker_replies_are_held(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::{fixture, pump};

    let (_files, window, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    let frames = [48_000, 72_000];
    for frame in frames {
        editor.update(&mut visual, |view, _| view.place_locator_play_start(frame));
        visual.simulate_keystrokes("i");
        visual.run_until_parked();
    }
    visual.simulate_keystrokes("i"); // A repeat at the same position.
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.worker_requests_pending, 2);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locators()
                .is_empty()
        );
    });
    let selectors = ["locator-play-pending-48000", "locator-play-pending-72000"];
    let previews = selectors.map(|selector| visual.debug_bounds(selector).unwrap());
    let guides = ["locator-line-pending-48000", "locator-line-pending-72000"];
    let guide_bounds = guides.map(|selector| visual.debug_bounds(selector).unwrap());

    for (index, frame) in frames.into_iter().enumerate() {
        let id = editor.update(&mut visual, |view, cx| {
            let event = next_terminal(view);
            let Some(hex_project::Created::ArrangementLocator(id)) = event.created else {
                panic!("expected committed locator");
            };
            assert_eq!(
                event.state.project.arrangement_locator(id).unwrap().frame(),
                frame
            );
            view.receive(event);
            cx.notify();
            id
        });
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, "locator-play", id), previews[index]);
        assert_eq!(bounds(&mut visual, "locator-line", id), guide_bounds[index]);
        assert!(visual.debug_bounds(selectors[index]).is_none());
        if let Some(next) = selectors.get(index + 1) {
            assert_eq!(visual.debug_bounds(next), Some(previews[index + 1]));
        }
    }

    for (request, expected) in [(Request::Undo, 1), (Request::Redo, 2)] {
        editor.update(&mut visual, |view, _| assert!(view.enqueue(request)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_locators()
                    .len(),
                expected
            );
        });
    }
}

#[gpui::test]
fn refused_and_replaced_locator_adds_release_their_previews(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::fixture;

    let (_files, window, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    editor.update(&mut visual, |view, _| {
        view.place_locator_play_start(hex_project::MAX_SPARSE_AUDIO_TIMELINE_FRAMES + 1);
    });
    visual.simulate_keystrokes("i");
    editor.update(&mut visual, |view, cx| {
        assert_eq!(view.locators.adds.len(), 1);
        let refused = next_terminal(view);
        assert!(
            refused
                .error
                .as_deref()
                .unwrap()
                .contains("outside the arrangement")
        );
        view.receive(refused);
        assert!(view.locators.adds.is_empty());
        assert!(view.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        }));
        cx.notify();
    });
    editor.update(&mut visual, |view, cx| {
        let replacement = next_terminal(view);
        assert!(replacement.project_replaced && replacement.error.is_none());
        // Simulate a locally queued preview left when the source changes.
        let frame = 48_000;
        view.locators.adds.push(PendingAdd {
            receipt: Receipt::new(view.project_generation),
            frame,
        });
        view.pending_requests
            .push_back(Request::Edit(Command::AddArrangementLocator { frame }));
        view.locators.review_frames.insert(frame);
        view.receive(replacement);
        assert!(view.locators.review_frames.is_empty());
        assert!(view.locators.adds.is_empty());
        assert!(view.pending_requests.is_empty());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locators()
                .is_empty()
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-locator-strip").is_none());
}

#[gpui::test]
fn locator_click_uses_the_captured_grid_only_while_playing(cx: &mut TestAppContext) {
    use crate::editor::arrangement_seek::{SeekKind, SeekStage};
    use hex_engine::{LaunchQuantization, PlaybackCommand};

    let region = hex_project::LoopRegion::new(0, 96_000, true).unwrap();
    let mut fixture = super::super::worker_ui_fixture::open_with_section_support(
        cx,
        vec![
            Command::SetLoopRegion {
                start: region.start(),
                end: region.end(),
                enabled: true,
            },
            Command::AddArrangementLocator { frame: 23_960 },
            Command::AddArrangementLocator { frame: 48_000 },
        ],
        false,
        true,
    );
    fixture.editor.update(cx, |view, cx| {
        hex_preferences::save_playback_preferences(
            view.settings.playback.path.as_ref().unwrap(),
            hex_preferences::PlaybackPreferences {
                note_chase: false,
                ..Default::default()
            },
        )
        .unwrap();
        view.settings.playback.loading = true;
        view.start_playback_preferences_load(cx);
    });
    let deadline = crate::test_wait::hang_deadline();
    while fixture
        .editor
        .read_with(cx, |view, _| view.settings.playback.loading)
    {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "saved playback settings load"
        );
    }
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    let [first, second] = fixture.editor.update(&mut visual, |view, cx| {
        view.session_grid.quantization = LaunchQuantization::Quarter;
        let locators = view.state.as_ref().unwrap().project.arrangement_locators();
        cx.notify();
        [locators[0].id(), locators[1].id()]
    });
    visual.run_until_parked();
    let click_locator = |visual: &mut VisualTestContext, id| {
        let marker = bounds(visual, "locator-play", id).center();
        visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    };

    click_locator(&mut visual, first);
    fixture.backend.render(&mut [[0.0; 2]; 16]).unwrap();
    fixture.editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 23_960,
                note_chase: hex_engine::NoteChase::Off
            })
        );
        assert!(view.audio_pub.prepared_seek.is_none());
        assert!(view.audio_status().unwrap().playing.load(Acquire));
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 23_976);
    });

    click_locator(&mut visual, second);
    fixture.editor.update(&mut visual, |view, _| {
        let seek = view
            .audio_pub
            .prepared_seek
            .as_ref()
            .expect("queued locator seek");
        assert_eq!(seek.note_chase, hex_engine::NoteChase::Off);
        assert!(matches!(
            seek.kind,
            SeekKind::Locator(LaunchQuantization::Quarter)
        ));
        assert_eq!(seek.destination_frame, 48_000);
        view.session_grid.quantization = LaunchQuantization::Immediate;
    });
    let deadline = crate::test_wait::hang_deadline();
    let published = loop {
        let revision = fixture.editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.is_none(), "{:?}", view.error);
            match view.audio_pub.prepared_seek.as_ref().map(|seek| seek.stage) {
                Some(SeekStage::Published { audio_revision }) => Some(audio_revision),
                _ => None,
            }
        });
        if let Some(revision) = revision {
            break revision;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "locator graph publication"
        );
        std::thread::yield_now();
    };
    fixture.backend.render(&mut [[0.0; 2]; 7]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        let status = view.audio_status().unwrap();
        assert_ne!(status.applied_revision.load(Acquire), published);
        assert_eq!(status.position.load(Acquire), 23_983);
    });
    fixture.backend.render(&mut [[0.0; 2]; 18]).unwrap();
    fixture.editor.update(&mut visual, |view, cx| {
        view.tick(cx);
        let status = view.audio_status().unwrap();
        assert_eq!(status.applied_revision.load(Acquire), published);
        assert_eq!(status.position.load(Acquire), 48_001);
        assert_eq!(
            view.state.as_ref().unwrap().project.loop_region(),
            Some(region)
        );
        assert!(view.audio_pub.prepared_seek.is_none());
    });

    click_locator(&mut visual, first);
    fixture.editor.read_with(&visual, |view, _| {
        assert!(
            view.audio_pub.prepared_seek.is_none(),
            "none uses ordinary playback"
        );
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(PlaybackCommand::PlayFromWithNoteChase {
                frame: 23_960,
                note_chase: hex_engine::NoteChase::Off
            })
        );
    });
    // A running PlayFrom continues the old clock for the 128-frame
    // transport fade at 48 kHz, then advances from the clicked marker.
    fixture.backend.render(&mut [[0.0; 2]; 16]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 48_017);
    });
    fixture.backend.render(&mut [[0.0; 2]; 128]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(view.audio_status().unwrap().position.load(Acquire), 23_976);
    });
    drop(visual);
    fixture.finish(cx);
}

#[gpui::test]
fn locator_shortcuts_navigate_stopped_and_running_without_stealing_text_focus(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering::Acquire;

    cx.update(super::super::bind_editor_keys);
    let mut fixture = super::super::worker_ui_fixture::open(
        cx,
        [1_000, 2_000, 3_000]
            .into_iter()
            .map(|frame| Command::AddArrangementLocator { frame })
            .collect(),
    );
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.update(|window, cx| {
        fixture
            .editor
            .update(cx, |view, _| window.focus(&view.focus));
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-alt-right");
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 1_000);
        assert!(!view.audio_status().unwrap().playing.load(Acquire));
    });
    visual.simulate_keystrokes("cmd-alt-right");
    visual.simulate_keystrokes("cmd-alt-left");
    visual.simulate_keystrokes("cmd-alt-left");
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.playback_start_frame(),
            1_000,
            "no wrap before first locator"
        );
    });
    fixture
        .editor
        .update(&mut visual, |view, _| view.computer_keyboard.enabled = true);
    visual.simulate_keystrokes("cmd-alt-right");
    visual.simulate_keystrokes("cmd-alt-right");
    visual.simulate_keystrokes("cmd-alt-right");
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.playback_start_frame(),
            3_000,
            "no wrap after last locator"
        );
        assert!(view.computer_keyboard.enabled);
    });
    fixture.editor.update(&mut visual, |view, _| {
        view.computer_keyboard.enabled = false
    });
    visual.update(|window, cx| {
        fixture
            .editor
            .update(cx, |view, _| window.focus(&view.locators.focus));
    });
    visual.simulate_keystrokes("cmd-alt-left");
    fixture.editor.read_with(&visual, |view, _| {
        assert_eq!(view.playback_start_frame(), 2_000)
    });

    fixture.editor.update(&mut visual, |view, _| {
        view.queue_audio_command(hex_engine::PlaybackCommand::PlayFrom(1_500));
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    fixture.editor.update(&mut visual, |view, cx| view.tick(cx));
    fixture.editor.read_with(&visual, |view, _| {
        assert!(!view.seek_pending(), "prior play seek still pending");
        assert!(
            view.locator_edit_available(),
            "locator navigation unavailable"
        );
    });
    let old_ticket = fixture
        .editor
        .read_with(&visual, |view, _| view.audio_pub.pending_seek_ticket);
    visual.simulate_keystrokes("cmd-alt-right");
    let ticket = fixture.editor.read_with(&visual, |view, _| {
        let ticket = view
            .audio_pub
            .pending_seek_ticket
            .expect("running navigation admitted a seek");
        assert_ne!(
            Some(ticket),
            old_ticket,
            "navigation must submit a fresh seek"
        );
        ticket
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let consumed = fixture.editor.read_with(&visual, |view, _| {
            view.audio_status().unwrap().consumed_command.load(Acquire) >= ticket
        });
        if consumed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "seek was not consumed"
        );
        std::thread::yield_now();
    }
    // A running seek is admitted before its 128-frame declick completes.
    fixture.backend.render(&mut [[0.0; 2]; 128]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(Acquire));
        let position = status.position.load(Acquire);
        assert!(status.consumed_command.load(Acquire) >= ticket);
        assert!((2_000..=2_192).contains(&position), "position {position}");
    });

    fixture.editor.update(&mut visual, |view, cx| view.tick(cx));
    visual.simulate_keystrokes("cmd-alt-left");
    let previous_ticket = fixture.editor.read_with(&visual, |view, _| {
        let previous = view
            .audio_pub
            .pending_seek_ticket
            .expect("previous locator admitted");
        assert_ne!(previous, ticket);
        previous
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if fixture.editor.read_with(&visual, |view, _| {
            view.audio_status().unwrap().consumed_command.load(Acquire) >= previous_ticket
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "previous seek was not consumed"
        );
        std::thread::yield_now();
    }
    fixture.backend.render(&mut [[0.0; 2]; 128]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        let status = view.audio_status().unwrap();
        assert!(status.playing.load(Acquire));
        assert!((2_000..=2_192).contains(&status.position.load(Acquire)));
    });

    let id = fixture.editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.arrangement_locators()[0].id()
    });
    visual.update(|window, cx| {
        fixture.editor.update(cx, |view, cx| {
            let source = Source {
                generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                id,
            };
            view.begin_locator_name(source, window, cx);
        });
    });
    visual.run_until_parked();
    fixture
        .editor
        .read_with(&visual, |view, _| assert!(view.locators.name.is_some()));
    let before = fixture.editor.read_with(&visual, |view, _| {
        view.audio_status().unwrap().position.load(Acquire)
    });
    visual.simulate_keystrokes("cmd-alt-right");
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    fixture.editor.read_with(&visual, |view, _| {
        assert!(view.locators.name.is_some());
        assert_eq!(
            view.audio_status().unwrap().position.load(Acquire),
            before + 64
        );
    });
    drop(visual);
    fixture.finish(cx);
}

#[gpui::test]
fn rendered_locator_add_play_move_rename_delete_and_cull(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let files = BatchFiles::new();
    let path = files.wav("locator.wav", 48_000, 24_000);
    let (window, editor) = editor_window(cx, Project::new(48_000).unwrap(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    pump_move(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Import(path)))
    });
    pump_move(&editor, &mut visual);
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_selector).expect("track lane");
    let cursor = point(lane.origin.x + lane.size.width * 0.25, lane.center().y);
    visual.simulate_mouse_down(cursor, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(cursor, MouseButton::Left, Modifiers::default());
    let cursor_frame = editor.read_with(&visual, |view, _| {
        view.interaction.edit_cursor.expect("placed cursor").frame
    });
    visual.simulate_keystrokes("i");
    pump_move(&editor, &mut visual);
    let id = editor.read_with(&visual, |view, _| {
        let locator = &view.state.as_ref().unwrap().project.arrangement_locators()[0];
        assert_eq!(locator.frame(), cursor_frame);
        locator.id()
    });
    visual.simulate_keystrokes("i");
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locators()
                .len(),
            1
        )
    });

    let marker = bounds(&mut visual, "locator-play", id).center();
    assert!(bounds(&mut visual, "locator-line", id).size.height > px(18.0));
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(marker, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, _| {
        assert!(view.test_probes.last_transport_command.is_none());
        assert_eq!(
            view.error.as_deref(),
            Some("playback source is unavailable · retry playback when audio is ready")
        );
        view.error = None;
        view.snap_grid = SnapGrid::Quarter;
    });

    let strip = visual.debug_bounds("arrangement-locator-strip").unwrap();
    let delta = strip.size.width * 0.18;
    let release = point(marker.x + delta, marker.y);
    let expected = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        let viewport = view.visible_viewport(state.timeline_frames);
        let width = f32::from(strip.size.width);
        let raw = (cursor_frame as f64
            + f64::from(f32::from(delta) / width) * viewport.length as f64)
            .round() as u64;
        let frame = view
            .snap_grid
            .snap_frame(&state.project, raw, viewport, width)
            .unwrap();
        assert_ne!(frame, raw);
        frame
    });
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(release, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = bounds(&mut visual, "locator-play", id);
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(&mut visual, |view, cx| {
        let pending = view.locators.pending.as_ref().expect("released preview");
        assert_eq!(pending.frame, expected);
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = view.service.try_event() {
                if event.request_complete {
                    assert!(event.error.is_none());
                    cx.notify();
                    return event;
                }
                view.receive(event);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "locator move terminal"
            );
            std::thread::yield_now();
        }
    });
    for _ in 0..2 {
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, "locator-play", id), preview);
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert!(view.locators.pending.is_none());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locator(id)
                .unwrap()
                .frame(),
            expected
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual, "locator-play", id), preview);

    let marker = preview.center();
    let release = point(marker.x - px(19.0), marker.y);
    let inverted_frame = editor.read_with(&visual, |view, _| {
        let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
        let delta = f64::from(f32::from(release.x - marker.x) / f32::from(strip.size.width));
        (expected as f64 + delta * viewport.length as f64).round() as u64
    });
    visual.simulate_mouse_down(marker, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(release, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        release,
        MouseButton::Left,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locator(id)
                .unwrap()
                .frame(),
            inverted_frame
        );
    });

    let name = bounds(&mut visual, "locator-name", id).center();
    visual.simulate_event(MouseDownEvent {
        position: name,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_mouse_up(name, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_input("Verse Hit");
    visual.simulate_keystrokes("enter");
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locator(id)
                .unwrap()
                .name(),
            Some("Verse Hit")
        )
    });

    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddArrangementLocator {
            frame: 1_000_000
        })))
    });
    pump_move(&editor, &mut visual);
    let hidden = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.arrangement_locators()[1].id()
    });
    for prefix in ["locator-play", "locator-line"] {
        let selector = Box::leak(format!("{prefix}-{}", hidden.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_none(),
            "offscreen locator was painted"
        );
    }
    visual.simulate_keystrokes("backspace");
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_locator(id)
                .is_none()
        )
    });
}

#[gpui::test]
fn review_markers_follow_the_audible_playhead_and_either_handle_removes_only_its_mark(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering::Relaxed;

    cx.update(super::super::bind_editor_keys);
    let fixture = super::super::worker_ui_fixture::open(
        cx,
        vec![
            Command::AddTrack {
                name: "audio".into(),
            },
            Command::AddArrangementLocator { frame: 24_000 },
        ],
    );
    let editor = &fixture.editor;
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            window.focus(&view.focus);
            view.viewport = Some(Viewport {
                start: 0,
                length: 96_000,
            });
            view.place_locator_play_start(4_000);
            let status = view.audio_status().unwrap();
            status.playing.store(true, Relaxed);
            status.position.store(24_512, Relaxed);
            status.audible_position.store(24_000, Relaxed);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let original = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    visual.simulate_keystrokes("cmd-i cmd-i");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.locators
                .review_frames
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [24_000]
        );
        view.audio_status()
            .unwrap()
            .audible_position
            .store(48_000, Relaxed);
    });
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("cmd-i").unwrap(),
        is_held: true,
    });
    assert_eq!(
        editor.read_with(&visual, |view, _| view.locators.review_frames.len()),
        1
    );
    visual.simulate_keystrokes("cmd-i");
    visual.run_until_parked();
    let top = visual.debug_bounds("review-marker-top-24000").unwrap();
    let bottom = visual.debug_bounds("review-marker-bottom-24000").unwrap();
    let permanent = original.project.arrangement_locators()[0].id();
    let line = bounds(&mut visual, "locator-line", permanent);
    assert!((top.center().x - line.center().x).abs() <= px(1.0));
    assert_eq!(top.center().x, bottom.center().x);
    assert!(top.bottom() < bottom.top());
    visual.simulate_click(top.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("review-marker-bottom-24000").is_none());
    let second = visual
        .debug_bounds("review-marker-bottom-48000")
        .unwrap()
        .center();
    visual.simulate_click(second, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.locators.review_frames.is_empty());
        assert_eq!(view.state.as_ref().unwrap().revision, original.revision);
        assert_eq!(view.state.as_ref().unwrap().project, original.project);
        assert!(view.pending_requests.is_empty());
        assert!(view.audio_status().unwrap().playing.load(Relaxed));
        assert_eq!(view.playback_start_frame(), 4_000);
    });
    drop(visual);
    fixture.finish(cx);
}
