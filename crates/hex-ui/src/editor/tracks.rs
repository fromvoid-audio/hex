use super::drag::{self, ClipDrag, LaneBounds};
use super::trim::TrimDrag;
use super::viewport::Viewport;
use super::waveform::{ClipTooltip, button, fade_envelope_samples, waveform_view};
use super::{EditorView, ViewState};
use crate::theme::{self, Palette};
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb,
};
use hex_project::{Clip, Track};

const TRACK_ROW_HEIGHT: f32 = 44.0;
pub(super) const TRACK_HEADER_WIDTH: f32 = 176.0;

impl EditorView {
    pub(super) fn track_row(
        &self,
        state: &ViewState,
        track: &Track,
        selected: bool,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Div {
        let track_id = track.id();
        let Some(viewport_end) = viewport.end() else {
            return div().w_full().h(px(TRACK_ROW_HEIGHT));
        };
        let mut clip_area = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(TRACK_ROW_HEIGHT))
            .overflow_hidden()
            .border_r_1()
            .border_b_1()
            .border_color(rgb(Palette::BORDER));
        for clip in track.clips() {
            let Some(end) = clip.start().checked_add(clip.duration()) else {
                continue;
            };
            if clip.start() >= viewport_end || end <= viewport.start {
                continue;
            }
            let selected_clip = self.selected_clip() == Some(clip.id());
            clip_area =
                clip_area.child(self.clip_view(state, track_id, clip, selected_clip, viewport, cx));
        }
        let mut midi_shape_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
        for clip in track.midi_clips() {
            let Some(preview) = state.midi_preview.get(clip.id()) else {
                continue;
            };
            let (start, end) = (preview.frame_start, preview.frame_end);
            if end <= start {
                continue;
            }
            if start >= viewport_end || end <= viewport.start {
                continue;
            }
            let selected_clip = self.selected_clip() == Some(clip.id());
            let left = start.max(viewport.start).saturating_sub(viewport.start) as f64
                / viewport.length as f64;
            let width = end
                .min(viewport_end)
                .saturating_sub(start.max(viewport.start)) as f64
                / viewport.length as f64;
            let midi_clip_id = clip.id();
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport_end);
            let note_shapes = preview
                .samples
                .iter()
                .take(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
                .filter_map(|sample| {
                    super::midi_thumbnail::note_shape(
                        sample.frame_start,
                        sample.frame_gate_end,
                        sample.pitch,
                        visible_start,
                        visible_end,
                    )
                })
                .take(midi_shape_budget);
            let description = format!(
                "{} source notes · {} arrangement occurrences · sampled preview",
                preview.authored_note_count, preview.occurrence_count,
            );
            let mut midi = div()
                .id(("midi-clip", clip.id().value()))
                .absolute()
                .top(px(4.0))
                .bottom(px(4.0))
                .left(relative(left as f32))
                .w(relative(width as f32))
                .min_w(px(4.0))
                .overflow_hidden()
                .cursor_pointer()
                .opacity(if selected_clip { 0.92 } else { 0.68 })
                .bg(rgb(if selected_clip {
                    Palette::CORAL_DARK
                } else {
                    Palette::CLIP
                }))
                .border_l_2()
                .border_color(rgb(if selected_clip {
                    Palette::CORAL
                } else {
                    Palette::CYAN
                }))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.set_selected_clip(track_id, midi_clip_id);
                    cx.notify();
                }));
            midi = midi.tooltip(move |_, cx| {
                cx.new(|_| ClipTooltip {
                    name: description.clone(),
                })
                .into()
            });
            let mut shown = 0;
            for (x, w, pitch) in note_shapes {
                shown += 1;
                let top = f32::from(127_u8.saturating_sub(pitch)) / 128.0;
                midi = midi.child(
                    div()
                        .absolute()
                        .left(relative(x))
                        .top(relative(top))
                        .w(relative(w))
                        .h(px(2.0))
                        .bg(rgb(Palette::CYAN)),
                );
            }
            midi_shape_budget = midi_shape_budget.saturating_sub(shown);
            if preview.occurrence_count > shown {
                midi = midi.child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(Palette::TEXT))
                        .bg(rgb(Palette::RAISED))
                        .child(theme::text(format!("{shown}/{}", preview.occurrence_count))),
                );
            }
            clip_area = clip_area.children([midi]);
        }
        let lane_entity = cx.entity();
        clip_area = clip_area.children([canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                let lane = LaneBounds {
                    left: f32::from(bounds.origin.x),
                    width: f32::from(bounds.size.width),
                };
                lane_entity.update(cx, |view, view_cx| {
                    let changed = view
                        .drag_lanes
                        .insert(track_id, lane)
                        .is_none_or(|old| old.left != lane.left || old.width != lane.width);
                    if changed {
                        view_cx.notify();
                    }
                });
            },
        )
        .absolute()
        .inset_0()]);
        clip_area = clip_area
            .drag_over::<ClipDrag>(|style, _, _, _| {
                style.border_1().border_color(rgb(Palette::CYAN))
            })
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, track_id, window, cx);
            }))
            .drag_over::<TrimDrag>(move |style, drag, _, _| {
                if drag.source_track == track_id {
                    style.border_1().border_color(rgb(Palette::CORAL))
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |view, drag: &TrimDrag, window, cx| {
                view.drop_trim(drag, track_id, window, cx);
            }));
        let track_name = track.name().to_owned();
        let track_name_view = div()
            .id(("track-name", track_id.value()))
            .w(px(TRACK_HEADER_WIDTH - 10.0))
            .flex_shrink_0()
            .min_w_0()
            .h(px(18.0))
            .cursor_pointer()
            .truncate()
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .child(theme::text(track.name()))
            .tooltip(move |_, cx| {
                cx.new(|_| ClipTooltip {
                    name: track_name.clone(),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                view.select_track(track_id);
                cx.notify();
            }));
        let track_controls = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                button("+", "increase track gain", ("gain-up", track_id.value())).on_click(
                    cx.listener(move |view, _, _, cx| {
                        view.gain_by(track_id, 0.1);
                        cx.notify();
                    }),
                ),
            )
            .child(
                button("−", "decrease track gain", ("gain-down", track_id.value())).on_click(
                    cx.listener(move |view, _, _, cx| {
                        view.gain_by(track_id, -0.1);
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .w(px(34.0))
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .child(theme::text(format!("{:.0}%", track.gain() * 100.0))),
            )
            .child(
                button(
                    "●",
                    if track.muted() {
                        "unmute track"
                    } else {
                        "mute track"
                    },
                    ("track-mute", track_id.value()),
                )
                .text_color(rgb(if track.muted() {
                    Palette::CORAL
                } else {
                    Palette::MUTED
                }))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.toggle_mute(track_id);
                    cx.notify();
                })),
            )
            .child(
                button(
                    "s",
                    "solo track · s · cmd+click additive",
                    ("track-solo", track_id.value()),
                )
                .text_color(rgb(if track.soloed() {
                    Palette::CORAL
                } else {
                    Palette::MUTED
                }))
                .on_click(cx.listener(
                    move |view, event: &gpui::ClickEvent, _, cx| {
                        view.toggle_solo(track_id, !event.modifiers().platform);
                        cx.notify();
                    },
                )),
            );
        let header = div()
            .w(px(TRACK_HEADER_WIDTH))
            .h(px(TRACK_ROW_HEIGHT))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_center()
            .px_1()
            .border_l_2()
            .border_b_1()
            .border_color(rgb(if selected {
                Palette::CORAL
            } else {
                Palette::BORDER
            }))
            .bg(rgb(if selected {
                Palette::RAISED
            } else {
                Palette::SURFACE
            }))
            .child(track_name_view)
            .child(track_controls);
        div()
            .w_full()
            .h(px(TRACK_ROW_HEIGHT))
            .flex_shrink_0()
            .flex()
            .bg(rgb(Palette::BACKGROUND))
            .child(clip_area)
            .child(header)
    }

    fn clip_view(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &Clip,
        selected: bool,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let clip_id = clip.id();
        let clip_duration = clip.duration();
        let Some(viewport_end) = viewport.end() else {
            return div().id(("clip", clip_id.value()));
        };
        let Some(clip_end) = clip.start().checked_add(clip.duration()) else {
            return div().id(("clip", clip_id.value()));
        };
        let visible_start = clip.start().max(viewport.start);
        let visible_end = clip_end.min(viewport_end);
        if visible_start >= visible_end || viewport.length == 0 {
            return div().id(("clip", clip_id.value()));
        }
        let visible_frames = visible_end - visible_start;
        let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
        let width = visible_frames as f64 / viewport.length as f64;
        let clipped_left = visible_start > clip.start();
        let drag_available = !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty();
        let drag_payload = if drag_available {
            self.drag_lanes.get(&track).map(|lane| {
                (
                    ClipDrag {
                        project_generation: self.project_generation,
                        revision: state.revision,
                        source_track: track,
                        clip: clip_id,
                        clip_start: clip.start(),
                        clip_duration: clip.duration(),
                        grab_offset: std::rc::Rc::new(std::cell::Cell::new(None)),
                        viewport_start: viewport.start,
                        viewport_length: viewport.length,
                        snap_grid: self.snap_grid,
                        preview_start: std::rc::Rc::new(std::cell::Cell::new(None)),
                    },
                    *lane,
                )
            })
        } else {
            None
        };
        let waveform = state.waveforms.get(&clip.asset());
        let mut content = div()
            .absolute()
            .top(px(4.0))
            .bottom(px(4.0))
            .left(relative(left as f32))
            .w(relative(width as f32))
            .min_w(px(3.0))
            .overflow_hidden()
            .cursor_pointer()
            .bg(rgb(if selected {
                Palette::CORAL_DARK
            } else {
                Palette::CLIP
            }))
            .hover(|style| style.bg(rgb(Palette::HOVER)));
        if !clipped_left {
            content = content.border_l_2().border_color(rgb(if selected {
                Palette::CORAL
            } else {
                Palette::CYAN
            }));
        }
        match waveform {
            Some(waveform) if width >= 0.035 => {
                content = content.child(waveform_view(waveform, clip, visible_start, visible_end));
            }
            Some(_) => {}
            None => {
                content = content.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text("missing media")),
                );
            }
        }
        let fade = clip.fade();
        let envelope_width = self
            .drag_lanes
            .get(&track)
            .map(|lane| (lane.width * width as f32).max(3.0))
            .unwrap_or(0.0);
        if envelope_width >= 40.0 && (fade.fade_in() > 0 || fade.fade_out() > 0) {
            let samples = fade_envelope_samples(fade, clip.start(), visible_start, visible_end);
            if samples.len() >= 2 {
                content = content.children([canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let left = f32::from(bounds.origin.x);
                        let top = f32::from(bounds.origin.y);
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        let mut path = PathBuilder::stroke(px(1.25));
                        for (index, (x, gain)) in samples.iter().copied().enumerate() {
                            let point =
                                point(px(left + width * x), px(top + height * (1.0 - gain)));
                            if index == 0 {
                                path.move_to(point);
                            } else {
                                path.line_to(point);
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(Palette::CYAN));
                        }
                    },
                )
                .absolute()
                .inset_0()]);
            }
        }
        let clip_pixel_width = self
            .drag_lanes
            .get(&track)
            .map(|lane| lane.width * width as f32)
            .unwrap_or(0.0);
        let trim_handles = self.trim_handles(
            state,
            track,
            clip,
            visible_start,
            visible_end,
            clip_pixel_width,
            viewport,
            cx,
        );
        let mut content = content
            .id(("clip", clip_id.value()))
            .tooltip(move |_, cx| {
                cx.new(|_| ClipTooltip {
                    name: format!("clip {} · {} frames", clip_id.value(), clip_duration),
                })
                .into()
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                view.set_selected_clip(track, clip_id);
                cx.notify();
            }));
        if let Some((payload, lane)) = drag_payload {
            let visible_start_for_drag = visible_start;
            let visible_end_for_drag = visible_end;
            let clip_start_for_drag = clip.start();
            let element_width = (f64::from(lane.width) * width).max(3.0) as f32;
            let element_left = lane.left + (f64::from(lane.width) * left) as f32;
            let has_left_handle = clip_pixel_width >= 20.0 && visible_start == clip.start();
            let has_right_handle = clip_pixel_width >= 20.0
                && clip.start().checked_add(clip.duration()) == Some(visible_end);
            let left_inset = if has_left_handle { 8.0 } else { 0.0 };
            let right_inset = if has_right_handle { 8.0 } else { 0.0 };
            let drag_entity = cx.entity();
            let move_entity = drag_entity.clone();
            let move_drag_entity = drag_entity.clone();
            let mouse_down_entity = drag_entity.clone();
            let mouse_down_payload = payload.clone();
            let clear_payload = payload.clone();
            let clear_entity = drag_entity.clone();
            let move_surface = div()
                .id(("clip-move", clip_id.value()))
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(left_inset))
                .right(px(right_inset))
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    mouse_down_entity.update(cx, |view, _| {
                        if view.drag_start_is_current(&mouse_down_payload) {
                            view.remember_pointer_down(
                                mouse_down_payload.project_generation,
                                mouse_down_payload.clip,
                                None,
                                f32::from(event.position.x),
                            );
                        } else {
                            view.clear_pointer_down(
                                mouse_down_payload.project_generation,
                                mouse_down_payload.clip,
                                None,
                            );
                        }
                    });
                })
                .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                    clear_entity.update(cx, |view, _| {
                        view.clear_pointer_down(
                            clear_payload.project_generation,
                            clear_payload.clip,
                            None,
                        );
                    });
                })
                .on_drag(payload, move |payload, _threshold_offset, window, cx| {
                    let original_down_x = drag_entity.update(cx, |view, _| {
                        if view.drag_start_is_current(payload) {
                            view.take_pointer_down(payload.project_generation, payload.clip, None)
                        } else {
                            view.clear_pointer_down(payload.project_generation, payload.clip, None);
                            None
                        }
                    });
                    let offset = original_down_x.and_then(|x| {
                        drag::grab_offset_from_pointer(
                            x,
                            element_left,
                            element_width,
                            visible_start_for_drag,
                            visible_end_for_drag,
                            clip_start_for_drag,
                        )
                    });
                    payload.grab_offset.set(offset);
                    let preview_start = drag_entity.update(cx, |view, _| {
                        view.drag_candidate_start(
                            payload,
                            f32::from(window.mouse_position().x),
                            lane,
                        )
                    });
                    payload.preview_start.set(preview_start);
                    cx.new(|_| DragPreview {
                        width: element_width,
                        valid: offset.is_some(),
                        start: payload.preview_start.clone(),
                        duration: payload.clip_duration,
                    })
                })
                .on_drag_move::<ClipDrag>(move |event, _, cx| {
                    let Some(payload) = event.dragged_item().downcast_ref::<ClipDrag>() else {
                        return;
                    };
                    let pointer_x = f32::from(event.event.position.x);
                    let start = move_entity.update(cx, |view, _| {
                        view.drag_candidate_start(payload, pointer_x, lane)
                    });
                    payload.preview_start.set(start);
                    move_drag_entity.update(cx, |_, cx| cx.notify());
                });
            content = content.children([move_surface]).children(trim_handles);
        }
        content
    }
}

struct DragPreview {
    width: f32,
    valid: bool,
    start: std::rc::Rc<std::cell::Cell<Option<u64>>>,
    duration: u64,
}

impl gpui::Render for DragPreview {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut Context<'_, Self>,
    ) -> impl gpui::IntoElement {
        let bounds = self.start.get().and_then(|start| {
            start
                .checked_add(self.duration)
                .map(|end| format!("{start}–{end}f"))
        });
        div()
            .w(px(self.width))
            .h(px(TRACK_ROW_HEIGHT - 8.0))
            .border_1()
            .border_color(rgb(if self.valid {
                Palette::CYAN
            } else {
                Palette::MUTED
            }))
            .bg(rgb(if self.valid {
                Palette::CLIP
            } else {
                Palette::SURFACE
            }))
            .text_xs()
            .text_color(rgb(Palette::TEXT))
            .child(crate::theme::text(
                bounds.unwrap_or_else(|| "move unavailable".into()),
            ))
    }
}
