//! Native header-key routing through a disposable canonical worker/history.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::service::{Event, Service};
use std::time::Duration;
fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            !view.edit_pending()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn header(visual: &mut VisualTestContext, track: TrackId, modifiers: Modifiers) {
    let b = visual
        .debug_bounds(Box::leak(
            format!("track-name-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    // The compact mixer now occupies the old top-right header coordinate.
    // Select through the actual track-name region rather than a child control.
    let p = b.center();
    visual.simulate_mouse_down(p, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(p, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn header_backspace_removes_one_track_and_restores_contents_in_one_undo(cx: &mut TestAppContext) {
    header_backspace_removes_tracks(cx, false, "backspace");
}

#[gpui::test]
fn header_backspace_removes_multiple_tracks_and_restores_contents_in_one_undo(
    cx: &mut TestAppContext,
) {
    header_backspace_removes_tracks(cx, true, "backspace");
}

#[gpui::test]
fn header_x_removes_selected_tracks_and_cmd_z_restores_them(cx: &mut TestAppContext) {
    header_backspace_removes_tracks(cx, true, "x");
}

fn header_backspace_removes_tracks(cx: &mut TestAppContext, multiple: bool, key: &str) {
    cx.update(crate::editor::bind_editor_keys);
    let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("delete.wav", 48_000, 48_000);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let imported = next(&service);
    let audio = imported.state.project.tracks()[0].id();
    service
        .submit(Request::AddNativeDevice {
            track: audio,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(
                hex_engine::native_device_catalog()
                    .unwrap()
                    .remove(0)
                    .type_id,
            ),
            before: None,
        })
        .unwrap();
    let with_device = next(&service);
    let device = with_device.state.project.track(audio).unwrap().devices()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "midi".into(),
        }))
        .unwrap();
    let event = next(&service);
    let midi = event.state.project.tracks()[1].id();
    let before = event.state.project.snapshot();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline delete test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(event.state);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    header(&mut visual, audio, Modifiers::default());
    // A real device numeric field takes focus from the selected header.
    let numeric = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .expect("utility gain numeric field");
    visual.simulate_click(numeric.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("12");
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(
            !view.edit_pending(),
            "numeric Backspace must not submit a track edit"
        );
    });
    visual.simulate_keystrokes("escape");
    header(&mut visual, audio, Modifiers::default());
    if multiple {
        header(
            &mut visual,
            midi,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
    }
    if key == "x" {
        editor.update(&mut visual, |view, _| view.close_prompt_open = true);
        visual.simulate_keystrokes(key);
        editor.read_with(&visual, |view, _| {
            assert!(view.last_enqueued_edit.is_none())
        });
        editor.update(&mut visual, |view, _| view.close_prompt_open = false);
    }
    visual.simulate_keystrokes(key);
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |view, _| {
        let p = &view.state.as_ref().unwrap().project;
        assert!(
            p.track(audio).is_none(),
            "header Backspace removes the selected track"
        );
        assert_eq!(p.track(midi).is_none(), multiple);
        assert!(view.selected_track_ids().is_empty());
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            before,
            "one Undo restores every removed track and its contents"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn header_backspace_respects_clip_rename_browser_and_empty_selection(cx: &mut TestAppContext) {
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use std::sync::Arc;
    cx.update(crate::editor::bind_editor_keys);
    let (project, track, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // With root focus and no object selection Backspace leaves regular tracks
    // and the protected Main strip intact.
    visual.simulate_keystrokes("backspace");
    assert!(visual.debug_bounds("main-track-header").is_some());
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().project.track(track).is_some());
        assert!(view.last_enqueued_edit.is_none());
    });
    // Platform-toggle off the only selected header: focus alone cannot delete.
    header(&mut visual, track, Modifiers::default());
    header(
        &mut visual,
        track,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |view, _| {
        assert!(view.selected_track_ids().is_empty());
        assert!(view.last_enqueued_edit.is_none());
    });
    header(&mut visual, track, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.simulate_input("draft");
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |view, cx| {
        assert!(view.track_name.edit.is_some());
        assert_eq!(
            view.track_name.edit.as_ref().unwrap().input.read(cx).text(),
            "draf"
        );
        assert!(view.last_enqueued_edit.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
    });
    visual.simulate_input("abc");
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "ab");
        assert!(view.last_enqueued_edit.is_none());
    });
    let b = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(b.center(), Modifiers::default());
    visual.simulate_keystrokes("backspace");
    editor.read_with(&visual, |view, _| {
        assert!(
            matches!(view.last_enqueued_edit,Some(Command::RemoveClip(id)) if id==clip),
            "clip focus keeps clip deletion"
        );
    });
}

#[gpui::test]
fn toolbar_remove_keeps_single_track_semantics_after_header_selection_toggle(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    for retained_focus in [false, true] {
        for toggled_out in [true, false] {
            let service = Service::new(48_000, 32, 8_000_000).unwrap();
            next(&service);
            for name in ["first", "focused"] {
                service
                    .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                    .unwrap();
                if name == "first" {
                    next(&service);
                }
            }
            let event = next(&service);
            let first = event.state.project.tracks()[0].id();
            let focused = event.state.project.tracks()[1].id();
            let before = event.state.project.snapshot();
            let handle = *cx.add_window(|_, _| gpui::Empty);
            let editor = cx
                .update_window(handle, |_, window, cx| {
                    let editor = window.replace_root(cx, |window, cx| {
                        EditorView::new(window, cx, service, Err("offline toolbar test".into()))
                    });
                    editor.update(cx, |view, cx| {
                        view.state = Some(event.state);
                        cx.notify();
                    });
                    editor
                })
                .unwrap();
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            header(&mut visual, first, Modifiers::default());
            let additive = Modifiers {
                platform: true,
                ..Default::default()
            };
            header(&mut visual, focused, additive);
            if toggled_out {
                header(&mut visual, focused, additive);
                visual.simulate_keystrokes("backspace");
                editor.read_with(&visual, |view, _| {
                    assert_eq!(view.selected_track_ids(), vec![first]);
                    assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                    assert!(
                        !view.edit_pending(),
                        "toggled-out header Backspace stays a no-op"
                    );
                });
            }
            visual.update(|window, cx| {
                editor.read_with(cx, |view, _| {
                    assert!(view.track_header_focused(window));
                    assert!(
                        view.track_name.focuses[&super::Target::Track(focused)].is_focused(window)
                    );
                    assert_eq!(view.selected_track, Some(focused));
                })
            });
            let button = visual
                .debug_bounds("remove selected track")
                .expect("actual toolbar remove control");
            visual.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::default());
            // Ordinary GPUI clicks transfer focus to the root. Also exercise the
            // same real button with header focus retained at dispatch; no native
            // focus behavior is inferred from this controlled test condition.
            if retained_focus {
                visual.update(|window, cx| {
                    editor.read_with(cx, |view, _| {
                        view.track_name.focuses[&super::Target::Track(focused)].focus(window)
                    })
                });
            }
            visual.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::default());
            visual.update(|window, cx| {
                editor.read_with(cx, |view, _| {
                    eprintln!(
                        "toolbar retained_focus={retained_focus} toggled_out={toggled_out} immediate_focus={} pending={} edit={:?}",
                        view.track_name.focuses[&super::Target::Track(focused)].is_focused(window),
                        view.edit_pending(),
                        view.last_enqueued_edit
                    );
                })
            });
            pump(&editor, &mut visual);
            let after = editor.read_with(&visual, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                assert!(
                    project.track(focused).is_none(),
                    "toolbar removes focused single track even after toggling it out"
                );
                assert!(
                    project.track(first).is_some(),
                    "toolbar does not inherit multi-header deletion"
                );
                project.snapshot()
            });
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
            });
            visual.simulate_keystrokes("cmd-shift-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
            });
        }
    }
}

#[gpui::test]
fn header_forward_delete_removes_track_and_keeps_previously_selected_clip(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("forward.wav", 48_000, 48_000);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav)).unwrap();
    let imported = next(&service);
    let audio = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.track(audio).unwrap().clips()[0].id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "target".into(),
        }))
        .unwrap();
    let event = next(&service);
    let target = event.state.project.tracks()[1].id();
    let before = event.state.project.snapshot();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(
                    window,
                    cx,
                    service,
                    Err("offline forward delete test".into()),
                )
            });
            editor.update(cx, |view, cx| {
                view.state = Some(event.state);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let name = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(name.center(), Modifiers::default());
    header(&mut visual, target, Modifiers::default());
    // Clip selection can outlive a header focus change (clip navigation and
    // acknowledgement reconciliation restore it), so hold that hazard state.
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            assert!(view.track_header_focused(window));
            view.selected_clip = Some(clip);
        })
    });
    // A routed DeleteClip from the focused header never removes the clip.
    visual.update(|window, cx| window.dispatch_action(Box::new(crate::editor::DeleteClip), cx));
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.last_enqueued_edit.is_none(),
            "header-focused DeleteClip must not remove the selected clip"
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_keystrokes("delete");
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |view, _| {
        let p = &view.state.as_ref().unwrap().project;
        assert!(p.track(target).is_none(), "header Delete removes the track");
        assert!(
            p.track(audio)
                .is_some_and(|t| t.clips().iter().any(|c| c.id() == clip)),
            "header Delete must not remove the previously selected clip"
        );
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
}
