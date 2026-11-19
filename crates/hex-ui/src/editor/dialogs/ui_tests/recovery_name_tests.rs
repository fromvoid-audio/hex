use super::*;
use hex_session::service::Request;

#[gpui::test]
fn manual_recovery_cannot_consume_selected_open_before_reconcile(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let injected = Rc::new(Cell::new(false));
    let seen = injected.clone();
    let chosen = path.clone();
    let _subscription = cx.update(|cx| {
        cx.observe(&editor, move |editor, cx| {
            if !editor.read_with(cx, |view, _| {
                view.pending_open_path.as_ref() == Some(&chosen)
            }) || seen.replace(true)
            {
                return;
            }
            cx.update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| {
                    for kind in [
                        crate::editor::project_recovery::OpenKind::Relink,
                        crate::editor::project_recovery::OpenKind::Untitled,
                        crate::editor::project_recovery::OpenKind::SavedEdits,
                        crate::editor::project_recovery::OpenKind::PreviousSave,
                    ] {
                        view.project_recovery_dialog(kind, window, cx);
                        assert_eq!(
                            view.pending_open_path.as_ref(),
                            Some(&chosen),
                            "manual recovery must not steal a selected Open"
                        );
                        assert!(!view.opening_project);
                    }
                })
            })
            .unwrap();
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path.clone());
    assert!(injected.get());
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("don't save");
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, target)
    });
}

fn draft_name(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    clip: bool,
) -> hex_project::TrackId {
    let track = editor.read_with(visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            if clip {
                view.selected_clip = Some(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(track)
                        .unwrap()
                        .midi_clips()[0]
                        .id(),
                );
                view.focus.focus(window);
            } else {
                view.track_header_focus(track, cx).focus(window);
            }
        })
    });
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(if clip {
                "clip-name-input"
            } else {
                "track-name-input"
            })
            .is_some()
    );
    visual.simulate_input("Uncommitted Name 🎹");
    visual.run_until_parked();
    track
}

fn renamed(project: &hex_project::Project, track: hex_project::TrackId, clip: bool) -> &str {
    let track = project.track(track).unwrap();
    if clip {
        track.midi_clips()[0].name()
    } else {
        track.name()
    }
}

fn rename_open(cx: &mut TestAppContext, clip: bool, answer: &str, initially_clean: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    if clip {
        editor.update(cx, |view, _| {
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            assert!(
                view.enqueue(Request::Edit(hex_project::Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::ONE,
                }))
            );
        });
        pump(&editor, cx, settled);
    }
    let saved = directory.0.join("current.hex");
    if initially_clean {
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::SaveNew(saved.clone())))
        });
        pump(&editor, cx, |view| {
            settled(view) && !view.state.as_ref().unwrap().unsaved
        });
    }
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let track = draft_name(&editor, &mut visual, clip);
    // File action while logical text focus remains in the actual inline editor.
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(Some(vec![path]));
    visual.run_until_parked();
    pump(&editor, cx, settled);
    visual.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "uncommitted name must participate in Save / Don't Save / Cancel"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(view.project_generation, generation);
        assert_eq!(
            renamed(&view.state.as_ref().unwrap().project, track, clip),
            "Uncommitted Name 🎹"
        );
    });
    cx.simulate_prompt_answer(if answer.starts_with("save as") {
        "save"
    } else {
        answer
    });
    visual.run_until_parked();
    if answer.starts_with("save as") {
        if answer == "save as failure" {
            std::fs::create_dir(directory.0.join("save-refused.hex")).unwrap();
        }
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| {
            if answer == "save as cancel" {
                None
            } else {
                Some(directory.0.join("save-refused.hex"))
            }
        });
        visual.run_until_parked();
        pump(&editor, cx, |view| {
            settled(view) && !view.project_open.busy()
        });
        if answer == "save as failure" {
            editor.read_with(cx, |view, _| assert!(view.error.is_some()));
        }
    }
    if answer == "cancel" || answer.starts_with("save as") {
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation);
            assert_eq!(
                renamed(&view.state.as_ref().unwrap().project, track, clip),
                "Uncommitted Name 🎹"
            );
            assert!(view.state.as_ref().unwrap().unsaved);
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, cx, settled);
        editor.read_with(cx, |view, _| {
            assert_ne!(
                renamed(&view.state.as_ref().unwrap().project, track, clip),
                "Uncommitted Name 🎹"
            )
        });
    } else {
        if answer == "save" && !initially_clean {
            assert!(cx.did_prompt_for_new_path());
            cx.simulate_new_path_selection(|_| Some(saved.clone()));
            visual.run_until_parked();
        }
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 1
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, target)
        });
        if answer == "save" {
            let saved = if initially_clean {
                saved
            } else {
                saved.with_extension("").join("current.hex")
            };
            select_open(&mut visual, cx, saved);
            pump(&editor, cx, |view| {
                settled(view) && view.project_generation == generation + 2
            });
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    renamed(&view.state.as_ref().unwrap().project, track, clip),
                    "Uncommitted Name 🎹"
                )
            });
        }
    }
}

#[gpui::test]
fn clean_track_rename_is_finalized_before_open_classification(cx: &mut TestAppContext) {
    rename_open(cx, false, "cancel", true);
}
#[gpui::test]
fn dirty_clip_rename_is_saved_by_original_open_intent(cx: &mut TestAppContext) {
    rename_open(cx, true, "save", false);
}

#[gpui::test]
fn clean_track_rename_save_persists_before_open(cx: &mut TestAppContext) {
    rename_open(cx, false, "save", true);
}
#[gpui::test]
fn clean_clip_rename_dont_save_replaces_once(cx: &mut TestAppContext) {
    rename_open(cx, true, "don't save", true);
}
#[gpui::test]
fn dirty_track_rename_cancel_remains_undoable(cx: &mut TestAppContext) {
    rename_open(cx, false, "cancel", false);
}
#[gpui::test]
fn dirty_track_rename_dont_save_replaces_once(cx: &mut TestAppContext) {
    rename_open(cx, false, "don't save", false);
}
#[gpui::test]
fn clip_rename_survives_open_save_as_cancel(cx: &mut TestAppContext) {
    rename_open(cx, true, "save as cancel", false);
}
#[gpui::test]
fn track_rename_survives_open_save_failure(cx: &mut TestAppContext) {
    rename_open(cx, false, "save as failure", false);
}
#[gpui::test]
fn invalid_name_refuses_open_without_consuming_draft(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    draft_name(&editor, &mut visual, false);
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("   ");
    visual.dispatch_action(OpenProject);
    assert!(!cx.did_prompt_for_paths());
    assert!(visual.debug_bounds("track-name-input").is_some());
    editor.read_with(cx, |view, _| {
        assert!(view.track_name.is_editing());
        assert_eq!(view.state.as_ref().unwrap().project, before);
        assert!(!view.project_open.busy());
    });
    visual.simulate_keystrokes("escape");
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.project_open.busy()));
}

#[gpui::test]
fn recovery_actions_refuse_while_open_picker_is_active(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    let actions: Vec<Box<dyn gpui::Action>> = vec![
        Box::new(crate::editor::OpenRelinkedProject),
        Box::new(crate::editor::RecoverUntitledProject),
        Box::new(crate::editor::RecoverSavedProject),
        Box::new(crate::editor::RecoverPreviousSave),
    ];
    for action in actions {
        visual.update(|window, cx| window.dispatch_action(action, cx));
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.project_open.busy());
            assert!(!view.opening_project);
            assert_eq!(
                view.error.as_deref(),
                Some("finish the current project operation first")
            );
        });
        assert!(!cx.has_pending_prompt());
        assert!(
            cx.did_prompt_for_paths(),
            "the original picker must remain pending"
        );
    }
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.project_open.busy()));
}
