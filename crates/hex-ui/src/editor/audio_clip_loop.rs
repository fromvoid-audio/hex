//! Audio clip loop brace in the event editor's retained-root viewport.
use super::*;
use crate::{
    editor::{musical_time::SnapGrid, viewport::Viewport},
    theme,
};
use gpui::{Bounds, Context, Div, MouseButton, Pixels, canvas, div, prelude::*, px, relative, rgb};
use hex_project::AudioClipEdit;

#[derive(Clone)]
pub(super) struct Gesture {
    generation: u64,
    revision: u64,
    clip: ClipId,
    root: AudioClipEdit,
    clip_start: u64,
    viewport: Viewport,
    bounds: Bounds<Pixels>,
    start_edge: bool,
    down_x: f32,
    preview: (u64, u64),
    looping: bool,
    grid: SnapGrid,
}
impl EditorView {
    pub(in crate::editor) fn reconcile_audio_loop(&mut self) {
        if self.clip_loop.audio_gesture.as_ref().is_some_and(|g| {
            g.generation != self.project_generation
                || self.detail_clip_id() != Some(g.clip)
                || self
                    .state
                    .as_ref()
                    .is_none_or(|s| s.revision != g.revision || s.project.clip(g.clip).is_none())
                || g.grid != self.audio_loop_grid()
                || self.audio_event_editor.source_view
        }) {
            self.cancel_audio_loop_gesture();
        }
    }
    pub(in crate::editor) fn cancel_audio_loop_gesture(&mut self) -> bool {
        self.clip_loop.audio_gesture.take().is_some()
    }
    fn audio_loop_update(&mut self, x: f32, platform: bool) {
        let Some(mut g) = self.clip_loop.audio_gesture.take() else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        if !x.is_finite()
            || self.project_generation != g.generation
            || state.revision != g.revision
            || self.detail_clip_id() != Some(g.clip)
            || self.edit_pending()
            || self.clip_loop.audio_bounds != Some(g.bounds)
            || self.event_viewport(&g.root) != g.viewport
            || g.grid != self.audio_loop_grid()
        {
            return;
        }
        if (x - g.down_x).abs() >= 3.0 {
            let tempo = state.project.tempo_map();
            let first = g.root.root_frame_at(g.root.view_start, tempo).ok();
            let horizon = g
                .root
                .root_frame_at(g.root.domain.clock.length(), tempo)
                .ok();
            let width = f32::from(g.bounds.size.width);
            if let (Some(first), Some(horizon)) = (first, horizon) {
                let offset =
                    (f64::from(x - g.down_x) / f64::from(width) * g.viewport.length as f64).round();
                let Ok(edge) = g.root.root_frame_at(
                    if g.start_edge {
                        g.root.view_start
                    } else {
                        g.root.view_end
                    },
                    tempo,
                ) else {
                    self.clip_loop.audio_gesture = Some(g);
                    return;
                };
                let raw = (i128::from(edge)
                    + offset.clamp(-(u64::MAX as f64), u64::MAX as f64) as i128)
                    .clamp(0, i128::from(horizon)) as u64;
                let absolute = i128::from(g.clip_start) + i128::from(raw) - i128::from(first);
                let snapped = u64::try_from(absolute)
                    .ok()
                    .and_then(|candidate| {
                        g.grid.with_platform_modifier(platform).snap_frame(
                            &state.project,
                            candidate,
                            Viewport {
                                start: g.clip_start.saturating_add(g.viewport.start),
                                length: g.viewport.length,
                            },
                            width,
                        )
                    })
                    .map_or(raw, |frame| {
                        (i128::from(first) + i128::from(frame) - i128::from(g.clip_start))
                            .clamp(0, i128::from(horizon)) as u64
                    });
                if let Ok(position) = g.root.root_position_at(snapped, tempo) {
                    if g.start_edge {
                        g.preview.0 = position.min(g.root.view_end - 1);
                    } else {
                        g.preview.1 =
                            position.clamp(g.root.view_start + 1, g.root.domain.clock.length());
                    }
                }
            }
        }
        self.clip_loop.audio_gesture = Some(g);
    }
    pub(in crate::editor) fn audio_loop_ruler(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let state = self.state.as_ref()?;
        let clip = self
            .detail_clip_id()
            .and_then(|id| state.project.clip(id))?;
        let id = clip.id();
        let generation = self.project_generation;
        let revision = state.revision;
        let root = clip.audio_event_root();
        let viewport = self.event_viewport(&root);
        if self.clip_loop.audio_gesture.as_ref().is_some_and(|g| {
            g.generation != self.project_generation || g.revision != state.revision || g.clip != id
        }) {
            self.clip_loop.audio_gesture = None;
        }
        let pending = self.pending_clip_loop_audio(id).map(Clip::audio_event_root);
        let (start, end) = self
            .clip_loop
            .audio_gesture
            .as_ref()
            .map(|g| g.preview)
            .or_else(|| pending.as_ref().map(|r| (r.view_start, r.view_end)))
            .unwrap_or((root.view_start, root.view_end));
        let display = pending.as_ref().unwrap_or(&root);
        let looping = display.placement.map_or(display.domain.looping, |p| {
            p.mode == hex_project::AudioViewMode::Repeat
        });
        let refusal = hex_project::preview_view_resize(
            clip,
            state.project.tempo_map(),
            AudioViewResizeOperation::CycleRange {
                start: root.view_start,
                end: root.view_end,
                looping,
            },
        )
        .err()
        .map(|e| e.to_string());
        let editable = refusal.is_none();
        let first = root
            .root_frame_at(root.view_start, state.project.tempo_map())
            .ok();
        let position = |value| {
            root.root_frame_at(value, state.project.tempo_map())
                .ok()
                .zip(first)
                .map(|(frame, first)| {
                    (frame as f64 - first as f64 - viewport.start as f64)
                        / viewport.length.max(1) as f64
                })
        };
        let (a, b) = (position(start), position(end));
        let color = theme::color(if looping {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Muted
        });
        let mut ruler = div()
            .id("audio-loop-ruler")
            .debug_selector(|| "audio-loop-ruler".into())
            .relative()
            .h(px(18.0))
            .flex_shrink_0()
            .w_full()
            .overflow_hidden()
            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
            .track_focus(&self.audio_event_focus);
        let description=refusal.unwrap_or_else(||format!("clip loop {} · drag ends · w/l uses time selection · hold cmd/control to invert snap",if looping {"on"} else {"off"}));
        ruler = ruler.tooltip(move |_, cx| {
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("clip loop"),
                description: description.clone().into(),
            })
            .into()
        });
        if let (Some(a), Some(b)) = (a, b) {
            if a < b && b > 0.0 && a < 1.0 {
                ruler = ruler.child(
                    div()
                        .absolute()
                        .top(px(5.0))
                        .left(relative(a.max(0.0) as f32))
                        .w(relative((b.min(1.0) - a.max(0.0)).max(0.0) as f32))
                        .h(px(2.0))
                        .bg(rgb(color)),
                );
            }
            for (start_edge, at) in [(true, a), (false, b)] {
                if !editable || !(0.0..=1.0).contains(&at) {
                    continue;
                }
                let selector = if start_edge {
                    "audio-loop-start"
                } else {
                    "audio-loop-end"
                };
                ruler = ruler.child(
                    div()
                        .id(selector)
                        .debug_selector(move || selector.into())
                        .absolute()
                        .left(relative(at as f32))
                        .ml(px(if start_edge { 0.0 } else { -7.0 }))
                        .top_0()
                        .w(px(7.0))
                        .h_full()
                        .cursor(if start_edge {
                            gpui::CursorStyle::ResizeLeft
                        } else {
                            gpui::CursorStyle::ResizeRight
                        })
                        .child(
                            div()
                                .absolute()
                                .top(px(3.0))
                                .w(px(3.0))
                                .h(px(9.0))
                                .bg(rgb(color))
                                .when(start_edge, |line| line.left_0())
                                .when(!start_edge, |line| line.right_0())
                                .debug_selector(move || {
                                    if start_edge {
                                        "audio-loop-start-marker".into()
                                    } else {
                                        "audio-loop-end-marker".into()
                                    }
                                }),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                                if !f32::from(event.position.x).is_finite()
                                    || view.edit_pending()
                                    || view.clip_loop.pending.is_some()
                                    || view.audio_event_editor.gesture.is_some()
                                {
                                    return;
                                }
                                let Some(state) = &view.state else {
                                    return;
                                };
                                if state.revision != revision
                                    || view.project_generation != generation
                                {
                                    return;
                                }
                                let Some(clip) = state
                                    .project
                                    .clip(id)
                                    .filter(|_| view.detail_clip_id() == Some(id))
                                else {
                                    return;
                                };
                                let Some(bounds) = view.clip_loop.audio_bounds.filter(|b| {
                                    f32::from(b.size.width).is_finite()
                                        && f32::from(b.size.width) > 0.0
                                }) else {
                                    return;
                                };
                                let root = clip.audio_event_root();
                                view.clip_loop.audio_gesture = Some(Gesture {
                                    generation: view.project_generation,
                                    revision: state.revision,
                                    clip: id,
                                    preview: (root.view_start, root.view_end),
                                    viewport: view.event_viewport(&root),
                                    root,
                                    clip_start: clip.start(),
                                    bounds,
                                    start_edge,
                                    down_x: f32::from(event.position.x),
                                    looping,
                                    grid: view.audio_loop_grid(),
                                });
                                view.audio_event_focus.focus(window);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                );
            }
        }
        let entity = cx.entity();
        Some(
            ruler.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        entity.update(cx, |view, _| {
                            let changed =
                                view.clip_loop.audio_bounds.is_some_and(|old| old != bounds);
                            view.clip_loop.audio_bounds = Some(bounds);
                            if changed {
                                view.cancel_audio_loop_gesture();
                            }
                        });
                        let e = entity.clone();
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if !phase.bubble() {
                                return;
                            }
                            e.update(cx, |view, cx| {
                                if view.clip_loop.audio_gesture.is_none() {
                                    return;
                                }
                                if event.pressed_button == Some(MouseButton::Left) {
                                    view.audio_loop_update(
                                        f32::from(event.position.x),
                                        event.modifiers.platform,
                                    );
                                } else {
                                    view.cancel_audio_loop_gesture();
                                }
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                        let e = entity.clone();
                        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if !phase.bubble() || event.button != MouseButton::Left {
                                return;
                            }
                            e.update(cx, |view, cx| {
                                if view.clip_loop.audio_gesture.is_none() {
                                    return;
                                }
                                view.audio_loop_update(
                                    f32::from(event.position.x),
                                    event.modifiers.platform,
                                );
                                if let Some(g) = view.clip_loop.audio_gesture.take()
                                    && g.preview != (g.root.view_start, g.root.view_end)
                                {
                                    view.commit_audio_clip_loop(
                                        g.clip,
                                        AudioViewResizeOperation::CycleRange {
                                            start: g.preview.0,
                                            end: g.preview.1,
                                            looping: g.looping,
                                        },
                                    );
                                }
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            ),
        )
    }
}
