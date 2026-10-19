//! Current-source input/Service/late-frame diagnostics for pulse2880.
use super::*;
use crate::editor::test_environment::Environment;
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{BeatTime, ClipId};
use hex_session::service::Service;
use std::{sync::Arc, time::Duration};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            let changed =
                view.tick(cx) | view.tick_edit_cursor(Instant::now() + Duration::from_millis(16));
            if changed {
                cx.notify();
            }
            !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(Instant::now() < until, "actual Service never settled");
        std::thread::yield_now();
    }
    for _ in 0..8 {
        editor.update(&mut *visual, |view, cx| {
            let _ = view.tick(cx);
            let _ = view.tick_edit_cursor(Instant::now() + Duration::from_millis(16));
            cx.notify();
        });
        visual.run_until_parked();
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Arc<Environment>,
    Vec<TrackId>,
    Vec<ClipId>,
) {
    cx.update(crate::editor::bind_editor_keys);
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
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        editor.update(&mut visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        if editor.read_with(&visual, |view, _| view.state.is_some()) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    let mut tracks = Vec::new();
    let mut clips = Vec::new();
    for index in 0..2 {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
                name: format!("range {index}")
            })))
        });
        pump(&editor, &mut visual);
        let track = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[index].id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(4 << 32)
            })))
        });
        pump(&editor, &mut visual);
        let clip = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(2 << 32),
                pitch: 60 + index as u8,
                channel: 0,
                velocity: 100
            })))
        });
        pump(&editor, &mut visual);
        tracks.push(track);
        clips.push(clip);
    }
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(Arc::ptr_eq(&environment, &view.test_environment));
            view.viewport = Some(super::super::viewport::Viewport {
                start: 0,
                length: 384_000,
            });
            view.snap_grid = super::super::musical_time::SnapGrid::Off;
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    (visual, editor, environment, tracks, clips)
}
fn lane(visual: &mut VisualTestContext, track: TrackId) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .expect("actual rendered lane")
}
fn pointer_range(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    tracks: &[TrackId],
) -> hex_project::TimeRange {
    let first = lane(visual, tracks[0]);
    let last = lane(visual, *tracks.last().unwrap());
    let a = point(
        first.origin.x + first.size.width * (24_000.0 / 384_000.0),
        first.origin.y + first.size.height * 0.7,
    );
    let b = point(
        last.origin.x + last.size.width * (72_000.0 / 384_000.0),
        last.origin.y + last.size.height * 0.7,
    );
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(b, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(b, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        let range = view
            .normalized_time_selection()
            .expect("actual pointer made time range");
        assert_eq!(range.tracks(), tracks);
        assert!(range.duration() > 0);
        range
    })
}
#[gpui::test]
fn real_command_d_three_times_retains_destination_range_after_worker_and_late_ticks(
    cx: &mut TestAppContext,
) {
    for rows in [1, 2] {
        let (mut visual, editor, _environment, tracks, originals) = fixture(cx);
        let mut expected = pointer_range(&mut visual, &editor, &tracks[..rows]);
        let original_project = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for iteration in 0..3 {
            let start = expected.end();
            let end = start + expected.duration();
            visual.simulate_keystrokes("cmd-d");
            pump(&editor, &mut visual);
            expected = editor.read_with(&visual, |view, _| {
                let actual = view
                    .normalized_time_selection()
                    .expect("duplicate retains range");
                assert_eq!(
                    (actual.start(), actual.end()),
                    (start, end),
                    "rows={rows} repetition={iteration}"
                );
                assert_eq!(actual.tracks(), &tracks[..rows]);
                assert!(
                    view.selected_clip.is_none(),
                    "range must not collapse into one clip"
                );
                for (&track, &source) in tracks[..rows].iter().zip(&originals) {
                    let data = view.state.as_ref().unwrap().project.track(track).unwrap();
                    assert!(data.midi_clips().iter().any(|clip| clip.id() == source));
                    assert!(data.midi_clips().iter().any(|clip| {
                        clip.id() != source
                            && view
                                .state
                                .as_ref()
                                .unwrap()
                                .project
                                .tempo_map()
                                .beat_to_frame(clip.start())
                                .unwrap()
                                == start
                    }));
                }
                actual
            });
        }
        for _ in 0..3 {
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |view, _| {
            let mut original = original_project;
            original.next_id = view.state.as_ref().unwrap().project.snapshot().next_id;
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                original,
                "three duplicates use three undo steps"
            );
        });
    }
}
#[gpui::test]
fn real_option_drag_then_command_d_retains_range_after_worker_and_late_ticks(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let original = pointer_range(&mut visual, &editor, &tracks[..1]);
    let row = lane(&mut visual, tracks[0]);
    let grab = original.start() + original.duration() / 2;
    let endframe = grab + original.duration() * 2;
    let a = point(
        row.origin.x + row.size.width * (grab as f32 / 384_000.0),
        row.origin.y + row.size.height * 0.15,
    );
    let b = point(
        row.origin.x + row.size.width * (endframe as f32 / 384_000.0),
        a.y,
    );
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(a, MouseButton::Left, option);
    visual.simulate_mouse_move(b, Some(MouseButton::Left), option);
    let expected = editor.read_with(&visual, |view, _| {
        let preview = view
            .time_range_move_display()
            .expect("actual Option-drag range preview");
        assert!(preview.copy_modifier);
        (preview.destination_start, preview.destination_end)
    });
    visual.simulate_mouse_up(b, MouseButton::Left, option);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let range = view
            .normalized_time_selection()
            .expect("range remains after copy");
        assert_eq!((range.start(), range.end()), expected);
        assert!(view.selected_clip.is_none());
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let range = view.normalized_time_selection().unwrap();
        assert_eq!(
            (range.start(), range.end()),
            (expected.1, expected.1 + expected.1 - expected.0)
        );
    });
}
