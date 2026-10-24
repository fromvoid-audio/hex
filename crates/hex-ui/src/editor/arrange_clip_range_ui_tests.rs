use super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use super::*;
use gpui::{
    Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, ClipId, Command, Project, TrackId};
use std::sync::Arc;

fn setup(
    cx: &mut TestAppContext,
    midi: bool,
    configure: impl FnOnce(&mut hex_project::Editor, ClipId),
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
    Project,
) {
    let (project, track, _, clip) = if midi {
        midi_project()
    } else {
        audio_project()
    };
    let mut model = hex_project::Editor::new(project, 16).unwrap();
    configure(&mut model, clip);
    let project = model.project().clone();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    (handle, editor, track, clip, project)
}
fn prepare(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    track: TrackId,
    clip: ClipId,
) {
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.clip_view_open = true;
            view.device_panel_active = false;
            view.viewport = Some(Viewport {
                start: 0,
                length: 384_000,
            });
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
}
fn at(
    editor: &Entity<EditorView>,
    visual: &VisualTestContext,
    track: TrackId,
    frame: u64,
) -> Point<Pixels> {
    let lane = editor.read_with(visual, |view, _| view.drag_lanes[&track]);
    point(
        px(lane.left + lane.width * frame as f32 / 384_000.0),
        px(lane.top + lane.height * 0.75),
    )
}
fn viewport(
    editor: &Entity<EditorView>,
    visual: &VisualTestContext,
    clip: ClipId,
    midi: bool,
) -> (u64, u64) {
    editor.read_with(visual, |view, _| {
        if midi {
            (view.piano_beat_start.raw(), view.piano_beat_span.raw())
        } else {
            let root = view
                .state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root();
            let range = view.event_viewport(&root);
            (range.start, range.length)
        }
    })
}
fn unchanged(editor: &Entity<EditorView>, visual: &VisualTestContext, project: &Project) {
    editor.read_with(visual, |view, _| {
        assert_eq!(&*view.state.as_ref().unwrap().project, project);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn arrange_range_live_highlight_fit_reverse_and_back_follow_audio_and_midi(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    for midi in [false, true] {
        let (handle, editor, track, clip, project) = setup(cx, midi, |_, _| {});
        let visual = &mut VisualTestContext::from_window(handle, cx);
        prepare(&editor, visual, track, clip);
        let original = viewport(&editor, visual, clip, midi);
        let down = at(&editor, visual, track, 24_000);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let end = at(&editor, visual, track, 72_000);
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        editor.read_with(visual, |view, _| {
            let range = view.normalized_time_selection().unwrap();
            assert_eq!((range.start(), range.end()), (24_000, 72_000));
            assert_eq!(view.detail_clip_id(), Some(clip));
        });
        let highlight = visual
            .debug_bounds("arrange-reflected-range-0")
            .expect("reflected source highlight during Arrange drag");
        let body = visual
            .debug_bounds(if midi {
                "midi-note-roll"
            } else {
                "audio-events-waveform"
            })
            .unwrap();
        assert!((highlight.origin.x - body.origin.x).abs() <= px(2.0));
        assert!((highlight.size.width - body.size.width).abs() <= px(2.0));
        assert_eq!(
            viewport(&editor, visual, clip, midi),
            if midi {
                (0, BeatTime::from_quarters(2).unwrap().raw())
            } else {
                (24_000, 48_000)
            }
        );
        let shorter = at(&editor, visual, track, 48_000);
        visual.simulate_mouse_move(shorter, MouseButton::Left, Modifiers::default());
        assert_eq!(
            viewport(&editor, visual, clip, midi),
            if midi {
                (0, BeatTime::ONE.raw())
            } else {
                (24_000, 24_000)
            }
        );
        let reverse = at(&editor, visual, track, 12_000);
        visual.simulate_mouse_move(reverse, MouseButton::Left, Modifiers::default());
        if midi {
            assert!(
                visual.debug_bounds("arrange-reflected-range-0").is_none(),
                "nonoverlap clears without fabricating source media"
            );
            assert_eq!(
                viewport(&editor, visual, clip, midi),
                (0, BeatTime::ONE.raw())
            );
        } else {
            assert_eq!(viewport(&editor, visual, clip, midi), (12_000, 12_000));
        }
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        if midi {
            editor.update(visual, |view, cx| {
                view.mode = EditorMode::Session;
                cx.notify();
            });
            visual.run_until_parked();
            editor.update(visual, |view, cx| {
                view.mode = EditorMode::Arrangement;
                cx.notify();
            });
            visual.run_until_parked();
            assert_eq!(
                viewport(&editor, visual, clip, midi),
                (0, BeatTime::from_quarters(2).unwrap().raw()),
                "reinitialized piano owner must refit even if the range identity returns unchanged"
            );
        }
        visual.update(|window, cx| {
            let view = editor.read(cx);
            if midi {
                view.midi_focus.focus(window);
            } else {
                view.audio_event_focus.focus(window);
            }
        });
        visual.simulate_keystrokes("x");
        assert_eq!(
            viewport(&editor, visual, clip, midi),
            original,
            "one Back restores the view before the whole drag"
        );
        visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window));
        visual.simulate_input("query");
        assert_eq!(viewport(&editor, visual, clip, midi), original);
        assert!(visual.debug_bounds("arrange-reflected-range-0").is_none());
        unchanged(&editor, visual, &project);
    }
}

#[gpui::test]
fn arrange_range_cancel_restores_view_and_closed_or_device_panes_keep_user_intent(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    for midi in [false, true] {
        for mode in [0, 1, 2] {
            let (handle, editor, track, clip, project) = setup(cx, midi, |_, _| {});
            let visual = &mut VisualTestContext::from_window(handle, cx);
            prepare(&editor, visual, track, clip);
            let original = viewport(&editor, visual, clip, midi);
            if mode != 0 {
                let button = visual
                    .debug_bounds(if mode == 1 {
                        "clip-view-toggle"
                    } else {
                        "device-detail-tab"
                    })
                    .unwrap()
                    .center();
                visual.simulate_click(button, Modifiers::default());
            }
            let down = at(&editor, visual, track, 48_000);
            let end = at(&editor, visual, track, 96_000);
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            if mode == 0 {
                assert_ne!(viewport(&editor, visual, clip, midi), original);
                visual.simulate_keystrokes("escape");
                assert_eq!(viewport(&editor, visual, clip, midi), original);
                assert!(visual.debug_bounds("arrange-reflected-range-0").is_none());
            } else {
                visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
                assert!(visual.debug_bounds("arrange-reflected-range-0").is_none());
                assert_eq!(viewport(&editor, visual, clip, midi), original);
                editor.read_with(visual, |view, _| {
                    assert_eq!(view.clip_view_open, mode != 1);
                    assert_eq!(view.device_panel_active, mode == 2);
                });
            }
            unchanged(&editor, visual, &project);
        }
    }
}

#[gpui::test]
fn arrange_range_loop_wrap_paints_two_source_spans_and_retains_back_from_noop_fit(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (handle, editor, track, clip, project) = setup(cx, false, |model, clip| {
        model
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 192_000,
                },
            })
            .unwrap();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    prepare(&editor, visual, track, clip);
    let original = viewport(&editor, visual, clip, false);
    let down = at(&editor, visual, track, 72_000);
    let end = at(&editor, visual, track, 120_000);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    for name in ["arrange-reflected-range-0", "arrange-reflected-range-1"] {
        let highlight = visual.debug_bounds(name).unwrap();
        assert!((highlight.size.width - body.size.width / 4.0).abs() <= px(2.0));
    }
    assert_eq!(viewport(&editor, visual, clip, false), original);
    let shorter = at(&editor, visual, track, 96_000);
    visual.simulate_mouse_move(shorter, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(shorter, MouseButton::Left, Modifiers::default());
    assert_eq!(viewport(&editor, visual, clip, false), (72_000, 24_000));
    visual.update(|window, cx| editor.read(cx).audio_event_focus.focus(window));
    visual.simulate_keystrokes("x");
    assert_eq!(viewport(&editor, visual, clip, false), original);
    unchanged(&editor, visual, &project);
}

#[gpui::test]
fn arrange_range_audio_crop_warp_and_generation_replacement_use_actual_source_view(
    cx: &mut TestAppContext,
) {
    for warp in [false, true] {
        let (handle, editor, track, clip, project) = setup(cx, false, |model, clip| {
            if warp {
                let settings = hex_project::default_audio_warp(
                    model.project().clip(clip).unwrap(),
                    model.project().tempo_map(),
                    hex_project::WarpMode::Tones { grain_ms: 32 },
                )
                .unwrap();
                model
                    .execute(Command::SetAudioClipWarp {
                        clip,
                        settings: Some(settings),
                    })
                    .unwrap();
            }
            model
                .execute(Command::TrimClipEdge {
                    clip,
                    edge: hex_project::TrimEdge::Left,
                    frame: 24_000,
                })
                .unwrap();
        });
        let visual = &mut VisualTestContext::from_window(handle, cx);
        prepare(&editor, visual, track, clip);
        let down = at(&editor, visual, track, 48_000);
        let end = at(&editor, visual, track, 72_000);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        assert_eq!(viewport(&editor, visual, clip, false), (24_000, 24_000));
        assert!(visual.debug_bounds("arrange-reflected-range-0").is_some());
        unchanged(&editor, visual, &project);
        editor.update(visual, |view, cx| {
            view.project_generation += 1;
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("arrange-reflected-range-0").is_none(),
            "old document range cannot reattach to reused clip ids"
        );
    }
}
