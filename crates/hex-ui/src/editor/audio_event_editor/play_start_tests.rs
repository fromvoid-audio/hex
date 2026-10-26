use super::*;
use hex_project::{AudioViewProjection, AudioViewResizeOperation};

fn marker_fraction(visual: &mut VisualTestContext) -> f32 {
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let marker = visual.debug_bounds("audio-play-start-marker").unwrap();
    f32::from(marker.left() - body.left()) / f32::from(body.size.width)
}
fn terminal(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
) -> hex_session::service::Event {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Ok(event) = editor.read_with(&*visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                return event;
            }
            editor.update(&mut *visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(std::time::Instant::now() < until, "play-start terminal");
        std::thread::yield_now();
    }
}
#[gpui::test]
fn audio_saved_start_hover_drag_delayed_ack_history_and_reopen(cx: &mut TestAppContext) {
    for scenario in ["plain", "crop", "frozen"] {
        let cx = &mut cx.new_app();
        let (files, mut visual, editor, clip) = open(cx);
        if scenario == "crop" {
            for command in [
                Command::SetAudioClipPitch { clip, cents: 123 },
                Command::TrimClipEdge {
                    clip,
                    edge: hex_project::TrimEdge::Left,
                    frame: 12_345,
                },
            ] {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(command)))
                });
                pump(&editor, &mut visual);
            }
        }
        if scenario == "frozen" {
            for command in [
                Command::SetAudioClipMusical {
                    clip,
                    enabled: true,
                },
                Command::SetTempoPoint {
                    beat: hex_project::BeatTime::from_quarters(2).unwrap(),
                    tempo: hex_project::Tempo::new(180_000_000).unwrap(),
                },
                Command::SetProjectTempo {
                    tempo: hex_project::Tempo::new(90_000_000).unwrap(),
                },
            ] {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(command)))
                });
                pump(&editor, &mut visual);
            }
        }
        let before_pcm = editor.read_with(&visual, |v, _| render_accepted_event(v, clip));
        let (before, first, last, cursor, expected) = editor.update(&mut visual, |v, cx| {
            let (_, root, _) = v.event_context().unwrap();
            v.audio_event_editor.grid = super::super::SnapGrid::Off;
            v.audio_event_editor.range = Some((
                root.view_start,
                root.view_start + (root.view_end - root.view_start) / 8,
            ));
            v.audio_event_editor.cursor = root.view_start + 1;
            cx.notify();
            (
                v.state.as_ref().unwrap().project.snapshot(),
                root.view_start,
                root.view_end,
                v.audio_event_editor.cursor,
                {
                    let tempo = v.state.as_ref().unwrap().project.tempo_map();
                    let first = root.root_frame_at(root.view_start, tempo).unwrap();
                    let end = root.root_frame_at(root.view_end, tempo).unwrap();
                    root.root_position_at(first + (end - first) / 4, tempo)
                        .unwrap()
                },
            )
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("audio-events-waveform").unwrap();
        let handle = visual
            .debug_bounds("audio-play-start-handle")
            .expect("a usable start handle");
        let down = handle.center();
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeLeftRight)
        );
        assert_eq!(
            visual.rendered_background("audio-play-start-marker"),
            Some(
                gpui::Hsla::from(gpui::rgb(crate::theme::color(
                    crate::theme::PaletteRole::Primary
                )))
                .into()
            )
        );
        let up = point(down.x + body.size.width * 0.25, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!((marker_fraction(&mut visual) - 0.25).abs() < 0.002);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        let terminal = terminal(&mut visual, &editor);
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..2 {
            repaint(&mut visual, &editor);
            assert!((marker_fraction(&mut visual) - 0.25).abs() < 0.002);
            editor.read_with(&visual, |v, _| {
                assert!(v.audio_event_editor.gesture.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert_eq!(v.audio_event_editor.cursor, cursor);
            });
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        repaint(&mut visual, &editor);
        assert!(
            (marker_fraction(&mut visual) - 0.25).abs() < 0.002,
            "ack keeps the source camera"
        );
        let after = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let source = state.project.clip(clip).unwrap();
            let root = source.audio_event_root();
            assert_eq!((root.view_start, root.view_end), (first, last));
            let position = AudioViewProjection::new(source, state.project.tempo_map())
                .unwrap()
                .play_start_position()
                .unwrap()
                .unwrap();
            assert!((i128::from(position) - i128::from(expected)).abs() <= 1);
            assert_eq!(v.audio_event_editor.cursor, cursor);
            let root_first = root
                .root_frame_at(first, state.project.tempo_map())
                .unwrap();
            let root_start = root
                .root_frame_at(position, state.project.tempo_map())
                .unwrap();
            let skip = usize::try_from(root_start - root_first).unwrap();
            let pcm = render_accepted_event(v, clip);
            let remaining = before_pcm.len() - skip;
            assert_eq!(
                &pcm[..remaining],
                &before_pcm[skip..],
                "prepared worker audio starts at the displayed source point: {scenario}"
            );
            assert!(
                pcm[remaining..].iter().all(|frame| *frame == [0.0; 2]),
                "once tail is silent: {scenario}"
            );
            assert!(matches!(
                v.last_enqueued_edit,
                Some(Command::ResizeAudioClipView {
                    operation: AudioViewResizeOperation::PlayStart { .. },
                    ..
                })
            ));
            state.project.snapshot()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        assert!(
            marker_fraction(&mut visual).abs() < 0.002,
            "undo restores the saved marker inside the same source window: {scenario}"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        assert!(
            (marker_fraction(&mut visual) - 0.25).abs() < 0.003,
            "redo preserves the source window: {scenario}"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
        let path = files.root().join("audio-play-start.hex");
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
}

#[gpui::test]
fn audio_saved_start_release_modifier_and_outside_clamp_use_final_pointer(cx: &mut TestAppContext) {
    for (fine, outside) in [(false, false), (true, false), (true, true)] {
        let cx = &mut cx.new_app();
        let (_files, mut visual, editor, clip) = open(cx);
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.grid = super::super::SnapGrid::Quarter;
            cx.notify();
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("audio-events-waveform").unwrap();
        let down = visual
            .debug_bounds("audio-play-start-handle")
            .unwrap()
            .center();
        let up = point(
            down.x + body.size.width * if outside { 2.0 } else { 0.3 },
            down.y,
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(
            up,
            MouseButton::Left,
            Modifiers {
                platform: fine,
                ..Default::default()
            },
        );
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let clip = state.project.clip(clip).unwrap();
            let position = AudioViewProjection::new(clip, state.project.tempo_map())
                .unwrap()
                .play_start_position()
                .unwrap()
                .unwrap();
            let expected = if outside {
                95_999
            } else if fine {
                28_800
            } else {
                24_000
            };
            assert!(
                (i128::from(position) - expected).abs() <= 1,
                "fine={fine} outside={outside} position={position}"
            );
        });
    }
}

#[gpui::test]
fn injected_refused_start_terminal_retires_preview_with_old_canonical_camera(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, clip) = open(cx);
    let before = editor.read_with(&visual, |v, _| Arc::clone(v.state.as_ref().unwrap()));
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = visual
        .debug_bounds("audio-play-start-handle")
        .unwrap()
        .center();
    let up = point(down.x + body.size.width * 0.25, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    let mut event = terminal(&mut visual, &editor);
    // Only the receive/display boundary is injected here. The worker success,
    // current-format roundtrip and canonical invalid-range refusal are tested
    // separately; this is not backend failure-injection evidence.
    event.error = Some("injected preparation refusal".into());
    event.state = Arc::clone(&before);
    for _ in 0..2 {
        repaint(&mut visual, &editor);
        assert!((marker_fraction(&mut visual) - 0.25).abs() < 0.002);
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    repaint(&mut visual, &editor);
    assert!(marker_fraction(&mut visual).abs() < 0.002);
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_clip_loop_audio(clip).is_none());
        assert!(v.audio_event_editor.gesture.is_none());
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before.project.snapshot()
        );
        assert_eq!(v.state.as_ref().unwrap().can_undo, before.can_undo);
        assert!(v.error.is_some());
    });
}

#[gpui::test]
fn held_two_at_saved_start_selects_time_without_moving_audio_start(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, clip) = open(cx);
    focus_events(&mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds("audio-play-start-handle")
        .unwrap()
        .center();
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    let up = point(down.x + body.size.width * 0.25, down.y);
    let key = gpui::Keystroke::parse("2").unwrap();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: key.clone(),
        is_held: false,
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(matches!(
            v.audio_event_editor.gesture.as_ref().map(|g| g.kind),
            Some(Kind::Select)
        ))
    });
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.simulate_event(gpui::KeyUpEvent { keystroke: key });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.range.is_some_and(|(a, b)| a < b));
        assert!(v.pending_clip_loop_audio(clip).is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn directed_audio_saved_start_is_explicitly_readonly_until_its_read_contract_is_extended(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, clip) = open(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ReverseClips(vec![clip]))))
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    assert!(visual.debug_bounds("audio-play-start-handle").is_none());
    let at = visual
        .debug_bounds("audio-play-start-readonly")
        .unwrap()
        .center();
    visual.simulate_mouse_move(at, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow)
    );
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(at.x - px(20.0), at.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(
        point(at.x - px(20.0), at.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.gesture.is_none());
        assert!(v.pending_clip_loop_audio(clip).is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}
