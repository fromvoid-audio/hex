//! Event fades reuse the Arrangement handle shape and scalar edit geometry.
//! Only this editor's ordinary gesture/receipt owns previews; pointer capture
//! never prepares PCM or edits the canonical source until the one release.
use super::*;
use crate::{
    editor::fade_handles::{self, Handle},
    theme,
};
use gpui::{
    Context, IntoElement, MouseButton, PathBuilder, Point, Window, canvas, div, point, prelude::*,
    px, relative, rgb,
};
use hex_project::{AudioEventFade, ClipFade, TempoMap};

#[derive(Clone, Copy)]
pub(super) struct FadeGesture {
    event: AudioEvent,
    handle: Handle,
    original: ClipFade,
    source_frame_start: u64,
    pointer_source_frame: f64,
    origin_y: f32,
    pub(super) last_y: f32,
}
impl FadeGesture {
    pub(super) fn pointer_cursor(self) -> gpui::CursorStyle {
        if matches!(self.handle, Handle::InCurve | Handle::OutCurve) {
            gpui::CursorStyle::Crosshair
        } else {
            gpui::CursorStyle::ResizeLeftRight
        }
    }
}
fn source_window(root: &AudioClipEdit, event: AudioEvent, tempo: &TempoMap) -> Option<(u64, u64)> {
    let start = root
        .event_source_frame_at(event.source, event.source_start, tempo)
        .ok()?;
    let end = root
        .event_source_frame_at(
            event.source,
            event.source_start.checked_add(event.length())?,
            tempo,
        )
        .ok()?;
    (start < end).then_some((start, end))
}
// Cropping retains the authored envelope domain. F only exposes controls; it
// must not implicitly reanchor the envelope or use a different drag baseline.
fn projected_fade(
    root: &AudioClipEdit,
    event: AudioEvent,
    tempo: &TempoMap,
) -> Option<AudioEventFade> {
    if let Some(fade) = event.fade {
        fade.reproject(|at| root.event_source_frame_at(event.source, at, tempo))
            .ok()
    } else {
        let (start, end) = source_window(root, event, tempo)?;
        AudioEventFade::new(start, ClipFade::neutral(end - start).ok()?).ok()
    }
}
// Frame projection is lossy for fractional beats. Editing one endpoint must
// not quantize the other authored knots (or a bend-only edit quantize all four).
fn retain_unchanged_knots(
    original: ClipFade,
    projected: ClipFade,
    edited: ClipFade,
    canonical: ClipFade,
) -> Option<ClipFade> {
    let knots = |shape: ClipFade| {
        [
            shape.in_start(),
            shape.in_start() + shape.fade_in(),
            shape.span() - shape.out_end_offset() - shape.fade_out(),
            shape.span() - shape.out_end_offset(),
        ]
    };
    let mut result = knots(canonical);
    for ((result, original), (before, after)) in result
        .iter_mut()
        .zip(knots(original))
        .zip(knots(projected).into_iter().zip(knots(edited)))
    {
        if before == after {
            *result = original;
        }
    }
    let [in_start, in_end, out_start, out_end] = result;
    ClipFade::new(
        canonical.span(),
        0,
        in_end.checked_sub(in_start)?,
        out_end.checked_sub(out_start)?,
    )
    .ok()?
    .with_shape(
        in_start,
        canonical.span().checked_sub(out_end)?,
        edited.in_curve(),
        edited.out_curve(),
    )
    .ok()?
    .with_curve_positions(edited.in_position(), edited.out_position())
    .ok()
}
fn destination_frame(
    root: &AudioClipEdit,
    event: AudioEvent,
    tempo: &TempoMap,
    source_frame: u64,
    subframe: f64,
) -> Option<u64> {
    let raw = root
        .event_source_position_at(event.source, source_frame, tempo)
        .ok()?;
    let mut relative = i128::from(raw) - i128::from(event.source_start);
    if subframe > 0.0 {
        let next = root
            .event_source_position_at(event.source, source_frame.checked_add(1)?, tempo)
            .ok()?;
        relative =
            relative.checked_add(((next.checked_sub(raw)?) as f64 * subframe).round() as i128)?;
    }
    let at = u64::try_from(i128::from(event.start).checked_add(relative)?).ok()?;
    root.root_frame_at(at, tempo).ok()
}
// Continuous drawing/gesture coordinates, including virtual positions outside
// the root. Only the final bounded ClipFade geometry is sent to the model. This
// keeps curve motion sub-sample precise and permits a one-sample fade-out whose
// two endpoint samples coincide. The canonical frame/beat queries remain the
// authority inside each sample interval; no PCM mapping is changed here.
fn clock_coordinate(root: &AudioClipEdit, frame: f64, tempo: &TempoMap) -> Option<f64> {
    if !frame.is_finite() {
        return None;
    }
    if matches!(root.domain.clock, hex_project::AudioDomainClock::Frames(_)) {
        return Some(frame);
    }
    let total = root.root_frame_at(root.domain.clock.length(), tempo).ok()?;
    if total == 0 {
        return None;
    }
    let at = |sample: u64| -> Option<u64> {
        if sample == 0 {
            Some(0)
        } else if sample == total {
            Some(root.domain.clock.length())
        } else {
            root.root_position_at(sample, tempo).ok()
        }
    };
    if frame <= 0.0 {
        return Some(frame * at(1)? as f64).filter(|value| value.is_finite());
    }
    if frame >= total as f64 {
        let step = root.domain.clock.length().checked_sub(at(total - 1)?)?;
        return (step > 0)
            .then_some(root.domain.clock.length() as f64 + (frame - total as f64) * step as f64)
            .filter(|value| value.is_finite());
    }
    let low = (frame.floor() as u64).min(total - 1);
    let first = at(low)?;
    Some(first as f64 + at(low + 1)?.checked_sub(first)? as f64 * (frame - low as f64))
}
fn frame_coordinate(root: &AudioClipEdit, raw: f64, tempo: &TempoMap) -> Option<f64> {
    if !raw.is_finite() {
        return None;
    }
    if matches!(root.domain.clock, hex_project::AudioDomainClock::Frames(_)) {
        return Some(raw);
    }
    let total = root.root_frame_at(root.domain.clock.length(), tempo).ok()?;
    if total == 0 {
        return None;
    }
    let at = |sample: u64| -> Option<u64> {
        if sample == 0 {
            Some(0)
        } else if sample == total {
            Some(root.domain.clock.length())
        } else {
            root.root_position_at(sample, tempo).ok()
        }
    };
    if raw <= 0.0 {
        let step = at(1)?;
        return (step > 0).then_some(raw / step as f64);
    }
    if raw >= root.domain.clock.length() as f64 {
        let step = root.domain.clock.length().checked_sub(at(total - 1)?)?;
        return (step > 0)
            .then_some(total as f64 + (raw - root.domain.clock.length() as f64) / step as f64);
    }
    let guess = root
        .root_frame_at(raw.round() as u64, tempo)
        .ok()?
        .min(total);
    let low = if at(guess)? as f64 > raw {
        guess.saturating_sub(1)
    } else {
        guess.min(total - 1)
    };
    let first = at(low)?;
    let step = at(low + 1)?.checked_sub(first)?;
    (step > 0).then_some(low as f64 + (raw - first as f64) / step as f64)
}
impl EditorView {
    pub(in crate::editor) fn event_fade_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key == "escape"
            && self
                .audio_event_editor
                .gesture
                .as_ref()
                .is_some_and(|g| matches!(g.kind, Kind::Fade(_)))
        {
            self.audio_event_editor.gesture = None;
            cx.notify();
            cx.stop_propagation();
            return true;
        }
        if event.keystroke.key != "f"
            || event.keystroke.modifiers.modified()
            || self.mode != crate::editor::EditorMode::Arrangement
            || !self.audio_event_focus.is_focused(window)
            || self.computer_keyboard.enabled
            || self.theme_settings.open
            || self.context_menu.is_some()
            || self.gain_numeric_editing(cx)
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return false;
        }
        if !event.is_held && !self.audio_event_editor.fade_key_held {
            self.audio_event_editor.fade_key_held = true;
            cx.notify();
        }
        cx.stop_propagation();
        true
    }
    fn fade_pointer_source_frame(
        &self,
        root: &AudioClipEdit,
        event: AudioEvent,
        x: f32,
    ) -> Option<f64> {
        let bounds = self.audio_event_editor.bounds?;
        let width = f64::from(f32::from(bounds.size.width));
        if !x.is_finite() || !width.is_finite() || width <= 0.0 {
            return None;
        }
        let local = self.event_viewport(root);
        let tempo = self.state.as_ref()?.project.tempo_map();
        let first = root
            .root_frame_at(editor_bounds(root).0, tempo)
            .ok()?
            .checked_add(local.start)?;
        let frame = first as f64
            + (f64::from(x) - f64::from(f32::from(bounds.left()))) / width * local.length as f64;
        let raw = clock_coordinate(root, frame, tempo)?
            + (i128::from(event.source_start) - i128::from(event.start)) as f64;
        frame_coordinate(root, raw, tempo).filter(|frame| frame.is_finite())
    }
    pub(super) fn event_update_fade_pointer(&mut self, x: f32, y: f32, platform: bool) {
        if !x.is_finite() || !y.is_finite() || !self.event_context_current() || self.edit_pending()
        {
            self.audio_event_editor.gesture = None;
            return;
        }
        let Some(mut gesture) = self.audio_event_editor.gesture.clone() else {
            return;
        };
        let Kind::Fade(mut fade) = gesture.kind else {
            return;
        };
        let Some((_, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(pointer) = self.fade_pointer_source_frame(&root, fade.event, x) else {
            self.audio_event_editor.gesture = None;
            return;
        };
        let delta = pointer - fade.pointer_source_frame;
        let dy = y - fade.origin_y;
        let height = self
            .audio_event_editor
            .bounds
            .map_or(8.0, |b| (f32::from(b.size.height) - 44.0).max(8.0));
        let bend = (dy * 2002.0 / height).round().clamp(-2000.0, 2000.0) as i16;
        gesture.pointer_x = x;
        fade.last_y = y;
        gesture.moved |= (x - gesture.origin_x).abs() >= 1.0 || dy.abs() >= 1.0;
        gesture.operation = if matches!(fade.handle, Handle::InBottom | Handle::OutBottom) {
            if delta.round() == 0.0 {
                None
            } else {
                let relative = if fade.handle == Handle::InBottom {
                    fade.original.in_start()
                } else {
                    fade.original.span() - fade.original.out_end_offset()
                };
                let Some(last_frame) = root
                    .event_source_clock(fade.event.source)
                    .and_then(|clock| {
                        root.event_source_frame_at(
                            fade.event.source,
                            clock.length(),
                            state.project.tempo_map(),
                        )
                    })
                    .ok()
                else {
                    return;
                };
                let frame = (i128::from(fade.source_frame_start) + i128::from(relative))
                    .saturating_add(delta.round() as i128)
                    .clamp(0, i128::from(last_frame)) as u64;
                let Some(raw) = root
                    .event_source_position_at(fade.event.source, frame, state.project.tempo_map())
                    .ok()
                else {
                    return;
                };
                let at = (i128::from(fade.event.start) + i128::from(raw)
                    - i128::from(fade.event.source_start))
                .clamp(
                    i128::from(editor_bounds(&root).0),
                    i128::from(editor_bounds(&root).1),
                ) as u64;
                let at = if platform {
                    self.event_snap_position(at, self.audio_event_editor.grid == SnapGrid::Off)
                        .unwrap_or(at)
                } else {
                    at
                };
                let (start, end) = if fade.handle == Handle::InBottom {
                    (at.min(fade.event.end - 1), fade.event.end)
                } else {
                    (fade.event.start, at.max(fade.event.start + 1))
                };
                (start != fade.event.start || end != fade.event.end).then_some(
                    AudioEventEdit::Trim {
                        event: fade.event.id,
                        start,
                        end,
                    },
                )
            }
        } else {
            let next = fade_handles::edited_fade(fade.original, fade.handle, delta, bend);
            next.filter(|next| *next != fade.original).and_then(|next| {
                let tempo = state.project.tempo_map();
                let (first, end) = if let Some(original) = fade.event.fade {
                    (
                        original.source_start(),
                        original
                            .source_start()
                            .checked_add(original.shape().span())?,
                    )
                } else {
                    (
                        fade.event.source_start,
                        fade.event.source_start.checked_add(fade.event.length())?,
                    )
                };
                let frame_end = fade.source_frame_start.checked_add(next.span())?;
                let canonical = AudioEventFade::new(fade.source_frame_start, next)
                    .ok()?
                    .reproject(|at| {
                        if at == fade.source_frame_start {
                            Ok(first)
                        } else if at == frame_end {
                            Ok(end)
                        } else {
                            root.event_source_position_at(fade.event.source, at, tempo)
                                .map(|p| p.clamp(first, end))
                        }
                    })
                    .ok()?;
                let canonical = if let Some(original) = fade.event.fade {
                    AudioEventFade::new(
                        canonical.source_start(),
                        retain_unchanged_knots(
                            original.shape(),
                            fade.original,
                            next,
                            canonical.shape(),
                        )?,
                    )
                    .ok()?
                } else {
                    canonical
                };
                Some(AudioEventEdit::SetFade {
                    event: fade.event.id,
                    fade: (!canonical.shape().is_neutral()).then_some(canonical),
                })
            })
        };
        gesture.kind = Kind::Fade(fade);
        self.audio_event_editor.gesture = Some(gesture);
    }
    pub(in crate::editor) fn update_audio_event_fade_pointer(
        &mut self,
        position: Point<gpui::Pixels>,
        platform: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .audio_event_editor
            .gesture
            .as_ref()
            .is_some_and(|g| matches!(g.kind, Kind::Fade(_)))
        {
            return false;
        }
        // Focus effects are delivered after paint. Refuse a release against the
        // current focus immediately, even before the deferred blur listener.
        if !self.audio_event_focus.is_focused(window) {
            self.audio_event_editor.gesture = None;
            self.audio_event_editor.fade_key_held = false;
            cx.notify();
            return true;
        }
        self.event_update_fade_pointer(f32::from(position.x), f32::from(position.y), platform);
        cx.notify();
        true
    }
    pub(in crate::editor) fn finish_audio_event_fade_pointer(
        &mut self,
        position: Point<gpui::Pixels>,
        platform: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.update_audio_event_fade_pointer(position, platform, window, cx) {
            return false;
        }
        if let Some(gesture) = self.audio_event_editor.gesture.take()
            && gesture.moved
            && let Some(operation) = gesture.operation
        {
            self.event_commit(operation);
        }
        cx.notify();
        true
    }
    pub(super) fn event_fade_elements(
        &self,
        root: &AudioClipEdit,
        event: AudioEvent,
        visible_start: u64,
        visible_end: u64,
        viewport_length: u64,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let held = self.audio_event_editor.fade_key_held;
        let active = self
            .audio_event_editor
            .gesture
            .as_ref()
            .is_some_and(|g| matches!(g.kind,Kind::Fade(f) if f.event.id==event.id));
        if !held && !active && event.fade.is_none_or(|fade| fade.shape().is_neutral()) {
            return Vec::new();
        }
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };
        let Some(bounds) = self.audio_event_editor.bounds else {
            return Vec::new();
        };
        let Some(geometry) = projected_fade(root, event, state.project.tempo_map()) else {
            return Vec::new();
        };
        let Some((source_start, source_end)) =
            source_window(root, event, state.project.tempo_map())
        else {
            return Vec::new();
        };
        let width = f32::from(bounds.size.width) * (visible_end - visible_start) as f32
            / viewport_length.max(1) as f32;
        let height = f32::from(bounds.size.height) - 28.0;
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 16.0 {
            return Vec::new();
        }
        let projected = event.fade.map(|_| geometry);
        let controls = geometry.shape();
        let origin = geometry.source_start();
        // Capture only a bounded curve for paint, never a project clone per event.
        let count = (width.ceil() as usize).clamp(2, 96);
        let mut gains = [1.0; 96];
        for (index, gain) in gains.iter_mut().take(count).enumerate() {
            let frame = visible_start
                + ((visible_end - visible_start - 1) as u128 * index as u128 / (count - 1) as u128)
                    as u64;
            *gain = root
                .root_position_at(frame, state.project.tempo_map())
                .ok()
                .and_then(|at| {
                    event
                        .source_start
                        .checked_add(at.saturating_sub(event.start))
                })
                .and_then(|at| {
                    root.event_source_frame_at(event.source, at, state.project.tempo_map())
                        .ok()
                })
                .map_or(1.0, |at| {
                    projected.map_or(1.0, |f| {
                        at.checked_sub(f.source_start())
                            .map_or(0.0, |p| f.shape().gain_at(p))
                    })
                });
        }
        let mut elements = vec![
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let mut path = PathBuilder::stroke(px(1.0));
                    for (index, gain) in gains.into_iter().take(count).enumerate() {
                        let p = point(
                            b.left() + b.size.width * (index as f32 / (count - 1) as f32),
                            b.top() + b.size.height * (1.0 - gain),
                        );
                        if index == 0 {
                            path.move_to(p);
                        } else {
                            path.line_to(p);
                        }
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(theme::color(theme::PaletteRole::Primary)));
                    }
                },
            )
            .absolute()
            .top(px(16.0))
            .bottom_0()
            .left_0()
            .right_0()
            .into_any_element(),
        ];
        for (handle, (position, subframe), gain) in fade_handles::knots(controls) {
            if matches!(handle, Handle::InCurve) && controls.fade_in() < 3
                || matches!(handle, Handle::OutCurve) && controls.fade_out() < 3
            {
                continue;
            }
            let Some(source) = origin.checked_add(position) else {
                continue;
            };
            if source < source_start || source >= source_end {
                continue;
            }
            let Some(frame) =
                destination_frame(root, event, state.project.tempo_map(), source, subframe)
            else {
                continue;
            };
            if frame < visible_start || frame >= visible_end {
                continue;
            }
            let inset = 4.0_f32.min(width / 2.0);
            let x = (((frame - visible_start) as f32
                / (visible_end - visible_start).saturating_sub(1).max(1) as f32)
                * width)
                .clamp(inset, width - inset)
                / width;
            let narrow = width < 16.0 || controls.span() == 1;
            let y = match handle {
                Handle::InTop => 20.0,
                Handle::OutTop => {
                    if narrow {
                        28.0
                    } else {
                        20.0
                    }
                }
                Handle::InBottom => height - 4.0,
                Handle::OutBottom => height - if narrow { 12.0 } else { 4.0 },
                _ => 16.0 + (height - 16.0) * (1.0 - gain).clamp(0.1, 0.9),
            }
            .clamp(4.0, height - 4.0);
            let selector = format!("audio-event-{}-{}", handle.name(), event.id);
            elements.push(
                div()
                    .id((handle.name(), event.id))
                    .debug_selector(move || selector.clone())
                    .absolute()
                    .left(relative(x))
                    .top(px(y))
                    .ml(px(-4.0))
                    .mt(px(-4.0))
                    .w(px(8.0))
                    .h(px(8.0))
                    .children([fade_handles::handle_shape(
                        matches!(handle, Handle::InCurve | Handle::OutCurve),
                        active,
                    )])
                    .cursor(if matches!(handle, Handle::InCurve | Handle::OutCurve) {
                        gpui::CursorStyle::Crosshair
                    } else {
                        gpui::CursorStyle::ResizeLeftRight
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::editor::waveform::ClipTooltip {
                            kind: crate::tooltips::Kind::control(handle.name().replace('-', " ")),
                            name: format!(
                                "{} · double click to reset event fades",
                                handle.name().replace('-', " ")
                            ),
                        })
                        .into()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &gpui::MouseDownEvent, window, cx| {
                            if !v.edit_pending()
                                && v.event_context_current()
                                && v.audio_event_editor.gesture.is_none()
                                && f32::from(e.position.x).is_finite()
                                && f32::from(e.position.y).is_finite()
                            {
                                v.audio_event_focus.focus(window);
                                v.advance_selection_intent();
                                v.audio_event_editor.selected = Some(event.id);
                                v.audio_event_editor.range = None;
                                if e.click_count >= 2 {
                                    v.event_commit(AudioEventEdit::SetFade {
                                        event: event.id,
                                        fade: None,
                                    });
                                } else if let Some((_, root, _)) = v.event_context()
                                    && let Some(anchor) = v.fade_pointer_source_frame(
                                        &root,
                                        event,
                                        f32::from(e.position.x),
                                    )
                                {
                                    let fade = FadeGesture {
                                        event,
                                        handle,
                                        original: controls,
                                        source_frame_start: origin,
                                        pointer_source_frame: anchor,
                                        origin_y: f32::from(e.position.y),
                                        last_y: f32::from(e.position.y),
                                    };
                                    if !v.capture_audio_event_pointer(Kind::Fade(fade), window, cx)
                                    {
                                        return;
                                    }
                                    v.audio_event_editor.gesture = Some(Gesture {
                                        anchor: 0,
                                        pointer_x: f32::from(e.position.x),
                                        origin_x: f32::from(e.position.x),
                                        moved: false,
                                        kind: Kind::Fade(fade),
                                        placement: None,
                                        operation: None,
                                    });
                                }
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .into_any_element(),
            );
        }
        elements
    }
}
