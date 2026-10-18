//! Native action dispatch and canonical history for track keyboard shortcuts.
use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{Command, Created, Editor};
use std::sync::Arc;

#[gpui::test]
fn command_t_uses_existing_add_track_and_one_canonical_undo(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, _, _) = audio_project();
    let original = project.clone();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-t");
    let command = editor.read_with(&visual, |view, _| {
        assert!(
            matches!(&view.last_enqueued_edit, Some(Command::AddTrack {name}) if name == "audio 3")
        );
        view.last_enqueued_edit.clone().unwrap()
    });
    let mut canonical = Editor::new(original.clone(), 8).unwrap();
    let Created::Track(id) = canonical.execute(command).unwrap() else {
        panic!("track result")
    };
    assert_eq!(canonical.project().tracks().len(), 3);
    assert_eq!(canonical.project().track(id).unwrap().name(), "audio 3");
    let high_water = canonical.project().snapshot().next_id;
    assert!(canonical.undo());
    let mut expected = original.snapshot();
    expected.next_id = high_water;
    assert_eq!(canonical.project().snapshot(), expected);
    assert!(!canonical.undo());
    assert!(canonical.redo());
    assert_eq!(canonical.project().track(id).unwrap().name(), "audio 3");
}

#[gpui::test]
fn command_t_preserves_browser_and_clip_name_text_focus(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for clip_name in [false, true] {
        let (project, _, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                if clip_name {
                    view.selected_clip = Some(clip);
                    view.begin_clip_name(window, cx);
                } else {
                    view.browser.search.read(cx).focus(window);
                }
            })
        });
        visual.run_until_parked();
        let focus = visual.update(|window, cx| window.focused(cx).unwrap());
        visual.simulate_keystrokes("cmd-t");
        visual.update(|window, _| assert!(focus.is_focused(window)));
        editor.read_with(&visual, |view, _| {
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(view.worker_requests_pending, 0);
        });
    }
}
