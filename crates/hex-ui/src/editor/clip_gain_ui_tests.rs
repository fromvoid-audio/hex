//! Actual top-edge input; canonical work starts only at release.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use std::sync::Arc;

fn option() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::default()
    }
}

#[gpui::test]
fn g_groups_midi_clips_with_different_legacy_curves(cx: &mut TestAppContext) {
    use super::drag::group_drag_ui_test::{click, fixture, pump};
    use hex_project::{BeatTime, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let (_files, handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
            name: "notes".into(),
        })));
    });
    pump(&editor, &mut visual);
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[1].id()
    });
    let mut clips = Vec::new();
    for (start, gain) in [(BeatTime::ZERO, 0.5), (BeatTime::from_raw(3 << 32), 2.0)] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start,
                length: BeatTime::ONE,
            })));
        });
        pump(&editor, &mut visual);
        let clip = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .last()
                .unwrap()
                .id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(
                view.enqueue(Request::Edit(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: MidiClipVolume::new(
                        vec![MidiClipVolumePoint::linear(
                            BeatTime::ZERO,
                            MidiClipGain::new(gain).unwrap()
                        )],
                        BeatTime::ONE,
                    )
                    .unwrap(),
                }))
            );
        });
        pump(&editor, &mut visual);
        clips.push(clip);
    }
    click(&mut visual, clips[0], false);
    click(&mut visual, clips[1], true);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_clip_ids(), clips)
    });
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let row = view.state.as_ref().unwrap().project.track(track).unwrap();
        assert_eq!(row.midi_clips().len(), 1);
        let volume = row.midi_clips()[0].volume();
        assert_eq!(volume.gain_at(BeatTime::ZERO).linear(), 0.5);
        assert_eq!(volume.gain_at(BeatTime::from_raw(3 << 32)).linear(), 2.0);
        assert!(view.error.is_none(), "{:?}", view.error);
    });
}

#[gpui::test]
fn midi_clip_has_no_audio_gain_handle(cx: &mut TestAppContext) {
    let (mut visual, _editor, _environment, clip) = midi_canvas::creation_tests::fixture(cx);
    let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
    assert!(visual.debug_bounds(selector).is_none());
}

#[gpui::test]
fn plain_audio_top_edge_drag_moves_the_clip_without_changing_gain(cx: &mut TestAppContext) {
    let (project, track, _, clip) = clip_drag_ui_test::audio_project();
    let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.run_until_parked();
    assert_ne!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeUpDown)
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.active_clip_drag.is_some());
        assert!(!view.clip_gain.held());
    });
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let up = point(lane.origin.x + lane.size.width * 0.57, lane.center().y);
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(
            matches!(
                view.last_enqueued_edit,
                Some(Command::MoveClipWithOverwrite { clip: moved, track: target, start })
                    if moved == clip && target == track && start > 0
            ),
            "unexpected release: {:?}, {:?}",
            view.last_enqueued_edit,
            view.error
        );
        assert!(view.displayed_clip_gain(clip).is_none());
    });
}

#[gpui::test]
fn audio_top_edge_gain_changes_only_gain_and_releases_one_command(cx: &mut TestAppContext) {
    let (project, _, _, clip) = clip_drag_ui_test::audio_project();
    let original = project.clip(clip).unwrap().clone();
    let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
    let from = visual
        .debug_bounds(selector)
        .expect("top edge gain handle")
        .center();
    visual.simulate_modifiers_change(option());
    visual.simulate_mouse_move(from, None, option());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeUpDown),
        "gain edge hit cursor"
    );
    visual.simulate_mouse_down(from, MouseButton::Left, option());
    editor.read_with(&visual, |v, _| {
        assert!(v.clip_gain.held(), "gain pointer admitted")
    });
    visual.simulate_modifiers_change(Modifiers::default());
    let to = point(from.x, from.y + px(60.0));
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.clip_gain.held(), "gain pointer survives repaint");
        assert!(v.last_enqueued_edit.is_none());
        assert!(v.active_clip_drag.is_none() && v.active_trim_drag.is_none());
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(&visual, |v, _| v.last_enqueued_edit.clone().unwrap());
    assert!(
        matches!(command, Command::SetAudioClipGain { clip: id, millidb: -6_000 } if id == clip)
    );
    let mut canonical = hex_project::Editor::new(
        editor.read_with(&visual, |v, _| (*v.state.as_ref().unwrap().project).clone()),
        8,
    )
    .unwrap();
    canonical.execute(command).unwrap();
    assert_eq!(
        canonical.project().clip(clip).unwrap().gain().millidb(),
        -6_000
    );
    assert!(canonical.undo());
    assert_eq!(canonical.project().clip(clip).unwrap(), &original);
    assert!(!canonical.undo());
}

fn pump(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let ready = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.state.is_some() && !v.edit_pending()
        });
        visual.run_until_parked();
        if ready {
            return;
        }
        assert!(std::time::Instant::now() < until, "gain worker ready");
        std::thread::yield_now();
    }
}
fn terminal(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
) -> hex_session::service::Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(&*visual, |v, _| v.service.try_event())
            && event.request_complete
        {
            return event;
        }
        // Deliberately withhold installation until the request terminal.
        // The offline refusal fixture also retains its initial worker event.
        assert!(std::time::Instant::now() < until, "gain worker terminal");
        std::thread::yield_now();
    }
}
fn render(view: &EditorView, clip: ClipId) -> Vec<hex_engine::Frame> {
    let pending = view.pending_playback.as_ref().unwrap();
    let prepared = pending
        .prepared_revision
        .as_ref()
        .or_else(|| pending.playback.prepared_revision())
        .unwrap();
    assert_eq!(
        (pending.project_generation, pending.project_revision),
        (
            view.project_generation,
            view.state.as_ref().unwrap().revision
        )
    );
    let clip = view.state.as_ref().unwrap().project.clip(clip).unwrap();
    let mut renderer = hex_engine::Renderer::new(prepared.arrangement());
    renderer.seek(clip.start());
    renderer.play();
    let mut pcm = vec![[0.0; 2]; clip.duration() as usize];
    for block in pcm.chunks_mut(257) {
        renderer.render(block).unwrap();
    }
    pcm
}
#[gpui::test]
fn audio_gain_release_retains_value_until_real_ack_and_roundtrips_playback(
    cx: &mut TestAppContext,
) {
    let (files, window, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let painted_height = |visual: &VisualTestContext| {
        editor.read_with(visual, |view, _| {
            let cache = view.waveform_geometry_cache.borrow();
            let paint = cache.painted_waveforms().back().unwrap();
            let center = f32::from(paint.bounds.top() + paint.bounds.size.height * 0.25);
            let (top, bottom) = paint.ordinates[0];
            (center - top).abs().max((center - bottom).abs())
        })
    };
    let original_height = painted_height(&visual);
    assert!(original_height > 0.0);
    let (clip, before, original, pcm) = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let clip = state.project.tracks()[0].clips()[0].clone();
        (
            clip.id(),
            state.project.snapshot(),
            clip.clone(),
            render(v, clip.id()),
        )
    });
    let handle = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
    let value = Box::leak(format!("clip-gain-value-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(handle).unwrap().center();
    visual.simulate_modifiers_change(option());
    visual.simulate_mouse_move(down, None, option());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ResizeUpDown)
    );
    let up = point(down.x, down.y + px(60.0));
    // Escape cancels held input; the later mouse-up must not enqueue it.
    visual.simulate_mouse_down(down, MouseButton::Left, option());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!((painted_height(&visual) - original_height).abs() < 0.01);
    editor.read_with(&visual, |v, _| {
        assert!(!v.clip_gain.held());
        assert!(v.displayed_clip_gain(clip).is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.worker_requests_pending, 0);
    });
    visual.simulate_mouse_down(down, MouseButton::Left, option());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual
        .debug_bounds(value)
        .expect("held numeric gain preview");
    let held_height = painted_height(&visual);
    assert!(held_height > 0.0 && held_height < original_height * 0.65);
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    let event = terminal(&editor, &mut visual);
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(
        event.state.project.clip(clip).unwrap().gain().millidb(),
        -6_000
    );
    for _ in 0..2 {
        editor.update(&mut visual, |v, cx| {
            v.cancel_clip_gain(cx);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(value), Some(held));
        assert!((painted_height(&visual) - held_height).abs() < 0.01);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert_eq!(v.displayed_clip_gain(clip).unwrap().millidb(), -6_000);
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(value).is_none());
    assert!((painted_height(&visual) - held_height).abs() < 0.01);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.displayed_clip_gain(clip).is_none());
        let state = v.state.as_ref().unwrap();
        let changed = state.project.clip(clip).unwrap();
        assert_eq!(
            (
                changed.start(),
                changed.duration(),
                changed.source_offset(),
                changed.fade()
            ),
            (
                original.start(),
                original.duration(),
                original.source_offset(),
                original.fade()
            )
        );
        let gained = render(v, clip);
        let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
        assert!(pcm.iter().any(|f| f[0] != 0.0));
        assert_eq!(
            gained,
            pcm.iter().map(|f| f.map(|s| s * gain)).collect::<Vec<_>>()
        );
        state.project.snapshot()
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    let path = files.root().join("gain.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
    });
}

#[gpui::test]
fn audio_gain_shift_segments_bounds_and_noop_use_the_actual_handle(cx: &mut TestAppContext) {
    for (segments, expected) in [
        (vec![(60.0, true)], Some(-600)),
        (vec![(30.0, false), (10.0, true)], Some(-2_800)),
        (vec![(30.0, false), (0.0, true)], Some(-2_700)),
        (vec![(-60.0, false)], Some(6_000)),
        (vec![(1000.0, false)], Some(-60_000)),
        (vec![(-1000.0, false)], Some(24_000)),
        (vec![(10.0, false), (0.0, false)], None),
        (vec![(0.5, false)], None),
    ] {
        let cx = &mut cx.new_app();
        let (project, _, _, clip) = clip_drag_ui_test::audio_project();
        let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, option());
        let mut up = down;
        for (dy, fine) in segments {
            up = point(down.x, down.y + px(dy));
            visual.simulate_mouse_move(
                up,
                MouseButton::Left,
                Modifiers {
                    shift: fine,
                    ..Default::default()
                },
            );
            visual.run_until_parked();
        }
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            let got = v.last_enqueued_edit.as_ref().map(|c| match c {
                Command::SetAudioClipGain { clip: id, millidb } if *id == clip => *millidb,
                _ => panic!("unexpected command {c:?}"),
            });
            assert_eq!(got, expected);
        });
    }
}

#[gpui::test]
fn audio_gain_cancellation_and_existing_tools_keep_their_input_priority(cx: &mut TestAppContext) {
    for context in ["focus", "source", "button", "two", "fade", "selection"] {
        let cx = &mut cx.new_app();
        let (project, track, _, clip) = clip_drag_ui_test::audio_project();
        let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        if matches!(context, "two" | "fade") {
            visual.simulate_event(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(if context == "two" { "2" } else { "f" })
                    .unwrap(),
                is_held: false,
            });
            visual.run_until_parked();
            assert!(visual.debug_bounds(selector).is_none());
        }
        if context == "selection" {
            editor.update(&mut visual, |v, cx| {
                v.time_selection = Some(time_selection::TimeSelection {
                    anchor_track: Some(track),
                    end_track: Some(track),
                    anchor_frame: 0,
                    end_frame: 96_000,
                });
                cx.notify();
            });
            visual.run_until_parked();
        }
        visual.simulate_mouse_down(down, MouseButton::Left, option());
        let up = point(down.x, down.y + px(20.0));
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        match context {
            "focus" => {
                visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)))
            }
            "source" => editor.update(&mut visual, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            }),
            "button" => visual.simulate_mouse_move(up, None, Modifiers::default()),
            _ => (),
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(!v.clip_gain.held(), "{context}");
            assert!(v.displayed_clip_gain(clip).is_none(), "{context}");
            assert!(
                !matches!(v.last_enqueued_edit, Some(Command::SetAudioClipGain { .. })),
                "{context}"
            );
        });
    }
}

#[gpui::test]
fn audio_gain_real_refusal_retires_the_frozen_value_with_canonical_state(cx: &mut TestAppContext) {
    // The painted fixture has a clip; its independent real worker has no clip.
    // Release therefore reaches the real canonical validator and refuses.
    let (project, _, _, clip) = clip_drag_ui_test::audio_project();
    let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
    let label = Box::leak(format!("clip-gain-value-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let up = point(down.x, down.y + px(20.0));
    visual.simulate_mouse_down(down, MouseButton::Left, option());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds(label).unwrap();
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    let event = terminal(&editor, &mut visual);
    assert!(event.error.is_some());
    assert!(event.state.project.clip(clip).is_none());
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(label), Some(held));
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.displayed_clip_gain(clip).unwrap().millidb(), -2_000)
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(label).is_none());
    editor.read_with(&visual, |v, _| {
        assert!(v.displayed_clip_gain(clip).is_none());
        assert!(v.state.as_ref().unwrap().project.clip(clip).is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn momentary_tools_preserve_a_gain_drag_already_in_custody(cx: &mut TestAppContext) {
    for key in ["2", "f"] {
        let cx = &mut cx.new_app();
        let (project, _, _, clip) = clip_drag_ui_test::audio_project();
        let (window, editor) = clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-gain-handle-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let up = point(down.x, down.y + px(20.0));
        visual.simulate_mouse_down(down, MouseButton::Left, option());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
            is_held: false,
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.clip_gain.held(), "{key} cannot revoke admitted input")
        });
        assert!(visual.debug_bounds(selector).is_some());
        let up = point(up.x, up.y + px(10.0));
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(matches!(v.last_enqueued_edit, Some(Command::SetAudioClipGain { clip: id, millidb: -3_000 }) if id == clip)));
    }
}

#[gpui::test]
fn selected_audio_gain_preserves_offsets_and_owns_the_expanded_command_receipt(
    cx: &mut TestAppContext,
) {
    let (_files, window, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(window, cx);
    let (track, first) = editor.read_with(&visual, |v, _| {
        let track = &v.state.as_ref().unwrap().project.tracks()[0];
        (track.id(), track.clips()[0].id())
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::DuplicateClip(first))))
    });
    pump(&editor, &mut visual);
    let second = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .find(|c| c.id() != first)
            .unwrap()
            .id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetAudioClipGain {
            clip: second,
            millidb: 3_000
        })))
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.select_clip_header(track, first, Modifiers::default(), window, cx);
            v.select_clip_header(
                track,
                second,
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip_ids(), vec![first, second])
    });
    let selector = Box::leak(format!("clip-gain-handle-{}", first.value()).into_boxed_str());
    let label = Box::leak(format!("clip-gain-value-{}", first.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let up = point(down.x, down.y + px(60.0));
    visual.simulate_mouse_down(down, MouseButton::Left, option());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds(label).unwrap();
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(matches!(&v.last_enqueued_edit,
        Some(Command::EditClips { clips, edit: hex_project::ClipEdit::GainBy { millidb: -6_000 } }) if clips == &[first, second])));
    let event = terminal(&editor, &mut visual);
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(
        event.state.project.clip(first).unwrap().gain().millidb(),
        -6_000
    );
    assert_eq!(
        event.state.project.clip(second).unwrap().gain().millidb(),
        -3_000
    );
    // Selection can change after admission without changing frozen membership.
    editor.update(&mut visual, |v, cx| {
        v.clip_selection.only(track, first);
        v.selected_clip = Some(first);
        cx.notify();
    });
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(label), Some(held));
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(label).is_none());
    // No tick/abandonment cleanup is needed before the next admitted gesture.
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, option());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.clip_gain.held(),
            "exact scoped receipt retired at canonical receive"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
