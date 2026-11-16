//! Actual header/row input; TrackLayout remains the complete rollback owner.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent, div, prelude::*};
use std::{cell::Cell, rc::Rc};
struct Host {
    editor: gpui::Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left(px(600.))
                .top_0()
                .w(px(80.))
                .h(px(24.))
                .cursor(CursorStyle::Crosshair)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    visual: VisualTestContext,
    editor: gpui::Entity<EditorView>,
    tracks: Vec<TrackId>,
    actions: Rc<Cell<usize>>,
    start: gpui::Point<Pixels>,
}
fn open(cx: &mut TestAppContext, height: bool) -> Fixture {
    let (mut visual, editor, tracks) = fixture(cx, 3);
    let start = bounds(
        &mut visual,
        if height {
            "track-height-resize"
        } else {
            "track-width-resize"
        },
        tracks[0],
    )
    .center();
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
    });
    visual.run_until_parked();
    Fixture {
        visual,
        editor,
        tracks,
        actions,
        start,
    }
}
#[gpui::test]
fn track_size_capture_isolates_input_and_retains_axis_cursor(cx: &mut TestAppContext) {
    for height in [false, true] {
        let Fixture {
            mut visual,
            editor,
            tracks: _,
            actions,
            start,
        } = open(cx, height);
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.track_resize_active()));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "track sizing must isolate input before motion"
        );
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(if height {
                CursorStyle::ResizeRow
            } else {
                CursorStyle::ResizeColumn
            })
        );
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(actions.get(), 0);
        assert!(!editor.read_with(&visual, |v, _| v.track_resize_active()));
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn track_size_capture_loss_restores_full_snapshot_before_late_up(cx: &mut TestAppContext) {
    for height in [false, true] {
        for loss in ["button", "focus", "invalid", "escape", "revision"] {
            let Fixture {
                mut visual,
                editor,
                tracks,
                actions,
                start,
            } = open(cx, height);
            let outside = point(px(640.), px(10.));
            let original = editor.read_with(&visual, |v, _| {
                (
                    v.track_header_width(),
                    tracks
                        .iter()
                        .map(|id| v.track_height(*id))
                        .collect::<Vec<_>>(),
                )
            });
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let alt = Modifiers {
                alt: true,
                ..Default::default()
            };
            let moved = start
                + if height {
                    point(px(0.), px(28.))
                } else {
                    point(px(-28.), px(0.))
                };
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(moved, MouseButton::Left, alt);
            assert!(editor.read_with(&visual, |v, _| v.track_resize_active()));
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "invalid" => visual.simulate_mouse_move(
                    point(px(f32::NAN), px(f32::NAN)),
                    MouseButton::Left,
                    alt,
                ),
                "escape" => visual.simulate_keystrokes("escape"),
                "revision" => editor.update(&mut visual, |v, cx| {
                    let state = v.state.as_ref().unwrap();
                    v.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: state.canonical_audio_source.clone(),
                        comparison_audio: None,
                        project: Arc::clone(&state.project),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        midi_preview: Arc::clone(&state.midi_preview),
                        gain_preview: state.gain_preview.clone(),
                        timeline_frames: state.timeline_frames,
                        can_undo: state.can_undo,
                        can_redo: state.can_redo,
                        revision: state.revision + 1,
                        unsaved: state.unsaved,
                        document_path: state.document_path.clone(),
                    }));
                    cx.notify();
                }),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, cx| v.search_browser(w, cx));
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            position: outside,
                            button: MouseButton::Left,
                            modifiers: alt,
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                _ => unreachable!(),
            }
            visual.run_until_parked();
            if loss != "focus" {
                visual.simulate_mouse_up(outside, MouseButton::Left, alt);
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "height={height} loss={loss}");
            editor.read_with(&visual, |v, _| {
                assert!(!v.track_resize_active());
                assert_eq!(v.track_header_width(), original.0);
                assert_eq!(
                    tracks
                        .iter()
                        .map(|id| v.track_height(*id))
                        .collect::<Vec<_>>(),
                    original.1
                );
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(v.last_enqueued_edit.is_none());
            });
        }
    }
}

/// Three ordinary rows; setup/release/cancellation and native delivery excluded.
#[gpui::test]
#[ignore = "manual optimized track-size profile; no native acceptance"]
fn profile_track_size_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_TRACK_SIZE_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for height in [false, true] {
        let Fixture {
            mut visual,
            editor,
            tracks,
            ..
        } = open(cx, height);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            let start = bounds(
                &mut visual,
                if height {
                    "track-height-resize"
                } else {
                    "track-width-resize"
                },
                tracks[0],
            )
            .center();
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.track_resize_active()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = px(((round + step) % 7) as f32 * 2.);
                at = start
                    + if height {
                        point(px(0.), displacement)
                    } else {
                        point(-displacement, px(0.))
                    };
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(!editor.read_with(&visual, |v, _| v.track_resize_active()));
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "TRACK_SIZE_PROFILE height={height} size=1280x800 scale={scale} tracks=3 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
