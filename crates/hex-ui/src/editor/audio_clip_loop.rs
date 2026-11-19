//! Audio clip scope bar in the event editor's retained-root viewport.
//! Window input capture ends independently of the existing clip-loop receipt.
use super::*;
use crate::editor::audio_event_editor::{editor_bounds, event_grid_origin};
use crate::editor::{musical_time::SnapGrid, signed_frame_viewport::SignedFrameViewport};
use gpui::{Bounds, Context, Div, MouseButton, Pixels, canvas, div, prelude::*, px, relative};
use hex_project::AudioClipEdit;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handle {
    Start,
    End,
    Body,
}

impl Handle {
    fn cursor(self, held: bool) -> gpui::CursorStyle {
        match self {
            Self::Start => gpui::CursorStyle::ResizeLeft,
            Self::End => gpui::CursorStyle::ResizeRight,
            Self::Body if held => gpui::CursorStyle::ClosedHand,
            Self::Body => gpui::CursorStyle::OpenHand,
        }
    }
}

#[derive(Clone)]
pub(super) struct Gesture {
    generation: u64,
    revision: u64,
    clip: ClipId,
    root: AudioClipEdit,
    clip_start: u64,
    viewport: SignedFrameViewport,
    bounds: Bounds<Pixels>,
    handle: Handle,
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
    fn capture_audio_loop(
        &self,
        gesture: &Gesture,
        window: &mut gpui::Window,
        cx: &Context<Self>,
    ) -> bool {
        let generation = gesture.generation;
        let revision = gesture.revision;
        let clip = gesture.clip;
        let handle = gesture.handle;
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            handle.cursor(true),
            move |event, window, cx| {
                let moved = event.downcast_ref::<gpui::MouseMoveEvent>();
                let released = event
                    .downcast_ref::<gpui::MouseUpEvent>()
                    .filter(|e| e.button == MouseButton::Left);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.clip_loop.audio_gesture.as_ref().is_some_and(|g| {
                        g.generation == generation
                            && g.revision == revision
                            && g.clip == clip
                            && g.handle == handle
                    }) {
                        return;
                    }
                    let changed = if !view.clip_view_open
                        || view.device_panel_active
                        || view.audio_event_editor.source_view
                        || !view.audio_event_focus.is_focused(window)
                        || !f32::from(position.x).is_finite()
                        || !f32::from(position.y).is_finite()
                    {
                        view.cancel_audio_loop_gesture()
                    } else {
                        let platform = moved
                            .map(|e| e.modifiers.platform)
                            .or_else(|| released.map(|e| e.modifiers.platform))
                            .unwrap_or(false);
                        let changed = view.audio_loop_update(f32::from(position.x), platform);
                        if released.is_some()
                            && let Some(g) = view.clip_loop.audio_gesture.take()
                            && g.preview != (g.root.view_start, g.root.view_end)
                        {
                            view.commit_audio_clip_loop(
                                g.clip,
                                AudioViewResizeOperation::ScopeRange {
                                    start: g.preview.0,
                                    end: g.preview.1,
                                    looping: g.looping,
                                },
                            );
                        }
                        changed
                    };
                    if changed || released.is_some() {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.clip_loop.audio_gesture.as_ref().is_some_and(|g| {
                        g.generation == generation
                            && g.revision == revision
                            && g.clip == clip
                            && g.handle == handle
                    }) {
                        view.cancel_audio_loop_gesture();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    fn audio_loop_update(&mut self, x: f32, platform: bool) -> bool {
        let Some(mut g) = self.clip_loop.audio_gesture.take() else {
            return false;
        };
        let Some(state) = &self.state else {
            return true;
        };
        if !x.is_finite()
            || self.project_generation != g.generation
            || state.revision != g.revision
            || self.detail_clip_id() != Some(g.clip)
            || self.edit_pending()
            || self.clip_loop.audio_bounds != Some(g.bounds)
            || self.event_frame_window(&g.root) != g.viewport
            || g.grid != self.audio_loop_grid()
        {
            return true;
        }
        let previous = g.preview;
        if (x - g.down_x).abs() >= 3.0 {
            let tempo = state.project.tempo_map();
            let first = g.root.root_frame_at(event_grid_origin(&g.root), tempo).ok();
            let horizon = g
                .root
                .root_frame_at(g.root.domain.clock.length(), tempo)
                .ok();
            let width = f32::from(g.bounds.size.width);
            if let (Some(first), Some(horizon)) = (first, horizon) {
                let offset =
                    (f64::from(x - g.down_x) / f64::from(width) * g.viewport.length as f64).round();
                let Ok(edge) = g.root.root_frame_at(
                    match g.handle {
                        Handle::Start | Handle::Body => g.root.view_start,
                        Handle::End => g.root.view_end,
                    },
                    tempo,
                ) else {
                    self.clip_loop.audio_gesture = Some(g);
                    return false;
                };
                let raw = (i128::from(edge)
                    + offset.clamp(-(u64::MAX as f64), u64::MAX as f64) as i128)
                    .clamp(0, i128::from(horizon)) as u64;
                let absolute = i128::from(g.clip_start) + i128::from(raw) - i128::from(first);
                let snapped = self
                    .event_project_window(&g.root)
                    .and_then(|window| {
                        g.grid.with_platform_modifier(platform).snap_signed_frame(
                            &state.project,
                            absolute,
                            window,
                            width,
                        )
                    })
                    .map_or(raw, |frame| {
                        (i128::from(first) + frame - i128::from(g.clip_start))
                            .clamp(0, i128::from(horizon)) as u64
                    });
                if let Ok(position) = g.root.root_position_at(snapped, tempo) {
                    match g.handle {
                        Handle::Start => g.preview.0 = position.min(g.root.view_end - 1),
                        Handle::End => {
                            g.preview.1 =
                                position.clamp(g.root.view_start + 1, g.root.domain.clock.length());
                        }
                        Handle::Body => {
                            let length = g.root.view_end - g.root.view_start;
                            let start = position.min(g.root.domain.clock.length() - length);
                            g.preview = (start, start + length);
                        }
                    }
                }
            }
        } else if g.handle == Handle::Body {
            g.preview = (g.root.view_start, g.root.view_end);
        }
        let changed = g.preview != previous;
        self.clip_loop.audio_gesture = Some(g);
        changed
    }
    pub(in crate::editor) fn audio_scope_preview_active(
        &self,
        clip: ClipId,
        state: &super::super::ViewState,
    ) -> bool {
        self.pending_clip_loop_audio(clip).is_some()
            || self.clip_loop.audio_gesture.as_ref().is_some_and(|g| {
                g.clip == clip
                    && g.generation == self.project_generation
                    && g.revision == state.revision
                    && g.preview != (g.root.view_start, g.root.view_end)
            })
            || (self.detail_clip_id() == Some(clip)
                && self.held_audio_play_start_position().is_some())
    }
    /// Arrangement geometry and peaks share the detail editor's held and
    /// released scope/start edit through its ordinary receipt.
    pub(in crate::editor) fn audio_scope_display_clip<'a>(
        &'a self,
        clip: &'a Clip,
        state: &super::super::ViewState,
    ) -> std::borrow::Cow<'a, Clip> {
        if let Some(g) = self.clip_loop.audio_gesture.as_ref().filter(|g| {
            g.clip == clip.id()
                && g.generation == self.project_generation
                && g.revision == state.revision
                && g.preview != (g.root.view_start, g.root.view_end)
        }) && let Some(asset) = state.project.asset(clip.asset())
            && let Ok(preview) = hex_project::preview_audio_view_resize(
                clip,
                state.project.tempo_map(),
                asset.playback_frames(),
                AudioViewResizeOperation::ScopeRange {
                    start: g.preview.0,
                    end: g.preview.1,
                    looping: g.looping,
                },
            )
        {
            return std::borrow::Cow::Owned(preview);
        }
        if self.detail_clip_id() == Some(clip.id())
            && let Some(position) = self.held_audio_play_start_position()
            && let Some(asset) = state.project.asset(clip.asset())
            && let Ok(preview) = hex_project::preview_audio_view_resize(
                clip,
                state.project.tempo_map(),
                asset.playback_frames(),
                AudioViewResizeOperation::PlayStart { position },
            )
        {
            return std::borrow::Cow::Owned(preview);
        }
        std::borrow::Cow::Borrowed(self.pending_clip_loop_audio(clip.id()).unwrap_or(clip))
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
        let viewport = self.event_frame_window(&root);
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
            AudioViewResizeOperation::ScopeRange {
                start: root.view_start,
                end: root.view_end,
                looping,
            },
        )
        .err()
        .map(|e| e.to_string());
        let editable = refusal.is_none();
        let first = root
            .root_frame_at(editor_bounds(&root).0, state.project.tempo_map())
            .ok();
        let position = |value| {
            root.root_frame_at(value, state.project.tempo_map())
                .ok()
                .zip(first)
                .and_then(|(frame, first)| {
                    viewport
                        .shifted(i128::from(first))?
                        .fraction(i128::from(frame))
                })
        };
        let (a, b) = (position(start), position(end));
        let mut ruler = crate::editor::timeline_strip::loop_surface()
            .id("audio-loop-ruler")
            .debug_selector(|| "audio-loop-ruler".into())
            .w_full()
            .track_focus(&self.audio_event_focus);
        let description=refusal.unwrap_or_else(||format!("clip scope · looping {} · drag scope bar or its ends · w/l uses time selection · hold cmd/control to invert snap",if looping {"on"} else {"off"}));
        ruler = ruler.tooltip(move |_, cx| {
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("scope bar"),
                description: description.clone().into(),
            })
            .into()
        });
        if let (Some(a), Some(b)) = (a, b) {
            ruler = ruler.children(crate::editor::timeline_strip::loop_range(
                a,
                b,
                looping,
                ["audio-loop-start-marker", "audio-loop-end-marker"],
            ));
            let body_start = a.clamp(0.0, 1.0);
            let body_end = b.clamp(0.0, 1.0);
            // Insert the body first so the existing edge handles retain hit priority.
            for (handle, at, selector) in [
                (Handle::Body, body_start, "audio-scope-body"),
                (Handle::Start, a, "audio-loop-start"),
                (Handle::End, b, "audio-loop-end"),
            ] {
                if !editable
                    || !(0.0..=1.0).contains(&at)
                    || (handle == Handle::Body && body_start >= body_end)
                {
                    continue;
                }
                ruler = ruler.child(
                    div()
                        .id(selector)
                        .debug_selector(move || selector.into())
                        .absolute()
                        .left(relative(at as f32))
                        .top_0()
                        .h_full()
                        .when(handle == Handle::Body, |target| {
                            target.w(relative((body_end - body_start) as f32))
                        })
                        .when(handle != Handle::Body, |target| target.w(px(7.0)))
                        .when(handle == Handle::End, |target| target.ml(px(-7.0)))
                        .cursor(handle.cursor(false))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                                if !f32::from(event.position.x).is_finite()
                                    || !f32::from(event.position.y).is_finite()
                                    || !view.clip_view_open
                                    || view.device_panel_active
                                    || view.audio_event_editor.source_view
                                    || view.edit_pending()
                                    || view.clip_loop.pending.is_some()
                                    || view.clip_loop.audio_gesture.is_some()
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
                                let gesture = Gesture {
                                    generation: view.project_generation,
                                    revision: state.revision,
                                    clip: id,
                                    preview: (root.view_start, root.view_end),
                                    viewport: view.event_frame_window(&root),
                                    root,
                                    clip_start: clip.start(),
                                    bounds,
                                    handle,
                                    down_x: f32::from(event.position.x),
                                    looping,
                                    grid: view.audio_loop_grid(),
                                };
                                if !view.capture_audio_loop(&gesture, window, cx) {
                                    return;
                                }
                                view.clip_loop.audio_gesture = Some(gesture);
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
                        // Canonical context is reconciled by clip_detail_view;
                        // only witness this existing held geometry during paint.
                        let view = entity.read(cx);
                        if view.clip_view_open
                            && !view.device_panel_active
                            && !view.audio_event_editor.source_view
                            && view.audio_event_focus.is_focused(window)
                            && let Some(g) = view.clip_loop.audio_gesture.as_ref()
                            && g.bounds == bounds
                            && g.generation == view.project_generation
                            && view
                                .state
                                .as_ref()
                                .is_some_and(|s| s.revision == g.revision)
                        {
                            window.renew_pointer_capture(entity.entity_id(), g.handle.cursor(true));
                        }
                    },
                )
                .absolute()
                .inset_0(),
            ),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "audio_loop_capture_tests.rs"]
mod capture_tests;
