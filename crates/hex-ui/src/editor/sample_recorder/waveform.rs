//! View and gesture state for auditioning an immutable take.

use super::{EditorView, Row, button, recorder_button, selection::Range};
use crate::theme;
use gpui::{
    Bounds, Context, Div, Modifiers, MouseButton, Pixels, Point, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};
use hex_audio::{
    sample_preview::{Phase, Snapshot},
    sample_recording::Take,
};
use hex_session::service::take_audio::{self, TakeSource};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Waveform {
    pub(super) take: Take,
    pub(super) name: String,
    peaks: Option<Rc<[f32]>>,
    cancel: Arc<AtomicBool>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    overview: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub(super) range: Range,
    view: Range,
    gesture: Option<Gesture>,
    pub(super) excerpt: super::excerpt::ExcerptState,
    snap: bool,
    repeat: bool,
    preview: Option<(u64, u64)>,
    snapshot: Option<Snapshot>,
}
impl Drop for Waveform {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
struct Gesture {
    kind: Edge,
    anchor: u64,
    original: Range,
    snap_step: Option<f64>,
}
#[derive(Clone, Copy)]
enum Edge {
    Draw,
    Left,
    Right,
    Slide,
}

impl EditorView {
    pub(super) fn open_recorded_take(&mut self, row: Row, cx: &mut Context<Self>) {
        self.stop_take_preview();
        let take = row.take;
        if take.frames == 0 {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let range = Range::full(take.frames);
        self.sample_recorder.waveform = Some(Waveform {
            take: take.clone(),
            name: row.name,
            peaks: None,
            cancel: cancel.clone(),
            bounds: Default::default(),
            overview: Default::default(),
            range,
            view: range,
            gesture: None,
            excerpt: Default::default(),
            snap: false,
            repeat: true,
            preview: None,
            snapshot: None,
        });
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let worker_cancel = cancel.clone();
            let peaks = cx
                .background_executor()
                .spawn(async move { take_audio::peaks(take_source(&take), &worker_cancel) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(wave) = view.sample_recorder.waveform.as_mut()
                    && Arc::ptr_eq(&wave.cancel, &cancel)
                {
                    match peaks {
                        Ok(peaks) => wave.peaks = Some(peaks.into()),
                        Err(reason) => view.sample_recorder.error = Some(reason),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn stop_take_preview(&mut self) {
        let preview = self
            .sample_recorder
            .waveform
            .as_mut()
            .and_then(|wave| wave.preview.take());
        if let Some((output, generation)) = preview
            && let Some(audio) = self.audio.as_mut()
            && audio.control.output_identity() == Some(output)
            && audio
                .control
                .preview_snapshot()
                .is_some_and(|p| p.generation == generation)
        {
            audio.control.preview_cancel();
        }
    }

    fn play_take_range(&mut self, cx: &mut Context<Self>) {
        let preview = self
            .sample_recorder
            .waveform
            .as_ref()
            .and_then(|wave| wave.preview);
        let active = preview.is_some_and(|(output, generation)| {
            self.audio.as_ref().is_some_and(|audio| {
                audio.control.output_identity() == Some(output)
                    && audio.control.preview_snapshot().is_some_and(|p| {
                        p.generation == generation
                            && matches!(p.phase, Phase::Preparing | Phase::Playing)
                    })
            })
        });
        self.stop_take_preview();
        if active {
            cx.notify();
            return;
        }
        if self.sample_recorder.audition_job {
            self.sample_recorder.error = Some("previous take audition is still preparing".into());
            cx.notify();
            return;
        }
        self.sample_recorder.error = None;
        let Some(wave) = self.sample_recorder.waveform.as_mut() else {
            return;
        };
        let Some(audio) = self.audio.as_mut() else {
            self.sample_recorder.error = Some("take audition requires an audio output".into());
            cx.notify();
            return;
        };
        let Some(output) = audio.control.output_identity() else {
            return;
        };
        let generation = match audio.control.preview_request() {
            Ok(generation) => generation,
            Err(error) => {
                self.sample_recorder.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let _ = audio.control.preview_set_gain(1.0);
        wave.preview = Some((output, generation));
        self.sample_recorder.audition_job = true;
        let rate = audio.sample_rate;
        let (take, range, repeat) = (wave.take.clone(), wave.range, wave.repeat);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let pcm = cx
                .background_executor()
                .spawn(async move {
                    take_audio::decode(take_source(&take), range.start..range.end, rate)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.sample_recorder.audition_job = false;
                let Some(wave) = view.sample_recorder.waveform.as_mut() else {
                    return;
                };
                let Some(audio) = view.audio.as_mut() else {
                    return;
                };
                if wave.preview != Some((output, generation))
                    || audio.control.output_identity() != Some(output)
                    || audio
                        .control
                        .preview_snapshot()
                        .is_none_or(|p| p.generation != generation)
                {
                    return;
                }
                let result = pcm.and_then(|audio_pcm| {
                    if repeat {
                        audio.control.preview_publish_loop(generation, audio_pcm)
                    } else {
                        audio.control.preview_publish(generation, audio_pcm)
                    }
                    .map_err(|error| error.to_string())
                });
                if let Err(error) = result {
                    audio.control.preview_cancel();
                    wave.preview = None;
                    view.sample_recorder.error = Some(error);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn tick_take_preview(&mut self) -> bool {
        let Some(wave) = self.sample_recorder.waveform.as_mut() else {
            return false;
        };
        let snapshot = wave.preview.and_then(|(output, generation)| {
            self.audio
                .as_ref()
                .filter(|audio| audio.control.output_identity() == Some(output))
                .and_then(|audio| audio.control.preview_snapshot())
                .filter(|snapshot| snapshot.generation == generation)
        });
        let changed = wave.snapshot != snapshot;
        wave.snapshot = snapshot;
        changed && self.sample_recorder.visible
    }

    fn take_drag_start(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        self.stop_take_preview();
        let Some(wave) = self.sample_recorder.waveform.as_mut() else {
            return;
        };
        let Some(bounds) = wave.bounds.get() else {
            return;
        };
        let at = frame_at(wave.view, bounds, position);
        let near = |frame| {
            frame >= wave.view.start
                && frame <= wave.view.end
                && (x_at(wave.view, bounds, frame) - position.x).abs() <= px(5.0)
        };
        let inside = at >= wave.range.start && at < wave.range.end;
        let slide_handle = inside && position.y >= bounds.bottom() - px(6.0);
        let kind = if near(wave.range.start) {
            Edge::Left
        } else if near(wave.range.end) {
            Edge::Right
        } else if slide_handle || (modifiers.alt && inside) {
            Edge::Slide
        } else {
            Edge::Draw
        };
        let snap_step = wave
            .snap
            .then(|| {
                let bpm = self
                    .state
                    .as_ref()?
                    .project
                    .tempo_map()
                    .tempo_at(hex_project::BeatTime::ZERO)
                    .micro_bpm();
                Some(
                    (f64::from(wave.take.sample_rate) * 60_000_000.0 / f64::from(bpm) / 4.0)
                        .max(1.0),
                )
            })
            .flatten();
        wave.gesture = Some(Gesture {
            kind,
            anchor: at,
            original: wave.range,
            snap_step,
        });
        self.take_drag_move(position, modifiers);
    }

    fn take_drag_move(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        let Some(wave) = self.sample_recorder.waveform.as_mut() else {
            return;
        };
        let Some(gesture) = wave.gesture else { return };
        let Some(bounds) = wave.bounds.get() else {
            return;
        };
        let frames = wave.take.frames;
        let raw = frame_at(wave.view, bounds, position);
        let original = gesture.original;
        let (root, step) = if modifiers.alt {
            match gesture.kind {
                Edge::Left => (original.start, Some(original.frames() as f64)),
                Edge::Right => (original.end, Some(original.frames() as f64)),
                _ => (original.start, None),
            }
        } else if modifiers.shift {
            (original.start, None)
        } else {
            (original.start, gesture.snap_step)
        };
        let snap = |frame: u64| {
            if !modifiers.alt
                && !modifiers.shift
                && wave.snap
                && let Some(recorded) = super::beat_guides::snap(&wave.take.timing, frame)
            {
                return recorded;
            }
            let Some(step) = step else { return frame };
            let offset = (frame as f64 - root as f64) / step;
            let candidate = root as f64 + offset.round() * step;
            if modifiers.alt && !(0.0..=frames as f64).contains(&candidate) {
                return root;
            }
            candidate.clamp(0.0, frames as f64).round() as u64
        };
        let at = snap(raw);
        wave.range = match gesture.kind {
            Edge::Draw => Range::between(snap(gesture.anchor), at, frames),
            Edge::Left if modifiers.alt && at >= original.end => original,
            Edge::Right if modifiers.alt && at <= original.start => original,
            Edge::Left => Range::between(at.min(original.end - 1), original.end, frames),
            Edge::Right => Range::between(original.start, at.max(original.start + 1), frames),
            Edge::Slide => original.slide(gesture.anchor, raw, frames, modifiers.alt),
        };
    }

    fn take_drag_finish(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        self.take_drag_move(position, modifiers);
        if self
            .sample_recorder
            .waveform
            .as_mut()
            .is_some_and(|wave| wave.gesture.take().is_some())
        {
            self.prepare_take_excerpt(cx);
        }
    }

    pub(super) fn selecting(&self) -> bool {
        self.sample_recorder
            .waveform
            .as_ref()
            .is_some_and(|wave| wave.gesture.is_some())
    }

    pub(super) fn take_waveform_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let wave = self.sample_recorder.waveform.as_ref()?;
        let full = Range::full(wave.take.frames);
        let view = wave.view;
        let range = wave.range;
        let preview = wave.preview.and_then(|(output, generation)| {
            self.audio
                .as_ref()
                .filter(|audio| audio.control.output_identity() == Some(output))
                .and_then(|audio| audio.control.preview_snapshot())
                .filter(|p| p.generation == generation)
        });
        let playing = preview.is_some_and(|p| matches!(p.phase, Phase::Playing | Phase::Preparing));
        let playhead = preview
            .filter(|p| p.phase == Phase::Playing && p.frames > 0)
            .map(|p| {
                range.start
                    + (u128::from(range.frames()) * u128::from(p.frame) / u128::from(p.frames))
                        as u64
            });
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .min_w_0()
            .child(div().flex_1().min_w_0().truncate().child(theme::text(
                hex_session::service::text::verbatim(&wave.name),
            )))
            .child(
                recorder_button(
                    if playing { "stop audition" } else { "audition" },
                    "play or stop the selected range",
                    ("take-audition", 0),
                )
                .on_click(cx.listener(|view, _, _, cx| view.play_take_range(cx))),
            )
            .child(
                recorder_button(
                    if wave.repeat { "loop on" } else { "loop off" },
                    "toggle loop audition",
                    ("take-loop", 0),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.stop_take_preview();
                    if let Some(wave) = view.sample_recorder.waveform.as_mut() {
                        wave.repeat = !wave.repeat;
                    }
                    cx.notify();
                })),
            )
            .child(
                recorder_button(
                    if wave.snap { "snap on" } else { "snap off" },
                    if wave.take.timing.is_empty() {
                        "snap uses project tempo · shift adjusts freely"
                    } else {
                        "snap uses recorded beats where available · shift adjusts freely"
                    },
                    ("take-snap", 0),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(wave) = view.sample_recorder.waveform.as_mut() {
                        wave.snap = !wave.snap;
                    }
                    cx.notify();
                })),
            )
            .children([true, false].map(|inward| {
                button(
                    if inward { "+" } else { "−" },
                    "zoom waveform",
                    ("take-zoom", u64::from(inward)),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(wave) = view.sample_recorder.waveform.as_mut() {
                        let center = wave.range.start + wave.range.frames() / 2;
                        wave.view = wave.view.zoom(center, wave.take.frames, inward);
                    }
                    cx.notify();
                }))
            }));
        let controls = controls.children(self.take_excerpt_grip(cx));
        let guides = super::beat_guides::visible(&wave.take.timing, view, wave.take.sample_rate);
        let body = div()
            .id("take-waveform")
            .debug_selector(|| "take-waveform".into())
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    view.take_drag_start(event.position, event.modifiers);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &gpui::MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left)
                    && view
                        .sample_recorder
                        .waveform
                        .as_ref()
                        .is_some_and(|wave| wave.gesture.is_some())
                {
                    view.take_drag_move(event.position, event.modifiers);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseUpEvent, _, cx| {
                    view.take_drag_finish(event.position, event.modifiers, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseUpEvent, _, cx| {
                    if view
                        .sample_recorder
                        .waveform
                        .as_ref()
                        .is_some_and(|wave| wave.gesture.is_some())
                    {
                        view.take_drag_finish(event.position, event.modifiers, cx);
                        cx.notify();
                    }
                }),
            )
            .child(wave_canvas(
                wave.bounds.clone(),
                wave.peaks.clone(),
                full,
                view,
                range,
                playhead,
                Some(guides),
            ));
        let overview = div()
            .id("take-navigator")
            .h(px(12.0))
            .flex_shrink_0()
            .relative()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(wave) = view.sample_recorder.waveform.as_mut()
                        && let Some(bounds) = wave.overview.get()
                    {
                        let at = frame_at(Range::full(wave.take.frames), bounds, event.position);
                        wave.view = wave.view.pan(at, wave.take.frames);
                        cx.notify();
                    }
                }),
            )
            .child(wave_canvas(
                wave.overview.clone(),
                wave.peaks.clone(),
                full,
                full,
                view,
                None,
                None,
            ));
        Some(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(controls)
                .child(body)
                .child(overview),
        )
    }
}

pub(super) fn take_source(take: &Take) -> TakeSource<'_> {
    TakeSource {
        path: &take.path,
        frames: take.frames,
        sample_rate: take.sample_rate,
    }
}

fn frame_at(view: Range, bounds: Bounds<Pixels>, point: Point<Pixels>) -> u64 {
    let fraction = (f64::from(f32::from(point.x - bounds.left()))
        / f64::from(f32::from(bounds.size.width)))
    .clamp(0.0, 1.0);
    view.start + (fraction * view.frames() as f64).round() as u64
}
fn x_at(view: Range, bounds: Bounds<Pixels>, frame: u64) -> Pixels {
    let fraction = (frame.clamp(view.start, view.end) - view.start) as f64 / view.frames() as f64;
    bounds.left() + bounds.size.width * fraction as f32
}

fn wave_canvas(
    slot: Rc<Cell<Option<Bounds<Pixels>>>>,
    peaks: Option<Rc<[f32]>>,
    full: Range,
    view: Range,
    highlight: Range,
    playhead: Option<u64>,
    guides: Option<Vec<u64>>,
) -> impl IntoElement {
    canvas(
        move |bounds, _, _| slot.set(Some(bounds)),
        move |bounds, (), window, _| {
            let left = x_at(view, bounds, highlight.start);
            let right = x_at(view, bounds, highlight.end);
            if right > left {
                let tint =
                    gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Primary))).opacity(0.2);
                window.paint_quad(fill(
                    Bounds::new(
                        point(left, bounds.top()),
                        size(right - left, bounds.size.height),
                    ),
                    tint,
                ));
            }
            if let Some(peaks) = &peaks {
                let bins = peaks.len() as u64;
                let first = view.start * bins / full.end;
                let last = (view.end * bins).div_ceil(full.end).min(bins);
                let center = bounds.top() + bounds.size.height * 0.5;
                let color = rgb(theme::color(theme::PaletteRole::Text));
                for index in first..last {
                    let frame = index * full.end / bins;
                    let end = (index + 1) * full.end / bins;
                    let x = x_at(view, bounds, frame);
                    let width = (x_at(view, bounds, end) - x).max(px(1.0));
                    let half = bounds.size.height * 0.45 * peaks[index as usize].min(1.0);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x, center - half),
                            size(width, (half * 2.0).max(px(1.0))),
                        ),
                        color,
                    ));
                }
            }
            if let Some(guides) = &guides {
                let color =
                    gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Text))).opacity(0.28);
                for &frame in guides {
                    let x = x_at(view, bounds, frame);
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.top()), size(px(1.0), bounds.size.height)),
                        color,
                    ));
                }
            }
            if guides.is_some() && right - left >= px(12.0) {
                let middle = left + (right - left) * 0.5;
                let handle = Bounds::new(
                    point(middle - px(5.0), bounds.bottom() - px(4.0)),
                    size(px(10.0), px(3.0)),
                );
                window.paint_quad(fill(handle, rgb(theme::color(theme::PaletteRole::Primary))));
            }
            for frame in [Some(highlight.start), Some(highlight.end), playhead]
                .into_iter()
                .flatten()
            {
                if frame < view.start || frame > view.end {
                    continue;
                }
                let x = x_at(view, bounds, frame).min(bounds.right() - px(1.0));
                let color = rgb(theme::color(theme::PaletteRole::Primary));
                window.paint_quad(fill(
                    Bounds::new(point(x, bounds.top()), size(px(1.0), bounds.size.height)),
                    color,
                ));
            }
        },
    )
    .size_full()
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use gpui::{TestAppContext, VisualTestContext};
    use hex_audio::sample_recording::Source;

    #[gpui::test]
    fn take_waveform_drag_selects_exact_original_frames(cx: &mut TestAppContext) {
        let (project, _, _, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |view, cx| {
            view.sample_recorder.visible = true;
            let full = Range::full(1024);
            view.sample_recorder.waveform = Some(Waveform {
                take: Take {
                    source: Source::Main,
                    path: "unused.wav".into(),
                    frames: 1024,
                    sample_rate: 48_000,
                    timing: Default::default(),
                },
                name: "take".into(),
                peaks: Some(vec![0.5; 8].into()),
                cancel: Arc::default(),
                bounds: Default::default(),
                overview: Default::default(),
                range: full,
                view: full,
                gesture: None,
                excerpt: Default::default(),
                snap: false,
                repeat: true,
                preview: None,
                snapshot: None,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let b = visual.debug_bounds("take-waveform").unwrap();
        let start = point(b.left() + b.size.width * 0.25, b.center().y);
        let end = point(b.left() + b.size.width * 0.75, b.center().y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let wave = view.sample_recorder.waveform.as_ref().unwrap();
            assert_eq!(
                wave.range,
                Range {
                    start: 256,
                    end: 768
                }
            );
            assert!(wave.gesture.is_none());
        });
    }
}
