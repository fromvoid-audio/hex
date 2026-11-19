use super::*;
use crate::editor::arrangement_sections::tests::{fixture, next, position};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::service::Request;

fn open(cx: &mut TestAppContext) -> (Entity<EditorView>, VisualTestContext, [SectionId; 2]) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, ids) = fixture(cx);
    editor.update(cx, |view, _| view.section_autoplay = false);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (editor, visual, ids)
}

fn click(editor: &Entity<EditorView>, visual: &mut VisualTestContext, quarters: u64) {
    let point = editor.read_with(visual, |view, _| position(view, quarters));
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

fn acknowledge(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let terminal = editor.update(visual, |view, _| next(&view.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn keyboard_paste_uses_boundaries_and_retains_gap_copy_selection_through_exact_ack(
    cx: &mut TestAppContext,
) {
    for (cursor, destination, gap) in [(0, 0, false), (2, 0, false), (6, 4, false), (14, 14, true)]
    {
        let (editor, mut visual, ids) = open(cx);
        click(&editor, &mut visual, 2);
        visual.simulate_keystrokes("cmd-c");
        let before = editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.clipboard.unwrap().section, ids[0]);
            view.state.as_ref().unwrap().project.snapshot()
        });
        click(&editor, &mut visual, cursor);
        visual.simulate_keystrokes("cmd-v");
        visual.run_until_parked();
        let (copy_index, gap_index) = editor.read_with(&visual, |view, _| {
            let pending = view
                .sections
                .pending
                .as_ref()
                .expect("typed section paste pending");
            assert!(copied_section_source(&pending.command).is_some());
            let copy_index = pending
                .display
                .markers
                .iter()
                .position(|m| {
                    m.id.is_none() && m.start == BeatTime::from_quarters(destination).unwrap()
                })
                .unwrap();
            let gap_index = pending.display.markers.iter().position(|m| {
                m.id.is_none()
                    && m.start == BeatTime::from_quarters(12).unwrap()
                    && m.end == BeatTime::from_quarters(14).unwrap()
            });
            assert_eq!(gap_index.is_some(), gap);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            (copy_index, gap_index)
        });
        let selector = Box::leak(format!("arrangement-section-{copy_index}").into_boxed_str());
        let selected = Box::leak(
            format!("arrangement-section-selected-trailing-{copy_index}").into_boxed_str(),
        );
        let geometry = visual.debug_bounds(selector).unwrap();
        let terminal = editor.update(&mut visual, |view, _| next(&view.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let Some(hex_project::Created::ArrangementSection(copy)) = terminal.created else {
            panic!("created copy");
        };
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(selector).unwrap(), geometry);
            assert!(visual.debug_bounds(selected).is_some());
            if let Some(index) = gap_index {
                let gap_selected = Box::leak(
                    format!("arrangement-section-selected-trailing-{index}").into_boxed_str(),
                );
                assert!(
                    visual.debug_bounds(gap_selected).is_none(),
                    "gap must not select with the copy"
                );
            }
        }
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert!(view.sections.pending.is_none());
            assert_eq!(view.sections.selected, Some(copy));
            let state = view.state.as_ref().unwrap();
            assert_eq!(view.sections.clipboard.unwrap().revision, state.revision);
            assert_eq!(
                state.project.arrangement_section(copy).unwrap().start(),
                BeatTime::from_quarters(destination).unwrap()
            );
            assert_eq!(
                state.project.tracks()[0]
                    .midi_clips()
                    .iter()
                    .map(|clip| clip.length().raw())
                    .sum::<u64>(),
                BeatTime::from_quarters(20).unwrap().raw(),
                "all-track content was inserted"
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), geometry);
        // Undo uses the ordinary worker/history path, including its monotonic allocator.
        let undo = editor.update(&mut visual, |view, _| {
            view.enqueue(Request::Undo);
            next(&view.service)
        });
        editor.update(&mut visual, |view, cx| {
            view.receive(undo);
            let mut restored = view.state.as_ref().unwrap().project.snapshot();
            assert!(restored.next_id >= before.next_id);
            restored.next_id = before.next_id;
            assert_eq!(restored, before);
            cx.notify();
        });
    }
}

#[gpui::test]
fn clipboard_empty_stale_and_repeated_placements_keep_their_section_scope(cx: &mut TestAppContext) {
    let (editor, mut visual, ids) = open(cx);
    click(&editor, &mut visual, 2);
    visual.simulate_keystrokes("cmd-v");
    editor.read_with(&visual, |view, _| {
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("section clipboard is empty")
        );
        assert_eq!(view.worker_requests_pending, 0);
    });
    visual.simulate_keystrokes("cmd-c");
    visual.simulate_keystrokes("cmd-d");
    acknowledge(&editor, &mut visual);
    for cursor in [6, 14] {
        click(&editor, &mut visual, cursor);
        visual.simulate_keystrokes("cmd-v");
        acknowledge(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.clipboard.unwrap().section, ids[0]);
            assert_eq!(
                view.sections.clipboard.unwrap().revision,
                view.state.as_ref().unwrap().revision
            );
        });
    }
    // An unrelated canonical source edit invalidates the strict reference.
    let edited = editor.update(&mut visual, |view, _| {
        view.section_metadata(Command::RenameArrangementSection {
            section: ids[0],
            name: "changed".into(),
        });
        next(&view.service)
    });
    assert!(edited.error.is_none(), "{:?}", edited.error);
    editor.update(&mut visual, |view, cx| {
        view.receive(edited);
        cx.notify();
    });
    click(&editor, &mut visual, 2);
    visual.simulate_keystrokes("cmd-v");
    editor.read_with(&visual, |view, _| {
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("copy the section again")
        );
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.sections.pending.is_none());
    });
}

#[gpui::test]
fn section_menu_copy_paste_and_project_replacement_keep_clipboard_custody(cx: &mut TestAppContext) {
    let (editor, mut visual, ids) = open(cx);
    click(&editor, &mut visual, 2);
    for (label, point_beat) in [
        ("copy section · cmd-c", 2),
        ("paste section at cursor · cmd-v", 10),
    ] {
        if label.starts_with("paste") {
            click(&editor, &mut visual, 14);
        }
        let at = editor.read_with(&visual, |view, _| position(view, point_beat));
        visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let index = editor.read_with(&visual, |view, _| {
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == label)
                .unwrap()
        });
        let panel = visual.debug_bounds("context-menu-panel").unwrap();
        let item = gpui::point(
            panel.left() + gpui::px(20.0),
            panel.top() + gpui::px(14.0 + index as f32 * 24.0),
        );
        visual.simulate_mouse_down(item, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(item, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(view.sections.clipboard.unwrap().section, ids[0]);
        });
    }
    editor.read_with(&visual, |view, _| {
        assert!(matches!(view.sections.pending.as_ref().map(|p| &p.command),
            Some(Command::AppendArrangementSectionCopy { start, .. }) if *start == BeatTime::from_quarters(14).unwrap()));
    });
    acknowledge(&editor, &mut visual);
    let replaced = editor.update(&mut visual, |view, _| {
        view.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        });
        next(&view.service)
    });
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    assert!(replaced.project_replaced);
    editor.update(&mut visual, |view, _| {
        view.receive(replaced);
        assert!(view.sections.clipboard.is_none());
        assert!(view.sections.pending.is_none());
    });
}
