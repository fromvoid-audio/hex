//! Real worker/UI regressions for locally coalesced tuning and lifecycle barriers.
use super::*;
use crate::editor::UI_REQUEST_CAPACITY;
use gpui::{AnyWindowHandle, AppContext, Entity, TestAppContext};
use hex_project::{BeatTime, Command, Created};
use hex_session::service::Event;
use hex_tuning::ProjectTuning;
use std::sync::{Arc, atomic::Ordering::Relaxed};
use std::{path::PathBuf, time::Duration};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scratch() -> Scratch {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-tuning-lifecycle-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    Scratch(path)
}
fn next(editor: &Entity<EditorView>, cx: &TestAppContext) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match editor.read_with(cx, |view, _| view.service.try_event()) {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(error) => panic!("worker did not reply: {error}"),
        }
    }
}
fn window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<EditorView>) {
    let result = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let initial = next(&result.1, cx);
    result.1.update(cx, |view, _| {
        view.receive(initial);
        view.tuning.open = true;
    });
    result
}
fn tuning(hz: f64) -> ProjectTuning {
    let original = ProjectTuning::default();
    let mut scale = original.scale();
    scale.set_reference(60, hz, 0).unwrap();
    ProjectTuning::new(true, scale, original.audition()).unwrap()
}
fn begin(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
    editor.update(cx, |view, _| {
        view.tuning
            .client
            .begin(
                view.project_generation,
                view.state.as_ref().unwrap().project.tuning(),
            )
            .unwrap();
        assert!(view.pump_tuning_gesture());
    });
    let event = next(editor, cx);
    assert!(event.tuning_gesture_ack.as_ref().unwrap().accepted);
    editor.update(cx, |view, _| {
        view.receive(event);
    });
}
fn preview(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
    begin(editor, cx);
    editor.update(cx, |view, _| {
        view.tuning.client.change(tuning(333.0)).unwrap();
        assert!(view.pump_tuning_gesture());
    });
    let event = next(editor, cx);
    assert!(event.tuning_preview.is_some());
    editor.update(cx, |view, _| {
        view.receive(event);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(!view.state.as_ref().unwrap().unsaved);
    });
}
fn settle(editor: &Entity<EditorView>, cx: &mut TestAppContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.tuning.client.snapshot().is_none()
                && view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture.snapshot().is_none()
                && view.device_gesture_barriers.is_empty()
                && view.tuning.deferred_library.is_none()
                && view.tuning.library_pending.is_none()
        });
        if done {
            return;
        }
        assert!(Instant::now() < deadline, "tuning requests did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[gpui::test]
fn review_lifecycle_save_waits_for_latest_local_tuning_terminal(cx: &mut TestAppContext) {
    for reserve_terminal in [false, true] {
        let directory = scratch();
        let path = directory.0.join("project.sqlite");
        let (_, editor) = window(cx);
        preview(&editor, cx);
        editor.update(cx, |view, _| {
            view.pending_requests
                .resize_with(UI_REQUEST_CAPACITY, || Request::Undo);
            assert!(!view.enqueue(Request::SaveNew(path.clone())));
            assert!(
                !view.tuning.client.has_pending_terminal(),
                "overflow must not finalize a draft"
            );
            view.pending_requests.clear();
            view.tuning.client.change(tuning(444.0)).unwrap();
            if reserve_terminal {
                view.tuning.client.finish(tuning(444.0)).unwrap();
            }
            view.saving_project = true;
            assert!(view.enqueue(Request::SaveNew(path.clone())));
        });
        settle(&editor, cx);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.tuning(), tuning(444.0));
            assert!(!view.state.as_ref().unwrap().unsaved);
        });
        assert!(path.exists());
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Open(path))));
        let reopened = next(&editor, cx);
        assert!(reopened.error.is_none(), "{:?}", reopened.error);
        assert_eq!(reopened.state.project.tuning(), tuning(444.0));
    }
}
#[gpui::test]
fn review_lifecycle_close_and_quit_confirm_acknowledged_uncommitted_tuning(
    cx: &mut TestAppContext,
) {
    for quit in [false, true] {
        let (handle, editor) = window(cx);
        preview(&editor, cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                assert!(view.pending_requests.is_empty());
                if quit {
                    view.request_quit(window, cx);
                } else {
                    assert!(!view.request_window_close(window, cx));
                }
                assert!(view.automation_close_preflight.frozen());
                assert!(view.tuning.client.has_pending_terminal());
            });
        })
        .unwrap();
        super::super::close::wait_for_close_prompt(&editor, cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert!(state.unsaved);
            assert_eq!(state.project.tuning(), tuning(333.0));
            assert!(!view.close_prompt_open);
        });
    }
}
#[gpui::test]
fn review_lifecycle_stop_cancels_late_audition_preparation_and_publication(
    cx: &mut TestAppContext,
) {
    let (_, editor) = window(cx);
    let input = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    input.set_key(0, Some((60, 100, 1))).unwrap();
    editor.update(cx, |view, _| {
        view.pending_playback = None;
        view.tuning.audition_input = Some(Arc::clone(&input));
        view.service
            .submit(Request::PrepareTuningAudition {
                revision: view.state.as_ref().unwrap().revision,
                input: Arc::clone(&input),
            })
            .unwrap();
        view.stop();
        assert!(
            input.is_cancelled(),
            "Stop must invalidate queued as well as installed input"
        );
    });
    let mut late = next(&editor, cx);
    if let Some(playback) = late.playback.as_mut() {
        let mut frames = [[0.0; 2]; 128];
        playback.render(&mut frames).unwrap();
        assert!(frames.iter().all(|frame| *frame == [0.0; 2]));
    }
    editor.update(cx, |view, _| {
        view.receive(late);
        assert!(view.tuning.audition_input.is_none());
        assert!(view.pending_playback.is_none());
    });
}
#[gpui::test]
fn review_lifecycle_failed_tuning_terminal_refuses_save_as_and_deferred_export(
    cx: &mut TestAppContext,
) {
    let directory = scratch();
    let project_path = directory.0.join("must-not-save.sqlite");
    let export_path = directory.0.join("must-not-export.freqs");
    let (_, editor) = window(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertTrackWithDevice {
            name: "notes".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })))
    });
    let added = next(&editor, cx);
    let track = added.state.project.tracks()[0].id();
    editor.update(cx, |view, _| {
        view.receive(added);
        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE
        })));
    });
    let placed = next(&editor, cx);
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip expected")
    };
    editor.update(cx, |view, _| {
        view.receive(placed);
        assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100
        })));
    });
    let authored = next(&editor, cx);
    editor.update(cx, |view, _| {
        view.receive(authored);
    });
    begin(&editor, cx);
    let mut scale = ProjectTuning::default().scale();
    scale.set_reference(0, 20_000.0, 0).unwrap();
    let invalid = ProjectTuning::new(true, scale, hex_tuning::Audition::default()).unwrap();
    editor.update(cx, |view, _| {
        view.tuning.client.finish(invalid).unwrap();
        view.tuning.deferred_library = Some((
            hex_session::tuning_gesture::TuningLibraryOperation::Export {
                path: export_path.clone(),
                name: "invalid draft".into(),
            },
            None,
        ));
        view.saving_project = true;
        assert!(view.enqueue(Request::SaveNew(project_path.clone())));
    });
    settle(&editor, cx);
    editor.read_with(cx, |view, _| {
        assert!(view.tuning.error.is_some());
        assert!(!view.saving_project);
        assert!(
            view.tuning.deferred_library.is_none(),
            "failed commit must not leave a future stale write"
        );
    });
    assert!(
        !project_path.exists(),
        "failed tuning must not save older canonical data instead"
    );
    assert!(!export_path.exists());
}

#[gpui::test]
fn review_final_empty_project_replacement_confirms_active_tuning_draft(cx: &mut TestAppContext) {
    for new_project in [false, true] {
        let scratch = scratch();
        let (handle, editor) = window(cx);
        preview(&editor, cx);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                let mode = hex_session::DocumentAudioMode::Resident;
                if new_project {
                    view.new_project(mode, window, cx);
                    assert!(view.project_open.busy());
                } else {
                    let path = scratch.0.join("other.sqlite");
                    view.choose_project(path.clone(), mode);
                    assert_eq!(view.pending_open_path.as_ref(), Some(&path));
                }
                assert!(
                    !view.opening_project,
                    "first request must obtain replacement confirmation"
                );
                assert!(!view.tuning.client.has_pending_terminal());
                assert_eq!(view.tuning.client.snapshot().unwrap().tuning, tuning(333.0));
            })
        })
        .unwrap();
    }
}
#[gpui::test]
fn review_final_period_refusal_preserves_the_draft_and_reports_recovery(cx: &mut TestAppContext) {
    let (_, editor) = window(cx);
    editor.update(cx, |view, cx| {
        let original = view.tuning.value;
        let mut scale = original.scale();
        assert!(scale.move_degree(1, 1.0));
        let current = ProjectTuning::new(false, scale, original.audition()).unwrap();
        view.tuning.value = current;
        view.tuning_numeric(
            Field::Period,
            NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value: 600.0,
                gesture: None,
            },
            cx,
        );
        assert!(
            view.tuning.error.is_some(),
            "structurally rejected compression needs an explanation"
        );
        assert_eq!(view.tuning.value, current);
        assert!(view.tuning.client.snapshot().is_none());
        view.tuning_numeric(
            Field::Period,
            NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value: 1800.0,
                gesture: None,
            },
            cx,
        );
        assert!(view.tuning.error.is_none());
        assert_eq!(view.tuning.value.scale().period_cents(), 1800.0);
    });
}

#[gpui::test]
fn review_final_tuning_library_waits_for_latest_device_terminal(cx: &mut TestAppContext) {
    use hex_session::device_gesture::{GestureTarget, ParameterValue};
    for action in ["list", "current export", "old dialog export"] {
        let directory = scratch();
        let path = directory.0.join("scale.freqs");
        let (_, editor) = window(cx);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
                name: "send".into()
            })))
        });
        let added = next(&editor, cx);
        let track = added.state.project.tracks()[0].id();
        editor.update(cx, |view, _| {
            view.receive(added);
            assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
                name: "return".into()
            })));
        });
        let added = next(&editor, cx);
        let return_track = added.state.project.return_tracks()[0].id();
        editor.update(cx, |view, _| {
            view.receive(added);
            view.begin_device_gesture(
                GestureTarget::ReturnSend {
                    track,
                    return_track,
                },
                ParameterValue::Float(0.0),
            )
            .unwrap();
            assert!(view.pump_device_gesture());
        });
        let begun = next(&editor, cx);
        editor.update(cx, |view, _| {
            view.receive(begun);
            view.change_device_gesture(ParameterValue::Float(0.2))
                .unwrap();
            assert!(view.pump_device_gesture());
        });
        let changed = next(&editor, cx);
        editor.update(cx, |view, _| {
            view.receive(changed);
            view.change_device_gesture(ParameterValue::Float(0.3))
                .unwrap();
            if action == "list" {
                view.request_tuning_library_list();
            } else {
                let owner = (action == "old dialog export").then(|| {
                    (
                        view.project_generation,
                        view.state.as_ref().unwrap().revision,
                    )
                });
                view.request_tuning_library(
                    hex_session::tuning_gesture::TuningLibraryOperation::Export {
                        path: path.clone(),
                        name: "device terminal".into(),
                    },
                    owner,
                );
            }
            assert!(
                view.device_gesture.has_pending_terminal(),
                "library barrier must reserve the latest local device terminal"
            );
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "after library".into()
            })));
        });
        settle(&editor, cx);
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .sends()[0]
                    .gain(),
                0.3
            );
            assert!(view.device_gesture.snapshot().is_none());
            assert!(view.tuning.library_pending.is_none());
            if action == "old dialog export" {
                assert!(
                    view.tuning
                        .error
                        .as_deref()
                        .is_some_and(|message| message.contains("changed project revision"))
                );
                assert!(!path.exists());
            } else {
                assert!(
                    view.tuning.error.is_none(),
                    "{action}: {:?}",
                    view.tuning.error
                );
                if action == "current export" {
                    assert!(std::fs::metadata(&path).unwrap().len() > 0);
                }
            }
        });
    }
}

#[gpui::test]
fn review_audition_worker_failure_survives_the_normal_tick(cx: &mut TestAppContext) {
    let (handle, editor) = window(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
            reservation: Some(hex_session::SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 1,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 1024 * 1024,
            }),
        }));
    });
    let created = next(&editor, cx);
    assert!(created.project_replaced);
    assert!(created.error.is_none());
    editor.update(cx, |view, cx| {
        view.receive(created);
        view.sync_tuning(cx);
    });
    let input = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.tuning.open = true;
                view.tuning.audition_keyboard.enabled = true;
                view.tuning.focus.focus(window);
                assert!(view.tuning_audition_down(
                    &gpui::KeyDownEvent {
                        keystroke: gpui::Keystroke::parse("a").unwrap(),
                        is_held: false,
                    },
                    window,
                    cx,
                ));
                Arc::clone(
                    view.tuning
                        .audition_input
                        .as_ref()
                        .expect("owned preparation"),
                )
            })
        })
        .unwrap();
    let deadline = crate::test_wait::hang_deadline();
    while !input.is_cancelled() {
        assert!(Instant::now() < deadline, "worker did not refuse audition");
        std::thread::sleep(Duration::from_millis(2));
    }
    // Do not take the completion out of the queue or call receive directly:
    // exercise the helper/event ordering in the actual UI tick.
    loop {
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.worker_requests_pending == 0
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "completion did not drain");
        std::thread::sleep(Duration::from_millis(2));
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.tuning.error.as_deref(),
            Some("tuning audition preparation is stale or unavailable"),
        );
        assert!(view.tuning.audition_input.is_none());
        assert!(
            view.tuning
                .audition_keyboard
                .desired_notes()
                .iter()
                .all(Option::is_none)
        );
    });
}
