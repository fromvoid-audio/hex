//! View-owned detail sizing. Capture survives preview collapse; cancellation
//! restores the prior open state and preferred size. No musical edit is emitted.
use super::EditorView;
use crate::theme;
use gpui::{
    Context, Div, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window,
    div, prelude::*, px, rgb,
};

pub(super) const MIN_HEIGHT: f32 = 128.0;
const COLLAPSE_HEIGHT: f32 = 48.0;

#[derive(Default)]
pub(super) struct State {
    preferred: Option<f32>,
    resize: Option<Resize>,
}
struct Resize {
    y: f32,
    height: f32,
    preferred: Option<f32>,
    generation: u64,
}
impl State {
    pub(super) fn is_active(&self) -> bool {
        self.resize.is_some()
    }
    pub(super) fn preferred(&self, natural: f32, minimum: f32) -> f32 {
        self.preferred.map_or(natural, |height| height.max(minimum))
    }
}
impl EditorView {
    pub(super) fn cancel_clip_detail_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(resize) = self.clip_detail_resize.resize.take() else {
            return false;
        };
        self.clip_detail_resize.preferred = resize.preferred;
        self.clip_view_open = true;
        cx.notify();
        true
    }
    pub(super) fn reconcile_clip_detail_resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.clip_detail_resize.resize.as_ref().is_some_and(|r| {
            r.generation != self.project_generation
                || self.device_panel_active
                || !self.focus.is_focused(window)
        }) {
            self.cancel_clip_detail_resize(cx);
        }
    }
    fn resize_clip_detail(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.reconcile_clip_detail_resize(window, cx);
        let Some(resize) = self.clip_detail_resize.resize.as_ref() else {
            return false;
        };
        let delta = resize.y - f32::from(position.y);
        if delta == 0.0 {
            self.clip_detail_resize.preferred = resize.preferred;
            self.clip_view_open = true;
            cx.notify();
            return true;
        }
        let requested = resize.height + delta;
        if !requested.is_finite() {
            return self.cancel_clip_detail_resize(cx);
        }
        let session_audio = self.mode == super::EditorMode::Session
            && self.selected_cell.is_some_and(|(track, scene)| {
                self.state.as_ref().is_some_and(|state| {
                    matches!(
                        state.project.session_cell(track, scene),
                        Some(hex_project::SessionCellContent::Audio(_))
                    )
                })
            });
        let minimum = if session_audio {
            super::session_audio_panel::DETAIL_HEIGHT
        } else {
            MIN_HEIGHT
                + if self.midi_clip().is_some() {
                    self.midi_velocity.visible_height()
                } else {
                    0.0
                }
        };
        self.clip_view_open = requested >= COLLAPSE_HEIGHT.min(minimum / 2.0);
        if !self.clip_view_open {
            self.clip_detail_resize.preferred = Some(resize.height);
        } else {
            // Keep the piano useful even with an expanded velocity lane. The
            // shared workspace budget caps the viewport in smaller windows.
            self.clip_detail_resize.preferred = Some(
                self.project_spectrum_detail_height(window, requested.max(minimum))
                    .max(minimum),
            );
        }
        cx.notify();
        true
    }
    pub(super) fn clip_detail_resize_handle(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        div()
            .id("clip-detail-resize")
            .debug_selector(|| "clip-detail-resize".into())
            .h(px(4.0))
            .flex_shrink_0()
            .bg(rgb(theme::color(theme::PaletteRole::Border)))
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.cancel_midi_note_gesture();
                    view.audio_event_editor.gesture = None;
                    view.cancel_project_spectrum_resize(cx);
                    window.focus(&view.focus);
                    view.clip_detail_resize.resize = Some(Resize {
                        y: f32::from(event.position.y),
                        height: view.clip_detail_body_height,
                        preferred: view.clip_detail_resize.preferred,
                        generation: view.project_generation,
                    });
                    cx.notify();
                }),
            )
    }
    pub(super) fn clip_detail_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let handled = moved.update(cx, |view, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            view.cancel_clip_detail_resize(cx)
                        } else {
                            view.resize_clip_detail(event.position, window, cx)
                        }
                    });
                    if handled {
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    let handled = released.update(cx, |view, cx| {
                        let handled = view.resize_clip_detail(event.position, window, cx);
                        view.clip_detail_resize.resize = None;
                        handled
                    });
                    if handled {
                        cx.stop_propagation();
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}
