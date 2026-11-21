use super::*;
use crate::editor::arrangement_sections::tests::{fixture, next, position};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_session::service::Request;

fn open(cx: &mut TestAppContext) -> (Entity<EditorView>, VisualTestContext, [SectionId; 2]) {
    cx.update(crate::editor::bind_editor_keys);
    let (handle, editor, ids) = fixture(cx);
    editor.update(cx, |view, _| view.section_autoplay = false);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (editor, visual, ids)
}

fn click_header(editor: &Entity<EditorView>, visual: &mut VisualTestContext, quarters: u64) {
    let point = editor.read_with(visual, |view, _| position(view, quarters));
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

fn click_lower_body(editor: &Entity<EditorView>, visual: &mut VisualTestContext, quarters: u64) {
    let pt = editor.read_with(visual, |view, _| {
        let base = position(view, quarters);
        let [_, top, _, height] = view.section_layout().unwrap();
        point(base.x, px(top + height * 0.75))
    });
    visual.simulate_mouse_down(pt, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(pt, MouseButton::Left, Modifiers::default());
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
fn keyboard_paste_uses_exact_cursor_from_header_or_lower_body_through_exact_ack(
    cx: &mut TestAppContext,
) {
    // Fixture has sections [0, 4) and [8, 12).
    // Test boundary (0), split inside section (2), interior gap (6), and beyond end (14).
    for (cursor, lower_body, split, gap) in [
        (0, false, false, false),
        (2, true, true, false),
        (6, true, false, false),
        (14, true, false, true),
    ] {
        let (editor, mut visual, ids) = open(cx);
        click_header(&editor, &mut visual, 2);
        visual.simulate_keystrokes("cmd-c");
        let before = editor.read_with(&visual, |view, _| {
            let captured = view.sections.clipboard.as_ref().expect("copied section");
            assert_eq!(
                captured.clipboard.length(),
                BeatTime::from_quarters(4).unwrap()
            );
            assert_eq!(view.sections.selected, Some(ids[0]));
            view.state.as_ref().unwrap().project.snapshot()
        });
        if lower_body {
            click_lower_body(&editor, &mut visual, cursor);
        } else {
            click_header(&editor, &mut visual, cursor);
        }
        assert!(
            visual.debug_bounds("section-edit-cursor").is_some(),
            "section cursor must be visible at beat {cursor} (lower_body={lower_body})"
        );
        visual.simulate_keystrokes("cmd-v");
        visual.run_until_parked();
        let (copy_index, gap_index) = editor.read_with(&visual, |view, _| {
            let pending = view
                .sections
                .pending
                .as_ref()
                .expect("typed section paste pending");
            assert!(matches!(
                pending.command,
                Command::PasteArrangementSection { at, .. }
                    if at == BeatTime::from_quarters(cursor).unwrap()
            ));
            let copy_index = pending
                .display
                .markers
                .iter()
                .position(|m| m.id.is_none() && m.start == BeatTime::from_quarters(cursor).unwrap())
                .unwrap();
            let gap_index = pending.display.markers.iter().position(|m| {
                m.id.is_none()
                    && m.start == BeatTime::from_quarters(12).unwrap()
                    && m.end == BeatTime::from_quarters(14).unwrap()
            });
            assert_eq!(gap_index.is_some(), gap);
            if split {
                assert_eq!(pending.display.markers.len(), 4);
            }
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
            panic!("created pasted section");
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
                    "gap must not select with the pasted section"
                );
            }
        }
        editor.update(&mut visual, |view, cx| {
            view.receive(terminal);
            assert!(view.sections.pending.is_none());
            assert_eq!(view.sections.selected, Some(copy));
            let state = view.state.as_ref().unwrap();
            assert_eq!(
                state.project.arrangement_section(copy).unwrap().start(),
                BeatTime::from_quarters(cursor).unwrap()
            );
            assert_eq!(
                view.sections.cursor.unwrap().beat,
                BeatTime::from_quarters(cursor + 4).unwrap(),
                "cursor advances to pasted section end"
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
fn cut_commits_on_ack_pastes_repeatedly_and_survives_unrelated_edits(cx: &mut TestAppContext) {
    let (editor, mut visual, ids) = open(cx);
    click_header(&editor, &mut visual, 2);
    // Empty clipboard refuses cleanly.
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

    // Cut the first section [0, 4).
    let before_cut = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("cmd-x");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let pending = view.sections.pending.as_ref().expect("pending cut");
        assert!(matches!(
            pending.command,
            Command::DeleteArrangementSection(id) if id == ids[0]
        ));
        assert!(
            view.sections.clipboard.is_none(),
            "clipboard commits only after canonical deletion succeeds"
        );
    });
    acknowledge(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.sections.clipboard.is_some());
        assert_eq!(view.sections.cursor.unwrap().beat, BeatTime::ZERO);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .len(),
            1
        );
    });

    // Unrelated edit does not invalidate the captured section clipboard.
    let edited = editor.update(&mut visual, |view, _| {
        view.section_metadata(Command::RenameArrangementSection {
            section: ids[1],
            name: "renamed".into(),
        });
        next(&view.service)
    });
    assert!(edited.error.is_none(), "{:?}", edited.error);
    editor.update(&mut visual, |view, cx| {
        view.receive(edited);
        cx.notify();
    });
    visual.run_until_parked();

    // Place cursor via lower-body click at beat 8 (past remaining [4, 8) section)
    // and paste twice consecutively without re-clicking.
    click_lower_body(&editor, &mut visual, 8);
    visual.simulate_keystrokes("cmd-v");
    acknowledge(&editor, &mut visual);
    visual.simulate_keystrokes("cmd-v");
    acknowledge(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let sections = view.state.as_ref().unwrap().project.arrangement_sections();
        assert_eq!(sections.len(), 3);
        assert_eq!(
            (sections[1].start(), sections[1].end()),
            (
                BeatTime::from_quarters(8).unwrap(),
                BeatTime::from_quarters(12).unwrap()
            )
        );
        assert_eq!(
            (sections[2].start(), sections[2].end()),
            (
                BeatTime::from_quarters(12).unwrap(),
                BeatTime::from_quarters(16).unwrap()
            )
        );
        assert_eq!(
            view.sections.cursor.unwrap().beat,
            BeatTime::from_quarters(16).unwrap()
        );
    });

    // Three undos restore the exact state before cut (2 pastes + 1 rename + 1 cut = 4 undos).
    for _ in 0..4 {
        let undo = editor.update(&mut visual, |view, _| {
            view.enqueue(Request::Undo);
            next(&view.service)
        });
        editor.update(&mut visual, |view, cx| {
            view.receive(undo);
            cx.notify();
        });
    }
    editor.read_with(&visual, |view, _| {
        let mut restored = view.state.as_ref().unwrap().project.snapshot();
        restored.next_id = before_cut.next_id;
        assert_eq!(restored, before_cut);
    });
}

#[gpui::test]
fn section_and_empty_lane_menus_cut_copy_paste_and_project_replacement_clears_clipboard(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, _) = open(cx);
    click_header(&editor, &mut visual, 2);

    // Right-click section to Cut via context menu.
    let at_section = editor.read_with(&visual, |view, _| position(view, 2));
    visual.simulate_mouse_down(at_section, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at_section, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let cut_index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| item.label == "cut section · cmd-x")
            .unwrap()
    });
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    let cut_item = point(
        panel.left() + px(20.0),
        panel.top() + px(14.0 + cut_index as f32 * 24.0),
    );
    visual.simulate_mouse_down(cut_item, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(cut_item, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    acknowledge(&editor, &mut visual);

    // Right-click empty space in the lower body at beat 10 to place cursor and Paste.
    let at_empty = editor.read_with(&visual, |view, _| {
        let base = position(view, 10);
        let [_, top, _, height] = view.section_layout().unwrap();
        point(base.x, px(top + height * 0.75))
    });
    visual.simulate_mouse_down(at_empty, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(at_empty, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let paste_index = editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.sections.cursor.unwrap().beat,
            BeatTime::from_quarters(10).unwrap()
        );
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| item.label == "paste section at cursor · cmd-v")
            .unwrap()
    });
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    let paste_item = point(
        panel.left() + px(20.0),
        panel.top() + px(14.0 + paste_index as f32 * 24.0),
    );
    visual.simulate_mouse_down(paste_item, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(paste_item, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(matches!(
            view.sections.pending.as_ref().map(|p| &p.command),
            Some(Command::PasteArrangementSection { at, .. })
                if *at == BeatTime::from_quarters(10).unwrap()
        ));
    });
    acknowledge(&editor, &mut visual);

    // Opening a new project clears the section clipboard.
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
