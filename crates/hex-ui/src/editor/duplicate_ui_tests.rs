//! Rendered whole-clip shortcuts and ordered real-worker terminal regressions.
use super::*;
use crate::editor::drag::group_drag_ui_test::{click, fixture, pump};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
use std::time::{Duration, Instant};

fn press(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("cmd-d");
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
fn shift_selected_audio_group_command_d_copies_all_members_and_repeats_after_ack(
    cx: &mut TestAppContext,
) {
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
        assert_eq!(v.duplicate_intent.queued_repeats, 1);
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
fn mixed_and_midi_only_clip_selection_command_d_selects_actual_committed_births(
    cx: &mut TestAppContext,
) {
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
            assert_eq!(v.duplicate_intent.queued_repeats, 0);
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
        let until = Instant::now() + Duration::from_secs(10);
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
        assert_eq!(v.duplicate_intent.queued_repeats, 0);
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
        assert_eq!(v.duplicate_intent.queued_repeats, MAX_QUEUED_REPEATS);
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
            assert!(v.close_prompt_open);
            assert_eq!(v.duplicate_intent.queued_repeats, 1);
            v.duplicate_selected();
            assert_eq!(
                v.duplicate_intent.queued_repeats, 1,
                "modal sheet rejects new presses"
            );
        })
    });
    pump(&editor, &mut visual);
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
        fn canonical_changed(&mut self, _: Arc<hex_session::CanonicalAudioSourceSnapshot>) {}
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
    editor.update(&mut visual, |v, _| {
        v.new_project(hex_session::DocumentAudioMode::Resident);
        v.new_project(hex_session::DocumentAudioMode::Resident);
        assert!(v.opening_project);
        assert!(
            v.duplicate_intent.is_pending(),
            "admitted edit still owns its terminal"
        );
        assert_eq!(v.duplicate_intent.queued_repeats, 0);
    });
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
