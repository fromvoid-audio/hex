//! View-owned detail sizing. Capture survives preview collapse; cancellation
//! restores the prior open state and preferred size. Clip bodies keep 128px plus
//! the velocity lane; compact Session audio keeps its 38px row. Collapse occurs
//! below 48px (half the minimum for smaller content). No musical edit is emitted.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, IntoElement, Window};
use hex_widgets::{
    SplitterAxis, SplitterEvent, SplitterGesture, SplitterHandleConfig, SplitterInput,
};

pub(super) const MIN_HEIGHT: f32 = 128.0;
const COLLAPSE_HEIGHT: f32 = 48.0;

#[derive(Default)]
pub(super) struct State {
    preferred: Option<f32>,
    resize: Option<Resize>,
}
struct Resize {
    gesture: SplitterGesture,
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
                || !r.gesture.matches_window(window)
                || self.device_panel_active
                || !self.focus.is_focused(window)
        }) {
            self.cancel_clip_detail_resize(cx);
        }
    }
    fn resize_clip_detail(&mut self, delta: f32, window: &Window, cx: &mut Context<Self>) -> bool {
        self.reconcile_clip_detail_resize(window, cx);
        let Some(resize) = self.clip_detail_resize.resize.as_ref() else {
            return false;
        };
        if delta == 0.0 {
            self.clip_detail_resize.preferred = resize.preferred;
            self.clip_view_open = true;
            cx.notify();
            return true;
        }
        let requested = resize.height - delta;
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
            self.clip_detail_resize.preferred = resize.preferred;
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
    pub(super) fn clip_detail_splitter_input(&self, cx: &Context<Self>) -> SplitterInput<Self> {
        SplitterInput::new(
            cx.entity(),
            |view| view.clip_detail_resize.resize.as_ref().map(|r| r.gesture),
            Self::clip_detail_splitter_event,
        )
    }
    pub(super) fn clip_detail_splitter_event(
        &mut self,
        event: SplitterEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            SplitterEvent::Begin(gesture) => {
                self.cancel_midi_note_gesture();
                self.audio_event_editor.gesture = None;
                self.cancel_project_spectrum_resize(cx);
                window.focus(&self.focus);
                self.clip_detail_resize.resize = Some(Resize {
                    gesture,
                    height: self.clip_detail_body_height,
                    preferred: self.clip_detail_resize.preferred,
                    generation: self.project_generation,
                });
                cx.notify();
                true
            }
            SplitterEvent::Change(delta) | SplitterEvent::Commit(delta) => {
                if self.clip_detail_resize.resize.is_none() {
                    return false;
                }
                self.resize_clip_detail(delta, window, cx);
                if matches!(event, SplitterEvent::Commit(_)) {
                    self.clip_detail_resize.resize = None;
                    cx.notify();
                }
                true
            }
            SplitterEvent::Cancel(_) => self.cancel_clip_detail_resize(cx),
        }
    }
    pub(super) fn clip_detail_resize_handle(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        self.clip_detail_splitter_input(cx).handle(
            "clip-detail-resize".into(),
            SplitterHandleConfig::thin(
                SplitterAxis::Vertical,
                theme::color(theme::PaletteRole::Border),
            ),
        )
    }
    pub(super) fn clip_detail_pointer_events(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.clip_detail_splitter_input(cx).pointer_layer()
    }
}
