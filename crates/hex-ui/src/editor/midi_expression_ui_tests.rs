//! Rendered per-note expression editing, delayed-ACK receipt custody, overlap
//! disambiguation, independent playback PCM, and Undo/Redo/save-reopen tests.
use super::super::EditorView;
use super::super::midi_canvas::creation_tests::{fixture, pump};
use super::ExpressionLaneMode;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, Created, NoteExpression};
use hex_session::service::{Event, Request};
use std::time::Instant;

fn terminal(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(&*visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                return event;
            }
            editor.update(&mut *visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(Instant::now() < deadline, "real worker terminal");
        std::thread::yield_now();
    }
}

fn repaint(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(&mut *visual, |_, cx| cx.notify());
    visual.run_until_parked();
}

#[gpui::test]
fn overlapping_note_expression_edit_uses_receipt_across_delayed_ack_and_produces_distinct_pcm(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _env, clip) = fixture(cx);
    // Add two overlapping notes on different pitches (60 and 64) spanning beats 0..2,
    // plus a third note (pitch 72) that we will delete to verify stale NoteId refusal.
    let dur = BeatTime::from_quarters(2).unwrap();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: dur,
            pitch: 60,
            channel: 1,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: dur,
            pitch: 64,
            channel: 2,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: dur,
            duration: BeatTime::ONE,
            pitch: 72,
            channel: 3,
            velocity: 100,
        })));
    });
    pump(&editor, &mut visual);

    let (note_60, note_64, deleted_note) = editor.read_with(&visual, |v, _| {
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        let a = notes.iter().find(|n| n.pitch() == 60).unwrap().id();
        let b = notes.iter().find(|n| n.pitch() == 64).unwrap().id();
        let c = notes.iter().find(|n| n.pitch() == 72).unwrap().id();
        (a, b, c)
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RemoveMidiNote(deleted_note))));
    });
    pump(&editor, &mut visual);

    // Switch bottom lane to per-note pitch expression and render once so lane bounds exist.
    editor.update(&mut visual, |v, cx| {
        v.midi_editor.velocity.mode = ExpressionLaneMode::NotePitch;
        v.midi_editor.selection.clear();
        cx.notify();
    });
    visual.run_until_parked();
    let lane_bounds = visual
        .debug_bounds("midi-expression-lane")
        .expect("lane bounds");

    // 1. Clicking over two overlapping notes with empty selection MUST refuse rather than silently picking one!
    let click_x = lane_bounds.origin.x + lane_bounds.size.width * 0.12;
    let high_y = lane_bounds.origin.y + px(8.0);
    visual.simulate_mouse_down(point(click_x, high_y), MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.pending_drag.is_none());
        assert!(
            v.status.contains("select one note"),
            "expected overlap disambiguation refusal, got: {}",
            v.status
        );
    });

    // 2. Selecting BOTH overlapping notes MUST also refuse explicitly!
    editor.update(&mut visual, |v, cx| {
        v.midi_editor.selection.clear();
        v.midi_editor
            .selection
            .add_many(&[note_60, note_64])
            .unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(point(click_x, high_y), MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.pending_drag.is_none());
        assert!(v.status.contains("single note"));
    });

    // 3. Select ONLY note_60 and click high pitch offset -> frozen preview MUST stay painted
    //    across a completed repaint while worker ACK is held!
    editor.update(&mut visual, |v, cx| {
        v.midi_editor.selection.set_single(note_60);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(point(click_x, high_y), MouseButton::Left, Modifiers::none());
    // Hold worker terminal and complete a full repaint: pending_drag MUST hold the preview!
    repaint(&editor, &mut visual);
    let preview_60 = editor.read_with(&visual, |v, _| {
        assert!(
            v.midi_editor.pending_drag.is_some(),
            "delayed ACK must retain frozen expression preview"
        );
        let (expr, pts) = v
            .displayed_note_expression(clip, note_60)
            .expect("displayed preview for note_60");
        assert!(expr.pitch_offset_cents > 1_000);
        assert_eq!(pts.len(), 1);
        // Overlapping note_64 MUST remain completely unaffected!
        assert!(v.displayed_note_expression(clip, note_64).is_none());
        expr
    });

    // Deliver exact terminal -> receipt retires atomically with canonical installation.
    let done = terminal(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.receive(done);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_editor.pending_drag.is_none());
        let src = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        let n60 = src.notes().iter().find(|n| n.id() == note_60).unwrap();
        let n64 = src.notes().iter().find(|n| n.id() == note_64).unwrap();
        assert_eq!(n60.expression(), preview_60);
        assert_eq!(n60.expression_points().len(), 1);
        assert_eq!(n64.expression(), NoteExpression::default());
        assert!(n64.expression_points().is_empty());
    });

    // 4. Select ONLY note_64 and click low pitch offset (-cents).
    let low_y = lane_bounds.origin.y + lane_bounds.size.height - px(8.0);
    editor.update(&mut visual, |v, cx| {
        v.midi_editor.selection.set_single(note_64);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(point(click_x, low_y), MouseButton::Left, Modifiers::none());
    let done2 = terminal(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.receive(done2);
        cx.notify();
    });
    visual.run_until_parked();
    let (expr_60, expr_64, snapshot_after) = editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        let src = project.midi_source(clip).unwrap();
        let n60 = src.notes().iter().find(|n| n.id() == note_60).unwrap();
        let n64 = src.notes().iter().find(|n| n.id() == note_64).unwrap();
        assert!(n60.expression().pitch_offset_cents > 1_000);
        assert!(n64.expression().pitch_offset_cents < -1_000);
        (n60.expression(), n64.expression(), project.clone())
    });

    // 5. Strict save/reopen round trip preserves edited note expression and curve points!
    let encoded = hex_storage::encode(&snapshot_after).unwrap();
    let reopened = hex_storage::decode(&encoded).unwrap();
    assert_eq!(&reopened, &*snapshot_after);

    // 6. Single Undo reverts ONLY note_64's edit; Redo restores it!
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let src = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(
            src.notes()
                .iter()
                .find(|n| n.id() == note_60)
                .unwrap()
                .expression(),
            expr_60
        );
        assert_eq!(
            src.notes()
                .iter()
                .find(|n| n.id() == note_64)
                .unwrap()
                .expression(),
            NoteExpression::default()
        );
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Redo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let src = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(
            src.notes()
                .iter()
                .find(|n| n.id() == note_64)
                .unwrap()
                .expression(),
            expr_64
        );
    });

    // 7. Deleted/stale NoteId is refused cleanly without mutating canonical notes.
    let before_bad = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiNoteExpression {
            clip,
            note: deleted_note,
            expression: expr_60,
            points: Vec::new(),
        })));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(&v.state.as_ref().unwrap().project, &before_bad);
    });

    // 8. Independent playback PCM: two same-pitch (pitch 60) notes on two FM4 tracks edited to
    //    distinct per-note pitch offsets (+700 vs -500 cents) produce measurably different PCM!
    let render_fm4_with_pitch_offset = |cents: i16| -> Vec<[f32; 2]> {
        let mut proj_ed =
            hex_project::Editor::new(hex_project::Project::new(8_000).unwrap(), 8).unwrap();
        let fm4_state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.fm4"),
        )
        .unwrap();
        let Created::Track(t) = proj_ed
            .execute(Command::InsertTrackWithDevice {
                name: "fm4".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: fm4_state,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(c) = proj_ed
            .execute(Command::PlaceMidiClip {
                track: t,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let Created::Note(n) = proj_ed
            .execute(Command::AddMidiNote {
                clip: c,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 1,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note")
        };
        proj_ed
            .execute(Command::SetMidiNoteExpression {
                clip: c,
                note: n,
                expression: NoteExpression {
                    pitch_offset_cents: cents,
                    timbre: 0,
                    pressure: 0,
                },
                points: Vec::new(),
            })
            .unwrap();
        let rev = std::sync::Arc::new(
            hex_engine::PreparedRevision::prepare(
                proj_ed.project(),
                &std::collections::BTreeMap::new(),
                std::sync::Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut pb = hex_engine::SessionPlayback::new(rev).unwrap();
        pb.apply(hex_engine::SessionPlaybackCommand::Play).unwrap();
        let mut pcm = vec![[0.0; 2]; 512];
        pb.render(&mut pcm).unwrap();
        pcm
    };
    let pcm_up = render_fm4_with_pitch_offset(700);
    let pcm_down = render_fm4_with_pitch_offset(-500);
    let max_diff = pcm_up
        .iter()
        .zip(pcm_down.iter())
        .map(|(a, b)| (a[0] - b[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_diff > 0.1,
        "distinct edited per-note pitch offsets must produce independent playback PCM (max_diff={max_diff})"
    );
}
