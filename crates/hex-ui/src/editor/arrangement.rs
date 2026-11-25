use super::drag::BrowserAudioDrag;
use super::viewport::Viewport;
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    Context, Div, ExternalPaths, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, Window, canvas, div, list, point, prelude::*, px, relative, rgb,
};
use std::sync::Arc;

/// Visible leading gutter width before frame `0` (bar `1`) in the Arrangement timeline.
pub(super) const LEADING_BUFFER_WIDTH: f32 = 12.0;

/// True when `x` lies inside `[left - LEADING_BUFFER_WIDTH, left + width)` (including the leading buffer).
pub(super) fn timeline_x_in_bounds_or_buffer(x: f32, left: f32, width: f32) -> bool {
    x.is_finite()
        && left.is_finite()
        && width.is_finite()
        && width > 0.0
        && x >= left - LEADING_BUFFER_WIDTH
        && x < left + width
}

/// True when `position` lies inside `bounds` or its immediately adjacent leading buffer on the left.
pub(super) fn bounds_or_leading_buffer_contains(
    bounds: gpui::Bounds<gpui::Pixels>,
    position: gpui::Point<gpui::Pixels>,
) -> bool {
    let x = f32::from(position.x);
    let y = f32::from(position.y);
    let left = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    let top = f32::from(bounds.origin.y);
    let height = f32::from(bounds.size.height);
    timeline_x_in_bounds_or_buffer(x, left, width)
        && y.is_finite()
        && top.is_finite()
        && height.is_finite()
        && height > 0.0
        && y >= top
        && y < top + height
}

impl EditorView {
    pub(super) fn begin_arrangement_pan(&mut self, x: f32, y: f32) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.mode != super::EditorMode::Arrangement
            || self.context_menu.is_some()
            || self.theme_settings.open
            || self.tuning.open
            || super::device_panel::picker_is_open(self)
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.loop_rail_gesture.is_some()
            || self.locked_play_start_drag_active()
            || self.track_resize_active()
            || self.arrangement_bounds.is_none()
            || !x.is_finite()
            || !y.is_finite()
        {
            return false;
        }
        let Some(bounds) = self.arrangement_bounds else {
            return false;
        };
        let timeline_width = (bounds.width - self.track_header_width()).max(0.0);
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
            let width = (bounds.width - self.track_header_width()).max(0.0);
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
            let next =
                current.pan_by_frames(self.viewport_horizon(state.timeline_frames), frame_delta);
            self.viewport = (next != Viewport::fit(self.viewport_horizon(state.timeline_frames)))
                .then_some(next);
        }
        if dy != 0.0 {
            let base_handle = self.track_scroll.clone();
            let offset = base_handle.offset();
            let max_offset = base_handle.max_offset();
            let y = (offset.y + px(dy)).clamp(-max_offset.height, px(0.0));
            base_handle.set_offset(point(offset.x, y));
        }
        dx != 0.0 || dy != 0.0
    }

    pub(super) fn cancel_arrangement_pan(&mut self) -> bool {
        if self.arrangement_pan.take().is_none() {
            return false;
        }
        if let Some(state) = self.state.as_ref() {
            let visible = self.visible_viewport(state.timeline_frames);
            self.install_arranger_navigation(visible);
        }
        true
    }

    pub(super) fn arrangement(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let available_width = self.workspace_body_width(window);
        if self
            .track_layout
            .available_width
            .is_some_and(|previous| previous != available_width)
        {
            self.cancel_track_resize(cx);
            self.track_scroll.remeasure();
        }
        self.track_layout.available_width = Some(available_width);
        let Some(state) = self.state.as_ref().cloned() else {
            let viewport = Viewport {
                start: 0,
                length: 1,
            };
            let upper = div()
                .flex_1()
                .min_h_0()
                .bg(rgb(theme::color(theme::PaletteRole::Background)));
            let main = self.main_track_row(viewport, &[], &[], window.scale_factor(), window, cx);
            let returns = self.return_tracks_view(cx);
            return self.arrangement_surface(upper, returns, main, None, true, cx);
        };
        let locator_state = self
            .locator_display_state()
            .unwrap_or_else(|| Arc::clone(&state));
        self.sync_track_layout(state.project.tracks());
        let viewport = self.visible_viewport(state.timeline_frames);
        let clip_move_guide = self.displayed_clip_move_guide_frame(viewport);
        let clip_editor_start = self.focused_clip_editor_play_start(window);
        let guide_line = clip_move_guide
            .and_then(|frame| viewport.fraction(frame))
            .map(|position| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right(px(self.track_header_width()))
                    .children([vertical_timeline_guide(
                        "clip-move-guide-line",
                        position,
                        theme::time_position_marker_color(),
                        1.0 / window.scale_factor().max(1.0),
                    )])
            });
        let ruler_width =
            (self.workspace_body_width(window) - self.track_header_width() - LEADING_BUFFER_WIDTH)
                .max(0.0);
        if self.track_scroll.list.item_count() == 0 {
            let empty_viewport = viewport;
            self.refresh_locators(&locator_state, empty_viewport);
            let grid = super::musical_time::grid_lines(
                &state.project,
                empty_viewport,
                ruler_width,
                self.snap_grid,
            );
            let bands = super::musical_time::adaptive_grid_bands_for_grid(
                &state.project,
                empty_viewport,
                ruler_width,
                self.snap_grid,
            );
            let upper = div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(rgb(theme::color(theme::PaletteRole::Background)))
                .child(self.ruler(&state, ruler_width, clip_move_guide, clip_editor_start, cx))
                .children(self.locator_strip(&locator_state, empty_viewport, ruler_width, cx))
                .child(super::loop_rail::view(
                    self,
                    &state,
                    empty_viewport,
                    &grid,
                    cx,
                ))
                .children([self.arrangement_body(
                    self.decorate_focused_empty_tracks(
                        window.scale_factor(),
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .children([timeline_grid_layer(
                                &bands,
                                &grid,
                                empty_viewport,
                                self.track_header_width(),
                                window.scale_factor(),
                            )])
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                                    .child(theme::text(
                                        if self.arrangement_focus.projection.all_tracks_hidden() {
                                            "no clips in selected sections · q clears section focus"
                                        } else if self
                                            .arrangement_focus
                                            .projection
                                            .tag_filter()
                                            .is_some()
                                            && !state.project.tracks().is_empty()
                                        {
                                            "no tracks match tag filter"
                                        } else {
                                            "no tracks · import audio or add a track"
                                        },
                                    )),
                            )
                            .children([loop_guides(
                                &state,
                                self,
                                empty_viewport,
                                self.track_header_width(),
                            )])
                            .children([self.locator_lines(
                                empty_viewport,
                                self.track_header_width(),
                                "locator-line",
                            )])
                            .children(self.edit_cursor_layer(
                                empty_viewport,
                                clip_move_guide,
                                clip_editor_start,
                                cx,
                            )),
                    ),
                    cx,
                )]);
            let main = self.main_track_row(
                empty_viewport,
                &bands,
                &grid,
                window.scale_factor(),
                window,
                cx,
            );
            let returns = self.return_tracks_view(cx);
            return self.arrangement_surface(
                upper,
                returns,
                main,
                guide_line,
                empty_viewport.start == 0,
                cx,
            );
        }

        self.refresh_locators(&locator_state, viewport);
        let grid: std::rc::Rc<[super::musical_time::GridLine]> =
            super::musical_time::grid_lines(&state.project, viewport, ruler_width, self.snap_grid)
                .into();
        let bands = super::musical_time::adaptive_grid_bands_for_grid(
            &state.project,
            viewport,
            ruler_width,
            self.snap_grid,
        );
        let rows_viewport = viewport;
        let rows_grid = std::rc::Rc::clone(&grid);
        let selected_rows = self.time_selection_track_indices();
        let track_scale_factor = window.scale_factor();
        let rows_entity = cx.entity();
        let rows = list(self.track_scroll.list.clone(), move |index, _, cx| {
            rows_entity
                .update(cx, |this, cx| {
                    if let Some(proposal) = this.arrangement_proposal_at(index) {
                        return match proposal {
                            super::track_proposals::TrackProposal::Device { .. } => {
                                this.proposed_plugin_track_row(cx)
                            }
                            super::track_proposals::TrackProposal::Empty(id) => {
                                this.proposed_arrangement_empty_track(id)
                            }
                        };
                    }
                    let Some(state) = this.arrangement_rows_state() else {
                        return div();
                    };
                    let Some(index) = this.arrangement_list_track_index(index) else {
                        return div();
                    };
                    let Some(track) = state.project.tracks().get(index) else {
                        return div();
                    };
                    this.track_row(
                        &state,
                        track,
                        index,
                        selected_rows,
                        super::tracks::ClipViewTimeline {
                            viewport: rows_viewport,
                            grid: &rows_grid,
                            scale_factor: track_scale_factor,
                            caption_rem_size: None,
                        },
                        cx,
                    )
                })
                .into_any_element()
        })
        .w_full()
        .flex_1()
        .min_h_0()
        // Scroll past the final real row without inventing a track. A fraction
        // preserves usable track space even in a very short arranger viewport.
        .pb(relative(super::track_layout::BOTTOM_SCROLL_SPACE_RATIO));
        let mut content = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .children([timeline_grid_layer(
                &bands,
                &grid,
                viewport,
                self.track_header_width(),
                window.scale_factor(),
            )])
            .child(rows);
        content = content.children([loop_guides(
            &state,
            self,
            viewport,
            self.track_header_width(),
        )]);
        content = content.children([self.locator_lines(
            viewport,
            self.track_header_width(),
            "locator-line",
        )]);
        content = content.children(self.edit_cursor_layer(
            viewport,
            clip_move_guide,
            clip_editor_start,
            cx,
        ));
        content = content.children(self.track_reorder_line());
        let upper = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.ruler(&state, ruler_width, clip_move_guide, clip_editor_start, cx))
            .children(self.locator_strip(&locator_state, viewport, ruler_width, cx))
            .child(super::loop_rail::view(self, &state, viewport, &grid, cx))
            .children([self.arrangement_body(content, cx)])
            .children(super::gain_lane::view(
                self,
                &state,
                viewport,
                ruler_width,
                std::rc::Rc::clone(&grid),
                cx,
            ));
        let main = self.main_track_row(viewport, &bands, &grid, window.scale_factor(), window, cx);
        let returns = self.return_tracks_view(cx);
        self.arrangement_surface(upper, returns, main, guide_line, viewport.start == 0, cx)
    }

    fn arrangement_body(&self, mut body: Div, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        if self
            .time_range_move_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.copy_modifier)
        {
            body = body.cursor(gpui::CursorStyle::DragCopy);
        }
        let mut body = body
            .id("arrangement-content")
            .debug_selector(|| "arrangement-content".to_owned())
            .when(self.context_menu.is_none(), |body| {
                body.on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
                    if let gpui::ClickEvent::Mouse(event) = event {
                        view.place_empty_arrangement_cursor(event, window, cx);
                    }
                }))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_arranger_grid_context_menu(event.position, window, cx);
                }),
            );
        let header_width = self.track_header_width();
        let measure_entity = cx.entity();
        body = body.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    measure_entity.update(cx, |view, cx| {
                        if view.clip_paint_held() && view.focus.is_focused(window) {
                            window.renew_pointer_capture(cx.entity_id(), gpui::CursorStyle::Arrow);
                        }
                        view.arrangement_bounds = Some(super::drag::LaneBounds {
                            left: f32::from(bounds.origin.x),
                            width: f32::from(bounds.size.width),
                            top: f32::from(bounds.origin.y),
                            height: f32::from(bounds.size.height),
                        });
                        view.arrangement_bounds_epoch = view.drag_layout_generation;
                        view.track_reorder_painted();
                    });
                    let zoom_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _window, cx| {
                        if !phase.capture()
                            || !bounds_or_leading_buffer_contains(bounds, event.position)
                        {
                            return;
                        }
                        if Self::row_size_wheel(event) {
                            zoom_entity.update(cx, |view, _| view.remember_row_size_scroll());
                            return;
                        }
                        if f32::from(event.position.x)
                            >= f32::from(bounds.origin.x)
                                + (f32::from(bounds.size.width) - header_width).max(0.0)
                        {
                            return;
                        }
                        let delta_y = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                        if !delta_y.is_finite() || delta_y == 0.0 {
                            return;
                        }
                        let view = zoom_entity.read(cx);
                        if view.theme_settings.open
                            || view.tuning.open
                            || super::device_panel::picker_is_open(view)
                        {
                            return;
                        }
                        cx.stop_propagation();
                        zoom_entity.update(cx, |view, cx| {
                            if view.context_menu.is_some()
                                || view.theme_settings.open
                                || view.tuning.open
                                || super::device_panel::picker_is_open(view)
                            {
                                return;
                            }
                            if view.zoom_timeline_wheel_at(
                                event.position.x,
                                f32::from(bounds.origin.x),
                                (f32::from(bounds.size.width) - header_width).max(0.0),
                                delta_y,
                            ) {
                                cx.notify();
                            }
                        });
                    });
                    let start_entity = measure_entity.clone();
                    let pan_bounds = bounds;
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if !phase.capture()
                            || event.button != MouseButton::Middle
                            || !bounds_or_leading_buffer_contains(pan_bounds, event.position)
                            || f32::from(event.position.x)
                                >= f32::from(pan_bounds.origin.x)
                                    + (f32::from(pan_bounds.size.width) - header_width).max(0.0)
                        {
                            return;
                        }
                        let started = start_entity.update(cx, |view, cx| {
                            let started = view.begin_arrangement_pan(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            );
                            if started {
                                cx.notify();
                            }
                            started
                        });
                        if started {
                            cx.stop_propagation();
                        }
                    });
                    let move_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Middle) {
                            move_entity.update(cx, |view, cx| {
                                if view.cancel_arrangement_pan() {
                                    cx.notify();
                                }
                            });
                            return;
                        }
                        let moved = move_entity.update(cx, |view, cx| {
                            let moved = view.update_arrangement_pan(
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            );
                            if moved {
                                cx.notify();
                            }
                            moved
                        });
                        if moved {
                            cx.stop_propagation();
                        }
                    });
                    let up_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if !phase.capture() || event.button != MouseButton::Middle {
                            return;
                        }
                        let canceled = up_entity.update(cx, |view, cx| {
                            let canceled = view.cancel_arrangement_pan();
                            if canceled {
                                cx.notify();
                            }
                            canceled
                        });
                        if canceled {
                            cx.stop_propagation();
                        }
                    });
                    let cursor_down_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        let x = f32::from(event.position.x);
                        let y = f32::from(event.position.y);
                        let started = cursor_down_entity.update(cx, |view, cx| {
                            // Foreground controls own the press while open.
                            if view.context_menu.is_some()
                                || view.theme_settings.open
                                || view.tuning.open
                                || super::device_panel::picker_is_open(view)
                                || view.pointer_hits_bottom_play_start_marker(
                                    event.position,
                                    event.modifiers,
                                )
                            {
                                return false;
                            }
                            // The body still handles a click. Only a drag takes
                            // over the selection, using the full painted tail.
                            if let Some(lane) = view.painted_arrangement_tail(x, y) {
                                if event.click_count == 1
                                    && !event.modifiers.shift
                                    && !event.modifiers.alt
                                    && !event.modifiers.control
                                    && !cx.has_active_drag()
                                    && view.prepare_tail_time_selection(
                                        x,
                                        y,
                                        lane,
                                        event.modifiers.platform,
                                    )
                                {
                                    view.focus.focus(window);
                                }
                                return false;
                            }
                            if view.pointer_hits_section_launch(x, y) {
                                return false;
                            }
                            if view.time_selection_key_held {
                                let started = view.begin_main_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                    event.modifiers.shift,
                                ) || view.begin_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                    event.modifiers.shift,
                                );
                                if started {
                                    view.focus.focus(window);
                                    view.focus_detail_context(window, cx);
                                    cx.notify();
                                }
                                return started;
                            }
                            if view.begin_section_pointer(
                                x,
                                y,
                                event.click_count == 2,
                                event.modifiers,
                                cx,
                            ) {
                                view.focus.focus(window);
                                if !view.capture_section_drag(window, cx) {
                                    view.cancel_section_pointer();
                                }
                                cx.notify();
                                return true;
                            }
                            if view.pointer_hits_priority_clip_control(x, y, event.modifiers.alt) {
                                return false;
                            }
                            if event.click_count == 2
                                && let Some((track, _)) = view.visible_lane_at(x, y)
                                && let Some(clip) = view.visible_clip_at_pointer(track, x, y)
                            {
                                view.select_clip_header(track, clip, event.modifiers, window, cx);
                                view.focus.focus(window);
                                return true;
                            }
                            if event.click_count == 2
                                && !event.modifiers.shift
                                && view.begin_clip_paint(x, y, event.modifiers.platform)
                            {
                                view.focus.focus(window);
                                if !view.capture_clip_paint(window, cx) {
                                    view.cancel_clip_paint();
                                }
                                cx.notify();
                                return true;
                            }
                            let move_started = !event.modifiers.shift
                                && (view.begin_main_time_range_move_gesture(x, y)
                                    || view.begin_time_range_move_gesture(x, y));
                            let started = if move_started {
                                view.update_time_range_copy_modifier(event.modifiers.alt);
                                true
                            } else {
                                view.begin_main_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                    event.modifiers.shift,
                                ) || view.begin_time_selection_gesture(
                                    x,
                                    y,
                                    event.modifiers.platform,
                                    event.modifiers.shift,
                                )
                            };
                            if started {
                                view.focus.focus(window);
                                if !move_started {
                                    view.focus_detail_context(window, cx);
                                }
                                cx.notify();
                            }
                            started
                        });
                        if started {
                            // Selection and time-range move updates invalidate the live owner/overlays only.
                            cx.stop_propagation();
                        }
                    });
                    let cursor_move_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        let x = f32::from(event.position.x);
                        let y = f32::from(event.position.y);
                        let changed = cursor_move_entity.update(cx, |view, cx| {
                            if view.sections.held() {
                                let changed = if event.pressed_button == Some(MouseButton::Left) {
                                    view.update_section_pointer(x, y, event.modifiers)
                                } else {
                                    view.cancel_section_pointer()
                                };
                                if changed {
                                    cx.notify();
                                }
                                return changed;
                            }
                            if view.clip_paint_gesture.is_some() {
                                let changed = if event.pressed_button == Some(MouseButton::Left) {
                                    view.update_clip_paint(x, y, event.modifiers.platform)
                                } else {
                                    view.cancel_clip_paint()
                                };
                                if changed {
                                    cx.notify();
                                }
                                return changed;
                            }
                            let changed =
                                view.update_time_range_move_gesture(x, y, event.modifiers.platform)
                                    || view.update_time_selection_gesture(
                                        x,
                                        y,
                                        event.modifiers.platform,
                                    );
                            if changed {
                                cx.notify();
                            }
                            changed
                        });
                        if changed {
                            cx.stop_propagation();
                        }
                    });
                    let cursor_up_entity = measure_entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        cursor_up_entity.update(cx, |view, cx| {
                            let x = f32::from(event.position.x);
                            let y = f32::from(event.position.y);
                            if view.finish_section_pointer(x, y, event.modifiers) {
                                cx.stop_propagation();
                                cx.notify();
                                return;
                            }
                            if view.finish_clip_paint(x, y, event.modifiers.platform) {
                                cx.stop_propagation();
                                cx.notify();
                                return;
                            }
                            view.refresh_time_gesture_modifiers(
                                event.modifiers.platform,
                                event.modifiers.alt,
                            );
                            let _ =
                                view.update_time_range_move_gesture(x, y, event.modifiers.platform)
                                    || view.update_time_selection_gesture(
                                        x,
                                        y,
                                        event.modifiers.platform,
                                    );
                            let finished = view.finish_time_range_move_gesture()
                                || view.finish_time_selection_gesture();
                            if finished {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
        if let Some(selection) = self.time_selection.filter(|selection| {
            self.time_selection_project_scope && selection.anchor_frame != selection.end_frame
        }) {
            let owner = cx.entity();
            body = body.child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        let view = owner.read(cx);
                        let (Some(lane), Some(state)) =
                            (view.painted_arrangement_tail_bounds(), view.state.as_ref())
                        else {
                            return;
                        };
                        let viewport = view.visible_viewport(state.timeline_frames);
                        let (start, end) = (
                            selection.anchor_frame.min(selection.end_frame),
                            selection.anchor_frame.max(selection.end_frame),
                        );
                        let Some(view_end) = viewport.end() else {
                            return;
                        };
                        let start = start.max(viewport.start);
                        let end = end.min(view_end);
                        if start >= end {
                            return;
                        }
                        let (Some(left), Some(right)) =
                            (viewport.fraction(start), viewport.fraction(end))
                        else {
                            return;
                        };
                        let fill = gpui::Background::from(rgb(theme::time_selection_fill()))
                            .opacity(theme::TIME_SELECTION_OPACITY);
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(
                                point(px(lane.left + left * lane.width), px(lane.top)),
                                gpui::size(px((right - left) * lane.width), px(lane.height)),
                            ),
                            fill,
                        ));
                    },
                )
                .absolute()
                .inset_0(),
            );
        }
        body = body.children(self.arrangement_device_drop_preview());
        if let Some(drag) = self.active_browser_audio_drag.as_ref()
            && self.browser_sampler_drop_ready(drag, cx).is_ok()
            && let Some(line_y) = self.sampler_load.header_insertion_line_y
            && let Some(bounds) = self.arrangement_bounds
            && line_y >= bounds.top
            && line_y < bounds.top + bounds.height
        {
            body = body.child(
                div()
                    .id("sampler-track-insert-preview")
                    .debug_selector(|| "sampler-track-insert-preview".to_owned())
                    .absolute()
                    .right_0()
                    .top(px((line_y - bounds.top - 1.0).max(0.0)))
                    .w(px(header_width))
                    .h(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }
        let insertion_line_y = if self
            .state()
            .is_some_and(|state| state.project.tracks().is_empty())
        {
            None
        } else {
            self.pending_sample_drop
                .as_ref()
                .and_then(|drop| drop.insertion_line_y)
                .or_else(|| self.displayed_browser_audio_drag()?.insertion_line_y.get())
                .or_else(|| {
                    self.displayed_clip_drag()
                        .and_then(|drag| drag.insertion_line_y.get())
                })
        };
        if let Some(line_y) = insertion_line_y
            && let Some(bounds) = self.arrangement_bounds
        {
            let relative_y = line_y - bounds.top;
            if relative_y.is_finite() && relative_y >= 0.0 && relative_y < bounds.height {
                body = body.child(
                    div()
                        .id("track-insert-preview")
                        .debug_selector(|| "track-insert-preview".to_owned())
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px((relative_y - 1.0).max(0.0)))
                        .h(px(2.0))
                        .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                        .children(
                            self.displayed_clip_drag()
                                .filter(|drag| drag.alias_copy())
                                .map(|_| super::tracks::alias_drag_badge()),
                        ),
                );
            }
        }
        if matches!(
            self.audible_clip_move_position(),
            Some((
                hex_session::service::ClipMovePreviewDestination::NewTrack { .. },
                _
            ))
        ) {
            let entity = cx.entity();
            body = body.child(
                div()
                    .debug_selector(|| "new-track-audition-audible-marker".into())
                    .absolute()
                    .inset_0()
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, cx| {
                                let Some((x, y)) = entity.read(cx).new_track_audible_marker()
                                else {
                                    return;
                                };
                                let top = entity
                                    .read(cx)
                                    .arrangement_bounds
                                    .map_or(y, |bounds| bounds.top);
                                let marker_top = (y - 12.0).max(top);
                                let text = theme::text("audible");
                                let text_run = gpui::TextRun {
                                    len: text.len(),
                                    font: gpui::font(theme::FONT_FAMILY),
                                    color: rgb(theme::color(theme::PaletteRole::Background)).into(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                let shaped = window.text_system().shape_line(
                                    text,
                                    px(10.0),
                                    &[text_run],
                                    None,
                                );
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(px(x), px(marker_top)),
                                        gpui::size(px(1.0), px(12.0)),
                                    ),
                                    rgb(theme::color(theme::PaletteRole::Primary)),
                                ));
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(px(x + 2.0), px(marker_top)),
                                        gpui::size(shaped.width + px(4.0), px(12.0)),
                                    ),
                                    rgb(theme::color(theme::PaletteRole::Primary)),
                                ));
                                let _ = shaped.paint(
                                    point(px(x + 4.0), px(marker_top)),
                                    px(10.0),
                                    window,
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            );
        }
        body.children(
            self.time_selection_cursor_layer()
                .map(|layer| layer.right(px(header_width))),
        )
    }

    /// Resolve the applied insertion in the current painted layout, never the pointer target.
    pub(super) fn new_track_audible_marker(&self) -> Option<(f32, f32)> {
        if self.arrangement_bounds_epoch != self.drag_layout_generation {
            return None;
        }
        let (hex_session::service::ClipMovePreviewDestination::NewTrack { before_track }, frame) =
            self.audible_clip_move_position()?
        else {
            return None;
        };
        let state = self.state.as_ref()?;
        let bounds = self.arrangement_bounds?;
        let y = self.painted_track_insertion_line(before_track)?;
        let fraction = self
            .visible_viewport(state.timeline_frames)
            .fraction(frame)?;
        let width = bounds.width - self.track_header_width();
        if !y.is_finite() || y < bounds.top || y >= bounds.top + bounds.height || width <= 0.0 {
            return None;
        }
        Some((bounds.left + width * fraction, y))
    }

    /// Paint after the body and Main have measured their current-frame bounds.
    /// The shared timeline line spans gain/Return rows without owning any input.
    fn arrangement_playback_layer(&self, cx: &Context<Self>) -> Div {
        let entity = cx.entity();
        div()
            .debug_selector(|| "arrangement-playback-layer".into())
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        let view = entity.read(cx);
                        let Some(state) = view.state.as_ref() else {
                            return;
                        };
                        let (Some(body), Some(main)) =
                            (view.arrangement_bounds, view.main_track_lane_bounds)
                        else {
                            return;
                        };
                        let width = body.width - view.track_header_width();
                        let height = main.top - body.top;
                        if !width.is_finite()
                            || !height.is_finite()
                            || width <= 0.0
                            || height <= 0.0
                        {
                            return;
                        }
                        let viewport = view.visible_viewport(state.timeline_frames);
                        // Use the audible position, compensated for Main latency.
                        let Some(position) = viewport.fraction(view.audible_cursor_position())
                        else {
                            return;
                        };
                        let bounds = gpui::Bounds::new(
                            gpui::point(
                                px(body.left + width * position.clamp(0.0, 1.0)),
                                px(body.top),
                            ),
                            gpui::size(px(1.0), px(height)),
                        );
                        window.paint_quad(gpui::fill(
                            bounds,
                            rgb(theme::color(theme::PaletteRole::Text)),
                        ));
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn arrangement_surface(
        &self,
        upper: Div,
        returns: impl IntoElement,
        main: Div,
        clip_move_guide: Option<Div>,
        at_start: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let entity = cx.entity();
        let clip_entity = entity.clone();
        let device_entity = entity.clone();
        let leading_buffer = div()
            .id("arrangement-leading-buffer")
            .debug_selector(|| "arrangement-leading-buffer".into())
            .w(px(LEADING_BUFFER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .border_r_1()
            .border_color(rgb(theme::color(if at_start {
                theme::PaletteRole::Muted
            } else {
                theme::PaletteRole::Border
            })));
        let timeline_column = div()
            .id("arrangement-timeline-column")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(upper.flex_1().min_h_0())
            .child(returns)
            .child(main)
            .child(self.arrangement_playback_layer(cx))
            .children(clip_move_guide)
            .child(self.track_resize_events(cx))
            .children([self.track_reorder_events(cx)]);
        div()
            .debug_selector(|| "arrangement-surface".into())
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .children([leading_buffer, timeline_column])
            .on_drag_move::<super::drag::NativeDeviceDrag>(move |event, _, cx| {
                let Some(payload) = event
                    .dragged_item()
                    .downcast_ref::<super::drag::NativeDeviceDrag>()
                else {
                    return;
                };
                let pointer = event.event.position;
                device_entity.update(cx, |view, cx| {
                    view.update_arrangement_device_drop(
                        payload,
                        f32::from(pointer.x),
                        f32::from(pointer.y),
                    );
                    cx.notify();
                });
            })
            .on_drop(cx.listener(
                |view, payload: &super::drag::NativeDeviceDrag, window, cx| {
                    view.drop_arrangement_device(payload, window, cx);
                },
            ))
            .on_drop(cx.listener(|view, paths: &ExternalPaths, window, cx| {
                view.drop_external_audio_paths(paths.paths(), window, cx);
            }))
            .on_drag_move::<BrowserAudioDrag>(move |event, _, cx| {
                let Some(payload) = event.dragged_item().downcast_ref::<BrowserAudioDrag>() else {
                    return;
                };
                let pointer = event.event.position;
                let pointer = (f32::from(pointer.x), f32::from(pointer.y));
                payload.last_pointer.set(Some(pointer));
                payload.snap_modifier.set(event.event.modifiers.platform);
                let (preview, lane_hovered) = entity.update(cx, |view, _| {
                    view.browser_audio_drop_position(payload, pointer.0, pointer.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                entity.update(cx, |_, cx| cx.notify());
            })
            .on_drop(cx.listener(|view, payload: &BrowserAudioDrag, window, cx| {
                if let Some((before_track, _)) =
                    view.painted_sampler_track_gap(window.mouse_position())
                {
                    view.drop_browser_sample_on_new_track(payload, before_track, cx);
                } else {
                    view.drop_browser_audio(payload, window, cx);
                }
            }))
            // Continue the typed move once per input, even when the source
            // clip leaves the virtualized rows. Child receivers own release.
            .on_drag_move::<super::drag::ClipDrag>(move |event, window, cx| {
                let Some(payload) = event.dragged_item().downcast_ref::<super::drag::ClipDrag>()
                else {
                    return;
                };
                let pointer = event.event.position;
                let pointer = (f32::from(pointer.x), f32::from(pointer.y));
                payload.last_pointer.set(Some(pointer));
                payload.snap_modifier.set(event.event.modifiers.platform);
                payload.copy_modifier.set(event.event.modifiers.alt);
                cx.set_active_drag_cursor_style(
                    super::drag::copy_cursor(payload.copy_modifier.get()),
                    window,
                );
                let (preview, lane_hovered) = clip_entity.update(cx, |view, _| {
                    view.clip_drag_drop_position(payload, pointer.0, pointer.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                clip_entity.update(cx, |view, cx| {
                    #[cfg(test)]
                    {
                        view.drag_move_event_count = view.drag_move_event_count.saturating_add(1);
                    }
                    view.sync_held_clip_move_audition();
                    cx.notify();
                });
            })
            .on_drop(
                cx.listener(|view, payload: &super::drag::ClipDrag, window, cx| {
                    view.drop_clip_to_new_track(payload, window, cx);
                }),
            )
    }

    fn main_track_row(
        &mut self,
        viewport: Viewport,
        bands: &[super::musical_time::GridBand],
        lines: &[super::musical_time::GridLine],
        scale: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = self.state.clone();
        let automation = state.as_ref().and_then(|state| {
            super::device_automation::overlay(
                self,
                state,
                super::device_panel::DevicePanelOwner::Main,
                viewport,
                cx,
            )
        });
        let lane_entity = cx.entity();
        let section_coverage = state
            .as_ref()
            .filter(|state| !state.project.arrangement_sections().is_empty())
            .map(|_| crate::section_surface::SectionCoverage::default());
        let section_elements =
            self.section_elements(viewport, scale, window, cx, section_coverage.as_ref());
        let lane = div()
            .id("main-track-lane")
            .debug_selector(|| "main-track-lane".to_owned())
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.section_lane_menu(
                        f32::from(event.position.x),
                        f32::from(event.position.y),
                        event.position,
                        event.modifiers,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .children([main_track_grid_layer(bands, lines, viewport, scale)])
            .child(super::arrangement_map::view(
                self,
                viewport,
                section_coverage,
            ))
            .children(self.main_time_selection_overlay(viewport))
            .children(section_elements)
            .children([self.section_pointer_events(cx)])
            .children(automation)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, cx| {
                        lane_entity.update(cx, |view, _| {
                            view.main_track_lane_bounds = Some(super::drag::LaneBounds {
                                left: f32::from(bounds.origin.x),
                                width: f32::from(bounds.size.width),
                                top: f32::from(bounds.origin.y),
                                height: f32::from(bounds.size.height),
                            });
                            view.main_track_lane_epoch = view.drag_layout_generation;
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(self.time_selection_cursor_layer())
            .children([self.locator_lines(viewport, 0.0, "locator-main-line")]);
        let row = div()
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _, cx| {
                view.resize_row_wheel(None, event, cx);
            }))
            .debug_selector(|| "pinned-main-track".to_owned())
            .h(px(self.main_track_height()))
            .min_h(px(super::track_layout::HEADER_ONLY_HEIGHT))
            .flex_shrink()
            .flex()
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .child(lane)
            .child(
                div()
                    .id("main-track-header")
                    .on_drop(super::sampler_load::track_drop_listener(
                        cx,
                        None,
                        hex_project::InstrumentLoadPlacement::First,
                    ))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            view.open_automation_owner_menu(
                                super::device_panel::DevicePanelOwner::Main,
                                event.position,
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_click(cx.listener(|view, _, window, cx| {
                        super::device_panel::select_main(view, window, cx)
                    }))
                    .relative()
                    .children(self.main_track_resize_handles(cx))
                    .debug_selector(|| "main-track-header".to_owned())
                    .w(px(self.track_header_width()))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .pt(px(3.))
                    .gap_1()
                    .px_1()
                    .border_l_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                    .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                    .text_xs()
                    .child(
                        div()
                            .debug_selector(|| "main-track-name".into())
                            .child(theme::text("main")),
                    )
                    .child(div().flex_1())
                    .children([super::track_meters::render_main(
                        self.track_meters.main_reading(),
                        self.main_track_height(),
                    )])
                    .children(self.track_latency_indicator(super::track_latency::Owner::Main))
                    .child(self.sample_arm_button(super::sample_recorder::Arm::Main, cx))
                    .children(
                        (self.main_track_height() > super::track_layout::HEADER_ONLY_HEIGHT)
                            .then(|| self.main_track_controls(cx)),
                    ),
            );
        let extra = state.as_ref().map(|state| {
            super::device_automation::rows(
                self,
                state,
                super::device_panel::DevicePanelOwner::Main,
                viewport,
                cx,
            )
        });
        div()
            .flex()
            .flex_col()
            .flex_shrink()
            .min_h(px(super::track_layout::HEADER_ONLY_HEIGHT))
            .child(row)
            .children(extra.map(|rows| {
                div()
                    .id("main-automation-rows")
                    .max_h(px(super::device_automation::LANE_HEIGHT))
                    .overflow_y_scroll()
                    .child(rows)
            }))
    }

    fn ruler(
        &self,
        state: &ViewState,
        ruler_width: f32,
        clip_move_guide: Option<u64>,
        clip_editor_start: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Div {
        let viewport = self.visible_viewport(state.timeline_frames);
        let elapsed = div()
            .debug_selector(|| "arrangement-elapsed-row".into())
            .relative()
            .h(relative(0.5))
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .children(
                super::elapsed_time::ruler_ticks(
                    state.project.sample_rate(),
                    viewport,
                    ruler_width,
                )
                .into_iter()
                .filter_map(|tick| {
                    let x = viewport.fraction(tick.frame)?;
                    super::timeline_strip::ruler_tick(x, tick.label, false, false, false).map(
                        |tick_view| {
                            tick_view
                                .pb_0()
                                .line_height(px(10.0))
                                .debug_selector(move || {
                                    format!("elapsed-ruler-tick-{}", tick.frame)
                                })
                        },
                    )
                }),
            );
        let beats = div()
            .debug_selector(|| "arrangement-beat-row".into())
            .relative()
            .h(relative(0.5))
            .flex_shrink_0()
            .children(
                super::musical_time::ruler_ticks(&state.project, viewport, ruler_width)
                    .into_iter()
                    .filter_map(|tick| {
                        let x = viewport.fraction(tick.frame)?;
                        super::timeline_strip::ruler_tick(
                            x,
                            tick.label,
                            tick.bar_start,
                            tick.meter_marker,
                            tick.fragmentary_bar,
                        )
                        .map(|tick_view| {
                            tick_view
                                .pb_0()
                                .line_height(px(10.0))
                                .debug_selector(move || format!("ruler-tick-{}", tick.frame))
                        })
                    }),
            );
        let labels = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .children([elapsed, beats]);
        let entity = cx.entity();
        let seek_viewport = viewport;
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
                    kind: crate::tooltips::Kind::Help,
                    description:
                        "drag to seek · option-click or drag to lock play start · hold cmd to invert snap"
                            .into(),
                })
                .into()
            })
            .children([labels])
            .children([canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    entity.update(cx, |view, cx| {
                        if view.cancel_locked_play_start_drag_on_ruler_change(bounds) {
                            cx.defer_in(window, |_, _, cx| cx.notify());
                        }
                        view.update_scrub_geometry(bounds);
                    });
                    let mouse_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if !phase.bubble()
                            || event.button != MouseButton::Left
                            || !bounds_or_leading_buffer_contains(bounds, event.position)
                        {
                            return;
                        }
                        let started = mouse_entity.update(cx, |view, cx| {
                            if event.modifiers.alt {
                                if view.begin_ruler_locked_play_start_drag(
                                    event.position,
                                    event.modifiers,
                                    seek_viewport,
                                    bounds,
                                    cx,
                                ) {
                                    view.focus.focus(window);
                                    cx.notify();
                                    true
                                } else {
                                    false
                                }
                            } else if view.begin_scrub(
                                f32::from(event.position.x),
                                seek_viewport,
                                bounds,
                            ) {
                                view.focus.focus(window);
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        });
                        if started {
                            cx.stop_propagation();
                        }
                    });
                    let move_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        let changed = move_entity.update(cx, |view, cx| {
                            if view.locked_play_start_drag_active() {
                                let changed = if event.pressed_button == Some(MouseButton::Left) {
                                    view.update_locked_play_start_drag(
                                        event.position,
                                        event.modifiers.platform,
                                    )
                                } else {
                                    view.cancel_locked_play_start_drag()
                                };
                                if changed {
                                    cx.notify();
                                }
                                return true;
                            }
                            let changed = if event.pressed_button == Some(MouseButton::Left) {
                                view.update_scrub(f32::from(event.position.x), false)
                            } else {
                                view.lose_scrub_pointer()
                            };
                            if changed {
                                cx.notify();
                            }
                            changed
                        });
                        if changed {
                            cx.stop_propagation();
                        }
                    });
                    let up_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if !phase.capture() || event.button != MouseButton::Left {
                            return;
                        }
                        if up_entity.update(cx, |view, cx| {
                            if view.locked_play_start_drag_active() {
                                if view
                                    .finish_locked_play_start_drag(event.position, event.modifiers)
                                {
                                    cx.notify();
                                }
                                return true;
                            }
                            let changed = view.update_scrub(f32::from(event.position.x), true);
                            if changed {
                                cx.notify();
                            }
                            changed
                        }) {
                            cx.stop_propagation();
                        }
                    });
                    let wheel_entity = entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                        if !phase.capture()
                            || !bounds_or_leading_buffer_contains(bounds, event.position)
                        {
                            return;
                        }
                        let delta_y = -f32::from(event.physical_delta().pixel_delta(px(80.0)).y);
                        if !delta_y.is_finite() || delta_y == 0.0 {
                            return;
                        }
                        let view = wheel_entity.read(cx);
                        if view.theme_settings.open
                            || view.tuning.open
                            || super::device_panel::picker_is_open(view)
                        {
                            return;
                        }
                        cx.stop_propagation();
                        wheel_entity.update(cx, |view, cx| {
                            if view.context_menu.is_some()
                                || view.theme_settings.open
                                || view.tuning.open
                                || super::device_panel::picker_is_open(view)
                            {
                                return;
                            }
                            if view.zoom_timeline_wheel_at(
                                event.position.x,
                                f32::from(bounds.origin.x),
                                f32::from(bounds.size.width),
                                delta_y,
                            ) {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()])
            .children(self.edit_cursor_markers(
                viewport,
                hex_widgets::TimeMarkerEdge::Top,
                clip_move_guide,
                clip_editor_start,
                cx,
            ));
        super::timeline_strip::ruler_surface()
            .debug_selector(|| "arrangement-time-ruler".into())
            .flex()
            .items_center()
            .child(cells)
            .children([self.time_controls(cx)])
    }
}

fn vertical_timeline_guide(name: &'static str, position: f32, color: u32, width: f32) -> Div {
    div()
        .debug_selector(move || name.into())
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(width))
        .when(position == 1.0, |d| d.right_0())
        .when(position != 1.0, |d| d.left(relative(position)))
        .bg(rgb(color))
}

fn loop_guides(state: &ViewState, view: &EditorView, viewport: Viewport, header_width: f32) -> Div {
    let region = super::loop_rail::displayed_region(view, state);
    let mut layer = div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right(px(header_width));
    if let Some(region) = region
        && let Some(end) = viewport.end()
        && viewport.length > 0
    {
        let color = theme::color(if region.enabled() {
            theme::PaletteRole::Text
        } else {
            theme::PaletteRole::Muted
        });
        for (frame, name) in [
            (region.start(), "loop-guide-start"),
            (region.end(), "loop-guide-end"),
        ] {
            if (viewport.start..=end).contains(&frame) {
                let position = (frame - viewport.start) as f64 / viewport.length as f64;
                layer =
                    layer.children([vertical_timeline_guide(name, position as f32, color, 1.0)
                        .opacity(if region.enabled() { 0.94 } else { 0.62 })]);
            }
        }
    }
    layer
}

fn paint_timeline_grid_canvas(
    bands: &[super::musical_time::GridBand],
    lines: &[super::musical_time::GridLine],
    viewport: super::viewport::Viewport,
    scale: f32,
) -> impl IntoElement + use<> {
    let bands: std::rc::Rc<[super::musical_time::GridBand]> = std::rc::Rc::from(bands);
    let lines: std::rc::Rc<[super::musical_time::GridLine]> = std::rc::Rc::from(lines);
    let stroke = 1.0 / scale.max(1.0);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let Some(viewport_end) = viewport.end() else {
                return;
            };
            let width = f32::from(bounds.size.width);
            if viewport.length == 0 || !width.is_finite() || width <= 0.0 {
                return;
            }
            let paint = |window: &mut Window| {
                let lane_left = f64::from(bounds.left());
                let lane_w = f64::from(width);
                let phys_scale = f64::from(scale.max(1.0));
                let snap = |v: f64| ((v * phys_scale).round() / phys_scale) as f32;
                for band in bands.iter() {
                    let start = band.start_frame.max(viewport.start);
                    let end = band.end_frame.min(viewport_end);
                    if start >= end {
                        continue;
                    }
                    let left_frac = (start - viewport.start) as f64 / viewport.length as f64;
                    let right_frac = (end - viewport.start) as f64 / viewport.length as f64;
                    let left_px = snap(lane_left + left_frac * lane_w);
                    let right_px = snap(lane_left + right_frac * lane_w);
                    let rect = gpui::Bounds::new(
                        point(px(left_px), bounds.top()),
                        gpui::size(px((right_px - left_px).max(0.0)), bounds.size.height),
                    );
                    if !rect.is_empty() {
                        window.paint_quad(gpui::fill(
                            rect,
                            rgb(theme::time_grid_band_color(band.alternate)),
                        ));
                    }
                    #[cfg(feature = "ui-test-support")]
                    window.record_debug_bounds(
                        format!("arrangement-grid-band-{}", band.start_frame),
                        rect,
                    );
                }
                let primary = gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Primary)));
                let border = gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Border)));
                for line in lines.iter() {
                    let Some(offset) = line.frame.checked_sub(viewport.start) else {
                        continue;
                    };
                    if offset > viewport.length {
                        continue;
                    }
                    let position = (offset as f64 / viewport.length as f64).clamp(0.0, 1.0);
                    let x = snap(lane_left + position * lane_w);
                    let rect = gpui::Bounds::new(
                        point(px(x), bounds.top()),
                        gpui::size(px(stroke), bounds.size.height),
                    );
                    let color = if line.meter_marker {
                        primary.opacity(0.48)
                    } else if line.bar_start {
                        border
                    } else {
                        border.opacity(0.65)
                    };
                    window.paint_quad(gpui::fill(rect, color));
                    #[cfg(feature = "ui-test-support")]
                    window
                        .record_debug_bounds(format!("arrangement-grid-line-{}", line.frame), rect);
                }
            };
            let mask = window.content_mask().bounds;
            let clipped = bounds.intersect(&mask);
            let reserved = clipped.scale(window.scale_factor());
            if !clipped.is_empty()
                && !reserved.is_empty()
                && [
                    reserved.left(),
                    reserved.right(),
                    reserved.top(),
                    reserved.bottom(),
                ]
                .into_iter()
                .all(|edge| f64::from(edge).is_finite())
            {
                window.paint_layer(bounds, paint);
            } else {
                paint(window);
            }
        },
    )
    .absolute()
    .inset_0()
}

fn timeline_grid_layer(
    bands: &[super::musical_time::GridBand],
    lines: &[super::musical_time::GridLine],
    viewport: super::viewport::Viewport,
    header_width: f32,
    scale: f32,
) -> Div {
    div()
        .debug_selector(|| "arrangement-grid".to_owned())
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right(px(header_width))
        .child(paint_timeline_grid_canvas(bands, lines, viewport, scale))
}

fn main_track_grid_layer(
    bands: &[super::musical_time::GridBand],
    lines: &[super::musical_time::GridLine],
    viewport: Viewport,
    scale: f32,
) -> Div {
    div()
        .debug_selector(|| "main-track-grid".to_owned())
        .absolute()
        .inset_0()
        .child(paint_timeline_grid_canvas(bands, lines, viewport, scale))
}
