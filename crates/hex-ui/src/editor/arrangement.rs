use super::drag::BrowserAudioDrag;
use super::{EditorView, ViewState};
use crate::theme::{self, Palette};
use gpui::{
    Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent,
    Window, canvas, div, point, prelude::*, px, relative, rgb, uniform_list,
};

use super::tracks::TRACK_HEADER_WIDTH;
use super::viewport::Viewport;

impl EditorView {
    pub(super) fn begin_arrangement_pan(&mut self, x: f32, y: f32) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.mode != super::EditorMode::Arrangement
            || self.context_menu.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.arrangement_bounds.is_none()
            || !x.is_finite()
            || !y.is_finite()
        {
            return false;
        }
        let Some(bounds) = self.arrangement_bounds else {
            return false;
        };
        let timeline_width = (bounds.width - TRACK_HEADER_WIDTH).max(0.0);
        if timeline_width <= 0.0 {
            return false;
        }
        self.arrangement_pan = Some(super::ArrangementPan {
            generation: self.project_generation,
            revision: state.revision,
            last_x: x,
            last_y: y,
            timeline_width,
        });
        true
    }

    pub(super) fn update_arrangement_pan(&mut self, x: f32, y: f32) -> bool {
        let Some(mut pan) = self.arrangement_pan else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            self.arrangement_pan = None;
            return false;
        };
        if self.mode != super::EditorMode::Arrangement
            || self.project_generation != pan.generation
            || state.revision != pan.revision
        {
            self.arrangement_pan = None;
            return false;
        }
        if !x.is_finite() || !y.is_finite() {
            self.arrangement_pan = None;
            return false;
        }
        let dx = x - pan.last_x;
        let dy = y - pan.last_y;
        pan.last_x = x;
        pan.last_y = y;
        if let Some(bounds) = self.arrangement_bounds {
            let width = (bounds.width - TRACK_HEADER_WIDTH).max(0.0);
            if width > 0.0 {
                pan.timeline_width = width;
            }
        }
        self.arrangement_pan = Some(pan);

        let current = self.visible_viewport(state.timeline_frames);
        let frame_delta = -f64::from(dx) * current.length as f64 / f64::from(pan.timeline_width);
        if frame_delta.is_finite() && frame_delta != 0.0 {
            let frame_delta = frame_delta
                .round()
                .clamp(i128::MIN as f64, i128::MAX as f64) as i128;
            let next = current.pan_by_frames(state.timeline_frames, frame_delta);
            self.viewport = (next != Viewport::fit(state.timeline_frames)).then_some(next);
        }
        if dy != 0.0 {
            let base_handle = self.track_scroll.0.borrow().base_handle.clone();
            let offset = base_handle.offset();
            let max_offset = base_handle.max_offset();
            let y = (offset.y + px(dy)).clamp(-max_offset.height, px(0.0));
            base_handle.set_offset(point(offset.x, y));
        }
        dx != 0.0 || dy != 0.0
    }

    pub(super) fn cancel_arrangement_pan(&mut self) -> bool {
        self.arrangement_pan.take().is_some()
    }

    pub(super) fn arrangement(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state() else {
            return div().flex_1().min_h_0().bg(rgb(Palette::BACKGROUND));
        };
        let ruler_width = (f32::from(window.bounds().size.width)
            - self.browser_content_width(window)
            - TRACK_HEADER_WIDTH)
            .max(0.0);
        if state.project.tracks().is_empty() {
            let empty_viewport = self.visible_viewport(state.timeline_frames);
            let grid = super::musical_time::grid_lines(
                &state.project,
                empty_viewport,
                ruler_width,
                self.snap_grid,
            );
            let bands = super::musical_time::adaptive_grid_bands(
                &state.project,
                empty_viewport,
                ruler_width,
            );
            let overview = super::overview::widget(
                super::overview::cached(&self.overview_cache, self.project_generation, state),
                empty_viewport,
                cx.entity(),
            );
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(rgb(Palette::BACKGROUND))
                .children([overview])
                .child(self.ruler(state, ruler_width, cx))
                .children([self.arrangement_body(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .children([timeline_grid_layer(&bands, &grid, empty_viewport)])
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(rgb(Palette::MUTED))
                                .child(theme::text("no tracks · import audio or add a track")),
                        ),
                    cx,
                )]);
        }

        let viewport = self.visible_viewport(state.timeline_frames);
        let grid =
            super::musical_time::grid_lines(&state.project, viewport, ruler_width, self.snap_grid);
        let bands = super::musical_time::adaptive_grid_bands(&state.project, viewport, ruler_width);
        let overview = super::overview::widget(
            super::overview::cached(&self.overview_cache, self.project_generation, state),
            viewport,
            cx.entity(),
        );
        let rows_viewport = viewport;
        let rows_grid = grid.clone();
        let track_scale_factor = window.scale_factor();
        let playhead = self.cursor_position();
        let track_count = state.project.tracks().len();
        let rows = uniform_list(
            "track-rows",
            track_count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(state) = this.state.as_ref() else {
                    return Vec::new();
                };
                let mut items = Vec::with_capacity(range.end.saturating_sub(range.start));
                for index in range {
                    if let Some(track) = state.project.tracks().get(index) {
                        let selected = this.selected_track() == Some(track.id());
                        items.push(this.track_row(
                            state,
                            track,
                            index,
                            selected,
                            super::tracks::ClipViewTimeline {
                                viewport: rows_viewport,
                                grid: &rows_grid,
                                scale_factor: track_scale_factor,
                            },
                            cx,
                        ));
                    }
                }
                items
            }),
        )
        .w_full()
        .track_scroll(self.track_scroll.clone())
        .flex_1()
        .min_h_0();
        let mut content = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(rgb(Palette::BACKGROUND))
            .children([timeline_grid_layer(&bands, &grid, viewport)])
            .child(rows);
        if let Some(position) = viewport.fraction(playhead) {
            content = content.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right(px(TRACK_HEADER_WIDTH))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(position.clamp(0.0, 1.0)))
                            .w(px(1.0))
                            .bg(rgb(Palette::CYAN)),
                    ),
            );
        }
        content = content.children(self.edit_cursor_layer(viewport));
        content = self.arrangement_body(content, cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .children([overview])
            .child(self.ruler(state, ruler_width, cx))
            .child(content)
    }

    fn arrangement_body(&self, mut body: Div, cx: &mut Context<Self>) -> Div {
        body = body
            .debug_selector(|| "arrangement-content".to_owned())
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_track_context_menu(event.position, None, window, cx);
                }),
            );
        let measure_entity = cx.entity();
        body = body.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    measure_entity.update(cx, |view, _| {
                        view.arrangement_bounds = Some(super::drag::LaneBounds {
                            left: f32::from(bounds.origin.x),
                            width: f32::from(bounds.size.width),
                            top: f32::from(bounds.origin.y),
                            height: f32::from(bounds.size.height),
                        });
                        view.arrangement_bounds_epoch = view.drag_layout_generation;
                    });
                    let zoom_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if !phase.capture()
                            || !bounds.contains(&event.position)
                            || f32::from(event.position.x)
                                >= f32::from(bounds.origin.x)
                                    + (f32::from(bounds.size.width) - TRACK_HEADER_WIDTH).max(0.0)
                        {
                            return;
                        }
                        let delta_y = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                        if !delta_y.is_finite() || delta_y == 0.0 {
                            return;
                        }
                        cx.stop_propagation();
                        let did_zoom = zoom_entity.update(cx, |view, _| {
                            if view.context_menu.is_some() {
                                return false;
                            }
                            view.zoom_timeline_wheel_at(
                                event.position.x,
                                f32::from(bounds.origin.x),
                                (f32::from(bounds.size.width) - TRACK_HEADER_WIDTH).max(0.0),
                                delta_y,
                            )
                        });
                        if did_zoom {
                            window.refresh();
                        }
                    });
                    let start_entity = measure_entity.clone();
                    let pan_bounds = bounds;
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.capture()
                            || event.button != MouseButton::Middle
                            || !pan_bounds.contains(&event.position)
                            || f32::from(event.position.x)
                                >= f32::from(pan_bounds.origin.x)
                                    + (f32::from(pan_bounds.size.width) - TRACK_HEADER_WIDTH)
                                        .max(0.0)
                        {
                            return;
                        }
                        let started = start_entity.update(cx, |view, _| {
                            view.begin_arrangement_pan(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            )
                        });
                        if started {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let move_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Middle) {
                            let canceled =
                                move_entity.update(cx, |view, _| view.cancel_arrangement_pan());
                            if canceled {
                                window.refresh();
                            }
                            return;
                        }
                        let moved = move_entity.update(cx, |view, _| {
                            view.update_arrangement_pan(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            )
                        });
                        if moved {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let up_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if !phase.capture() || event.button != MouseButton::Middle {
                            return;
                        }
                        let canceled =
                            up_entity.update(cx, |view, _| view.cancel_arrangement_pan());
                        if canceled {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let cursor_down_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        let x = f32::from(event.position.x);
                        let y = f32::from(event.position.y);
                        let started = cursor_down_entity.update(cx, |view, _| {
                            let started = view.begin_time_range_move_gesture(x, y)
                                || view.begin_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                );
                            if started {
                                view.focus.focus(window);
                            }
                            started
                        });
                        if started {
                            // Selection handles own the gesture even when a clip is underneath.
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let cursor_move_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Left) {
                            let finished = cursor_move_entity.update(cx, |view, _| {
                                view.cancel_time_range_move_gesture()
                                    || view.finish_time_selection_gesture()
                            });
                            if finished {
                                window.refresh();
                            }
                            return;
                        }
                        let x = f32::from(event.position.x);
                        let y = f32::from(event.position.y);
                        let changed = cursor_move_entity.update(cx, |view, _| {
                            view.update_time_range_move_gesture(x, y, event.modifiers.platform)
                                || view.update_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                )
                        });
                        if changed {
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
                    let cursor_up_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        let finished = cursor_up_entity.update(cx, |view, _| {
                            view.finish_time_range_move_gesture()
                                || view.finish_time_selection_gesture()
                        });
                        if finished {
                            window.refresh();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
        let insertion_line_y = if self
            .state()
            .is_some_and(|state| state.project.tracks().is_empty())
        {
            None
        } else {
            self.active_browser_audio_drag
                .as_ref()
                .filter(|drag| self.browser_audio_drag_is_current(drag))
                .and_then(|drag| drag.insertion_line_y.get())
                .or_else(|| {
                    self.active_clip_drag
                        .as_ref()
                        .filter(|drag| self.drag_start_is_current(drag))
                        .and_then(|drag| drag.insertion_line_y.get())
                })
        };
        if let Some(line_y) = insertion_line_y {
            if let Some(bounds) = self.arrangement_bounds {
                let relative_y = line_y - bounds.top - 1.0;
                if relative_y.is_finite() && relative_y >= 0.0 && relative_y < bounds.height {
                    body = body.child(
                        div()
                            .id("track-insert-preview")
                            .debug_selector(|| "track-insert-preview".to_owned())
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(relative_y))
                            .h(px(2.0))
                            .bg(rgb(Palette::CYAN)),
                    );
                }
            }
        }
        let drag_entity = cx.entity();
        body = body.on_drag_move::<BrowserAudioDrag>(move |event, _, cx| {
            let Some(payload) = event.dragged_item().downcast_ref::<BrowserAudioDrag>() else {
                return;
            };
            let pointer = event.event.position;
            let pointer_xy = (f32::from(pointer.x), f32::from(pointer.y));
            payload.last_pointer.set(Some(pointer_xy));
            payload.snap_modifier.set(event.event.modifiers.platform);
            let (preview, lane_hovered) = drag_entity.update(cx, |view, _| {
                view.browser_audio_drop_position(payload, pointer_xy.0, pointer_xy.1)
            });
            payload.preview.set(preview);
            payload.lane_hovered.set(lane_hovered);
            drag_entity.update(cx, |_, cx| cx.notify());
        });
        body = body.on_drop(cx.listener(|view, payload: &BrowserAudioDrag, window, cx| {
            view.drop_browser_audio(payload, window, cx);
        }));
        let clip_entity = cx.entity();
        body = body.on_drag_move::<super::drag::ClipDrag>(move |event, _, cx| {
            let Some(payload) = event.dragged_item().downcast_ref::<super::drag::ClipDrag>() else {
                return;
            };
            let pointer = event.event.position;
            let pointer = (f32::from(pointer.x), f32::from(pointer.y));
            payload.last_pointer.set(Some(pointer));
            payload.snap_modifier.set(event.event.modifiers.platform);
            let (preview, lane_hovered) = clip_entity.update(cx, |view, _| {
                view.clip_drag_drop_position(payload, pointer.0, pointer.1)
            });
            payload.preview.set(preview);
            payload.lane_hovered.set(lane_hovered);
            clip_entity.update(cx, |_, cx| cx.notify());
        });
        body.on_drop(
            cx.listener(|view, payload: &super::drag::ClipDrag, window, cx| {
                view.drop_clip_to_new_track(payload, window, cx);
            }),
        )
    }

    fn ruler(&self, state: &ViewState, ruler_width: f32, cx: &mut Context<Self>) -> Div {
        let viewport = self.visible_viewport(state.timeline_frames);
        let labels = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .children(
                super::musical_time::ruler_ticks(&state.project, viewport, ruler_width)
                    .into_iter()
                    .map(|tick| {
                        let x = (tick.frame - viewport.start) as f32 / viewport.length as f32;
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(x))
                            .flex()
                            .items_center()
                            .border_l_1()
                            .border_color(rgb(if tick.meter_marker || tick.fragmentary_bar {
                                Palette::CORAL
                            } else {
                                Palette::GRID
                            }))
                            .pl_1()
                            .text_xs()
                            .text_color(rgb(if tick.meter_marker {
                                Palette::TEXT
                            } else {
                                Palette::MUTED
                            }))
                            .child(theme::text(tick.label))
                    }),
            );
        let entity = cx.entity();
        let seek_viewport = viewport;
        let mut loop_band = div().absolute().top_0().h(px(2.0));
        if let (Some(region), Some(viewport_end)) = (state.project.loop_region(), viewport.end()) {
            let start = region.start().max(viewport.start);
            let end = region.end().min(viewport_end);
            if start < end && viewport.length > 0 {
                let left = (start - viewport.start) as f64 / viewport.length as f64;
                let width = (end - start) as f64 / viewport.length as f64;
                loop_band = loop_band
                    .left(relative(left as f32))
                    .w(relative(width as f32))
                    .bg(rgb(if region.enabled() {
                        Palette::CORAL
                    } else {
                        Palette::MUTED
                    }));
            }
        }
        let cells = div()
            .id("timeline-ruler")
            .debug_selector(|| "timeline-ruler".to_owned())
            .relative()
            .h_full()
            .flex()
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .tooltip(|_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: "seek to position".into(),
                })
                .into()
            })
            .children([labels, loop_band])
            .children([canvas(
                |_, _, _| (),
                move |bounds, _, window, _cx| {
                    let mouse_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if !phase.bubble()
                            || event.button != MouseButton::Left
                            || !bounds.contains(&event.position)
                        {
                            return;
                        }
                        let Some(frame) = super::seek::pointer_to_frame(
                            f32::from(event.position.x),
                            f32::from(bounds.origin.x),
                            f32::from(bounds.size.width),
                            seek_viewport,
                        ) else {
                            return;
                        };
                        mouse_entity.update(cx, |view, _| {
                            if view.context_menu.is_none() {
                                view.request_seek(frame);
                            }
                        });
                    });
                    let wheel_entity = entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if !phase.capture() || !bounds.contains(&event.position) {
                            return;
                        }
                        let delta_y = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                        if !delta_y.is_finite() || delta_y == 0.0 {
                            return;
                        }
                        cx.stop_propagation();
                        let did_zoom = wheel_entity.update(cx, |view, _| {
                            if view.context_menu.is_some() {
                                return false;
                            }
                            view.zoom_timeline_wheel_at(
                                event.position.x,
                                f32::from(bounds.origin.x),
                                f32::from(bounds.size.width),
                                delta_y,
                            )
                        });
                        if did_zoom {
                            window.refresh();
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()]);
        div()
            .h(px(24.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(Palette::BORDER))
            .bg(rgb(Palette::SURFACE))
            .child(cells)
            .children([self.time_controls(cx)])
    }
}

pub(super) fn timeline_grid_elements(
    lines: &[super::musical_time::GridLine],
    viewport: super::viewport::Viewport,
) -> Vec<Div> {
    if viewport.length == 0 {
        return Vec::new();
    }
    lines
        .iter()
        .filter_map(|line| {
            let offset = line.frame.checked_sub(viewport.start)?;
            if offset > viewport.length {
                return None;
            }
            let position = (offset as f64 / viewport.length as f64) as f32;
            let (color, opacity) = if line.meter_marker {
                (Palette::CORAL, 0.48)
            } else if line.bar_start {
                (Palette::GRID, 0.9)
            } else {
                (Palette::GRID, 0.48)
            };
            Some(
                div()
                    .debug_selector(|| format!("arrangement-grid-line-{}", line.frame))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(position.clamp(0.0, 1.0)))
                    .w(px(if line.meter_marker { 1.5 } else { 1.0 }))
                    .bg(rgb(color))
                    .opacity(opacity),
            )
        })
        .collect()
}

fn timeline_grid_layer(
    bands: &[super::musical_time::GridBand],
    lines: &[super::musical_time::GridLine],
    viewport: super::viewport::Viewport,
) -> Div {
    div()
        .debug_selector(|| "arrangement-grid".to_owned())
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right(px(TRACK_HEADER_WIDTH))
        .children(timeline_band_elements(bands, viewport))
        .children(timeline_grid_elements(lines, viewport))
}

fn timeline_band_elements(
    bands: &[super::musical_time::GridBand],
    viewport: super::viewport::Viewport,
) -> Vec<Div> {
    if viewport.length == 0 {
        return Vec::new();
    }
    bands
        .iter()
        .filter_map(|band| {
            let start = band.start_frame.max(viewport.start);
            let end = band.end_frame.min(viewport.end()?);
            if start >= end {
                return None;
            }
            let left = (start - viewport.start) as f64 / viewport.length as f64;
            let width = (end - start) as f64 / viewport.length as f64;
            Some(
                div()
                    .debug_selector(|| format!("arrangement-grid-band-{}", band.start_frame))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(left as f32))
                    .w(relative(width as f32))
                    .bg(rgb(if band.alternate {
                        Palette::SURFACE
                    } else {
                        Palette::RAISED
                    }))
                    .opacity(if band.alternate { 0.16 } else { 0.08 }),
            )
        })
        .collect()
}
