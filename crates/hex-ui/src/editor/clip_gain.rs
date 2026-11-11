//! Option-drag on an Arrangement audio clip's top edge adjusts gain. The Window
//! lease owns held input only; the
//! ordinary FIFO receipt retains the released value until canonical installation.
//! Release freezes the ordinary clip-property scope: selected audio clips retain
//! their relative gains in one EditClips command; a single audio clip
//! uses its typed gain command.
//! No pointer update prepares audio.
use super::{
    EditorView, ViewState,
    drag::LaneBounds,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::theme;
use gpui::{
    Context, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, div, prelude::*, px,
    rgb,
};
use hex_project::{Clip, ClipGain, ClipId, Command, TrackId};
use hex_session::service::{Event, Request};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct State {
    held: Option<Drag>,
    pending: Option<Pending>,
}
struct Drag {
    generation: u64,
    revision: u64,
    clip: GainTarget,
    track: TrackId,
    viewport: Viewport,
    lane: LaneBounds,
    origin_y: f32,
    last_y: f32,
    moved: bool,
    value: f64,
    preview: ClipGain,
}
#[derive(Clone)]
struct GainTarget(Rc<Clip>);
impl GainTarget {
    fn id(&self) -> ClipId {
        self.0.id()
    }
    fn gain(&self) -> ClipGain {
        self.0.gain()
    }
    fn matches(&self, project: &hex_project::Project) -> bool {
        project.clip(self.0.id()) == Some(self.0.as_ref())
    }
}
struct Pending {
    command: Command,
    drag: Drag,
    receipt: Receipt,
}
impl Drag {
    fn source_current(&self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .is_some_and(|s| self.clip.matches(&s.project))
    }
    fn current(&self, view: &EditorView) -> bool {
        self.source_current(view)
            && view.mode == super::EditorMode::Arrangement
            && view.drag_available()
            && view.active_fade_drag.is_none()
            && view.active_clip_drag.is_none()
            && view.active_trim_drag.is_none()
            && view.active_midi_trim_drag.is_none()
            && view.time_selection_gesture.is_none()
            && view.time_range_move_gesture.is_none()
            && view.clip_paint_gesture.is_none()
            && !view.save_dialog_open
            && !view.export_dialog_open
            && !view.theme_settings.open
            && view.context_menu.is_none()
            && view.state.as_ref().is_some_and(|s| {
                s.revision == self.revision
                    && view.visible_viewport(s.timeline_frames) == self.viewport
            })
            && view.drag_lanes.get(&self.track).is_some_and(|lane| {
                (lane.left, lane.top, lane.width, lane.height)
                    == (
                        self.lane.left,
                        self.lane.top,
                        self.lane.width,
                        self.lane.height,
                    )
            })
    }
    fn command(&self) -> Command {
        Command::SetAudioClipGain {
            clip: self.clip.id(),
            millidb: self.preview.millidb(),
        }
    }
}
impl State {
    pub(super) fn held(&self) -> bool {
        self.held.is_some()
    }
}
impl EditorView {
    pub(super) fn cancel_clip_gain(&mut self, cx: &mut Context<Self>) {
        if self.clip_gain.held.take().is_some() {
            cx.notify();
        }
    }
    pub(super) fn reconcile_clip_gain(&mut self) {
        if self
            .clip_gain
            .held
            .as_ref()
            .is_some_and(|g| !g.current(self))
        {
            self.clip_gain.held = None;
        }
    }
    pub(super) fn clip_gain_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key == "escape" && self.clip_gain.held.is_some() {
            self.cancel_clip_gain(cx);
            cx.stop_propagation();
            return true;
        }
        false
    }
    pub(super) fn displayed_clip_gain(&self, clip: ClipId) -> Option<ClipGain> {
        self.clip_gain
            .held
            .as_ref()
            .filter(|d| d.current(self))
            .or_else(|| {
                self.clip_gain
                    .pending
                    .as_ref()
                    .filter(|p| p.drag.source_current(self))
                    .map(|p| &p.drag)
            })
            .filter(|d| d.clip.id() == clip)
            .map(|d| d.preview)
    }
    fn update_clip_gain(
        &mut self,
        position: Point<Pixels>,
        fine: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let y = f32::from(position.y);
        if !y.is_finite()
            || !f32::from(position.x).is_finite()
            || !self.focus.is_focused(window)
            || self
                .clip_gain
                .held
                .as_ref()
                .is_some_and(|d| !d.current(self))
        {
            self.cancel_clip_gain(cx);
            return;
        }
        if let Some(drag) = &mut self.clip_gain.held {
            // Integrate each segment so changing Shift does not jump the value.
            drag.value = (drag.value
                + (f64::from(drag.last_y) - f64::from(y)) * if fine { 10.0 } else { 100.0 })
            .clamp(
                f64::from(ClipGain::MIN_MILLIDB),
                f64::from(ClipGain::MAX_MILLIDB),
            );
            drag.last_y = y;
            drag.moved |= (f64::from(y) - f64::from(drag.origin_y)).abs() >= 1.0;
            let value = if !drag.moved {
                drag.clip.gain().millidb()
            } else {
                drag.value.round() as i32
            };
            if let Ok(gain) = ClipGain::new(value) {
                drag.preview = gain;
            }
            cx.notify();
        }
    }
    fn finish_clip_gain(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.clip_gain.held.take()
            && drag.current(self)
            && drag.preview != drag.clip.gain()
        {
            let selected = self.selected_clip_ids();
            let command = if selected.len() > 1 && selected.contains(&drag.clip.id()) {
                Command::EditClips {
                    clips: selected,
                    edit: hex_project::ClipEdit::GainBy {
                        // Both validated values are in -60_000..=24_000.
                        millidb: drag.preview.millidb() - drag.clip.gain().millidb(),
                    },
                }
            } else {
                drag.command()
            };
            self.clip_gain.pending = Some(Pending {
                command: command.clone(),
                drag,
                receipt: Receipt::new(self.project_generation),
            });
            if !self.enqueue(Request::Edit(command)) {
                self.retire_clip_gain();
            }
        }
        cx.notify();
    }
    pub(super) fn clip_gain_handle(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: &Clip,
        width: f32,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.gain_handle(
            state,
            track,
            GainTarget(Rc::new(clip.clone())),
            width,
            viewport,
            cx,
        )
    }
    fn gain_handle(
        &self,
        state: &ViewState,
        track: TrackId,
        clip: GainTarget,
        width: f32,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let clip_id = clip.id();
        let held = self
            .clip_gain
            .held
            .as_ref()
            .is_some_and(|d| d.clip.id() == clip_id && d.current(self));
        if !width.is_finite()
            || width < 28.0
            || (!held && (self.fade_key_held || self.time_selection_key_held))
            || self.track_height(track) <= super::track_layout::HEADER_ONLY_HEIGHT
        {
            return None;
        }
        let lane = *self.drag_lanes.get(&track)?;
        let generation = self.project_generation;
        let revision = state.revision;
        let original = clip;
        let owner = cx.entity_id();
        let displayed = self.displayed_clip_gain(clip_id);
        let value = displayed.unwrap_or(original.gain()).millidb();
        Some(
            div()
                .id(("clip-gain-handle", clip_id.value()))
                .debug_selector(move || format!("clip-gain-handle-{}", clip_id.value()))
                .absolute()
                .top_0()
                .left(px(9.0))
                .right(px(9.0))
                .h(px(4.0))
                .when(self.option_held || held, |s| {
                    s.cursor(gpui::CursorStyle::ResizeUpDown)
                        .hover(|s| s.bg(rgb(theme::color(theme::PaletteRole::Secondary))))
                })
                .when(held, |s| {
                    s.children([gpui::canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            window.renew_pointer_capture(owner, gpui::CursorStyle::ResizeUpDown);
                        },
                    )
                    .absolute()
                    .inset_0()])
                })
                .tooltip(move |_, cx| {
                    cx.new(|_| super::waveform::ClipTooltip {
                        kind: crate::tooltips::Kind::control("clip gain"),
                        name: format!(
                            "gain {:+.1} db · option-drag · shift for fine",
                            f64::from(value) / 1000.0
                        ),
                    })
                    .into()
                })
                .when(displayed.is_some(), |s| {
                    s.child(
                        div()
                            .debug_selector(move || format!("clip-gain-value-{}", clip_id.value()))
                            .absolute()
                            .top(px(4.0))
                            .left_0()
                            .text_xs()
                            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                            .child(theme::text(format!("{:+.1} db", f64::from(value) / 1000.0))),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        if !event.modifiers.alt {
                            return;
                        }
                        cx.stop_propagation();
                        let drag = Drag {
                            generation,
                            revision,
                            clip: original.clone(),
                            track,
                            viewport,
                            lane,
                            origin_y: f32::from(event.position.y),
                            last_y: f32::from(event.position.y),
                            moved: false,
                            value: f64::from(original.gain().millidb()),
                            preview: original.gain(),
                        };
                        if !drag.current(view)
                            || view.clip_gain.held.is_some()
                            || view.clip_gain.pending.is_some()
                            || view.fade_key_held
                            || view.time_selection_key_held
                            || !drag.origin_y.is_finite()
                        {
                            return;
                        }
                        let owner = cx.entity().downgrade();
                        let cancelled = owner.clone();
                        if window.capture_pointer(
                            cx.entity_id(),
                            MouseButton::Left,
                            gpui::CursorStyle::ResizeUpDown,
                            move |event, window, cx| {
                                let moved = event.downcast_ref::<MouseMoveEvent>();
                                let released = event
                                    .downcast_ref::<MouseUpEvent>()
                                    .filter(|e| e.button == MouseButton::Left);
                                if let Some((position, fine)) = moved
                                    .map(|e| (e.position, e.modifiers.shift))
                                    .or_else(|| released.map(|e| (e.position, e.modifiers.shift)))
                                {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.update_clip_gain(position, fine, window, cx);
                                        if released.is_some() {
                                            view.finish_clip_gain(cx);
                                        }
                                    });
                                }
                            },
                            move |_, cx| {
                                let _ = cancelled.update(cx, |view, cx| view.cancel_clip_gain(cx));
                            },
                            cx,
                        ) {
                            view.focus.focus(window);
                            view.clip_gain.held = Some(drag);
                            cx.notify();
                        }
                    }),
                )
                .on_click(|event, _, cx| {
                    if matches!(event, gpui::ClickEvent::Mouse(mouse) if mouse.down.modifiers.alt) {
                        cx.stop_propagation();
                    }
                })
                .into_any_element(),
        )
    }
    pub(super) fn clip_gain_matches(&self, request: &Request) -> bool {
        self.clip_gain.pending.as_ref().is_some_and(|p| {
            p.receipt.is_queued()
                && match (&p.command, request) {
                    (
                        Command::SetAudioClipGain {
                            clip: a,
                            millidb: av,
                        },
                        Request::Edit(Command::SetAudioClipGain {
                            clip: b,
                            millidb: bv,
                        }),
                    ) => (a, av) == (b, bv),
                    (
                        Command::EditClips { clips: a, edit: av },
                        Request::Edit(Command::EditClips { clips: b, edit: bv }),
                    ) => (a, av) == (b, bv),
                    _ => false,
                }
        })
    }

    pub(super) fn clip_gain_submitted(&mut self) {
        if let Some(p) = &mut self.clip_gain.pending {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn clip_gain_complete(&mut self, event: &Event) {
        if self.clip_gain.pending.as_mut().is_some_and(|p| {
            p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        }) {
            self.retire_clip_gain();
        }
    }
    pub(super) fn retire_clip_gain(&mut self) -> bool {
        self.clip_gain.pending.take().is_some()
    }
    pub(super) fn retire_abandoned_clip_gain(&mut self) -> bool {
        let abandoned = self.clip_gain.pending.as_ref().is_some_and(|p| {
            !p.drag.source_current(self)
                || (p.receipt.is_queued()
                    && !self
                        .pending_requests
                        .iter()
                        .any(|r| self.clip_gain_matches(r))
                    && !self
                        .device_gesture_barriers
                        .iter()
                        .any(|r| self.clip_gain_matches(r)))
        });
        abandoned && self.retire_clip_gain()
    }
}
