use super::drag::{self, ClipDrag, ClipPosition, LaneBounds};
use super::trim::TrimDrag;
use super::viewport::Viewport;
use super::waveform::{ClipTooltip, button, fade_envelope_samples, waveform_view};
use super::{EditorView, ViewState};
use crate::theme::{self, Palette};
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb,
};
use hex_project::{BeatTime, Clip, MidiClip, Track};

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
            .id(("track-lane", track_id.value()))
            .debug_selector(|| format!("arrangement-lane-{}", track_id.value()))
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
            let midi_drag_payload =
                self.midi_drag_payload(state, track_id, clip, start, end, viewport);
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
            if let Some((payload, lane)) = midi_drag_payload {
                midi = midi.children([self.clip_drag_surface(
                    payload,
                    lane,
                    visible_start,
                    visible_end,
                    start,
                    Some(clip.start()),
                    viewport,
                    clip.id(),
                    cx,
                )]);
            }
            clip_area = clip_area.children([midi]);
        }
        let clip_preview = self
            .active_clip_drag
            .as_ref()
            .filter(|drag| self.drag_start_is_current(drag))
            .and_then(|drag| drag.preview.get())
            .filter(|preview| preview.target_track == track_id);
        let browser_preview = self
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| self.browser_audio_drag_is_current(drag))
            .and_then(|drag| drag.preview.get())
            .filter(|preview| preview.target_track == track_id);
        if let Some(preview) = clip_preview.or(browser_preview) {
            let visible_start = preview.start_frame.max(viewport.start);
            let visible_end = preview.end_frame.min(viewport_end);
            if visible_start < visible_end {
                let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
                let width = (visible_end - visible_start) as f64 / viewport.length as f64;
                clip_area = clip_area.child(
                    div()
                        .id(("clip-drop-preview", track_id.value()))
                        .debug_selector(|| "clip-drop-preview".to_owned())
                        .absolute()
                        .top(px(3.0))
                        .bottom(px(3.0))
                        .left(relative(left as f32))
                        .w(relative(width as f32))
                        .min_w(px(3.0))
                        .border_1()
                        .border_color(rgb(Palette::CYAN))
                        .bg(rgb(Palette::CYAN))
                        .opacity(0.28),
                );
            }
        } else if let Some((pending_track, start)) = self
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| drag.lane_hovered.get())
            .and_then(|drag| drag.pending_start.get())
            .filter(|(pending_track, _)| *pending_track == track_id)
        {
            let _ = pending_track;
            if start >= viewport.start && start < viewport_end {
                let left = (start - viewport.start) as f64 / viewport.length as f64;
                clip_area = clip_area.child(
                    div()
                        .id(("browser-drop-pending", track_id.value()))
                        .debug_selector(|| "browser-drop-pending".to_owned())
                        .absolute()
                        .top(px(3.0))
                        .bottom(px(3.0))
                        .left(relative(left as f32))
                        .w(px(2.0))
                        .bg(rgb(Palette::CYAN)),
                );
            }
        }
        let lane_entity = cx.entity();
        clip_area = clip_area.children([canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                let lane = LaneBounds {
                    left: f32::from(bounds.origin.x),
                    width: f32::from(bounds.size.width),
                    top: f32::from(bounds.origin.y),
                    height: f32::from(bounds.size.height),
                };
                lane_entity.update(cx, |view, view_cx| {
                    let changed = view
                        .drag_lanes
                        .insert(track_id, lane)
                        .is_none_or(|old| old.left != lane.left || old.width != lane.width);
                    view.drag_lane_epochs
                        .insert(track_id, view.drag_layout_generation);
                    if changed {
                        view_cx.notify();
                    }
                });
            },
        )
        .absolute()
        .inset_0()]);
        clip_area = clip_area
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, track_id, window, cx);
            }))
            .on_drop(
                cx.listener(move |view, drag: &drag::BrowserAudioDrag, window, cx| {
                    view.drop_browser_audio(drag, window, cx);
                }),
            )
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
        let drag_payload = self
            .drag_available()
            .then(|| self.drag_lanes.get(&track).copied())
            .flatten()
            .map(|lane| {
                (
                    ClipDrag {
                        project_generation: self.project_generation,
                        revision: state.revision,
                        source_track: track,
                        clip: clip_id,
                        position: ClipPosition::Audio {
                            start: clip.start(),
                            duration: clip.duration(),
                        },
                        grab_offset: std::rc::Rc::new(std::cell::Cell::new(None)),
                        origin_pointer_x: std::rc::Rc::new(std::cell::Cell::new(None)),
                        viewport_start: viewport.start,
                        viewport_length: viewport.length,
                        snap_grid: self.snap_grid,
                        preview: std::rc::Rc::new(std::cell::Cell::new(None)),
                        ghost_label: format!("audio clip {}", clip_id.value()),
                        ghost_waveform: state.waveforms.get(&clip.asset()).cloned(),
                        lane_hovered: std::rc::Rc::new(std::cell::Cell::new(false)),
                    },
                    lane,
                )
            });
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
            let has_left_handle = clip_pixel_width >= 20.0 && visible_start == clip.start();
            let has_right_handle = clip_pixel_width >= 20.0
                && clip.start().checked_add(clip.duration()) == Some(visible_end);
            let left_inset = if has_left_handle { 8.0 } else { 0.0 };
            let right_inset = if has_right_handle { 8.0 } else { 0.0 };
            let move_surface = self
                .clip_drag_surface(
                    payload,
                    lane,
                    visible_start,
                    visible_end,
                    clip.start(),
                    None,
                    viewport,
                    clip_id,
                    cx,
                )
                .left(px(left_inset))
                .right(px(right_inset));
            content = content.children([move_surface]).children(trim_handles);
        }
        content
    }

    fn drag_available(&self) -> bool {
        !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
    }

    fn midi_drag_payload(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &MidiClip,
        start_frame: u64,
        end_frame: u64,
        viewport: Viewport,
    ) -> Option<(ClipDrag, LaneBounds)> {
        if !self.drag_available() || viewport.length == 0 {
            return None;
        }
        let lane = *self.drag_lanes.get(&track)?;
        let start = clip.start();
        let length = clip.length();
        let _ = start.raw().checked_add(length.raw())?;
        (start_frame < end_frame).then_some((
            ClipDrag {
                project_generation: self.project_generation,
                revision: state.revision,
                source_track: track,
                clip: clip.id(),
                position: ClipPosition::Midi { start, length },
                grab_offset: std::rc::Rc::new(std::cell::Cell::new(None)),
                origin_pointer_x: std::rc::Rc::new(std::cell::Cell::new(None)),
                viewport_start: viewport.start,
                viewport_length: viewport.length,
                snap_grid: self.snap_grid,
                preview: std::rc::Rc::new(std::cell::Cell::new(None)),
                ghost_label: format!("midi clip {}", clip.id().value()),
                ghost_waveform: None,
                lane_hovered: std::rc::Rc::new(std::cell::Cell::new(false)),
            },
            lane,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn clip_drag_surface(
        &self,
        payload: ClipDrag,
        lane: LaneBounds,
        visible_start: u64,
        visible_end: u64,
        source_start_frame: u64,
        source_start_beat: Option<BeatTime>,
        viewport: Viewport,
        clip_id: hex_project::ClipId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let visible_frames = visible_end.saturating_sub(visible_start);
        let left =
            visible_start.saturating_sub(viewport.start) as f64 / viewport.length.max(1) as f64;
        let width = visible_frames as f64 / viewport.length.max(1) as f64;
        let element_width = (f64::from(lane.width) * width).max(3.0) as f32;
        let element_left = lane.left + (f64::from(lane.width) * left) as f32;
        let drag_entity = cx.entity();
        let mouse_down_entity = drag_entity.clone();
        let mouse_up_entity = drag_entity.clone();
        let move_entity = drag_entity.clone();
        let mouse_down_payload = payload.clone();
        let mouse_up_payload = payload.clone();
        div()
            .id(("clip-move", clip_id.value()))
            .debug_selector(|| format!("clip-drag-surface-{}", clip_id.value()))
            .absolute()
            .inset_0()
            .cursor_pointer()
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
                mouse_up_entity.update(cx, |view, _| {
                    view.clear_pointer_down(
                        mouse_up_payload.project_generation,
                        mouse_up_payload.clip,
                        None,
                    );
                    view.clear_clip_drag(
                        mouse_up_payload.project_generation,
                        mouse_up_payload.clip,
                    );
                });
            })
            .on_drag(payload, move |payload, _, window, cx| {
                let original_down_x = drag_entity.update(cx, |view, _| {
                    if view.drag_start_is_current(payload) {
                        view.take_pointer_down(payload.project_generation, payload.clip, None)
                    } else {
                        view.clear_pointer_down(payload.project_generation, payload.clip, None);
                        None
                    }
                });
                let frame_offset = original_down_x.and_then(|x| {
                    drag::grab_offset_from_pointer(
                        x,
                        element_left,
                        element_width,
                        visible_start,
                        visible_end,
                        source_start_frame,
                    )
                });
                payload.origin_pointer_x.set(original_down_x);
                let offset = match (payload.position, frame_offset, source_start_beat) {
                    (ClipPosition::Audio { .. }, Some(offset), _) => Some(offset),
                    (ClipPosition::Midi { .. }, Some(offset), Some(start_beat)) => {
                        source_start_frame
                            .checked_add(offset)
                            .and_then(|grabbed_frame| {
                                drag_entity.update(cx, |view, _| {
                                    view.state
                                        .as_ref()?
                                        .project
                                        .tempo_map()
                                        .frame_to_beat(grabbed_frame)
                                        .ok()
                                })
                            })
                            .map(|beat| beat.raw().saturating_sub(start_beat.raw()))
                    }
                    _ => None,
                };
                payload.grab_offset.set(offset);
                let (preview, lane_hovered) = drag_entity.update(cx, |view, _| {
                    view.begin_clip_drag(payload);
                    let target = view.hovered_drag_target(
                        f32::from(window.mouse_position().x),
                        f32::from(window.mouse_position().y),
                    );
                    let preview = target.and_then(|(target, lane)| {
                        view.drag_candidate(
                            payload,
                            target,
                            f32::from(window.mouse_position().x),
                            lane,
                        )
                    });
                    (preview, target.is_some())
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                drag_entity.update(cx, |_, cx| cx.notify());
                let ghost_payload = payload.clone();
                cx.new(|_| ClipDragGhost {
                    payload: ghost_payload,
                })
            })
            .on_drag_move::<ClipDrag>(move |event, _, cx| {
                let Some(payload) = event.dragged_item().downcast_ref::<ClipDrag>() else {
                    return;
                };
                #[cfg(test)]
                move_entity.update(cx, |view, _| {
                    view.drag_move_event_count = view.drag_move_event_count.saturating_add(1);
                });
                let pointer = event.event.position;
                let (preview, lane_hovered) = move_entity.update(cx, |view, _| {
                    let target =
                        view.hovered_drag_target(f32::from(pointer.x), f32::from(pointer.y));
                    let preview = target.and_then(|(target, lane)| {
                        view.drag_candidate(payload, target, f32::from(pointer.x), lane)
                    });
                    (preview, target.is_some())
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                move_entity.update(cx, |_, cx| cx.notify());
            })
    }
}

struct ClipDragGhost {
    payload: ClipDrag,
}

impl gpui::Render for ClipDragGhost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut Context<'_, Self>,
    ) -> impl gpui::IntoElement {
        if self.payload.lane_hovered.get() {
            return div().w(px(0.0)).h(px(0.0));
        }
        let accent = match self.payload.position {
            ClipPosition::Audio { .. } => Palette::CYAN,
            ClipPosition::Midi { .. } => Palette::CORAL,
        };
        let mut bars = div()
            .absolute()
            .left(px(4.0))
            .right(px(4.0))
            .top(px(9.0))
            .bottom(px(4.0))
            .flex()
            .items_center()
            .gap(px(1.0));
        if let Some(waveform) = &self.payload.ghost_waveform {
            let count = waveform.bins.len().min(36);
            for index in 0..count {
                let bin_index = index * waveform.bins.len() / count.max(1);
                let amplitude = waveform.bins[bin_index]
                    .iter()
                    .map(|peak| peak.min.abs().max(peak.max.abs()))
                    .fold(0.0_f32, f32::max)
                    .clamp(0.08, 1.0);
                bars = bars.child(
                    div().flex_1().h_full().flex().items_center().child(
                        div()
                            .w_full()
                            .h(px(amplitude * 18.0))
                            .bg(rgb(Palette::WAVEFORM)),
                    ),
                );
            }
        } else {
            bars = bars.child(div().w_full().h(px(2.0)).bg(rgb(accent)));
        }
        div()
            .debug_selector(|| "clip-drag-ghost".to_owned())
            .w(px(164.0))
            .h(px(32.0))
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(accent))
            .bg(rgb(Palette::RAISED))
            .opacity(0.88)
            .child(
                div()
                    .absolute()
                    .left(px(4.0))
                    .top_0()
                    .right(px(3.0))
                    .h(px(10.0))
                    .text_xs()
                    .text_color(rgb(Palette::TEXT))
                    .line_clamp(1)
                    .child(theme::text(self.payload.ghost_label.clone())),
            )
            .child(bars)
    }
}
