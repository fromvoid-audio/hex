//! Exercise keyboard paste through the real worker and a delayed UI terminal.

use super::*;
use crate::editor::{
    drag::group_drag_ui_test::{click, fixture, pump},
    edit_cursor::EditCursor,
};
use gpui::{TestAppContext, VisualTestContext};
use hex_project::Created;
use hex_session::service::{ArrangementEditViewKind, ArrangementEditViewObservation, Request};
use std::time::{Duration, Instant};

#[gpui::test]
fn unread_first_paste_view_coalesces_without_flashing_back(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    visual.simulate_keystrokes("cmd-c");
    pump(&editor, &mut visual);
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let mut outcomes = Vec::new();
    for frame in [72_000, 120_000] {
        editor.read_with(&visual, |view, _| {
            view.service
                .submit(Request::PasteTimeRange { track, frame })
                .unwrap()
        });
        let deadline = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
                break event;
            }
            assert!(Instant::now() < deadline, "paste terminal");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(event.error.is_none());
        outcomes.push(event);
    }
    let second = outcomes.pop().unwrap();
    let first = outcomes.pop().unwrap();
    let Created::Clips {
        first: first_clip, ..
    } = first.created.unwrap()
    else {
        panic!("first clip");
    };
    let Created::Clips {
        first: second_clip, ..
    } = second.created.unwrap()
    else {
        panic!("second clip");
    };
    let observation = editor.read_with(&visual, |view, _| {
        view.service.try_arrangement_edit_view().unwrap()
    });
    assert_eq!(
        observation.first_previous_revision + 1,
        first.state.revision
    );
    assert_eq!(observation.previous_revision, first.state.revision);
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_paste_view(observation));
        assert!(Arc::ptr_eq(
            view.arrangement_paste_view().unwrap(),
            &second.state
        ));
        cx.notify();
    });
    visual.run_until_parked();
    let first_selector = Box::leak(format!("clip-name-{}", first_clip.value()).into_boxed_str());
    let second_selector = Box::leak(format!("clip-name-{}", second_clip.value()).into_boxed_str());
    let first_bounds = visual
        .debug_bounds(first_selector)
        .expect("first paste paints");
    let second_bounds = visual
        .debug_bounds(second_selector)
        .expect("second paste paints");
    editor.update(&mut visual, |view, cx| {
        view.receive(first);
        assert!(Arc::ptr_eq(
            view.arrangement_paste_view().unwrap(),
            &second.state
        ));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(first_selector), Some(first_bounds));
    assert_eq!(visual.debug_bounds(second_selector), Some(second_bounds));
    editor.update(&mut visual, |view, cx| {
        view.receive(second);
        assert!(view.arrangement_paste_view().is_none());
        cx.notify();
    });
}

#[gpui::test]
fn successive_committed_pastes_remain_visible_through_the_first_terminal(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    visual.simulate_keystrokes("cmd-c");
    pump(&editor, &mut visual);
    let (track, generation) = editor.read_with(&visual, |view, _| {
        (
            view.state.as_ref().unwrap().project.tracks()[0].id(),
            view.project_generation,
        )
    });
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::PasteTimeRange {
                track,
                frame: 72_000,
            })
            .unwrap()
    });
    let deadline = crate::test_wait::hang_deadline();
    let mut first = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "first paste terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    let first_view = editor.read_with(&visual, |view, _| {
        view.service.try_arrangement_edit_view().unwrap()
    });
    let Created::Clips {
        first: first_clip,
        last,
    } = first.created.unwrap()
    else {
        panic!("first clip");
    };
    assert_eq!(first_clip, last);
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_paste_view(first_view));
        cx.notify();
    });
    visual.run_until_parked();
    let first_selector = Box::leak(format!("clip-name-{}", first_clip.value()).into_boxed_str());
    assert!(visual.debug_bounds(first_selector).is_some());
    click(&mut visual, originals[1], false);
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::PasteTimeRange {
                track,
                frame: 120_000,
            })
            .unwrap()
    });
    let deadline = crate::test_wait::hang_deadline();
    let second = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "second paste terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    let second_view = editor.read_with(&visual, |view, _| {
        view.service.try_arrangement_edit_view().unwrap()
    });
    let Created::Clips {
        first: second_clip,
        last,
    } = second.created.unwrap()
    else {
        panic!("second clip");
    };
    assert_eq!(second_clip, last);
    let second_revision = second.state.revision;
    editor.update(&mut visual, |view, cx| {
        let invalid = |generation, previous_revision| ArrangementEditViewObservation {
            project_generation: generation,
            first_previous_revision: previous_revision,
            previous_revision,
            kind: ArrangementEditViewKind::Paste,
            state: Arc::downgrade(&second.state),
        };
        assert!(!view.observe_paste_view(invalid(generation + 1, first.state.revision)));
        assert!(!view.observe_paste_view(invalid(generation, first.state.revision + 1)));
        assert!(view.observe_paste_view(second_view));
        cx.notify();
    });
    visual.run_until_parked();
    let second_selector = Box::leak(format!("clip-name-{}", second_clip.value()).into_boxed_str());
    let first_bounds = visual
        .debug_bounds(first_selector)
        .expect("first paste paints");
    let second_bounds = visual
        .debug_bounds(second_selector)
        .expect("second paste paints");
    // A recovery refusal can accompany an already committed first paste.
    first.error = Some("automatic recovery: fixture conflict".into());
    editor.update(&mut visual, |view, cx| {
        view.receive(first);
        assert!(Arc::ptr_eq(
            view.arrangement_paste_view().unwrap(),
            &second.state
        ));
        assert_eq!(view.selected_clip, Some(originals[1]));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(first_selector), Some(first_bounds));
    assert_eq!(visual.debug_bounds(second_selector), Some(second_bounds));
    editor.update(&mut visual, |view, cx| {
        view.receive(second);
        assert!(view.arrangement_paste_view().is_none());
        assert_eq!(view.selected_clip, Some(originals[1]));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(second_selector), Some(second_bounds));

    // A source replacement retires a retained view and rejects old observations.
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::PasteTimeRange {
                track,
                frame: 168_000,
            })
            .unwrap()
    });
    let deadline = crate::test_wait::hang_deadline();
    let third = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "third paste terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    let third_view = editor.read_with(&visual, |view, _| {
        view.service.try_arrangement_edit_view().unwrap()
    });
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_paste_view(third_view));
        cx.notify();
    });
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap()
    });
    let deadline = crate::test_wait::hang_deadline();
    let replacement = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "replacement terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(replacement.project_replaced);
    editor.update(&mut visual, |view, cx| {
        view.receive(replacement);
        assert!(view.arrangement_paste_view().is_none());
        assert!(!view.observe_paste_view(ArrangementEditViewObservation {
            project_generation: generation,
            first_previous_revision: second_revision,
            previous_revision: second_revision,
            kind: ArrangementEditViewKind::Paste,
            state: Arc::downgrade(&third.state),
        }));
        cx.notify();
    });
}

#[gpui::test]
fn committed_paste_paints_before_ack_and_preserves_a_later_selection(cx: &mut TestAppContext) {
    let (_files, handle, editor, originals) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, originals[0], false);
    visual.simulate_keystrokes("cmd-c");
    pump(&editor, &mut visual);

    editor.update(&mut visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        view.time_selection = None;
        view.edit_cursor = Some(EditCursor {
            track: Some(state.project.tracks()[0].id()),
            frame: 72_000,
            generation: view.project_generation,
            pulse_color: 0,
            pulse_started: Instant::now(),
        });
    });
    visual.simulate_keystrokes("cmd-v");
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "paste terminal");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(terminal.request_complete && terminal.error.is_none());
    let Some(Created::Clips {
        first: pasted,
        last,
    }) = terminal.created
    else {
        panic!("pasted a single clip");
    };
    assert_eq!(pasted, last);
    let selector = Box::leak(format!("clip-name-{}", pasted.value()).into_boxed_str());
    assert!(visual.debug_bounds(selector).is_none());

    editor.update(&mut visual, |view, cx| {
        assert!(view.tick(cx));
        assert!(Arc::ptr_eq(
            view.arrangement_paste_view().unwrap(),
            &terminal.state
        ));
        assert!(view.state.as_ref().unwrap().project.clip(pasted).is_none());
        assert_eq!(view.selected_clip, Some(originals[0]));
        cx.notify();
    });
    visual.run_until_parked();
    let before_ack = visual
        .debug_bounds(selector)
        .expect("paste paints with held terminal");

    click(&mut visual, originals[1], false);
    assert_eq!(visual.debug_bounds(selector), Some(before_ack));
    editor.update(&mut visual, |view, cx| {
        assert_eq!(view.selected_clip, Some(originals[1]));
        view.receive(terminal);
        assert!(view.arrangement_paste_view().is_none());
        assert_eq!(view.selected_clip, Some(originals[1]));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector), Some(before_ack));

    editor.update(&mut visual, |view, _| {
        view.edit_cursor.as_mut().unwrap().frame = 120_000;
    });
    visual.simulate_keystrokes("cmd-v");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let track = &view.state.as_ref().unwrap().project.tracks()[0];
        let new = track
            .clips()
            .iter()
            .find(|clip| clip.start() == 120_000)
            .unwrap();
        assert_eq!(view.selected_clip, Some(new.id()));
        assert_eq!(view.selected_clip_ids(), vec![new.id()]);
    });
}
