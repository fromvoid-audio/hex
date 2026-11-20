use super::tests::{fixture, next, position};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, px};

#[gpui::test]
fn replacement_body_drop_freezes_one_indicator_selection_and_music_until_exact_ack(
    cx: &mut TestAppContext,
) {
    for (source_index, copy) in [(0, false), (1, false), (0, true), (1, true)] {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (from, to, before) = editor.read_with(cx, |v, _| {
            (
                position(v, if source_index == 0 { 2 } else { 10 }),
                position(v, if source_index == 0 { 10 } else { 2 }),
                v.state.as_ref().unwrap().project.snapshot(),
            )
        });
        let modifiers = Modifiers {
            alt: copy,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-drop-replace").is_some());
        assert!(visual.debug_bounds("section-drop-insertion").is_none());
        editor.read_with(cx,|v,_|{
            let g=v.sections.gesture.as_ref().unwrap();
            assert!(matches!(g.command,Some(Command::ReplaceArrangementSection{source,target,copy:mode})if source==ids[source_index]&&target==ids[1-source_index]&&mode==copy));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(),before);
        });
        let cue = visual.debug_bounds("section-drop-replace").unwrap();
        visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let expected = editor.read_with(cx, |v, _| {
            v.arrangement_rows_state().unwrap().project.snapshot()
        });
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let selected = if copy {
            let Some(hex_project::Created::ArrangementSection(id)) = terminal.created else {
                panic!("replacement identity")
            };
            id
        } else {
            ids[source_index]
        };
        for _ in 0..3 {
            visual.update(|w, _| w.refresh());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds("section-drop-replace").unwrap(), cue);
            assert!(visual.debug_bounds("section-drop-insertion").is_none());
        }
        let after = editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert!(v.sections.pending.is_none());
            assert_eq!(v.sections.selected, Some(selected));
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.snapshot(), expected);
            assert!(p.arrangement_section(ids[1 - source_index]).is_none());
            assert_eq!(p.arrangement_sections().len(), if copy { 2 } else { 1 });
            assert_eq!(
                p.tracks()[0]
                    .midi_clips()
                    .iter()
                    .map(|c| c.length().raw())
                    .sum::<u64>(),
                BeatTime::from_quarters(if copy { 16 } else { 12 })
                    .unwrap()
                    .raw()
            );
            let after = p.snapshot();
            cx.notify();
            after
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-drop-replace").is_none());
        assert!(visual.debug_bounds("section-drop-insertion").is_none());
        editor.update(cx, |v, cx| {
            v.service.submit(Request::Undo).unwrap();
            v.receive(next(&v.service));
            let mut restored = v.state.as_ref().unwrap().project.snapshot();
            assert!(restored.next_id >= after.next_id);
            restored.next_id = before.next_id;
            assert_eq!(restored, before);
            v.service.submit(Request::Redo).unwrap();
            v.receive(next(&v.service));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
            cx.notify();
        });
    }
}

#[gpui::test]
fn drop_feedback_switches_edges_body_self_marker_only_and_cancel(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (from, mut edge, body, self_body, beyond) = editor.read_with(cx, |v, _| {
        (
            position(v, 2),
            position(v, 8),
            position(v, 10),
            position(v, 3),
            position(v, 14),
        )
    });
    edge.x += px(1.0);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-insertion").is_some());
    assert!(visual.debug_bounds("section-drop-replace").is_none());
    visual.simulate_mouse_move(body, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-replace").is_some());
    assert!(visual.debug_bounds("section-drop-insertion").is_none());
    visual.simulate_mouse_move(
        self_body,
        MouseButton::Left,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-replace").is_none());
    assert!(visual.debug_bounds("section-drop-insertion").is_none());
    editor.read_with(cx, |v, _| {
        assert!(v.sections.gesture.as_ref().unwrap().command.is_none())
    });
    visual.simulate_mouse_move(beyond, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-insertion").is_some());
    visual.simulate_modifiers_change(Modifiers {
        alt: true,
        platform: true,
        ..Modifiers::default()
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-insertion").is_none());
    assert!(visual.debug_bounds("section-drop-replace").is_none());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(v.sections.gesture.is_none());
        assert!(v.sections.pending.is_none());
    });
}

#[gpui::test]
fn queued_tempo_completion_keeps_drop_feedback_frozen_until_exact_refusal(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |v, cx| {
        v.viewport = Some(v.visible_viewport(v.state.as_ref().unwrap().timeline_frames));
        cx.notify();
    });
    visual.run_until_parked();
    let (from, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let frozen = visual.debug_bounds("section-drop-replace").unwrap();
    editor.update(cx, |v, _| {
        let g = v.sections.gesture.take().unwrap();
        let mut receipt = Receipt::new(v.project_generation);
        assert!(receipt.submitted(2));
        v.sections.pending = Some(Pending {
            copied: None,
            generation: v.project_generation,
            revision: g.revision,
            command: g.command.unwrap(),
            display: g.display,
            layout: None,
            receipt,
            selection_intent_epoch: v.selection_intent_epoch,
        });
        v.worker_requests_pending = 2;
    });
    let (earlier, terminal) = editor.update(cx, |v, _| {
        v.service
            .submit(Request::Edit(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            }))
            .unwrap();
        let earlier = next(&v.service);
        assert!(earlier.error.is_none());
        let p = v.sections.pending.as_ref().unwrap();
        v.service
            .submit(Request::EditArrangementSection {
                project_generation: p.generation,
                revision: p.revision,
                command: p.command.clone(),
            })
            .unwrap();
        let terminal = next(&v.service);
        assert!(terminal.error.is_some());
        (earlier, terminal)
    });
    editor.update(cx, |v, cx| {
        v.receive(earlier);
        assert!(v.sections.pending.is_some());
        cx.notify();
    });
    for _ in 0..3 {
        visual.update(|w, _| w.refresh());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("section-drop-replace").unwrap(), frozen);
    }
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert!(v.sections.pending.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-drop-replace").is_none());
}
