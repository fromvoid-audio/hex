use super::drag::BrowserAudioDrag;
use super::{EditorView, ViewState};
use crate::theme::{self, Palette};
use gpui::{
    Context, Div, MouseButton, MouseDownEvent, Window, canvas, div, prelude::*, px, relative, rgb,
    uniform_list,
};

use super::tracks::TRACK_HEADER_WIDTH;

impl EditorView {
    pub(super) fn arrangement(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.state() else {
            return div().flex_1().min_h_0().bg(rgb(Palette::BACKGROUND));
        };
        let ruler_width = (f32::from(window.bounds().size.width)
            - self.browser_content_width(window)
            - TRACK_HEADER_WIDTH)
            .max(0.0);
        if state.project.tracks().is_empty() {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(rgb(Palette::BACKGROUND))
                .child(self.ruler(state, ruler_width, cx))
                .children([self.arrangement_body(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text("no tracks · import audio or add a track")),
                    cx,
                )]);
        }

        let viewport = self.visible_viewport(state.timeline_frames);
        let rows_viewport = viewport;
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
                        items.push(this.track_row(state, track, selected, rows_viewport, cx));
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
        content = self.arrangement_body(content, cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.ruler(state, ruler_width, cx))
            .child(content)
    }

    fn arrangement_body(&self, mut body: Div, cx: &mut Context<Self>) -> Div {
        body = body.debug_selector(|| "arrangement-content".to_owned());
        let measure_entity = cx.entity();
        body = body.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, cx| {
                    measure_entity.update(cx, |view, _| {
                        view.arrangement_bounds = Some(super::drag::LaneBounds {
                            left: f32::from(bounds.origin.x),
                            width: f32::from(bounds.size.width),
                            top: f32::from(bounds.origin.y),
                            height: f32::from(bounds.size.height),
                        });
                        view.arrangement_bounds_epoch = view.drag_layout_generation;
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
        let empty_drop_active = self
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| self.browser_audio_drag_is_current(drag))
            .is_some_and(|drag| {
                matches!(
                    drag.destination.get(),
                    Some(super::drag::BrowserDropDestination::NewTrack {
                        before_track: None,
                        ..
                    })
                )
            })
            || self
                .active_clip_drag
                .as_ref()
                .filter(|drag| self.drag_start_is_current(drag))
                .is_some_and(|drag| {
                    matches!(
                        drag.destination.get(),
                        Some(super::drag::ClipDropDestination::NewTrack {
                            before_track: None,
                            ..
                        })
                    )
                });
        let empty_arrangement = self
            .state()
            .is_some_and(|state| state.project.tracks().is_empty());
        if empty_arrangement && empty_drop_active {
            body = body.child(
                div()
                    .id("empty-track-drop-highlight")
                    .debug_selector(|| "empty-track-drop-highlight".to_owned())
                    .absolute()
                    .inset_0()
                    .bg(rgb(Palette::CYAN))
                    .opacity(0.055),
            );
        }
        if !empty_arrangement && empty_drop_active {
            if let (Some(bounds), Some(line_y)) = (self.arrangement_bounds, insertion_line_y) {
                let top = line_y - bounds.top;
                let height = bounds.top + bounds.height - line_y;
                if top.is_finite() && height.is_finite() && height > 0.0 {
                    body = body.child(
                        div()
                            .id("append-track-drop-highlight")
                            .debug_selector(|| "append-track-drop-highlight".to_owned())
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(top.max(0.0)))
                            .bottom_0()
                            .bg(rgb(Palette::CYAN))
                            .opacity(0.055),
                    );
                }
            }
        }
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
                move |bounds, _, window, _| {
                    let entity = entity.clone();
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
                        entity.update(cx, |view, _| view.request_seek(frame));
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
