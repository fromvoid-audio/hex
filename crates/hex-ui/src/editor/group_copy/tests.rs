//! Exercise keyboard paste through the real worker and a delayed UI terminal.

use super::*;
use crate::editor::{
    drag::group_drag_ui_test::{click, fixture, pump},
    edit_cursor::EditCursor,
};
use gpui::{TestAppContext, VisualTestContext};
use hex_project::Created;
use std::time::{Duration, Instant};

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
