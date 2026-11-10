//! Rendered whole-clip shortcuts and ordered real-worker terminal regressions.
use super::*;
use crate::editor::drag::group_drag_ui_test::{click, fixture, pump};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
use std::time::{Duration, Instant};

fn press(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("d");
}

fn all_ids(view: &EditorView) -> Vec<ClipId> {
    view.state
        .as_ref()
        .unwrap()
        .project
        .tracks()
        .iter()
        .flat_map(|t| {
            t.clips()
                .iter()
                .map(|c| c.id())
                .chain(t.midi_clips().iter().map(|c| c.id()))
        })
        .collect()
}

#[gpui::test]
fn shift_selected_audio_group_d_copies_all_members_and_repeats_after_ack(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    let selector = Box::leak(format!("clip-name-{}", originals[2].value()).into_boxed_str());
    let target = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(
        target,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip_ids(), originals);
        v.state.as_ref().unwrap().project.snapshot()
    });
    press(&editor, &mut visual);
    press(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.duplicate_intent.is_pending());
        assert_eq!(v.duplicate_intent.queued.len(), 1);
        assert_eq!(
            v.selected_clip_ids(),
            originals,
            "held ACK keeps source selection"
        );
    });
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(!v.duplicate_intent.is_pending());
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.tracks()[0].clips().len(), 9);
        let selected = v.selected_clip_ids();
        assert_eq!(selected.len(), 3);
        assert!(selected.iter().all(|id| !originals.contains(id)));
        for (index, id) in selected.iter().enumerate() {
            let copied = p.clip(*id).unwrap();
            assert_eq!(copied.start(), 288_000 + index as u64 * 48_000);
            assert_eq!(copied.source_offset(), index as u64 * 24_000);
        }
        p.snapshot()
    });
    for _ in 0..2 {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |v, _| {
        let restored = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = before;
        expected.next_id = restored.next_id;
        assert_eq!(restored, expected);
    });
    for _ in 0..2 {
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn mixed_and_midi_only_clip_selection_d_selects_actual_committed_births(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiTrack {
            name: "notes".into()
        })));
    });
    pump(&editor, &mut visual);
    let midi_track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[1].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::from_raw(2 << 32),
            length: BeatTime::ONE,
        })));
    });
    pump(&editor, &mut visual);
    let midi = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(midi_track)
            .unwrap()
            .midi_clips()[0]
            .id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1 << 31),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    click(&mut visual, originals[0], false);
    click(&mut visual, midi, true);
    let before = editor.read_with(&visual, |v, _| all_ids(v));
    press(&editor, &mut visual);
    pump(&editor, &mut visual);
    let copied_midi = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(all_ids(v).len(), before.len() + 2);
        let selected = v.selected_clip_ids();
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|id| !before.contains(id)));
        let p = &v.state.as_ref().unwrap().project;
        let copied = p
            .track(midi_track)
            .unwrap()
            .midi_clips()
            .iter()
            .find(|c| selected.contains(&c.id()))
            .unwrap();
        assert_eq!(p.midi_source(copied.id()).unwrap().notes().len(), 1);
        assert_ne!(
            p.midi_source(copied.id()).unwrap().notes()[0].id(),
            p.midi_source(midi).unwrap().notes()[0].id()
        );
        copied.id()
    });
    click(&mut visual, copied_midi, false);
    let before = editor.read_with(&visual, |v, _| all_ids(v));
    press(&editor, &mut visual);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(all_ids(v).len(), before.len() + 1);
        let selected = v.selected_clip_ids();
        assert_eq!(selected.len(), 1);
        assert!(!before.contains(&selected[0]));
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(selected[0])
                .unwrap()
                .notes()
                .len(),
            1
        );
    });
}

#[gpui::test]
fn escape_and_selection_aba_cancel_repeats_but_retain_admitted_duplicate(cx: &mut TestAppContext) {
    for escape in [true, false] {
        let (_files, handle, editor, originals) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        click(&mut visual, originals[0], false);
        press(&editor, &mut visual);
        press(&editor, &mut visual);
        if escape {
            visual.simulate_keystrokes("escape");
        } else {
            click(&mut visual, originals[1], false);
            click(&mut visual, originals[0], false);
        }
        editor.read_with(&visual, |v, _| {
            assert!(
                v.duplicate_intent.is_pending(),
                "input cancellation cannot abandon admitted custody"
            );
            assert_eq!(v.duplicate_intent.queued.len(), 0);
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(!v.duplicate_intent.is_pending());
            assert_eq!(all_ids(v).len(), originals.len() + 1);
            assert_eq!(
                v.selected_clip_ids(),
                vec![originals[0]],
                "late ACK must not steal selection, even after ABA"
            );
        });
        press(&editor, &mut visual);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(!v.duplicate_intent.is_pending());
            assert_ne!(
                v.selected_clip,
                Some(originals[0]),
                "next deliberate duplication works"
            );
        });
    }
}

#[gpui::test]
fn earlier_terminal_and_refusal_without_legacy_duplicate_flag_retire_only_owned_request(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    // An earlier worker edit makes the captured duplicate stale. Exercise the
    // receipt's FIFO boundary directly; ordinary input prevents this overlap.
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "earlier edit".into()
        })));
        v.start_clip_duplicate(v.selected_clip_ids(), 1, false);
        assert_eq!(v.worker_requests_pending, 2);
    });
    let receive = |editor: &Entity<EditorView>, visual: &mut VisualTestContext| {
        let until = crate::test_wait::hang_deadline();
        loop {
            let event = editor.read_with(&*visual, |v, _| v.service.try_event().ok());
            if let Some(event) = event {
                return event;
            }
            assert!(
                Instant::now() < until,
                "real worker terminal did not arrive"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let earlier = receive(&editor, &mut visual);
    assert!(earlier.request_complete && earlier.error.is_none());
    editor.update(&mut visual, |v, _| {
        v.receive(earlier);
        assert!(v.duplicate_intent.is_pending());
    });
    let refusal = receive(&editor, &mut visual);
    assert!(refusal.request_complete);
    assert!(
        !refusal.duplicate_complete,
        "refusal precedes legacy edit completion"
    );
    assert!(refusal.error.is_some());
    assert!(refusal.created.is_none());
    let unchanged = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    editor.update(&mut visual, |v, cx| {
        v.receive(refusal);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert_eq!(v.duplicate_intent.queued.len(), 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), unchanged);
        assert_eq!(all_ids(v), originals);
    });
    click(&mut visual, originals[0], false);
    press(&editor, &mut visual);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(!v.duplicate_intent.is_pending());
        assert_ne!(v.selected_clip, Some(originals[0]));
    });
}

#[gpui::test]
fn queued_duplicate_repeats_are_bounded_and_escape_discards_only_repeat_intentions(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    press(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        for _ in 0..MAX_QUEUED_REPEATS {
            v.duplicate_selected();
        }
        v.duplicate_selected();
        assert_eq!(
            v.duplicate_intent.queued.len(),
            usize::from(MAX_QUEUED_REPEATS)
        );
        assert!(v.error.as_deref().unwrap().contains("maximum 32"));
    });
    visual.simulate_keystrokes("escape");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert_eq!(all_ids(v).len(), originals.len() + 1);
    });
}

#[gpui::test]
fn close_prompt_preserves_previously_accepted_duplicate_continuations(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    press(&editor, &mut visual);
    press(&editor, &mut visual);
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(!v.request_window_close(window, cx));
            assert!(v.automation_close_preflight.frozen());
            assert_eq!(v.duplicate_intent.queued.len(), 1);
            v.duplicate_selected();
            assert_eq!(
                v.duplicate_intent.queued.len(),
                1,
                "close preflight rejects new presses"
            );
        })
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(&mut visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
        });
        visual.run_until_parked();
        if editor.read_with(&visual, |v, _| v.close_prompt_open) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "close preflight must reach the prompt"
        );
        std::thread::yield_now();
    }
    editor.read_with(&visual, |v, _| {
        assert!(v.close_prompt_open);
        assert!(!v.duplicate_intent.is_pending());
        let state = v.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 2);
        // The second copy overwrites the existing clip at 96,000 under the
        // ordinary clip overwrite policy; it still commits its own history.
        assert_eq!(all_ids(v).len(), originals.len() + 1);
        assert!(state.project.clip(originals[1]).is_none());
        let selected = state.project.clip(v.selected_clip.unwrap()).unwrap();
        assert_eq!((selected.start(), selected.source_offset()), (96_000, 0));
        assert_ne!(v.selected_clip, Some(originals[0]));
    });
    cx.simulate_prompt_answer("cancel");
    cx.run_until_parked();
    editor.read_with(cx, |v, _| assert!(!v.close_prompt_open));
}

#[gpui::test]
fn discarded_local_duplicate_queue_retires_while_admitted_request_keeps_custody(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    editor.update(&mut visual, |v, _| {
        // Model the deferred owner discarding an unsubmitted request on refusal.
        // A preceding local request causes normal enqueue to retain the duplicate.
        v.pending_requests.push_back(Request::SaveCurrent);
        v.start_clip_duplicate(v.selected_clip_ids(), 1, false);
        assert!(
            v.duplicate_intent
                .pending
                .as_ref()
                .unwrap()
                .receipt
                .is_queued()
        );
        assert!(
            !v.retire_abandoned_duplicate(),
            "queued exact payload retains custody"
        );
        v.pending_requests.clear();
        assert!(v.retire_abandoned_duplicate());
        assert!(!v.duplicate_intent.is_pending());
        v.duplicate_selected();
        assert!(v.duplicate_intent.is_pending());
        assert!(
            !v.retire_abandoned_duplicate(),
            "admitted work needs its terminal even without a local queue"
        );
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert_eq!(all_ids(v).len(), originals.len() + 1);
        assert_ne!(v.selected_clip, Some(originals[0]));
    });
}

#[gpui::test]
fn full_real_worker_queue_defers_duplicate_and_retires_its_own_terminal(cx: &mut TestAppContext) {
    use hex_session::service::{Service, WorkerAudioSink};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    // A test-only output adapter pauses the Session control worker, never an
    // audio callback. One bounded handshake prevents a queue-drain race.
    struct Gate {
        armed: Arc<AtomicBool>,
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }
    impl WorkerAudioSink for Gate {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(
            &mut self,
            _: Arc<hex_session::service::CanonicalAudioSourceSnapshot>,
        ) {
        }
        fn poll(&mut self) {
            if self.armed.swap(false, Ordering::AcqRel) {
                self.entered.try_send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        }
        fn close(&mut self) {}
    }
    let armed = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let service = Service::with_audio_sink(
        32,
        8_000_000,
        None,
        Box::new(Gate {
            armed: Arc::clone(&armed),
            entered: entered_tx,
            release: release_rx,
        }),
    )
    .unwrap();
    let (_files, handle, editor, originals) =
        crate::editor::drag::group_drag_ui_test::fixture_with_service(cx, service);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    armed.store(true, Ordering::Release);
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    editor.update(&mut visual, |v, _| {
        // SaveCurrent refuses this untitled fixture without changing revision.
        // The paused worker cannot free a slot until the duplicate is deferred.
        let mut full = false;
        for _ in 0..128 {
            match v.service.submit(Request::SaveCurrent) {
                Ok(()) => v.worker_requests_pending += 1,
                Err(hex_session::service::SubmitError::Full(_)) => {
                    full = true;
                    break;
                }
                Err(error) => panic!("unexpected queue admission: {error:?}"),
            }
        }
        assert!(full, "the real worker channel must reach backpressure");
        v.start_clip_duplicate(v.selected_clip_ids(), 0, false);
        assert!(
            v.duplicate_intent
                .pending
                .as_ref()
                .unwrap()
                .receipt
                .is_queued()
        );
        assert!(
            v.pending_requests
                .iter()
                .any(|request| v.pending_duplicate_matches(request))
        );
    });
    release_tx.try_send(()).unwrap();
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(all_ids(v).len(), originals.len() + 1);
        assert_ne!(v.selected_clip, Some(originals[0]));
    });
}

#[gpui::test]
fn confirmed_new_project_cancels_repeats_without_stranding_future_duplication(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    press(&editor, &mut visual);
    press(&editor, &mut visual);
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
            v.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
            assert!(v.project_open.busy() && !v.opening_project);
            assert!(
                v.duplicate_intent.is_pending(),
                "admitted edit still owns its terminal"
            );
            assert_eq!(v.duplicate_intent.queued.len(), 0);
        })
    });
    pump(&editor, &mut visual);
    visual.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("don't save");
    visual.run_until_parked();
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.project_generation, 1);
        assert!(!v.duplicate_intent.is_pending());
        assert!(all_ids(v).is_empty());
        assert!(v.error.is_none(), "{:?}", v.error);
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "new midi".into()
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track: v.selected_track.unwrap(),
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })));
    });
    pump(&editor, &mut visual);
    press(&editor, &mut visual);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(all_ids(v).len(), 2);
    });
}

#[gpui::test]
fn alias_and_independent_intentions_keep_order_through_delayed_terminals(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    visual.simulate_keystrokes("cmd-d d cmd-d");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.duplicate_intent.pending.as_ref().unwrap().kind,
            DuplicateKind::Alias
        );
        assert_eq!(
            v.duplicate_intent
                .queued
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![DuplicateKind::Independent, DuplicateKind::Alias]
        );
        assert_eq!(v.state.as_ref().unwrap().revision, before);
        assert_eq!(v.selected_clip, Some(originals[0]));
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().revision, before)
        });
    }
    pump(&editor, &mut visual);
    let selected = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(!v.duplicate_intent.is_pending());
        let p = &v.state.as_ref().unwrap().project;
        let at = |start| {
            p.tracks()[0]
                .clips()
                .iter()
                .find(|c| c.start() == start)
                .unwrap()
                .id()
        };
        let first_group = p.clip_alias(at(48_000)).unwrap();
        assert_eq!(Some(first_group), p.clip_alias(at(72_000)));
        let second_group = p.clip_alias(at(96_000)).unwrap();
        assert_eq!(Some(second_group), p.clip_alias(at(120_000)));
        assert_ne!(first_group, second_group);
        assert_eq!(v.selected_clip, Some(at(120_000)));
        at(120_000)
    });
    let marker = Box::leak(format!("clip-alias-{}", selected.value()).into_boxed_str());
    let header = Box::leak(format!("clip-name-{}", selected.value()).into_boxed_str());
    let icon = visual.debug_bounds(marker).unwrap();
    let header = visual.debug_bounds(header).unwrap();
    assert!(icon.origin.x >= header.origin.x && icon.right() <= header.right());
    assert!(icon.origin.y >= header.origin.y && icon.bottom() <= header.bottom());
    visual.simulate_keystrokes("i");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(v.state.as_ref().unwrap().project.clip_alias(selected), None);
        assert_eq!(v.selected_clip, Some(selected));
    });
    assert!(visual.debug_bounds(marker).is_none());
}

#[gpui::test]
fn console_alias_duplicate_selects_committed_cell_and_repeats_in_order(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (_files, handle, editor, originals) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let track = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddScene {
                name: "first".into()
            })))
        });
        pump(&editor, &mut visual);
        let scene = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .scenes()
                .last()
                .unwrap()
                .id()
        });
        let command = if midi {
            Command::CreateSessionMidiClip {
                track,
                scene,
                name: "midi".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            }
        } else {
            Command::CopyArrangementAudioClipToSession {
                source: originals[0],
                scene,
            }
        };
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump(&editor, &mut visual);
        let source = editor.read_with(&visual, |v, _| {
            match v
                .state
                .as_ref()
                .unwrap()
                .project
                .session_cell(track, scene)
                .unwrap()
            {
                SessionCellContent::Audio(c) => c.id(),
                SessionCellContent::Midi(c) => c.id(),
                _ => panic!("clip"),
            }
        });
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                if midi {
                    v.select_session_midi(track, scene, source);
                } else {
                    v.select_session_cell(track, scene);
                }
                v.focus.focus(window);
                cx.notify();
            })
        });
        visual.simulate_keystrokes("cmd-d cmd-d");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.selected_cell,
                Some((track, scene)),
                "source selection owns held terminal"
            );
            assert_eq!(v.duplicate_intent.queued.len(), 1);
        });
        pump(&editor, &mut visual);
        let selected = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(!v.duplicate_intent.is_pending());
            let p = &v.state.as_ref().unwrap().project;
            let (owner, destination) = v.selected_cell.unwrap();
            assert_eq!(owner, track);
            assert_ne!(destination, scene);
            let id = match p.session_cell(track, destination).unwrap() {
                SessionCellContent::Audio(c) => c.id(),
                SessionCellContent::Midi(c) => c.id(),
                _ => panic!("copied clip"),
            };
            assert_eq!(p.clip_alias(source), p.clip_alias(id));
            assert!(p.clip_alias(id).is_some());
            id
        });
        let marker = Box::leak(format!("clip-alias-{}", selected.value()).into_boxed_str());
        let badge = visual.debug_bounds(marker).unwrap();
        let surface = Box::leak(format!("console-clip-{}", selected.value()).into_boxed_str());
        let surface = visual.debug_bounds(surface).unwrap();
        assert!(badge.left() >= surface.left());
        assert!(
            badge.right() <= surface.right() - gpui::px(5.0),
            "alias leaves the cut clear"
        );
        assert!(badge.top() >= surface.top() && badge.bottom() <= surface.bottom());
        visual.simulate_keystrokes("d");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let (track, scene) = v.selected_cell.unwrap();
            let id = match p.session_cell(track, scene).unwrap() {
                SessionCellContent::Audio(c) => c.id(),
                SessionCellContent::Midi(c) => c.id(),
                _ => panic!("independent clip"),
            };
            assert_ne!(id, selected);
            assert!(p.clip_alias(id).is_none());
        });
    }
}

#[gpui::test]
fn alias_keys_leave_text_and_foreground_automation_ownership_intact(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    let (clip, revision, alias) = editor.read_with(&visual, |v, _| {
        let clip = v.selected_clip.unwrap();
        (
            clip,
            v.state.as_ref().unwrap().revision,
            v.state.as_ref().unwrap().project.clip_alias(clip).unwrap(),
        )
    });
    visual.simulate_keystrokes("cmd-r");
    visual.simulate_keystrokes("d i cmd-d");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.clip_name_edit.is_some());
        assert!(!v.duplicate_intent.is_pending());
        assert_eq!(v.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            v.state.as_ref().unwrap().project.clip_alias(clip),
            Some(alias)
        );
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.automation_view.enabled = true;
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("d i cmd-d");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.duplicate_intent.is_pending());
        assert_eq!(v.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            v.state.as_ref().unwrap().project.clip_alias(clip),
            Some(alias)
        );
    });
}

#[gpui::test]
fn diagnostics_reconstruct_real_pointer_shortcut_and_worker_receipt_without_text(
    cx: &mut TestAppContext,
) {
    let directory =
        std::env::temp_dir().join(format!("hex-input-action-log-{}", std::process::id()));
    let mut log =
        hex_diagnostics::SessionLog::start_with_context(directory.clone(), "fixture source")
            .unwrap();
    cx.update(|cx| {
        crate::install_session_log(cx, Ok(log.handle()));
        crate::install_action_diagnostics(cx, true);
    });
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    press(&editor, &mut visual);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_ne!(view.selected_clip, Some(originals[0]));
    });
    log.shutdown().unwrap();
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.contains("stage=pointer_down"), "{text}");
    assert!(text.contains("stage=key"), "{text}");
    assert!(text.contains("stage=handler"), "{text}");
    assert!(text.contains("duplicate_clips"), "{text}");
    assert!(text.contains("ui_action_id: Some("), "{text}");
    assert!(text.contains("ClientReceived"), "{text}");
    assert!(text.contains("stage=view_observed"), "{text}");
    println!("diagnostic fixture log:\n{text}");
    std::fs::remove_dir_all(directory).unwrap();
}
