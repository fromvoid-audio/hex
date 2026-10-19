//! Real focused key dispatch and canonical Service tempo edits.
use super::*;
use crate::editor::{bind_editor_keys, test_environment::Environment};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, size};
use hex_session::service::Service;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(Instant::now() < until, "real Service did not settle");
        std::thread::yield_now();
    }
}

fn fixture(cx: &mut TestAppContext) -> (VisualTestContext, Entity<EditorView>, Arc<Environment>) {
    cx.update(bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1280.), px(800.)));
    pump(&editor, &mut visual);
    (visual, editor, environment)
}

fn begin(visual: &mut VisualTestContext, selector: &'static str) {
    visual.run_until_parked();
    let bounds = visual.debug_bounds(selector).expect("rendered BPM field");
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn focused_project_arrows_adjust_local_draft_then_commit_exact_micro_bpm(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment) = fixture(cx);
    begin(&mut visual, "project-tempo-value");
    visual.simulate_keystrokes("up");
    editor.read_with(&visual, |view, cx| {
        let numeric = view.project_tempo_control.as_ref().unwrap().entity.read(cx);
        assert_eq!(
            numeric.display_text(),
            "121",
            "Up changes the focused BPM draft by one"
        );
        assert!(numeric.is_editing());
        assert!(view.last_enqueued_edit.is_none(), "arrow draft stays local");
    });
    visual.simulate_keystrokes("cmd-down cmd-down cmd-up down");
    visual.simulate_keystrokes("enter");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            119_900_000
        );
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Undo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            120_000_000
        )
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Redo));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .tempo_at(BeatTime::ZERO)
                .micro_bpm(),
            119_900_000
        )
    });
}
