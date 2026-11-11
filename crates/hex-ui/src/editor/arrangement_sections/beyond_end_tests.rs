use super::tests::{fixture, next, position};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

#[gpui::test]
fn beyond_end_move_and_copy_keep_gap_selection_music_and_exact_pending_ack(
    cx: &mut TestAppContext,
) {
    for (source_index, copy) in [(0, false), (1, false), (0, true), (1, true)] {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source = ids[source_index];
        let (from, to, before) = editor.read_with(cx, |v, _| {
            (
                position(v, if source_index == 0 { 2 } else { 10 }),
                position(v, 14),
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
        let appearance = editor.read_with(cx, |v, _| {
            let g = v.sections.gesture.as_ref().unwrap();
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(matches!(&g.command,
                Some(Command::AppendArrangementSectionCopy { start, .. } | Command::MoveArrangementSectionBeyondEnd { start, .. }) if *start == beat(14)));
            assert_eq!(copied_section_source(g.command.as_ref().unwrap()).is_some(), copy);
            g.gap_appearance.clone().unwrap()
        });
        if copy {
            visual.simulate_modifiers_change(Modifiers::default());
            visual.simulate_modifiers_change(modifiers);
            editor.read_with(cx, |v, _| {
                assert_eq!(
                    v.sections.gesture.as_ref().unwrap().gap_appearance.as_ref(),
                    Some(&appearance)
                )
            });
        }
        visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let (index, gap_index, layout) = editor.read_with(cx, |v, _| {
            let p = v.sections.pending.as_ref().unwrap();
            assert!(v.section_install_pending());
            let index = p
                .display
                .markers
                .iter()
                .position(|m| m.start == beat(14))
                .unwrap();
            let gap_index = p
                .display
                .markers
                .iter()
                .position(|m| m.name == appearance.0)
                .unwrap();
            let gap = &p.display.markers[gap_index];
            assert_eq!(
                (gap.start, gap.end, gap.color),
                (
                    beat(if copy {
                        12
                    } else if source_index == 0 {
                        8
                    } else {
                        4
                    }),
                    beat(14),
                    appearance.1
                )
            );
            let layout = v.arrangement_rows_state().unwrap().project.snapshot();
            assert_eq!(layout.tracks[0].id, before.tracks[0].id);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            (index, gap_index, layout)
        });
        let selector = Box::leak(format!("arrangement-section-{index}").into_boxed_str());
        let selected =
            Box::leak(format!("arrangement-section-selected-trailing-{index}").into_boxed_str());
        let gap_selected = Box::leak(
            format!("arrangement-section-selected-trailing-{gap_index}").into_boxed_str(),
        );
        let frozen = visual.debug_bounds(selector).unwrap();
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let moved_or_copy = if copy {
            let Some(hex_project::Created::ArrangementSection(id)) = terminal.created else {
                panic!("copy identity")
            };
            id
        } else {
            source
        };
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(selector).unwrap(), frozen);
            assert!(visual.debug_bounds(selected).is_some());
            assert!(visual.debug_bounds(gap_selected).is_none());
        }
        let after = editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert!(v.sections.pending.is_none());
            assert_eq!(v.sections.selected, Some(moved_or_copy));
            let project = &v.state.as_ref().unwrap().project;
            assert_eq!(
                project.snapshot(),
                layout,
                "clip rows and markers install together"
            );
            assert_eq!(
                project.arrangement_section(moved_or_copy).unwrap().start(),
                beat(14)
            );
            assert_eq!(
                project.tracks()[0]
                    .midi_clips()
                    .iter()
                    .map(|c| c.length().raw())
                    .sum::<u64>(),
                beat(if copy { 20 } else { 16 }).raw()
            );
            let after = project.snapshot();
            cx.notify();
            after
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), frozen);
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
fn beyond_end_move_obeys_grid_inversion_and_marker_only_latch(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (from, to) = editor.read_with(cx, |v, _| {
        let a = position(v, 14);
        let b = position(v, 15);
        (
            position(v, 10),
            point(
                px(f32::from(a.x) + (f32::from(b.x) - f32::from(a.x)) * 0.35),
                a.y,
            ),
        )
    });
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| {
        assert!(matches!(v.sections.gesture.as_ref().unwrap().command, Some(Command::MoveArrangementSectionBeyondEnd { start, .. }) if start == beat(14)));
    });
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    editor.read_with(cx, |v, _| {
        let expected = v.section_pointer(f32::from(to.x), f32::from(to.y), true).unwrap();
        assert_ne!(expected, beat(14));
        assert!(matches!(v.sections.gesture.as_ref().unwrap().command, Some(Command::MoveArrangementSectionBeyondEnd { start, .. }) if start == expected));
    });
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    });
    visual.simulate_modifiers_change(Modifiers::default());
    editor.read_with(cx, |v, _| {
        let g = v.sections.gesture.as_ref().unwrap();
        assert!(matches!(g.mode, Mode::Move(_)));
        assert!(matches!(
            g.command,
            Some(Command::MoveArrangementSectionMarker { .. })
        ));
        assert_eq!(
            g.display.markers.len(),
            2,
            "marker-only mode never inserts a gap"
        );
    });
    visual.simulate_keystrokes("escape");
}
