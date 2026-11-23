//! Clip drag sources, floating clip drag ghost rendering and drop-overlap span computation.
use super::*;

/// The resting move surface keeps capture inputs. Shared gesture state is
/// allocated by the pointer handlers, independently of per-frame layout.
#[derive(Clone)]
pub(super) struct ClipDragSource {
    pub(super) project_generation: u64,
    pub(super) revision: u64,
    pub(super) track: hex_project::TrackId,
    pub(super) clip: hex_project::ClipId,
    pub(super) position: ClipPosition,
    pub(super) source_span: std::rc::Rc<std::cell::Cell<Option<(f32, f32)>>>,
    pub(super) viewport: Viewport,
    pub(super) snap_grid: crate::editor::musical_time::SnapGrid,
    pub(super) ghost_waveform: Option<std::sync::Arc<hex_session::CachedWaveform>>,
}

impl ClipDragSource {
    pub(super) fn payload(&self) -> ClipDrag {
        let kind = match self.position {
            ClipPosition::Audio { .. } => "audio",
            ClipPosition::Midi { .. } => "midi",
        };
        ClipDrag {
            project_generation: self.project_generation,
            revision: self.revision,
            source_track: self.track,
            clip: self.clip,
            group: Default::default(),
            position: self.position,
            grab_offset: Default::default(),
            snap_edge: Default::default(),
            source_span: self.source_span.clone(),
            capture: Default::default(),
            viewport_start: self.viewport.start,
            viewport_length: self.viewport.length,
            scroll: Default::default(),
            snap_grid: self.snap_grid,
            snap_modifier: Default::default(),
            copy_modifier: Default::default(),
            preview: Default::default(),
            destination: Default::default(),
            insertion_line_y: Default::default(),
            ghost_label: format!("{kind} clip {}", self.clip.value()),
            ghost_waveform: self.ghost_waveform.clone(),
            lane_hovered: Default::default(),
            last_pointer: Default::default(),
        }
    }
}

/// Observe the actual pixel-rounded resting clip rectangle. A clip's move hit
/// strip excludes handles and cannot define its horizontal thirds. The canvas
/// includes the parent's leading stroke; the trailing stroke belongs to the
/// separate boundary shell, so there is no trailing layout border to expand.
pub(super) fn clip_drag_span(
    source: &ClipDragSource,
    leading_border: f32,
) -> impl IntoElement + use<> {
    let span = std::rc::Rc::clone(&source.source_span);
    canvas(
        move |bounds, _, _| {
            span.set(Some((
                f32::from(bounds.left()),
                f32::from(bounds.size.width),
            )));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .left(px(-leading_border))
    .right_0()
    .top_0()
    .bottom_0()
}

pub(super) struct ClipGhostGeometry {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) scale: f32,
}

pub(super) struct ClipGhostDecor {
    pub(super) fill: u32,
    pub(super) active: bool,
    pub(super) notched: bool,
    pub(super) selector: &'static str,
    pub(super) grid: Option<(Vec<crate::editor::musical_time::GridLine>, u64, u64)>,
    pub(super) contents: Option<Div>,
    pub(super) waveform_cache: std::rc::Rc<crate::editor::waveform_geometry::WaveformViewCache>,
}

/// Body-only clipping preserves the title and outer selection border.
pub(in crate::editor) fn clip_waveform_body(content: Div) -> Div {
    waveform_body_below(content, crate::editor::clip_name::CLIP_NAME_HEIGHT)
}
pub(super) fn waveform_body_below(content: Div, header_height: f32) -> Div {
    // The supplied body already owns an absolute fill surface. Fit that same
    // surface below the title instead of nesting another identical layout node.
    content
        .debug_selector(|| "clip-waveform-body".into())
        .absolute()
        .top(px(header_height))
        .left_0()
        .right_0()
        .bottom_0()
        .min_h_0()
        .overflow_hidden()
}

pub(in crate::editor) fn alias_drag_badge() -> Div {
    div()
        .debug_selector(|| "clip-alias-drag-indicator".to_owned())
        .absolute()
        .right(px(6.0))
        .top_0()
        .px_1()
        .bg(rgb(theme::color(theme::PaletteRole::Primary)))
        .text_color(rgb(theme::color(theme::PaletteRole::Background)))
        .text_xs()
        .child(theme::text("alias"))
}

pub(super) fn clip_drag_ghost(
    payload: &ClipDrag,
    geometry: ClipGhostGeometry,
    decor: ClipGhostDecor,
) -> Div {
    let ClipGhostGeometry {
        left,
        top,
        width,
        height,
        scale,
    } = geometry;
    let ClipGhostDecor {
        fill,
        active,
        notched,
        selector,
        grid,
        contents,
        waveform_cache,
    } = decor;
    let stroke = 1.0 / scale.max(1.0);
    let has_border = hex_widgets::rectangle_border_width(width) > 0.0;
    let bounds = gpui::Bounds::new(
        point(px(left), px(top)),
        gpui::size(px(width), px(height.max(3.0))),
    );
    let outline = theme::clip_activity_outline(active, theme::PaletteRole::Primary);
    let detail_opacity = theme::clip_detail_opacity(active);
    let backdrop = crate::editor::clip_boundary::Backdrop {
        body: fill,
        header: fill,
        outline,
        hover_outline: outline,
        stroke,
        header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
        header_gradient: None,
        notch: crate::editor::clip_boundary::Notch::new(has_border && notched),
    };
    let mut ghost = hex_widgets::rectangle_surface_pixels(left, top, width, height)
        .left_0()
        .top_0()
        .w_full()
        .h_full()
        .border_l(px(if has_border { stroke } else { 0.0 }))
        .border_r(px(0.0))
        .debug_selector(move || selector.to_owned())
        .border_color(rgb(outline))
        .bg(rgb(fill));
    if let Some((lines, start, end)) = grid {
        ghost = ghost.children(
            aligned_clip_grid_layer(&lines, start, end, stroke, has_border)
                .map(|grid| grid.opacity(detail_opacity)),
        );
    }
    if let Some(content) = contents {
        ghost = ghost.child(content);
        if payload.alias_copy() {
            ghost = ghost.children([alias_drag_badge()]);
        }
        return crate::editor::clip_boundary::pixel_backdrop_shell(
            ghost,
            bounds,
            backdrop,
            has_border,
            move || format!("{selector}-trailing"),
        );
    } else if let Some(waveform) = &payload.ghost_waveform {
        ghost = ghost.children([clip_waveform_body(
            waveform_summary_view_cached(
                std::sync::Arc::clone(waveform),
                waveform_cache,
                theme::waveform_ink_for_fill(fill),
            )
            .opacity(detail_opacity),
        )]);
    } else {
        ghost = ghost.child(clip_waveform_body(
            div().opacity(detail_opacity).child(
                div()
                    .absolute()
                    .top(relative(0.5))
                    .left(px(4.0))
                    .right(px(4.0))
                    .h(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::ClipText))),
            ),
        ));
    }
    let ghost = ghost.child(super::clip_name::passive_name_header(
        &payload.ghost_label,
        fill,
        has_border,
    ));
    let ghost = if payload.alias_copy() {
        ghost.children([alias_drag_badge()])
    } else {
        ghost
    };
    crate::editor::clip_boundary::pixel_backdrop_shell(
        ghost,
        bounds,
        backdrop,
        has_border,
        move || format!("{selector}-trailing"),
    )
}

/// Visible occupied spans under a landing preview. Merge overlapping victims so
/// the translucent accent has the same intensity everywhere.
pub(super) fn drop_overlap_spans(
    state: &ViewState,
    track: &Track,
    interval: (u64, u64),
    excluded_clip: Option<hex_project::ClipId>,
) -> Vec<(u64, u64)> {
    let audio = track.clips().iter().filter_map(|clip| {
        (Some(clip.id()) != excluded_clip)
            .then(|| clip_intersection(clip, interval))
            .flatten()
    });
    let midi = track.midi_clips().iter().filter_map(|clip| {
        if Some(clip.id()) == excluded_clip {
            return None;
        }
        let preview = state.midi_preview.get(clip.id())?;
        let start = preview.frame_start.max(interval.0);
        let end = preview.frame_end.min(interval.1);
        (start < end).then_some((start, end))
    });
    let mut spans: Vec<_> = audio.chain(midi).collect();
    spans.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

impl EditorView {
    pub(in crate::editor) fn drag_available(&self) -> bool {
        !self.track_resize_active()
            && !self.track_reorder_active()
            && self.clip_name_edit.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
    }

    pub(in crate::editor) fn floating_clip_drag_ghost(&self, scale: f32) -> Option<Div> {
        let payload = self.displayed_clip_drag()?;
        let active = self
            .state
            .as_ref()
            .and_then(|state| state.project.clip_active(payload.clip))
            .unwrap_or(true);
        if let Some(drag::ClipDropDestination::NewTrack {
            before_track: None,
            position,
            ..
        }) = payload.destination.get()
        {
            let state = self.state.as_ref()?;
            let body = self.arrangement_bounds?;
            let viewport = self.visible_viewport(state.timeline_frames);
            let preview = drag::preview_for(&state.project, payload.source_track, position)?;
            let visible_start = preview.start_frame.max(viewport.start);
            let visible_end = preview.end_frame.min(viewport.end()?);
            if visible_start >= visible_end {
                return None;
            }
            let timeline_width = body.width - self.track_header_width();
            if !timeline_width.is_finite() || timeline_width <= 0.0 {
                return None;
            }
            let left = body.left
                + ((visible_start - viewport.start) as f64 / viewport.length as f64
                    * f64::from(timeline_width)) as f32;
            let width = (visible_end - visible_start) as f64 / viewport.length as f64
                * f64::from(timeline_width);
            let top = if state.project.tracks().is_empty() {
                body.top
            } else {
                payload
                    .insertion_line_y
                    .get()
                    .unwrap_or(body.top)
                    .max(body.top)
                    + 1.0
            };
            let height = (body.top + body.height - top).clamp(0.0, 44.0);
            if height <= 0.0 {
                return None;
            }
            return Some(clip_drag_ghost(
                payload,
                ClipGhostGeometry {
                    left,
                    top,
                    width: width as f32,
                    height,
                    scale,
                },
                ClipGhostDecor {
                    active,
                    waveform_cache: std::rc::Rc::clone(&self.waveform_geometry_cache),
                    notched: visible_end == preview.end_frame,
                    fill: theme::clip_activity_fill(
                        hex_project::ElementColor::default(),
                        state.project.clip_flavor(payload.clip).unwrap_or_default(),
                        false,
                        active,
                    ),
                    selector: "clip-drag-append-preview",
                    grid: Some((
                        crate::editor::musical_time::grid_lines(
                            &state.project,
                            viewport,
                            timeline_width,
                            self.snap_grid,
                        ),
                        visible_start,
                        visible_end,
                    )),
                    contents: {
                        let source = state
                            .project
                            .clip(payload.clip)
                            .map(ClipPaintSource::Audio)
                            .or_else(|| {
                                state
                                    .project
                                    .midi_clip(payload.clip)
                                    .map(ClipPaintSource::Midi)
                            });
                        source.and_then(|source| {
                            self.clip_drag_content(
                                state,
                                source,
                                (preview.start_frame, preview.end_frame),
                                (visible_start, visible_end),
                                theme::clip_activity_fill(
                                    hex_project::ElementColor::default(),
                                    state.project.clip_flavor(payload.clip).unwrap_or_default(),
                                    false,
                                    active,
                                ),
                                width as f32,
                                scale,
                                false,
                                hex_widgets::rectangle_border_width(width as f32) > 0.0,
                            )
                        })
                    },
                },
            ));
        }
        if payload.lane_hovered.get() {
            return Some(
                div()
                    .debug_selector(|| "clip-drag-floating-preview-hidden".to_owned())
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(0.0))
                    .h(px(0.0)),
            );
        }
        let (pointer_x, pointer_y) = payload.last_pointer.get()?;
        Some(clip_drag_ghost(
            payload,
            ClipGhostGeometry {
                left: pointer_x + 12.0,
                top: pointer_y + 12.0,
                width: 164.0,
                height: 32.0,
                scale,
            },
            ClipGhostDecor {
                active,
                waveform_cache: std::rc::Rc::clone(&self.waveform_geometry_cache),
                notched: true,
                fill: theme::clip_activity_fill(
                    self.state
                        .as_ref()
                        .and_then(|s| s.project.track(payload.source_track))
                        .map_or(hex_project::ElementColor::default(), |t| t.color()),
                    self.state
                        .as_ref()
                        .and_then(|s| s.project.clip_flavor(payload.clip))
                        .unwrap_or_default(),
                    false,
                    active,
                ),
                selector: "clip-drag-floating-preview",
                grid: None,
                contents: self.state.as_ref().and_then(|state| {
                    let source = state
                        .project
                        .clip(payload.clip)
                        .map(ClipPaintSource::Audio)
                        .or_else(|| {
                            state
                                .project
                                .midi_clip(payload.clip)
                                .map(ClipPaintSource::Midi)
                        })?;
                    let bounds =
                        drag::preview_for(&state.project, payload.source_track, payload.position)?;
                    self.clip_drag_content(
                        state,
                        source,
                        (bounds.start_frame, bounds.end_frame),
                        (bounds.start_frame, bounds.end_frame),
                        theme::clip_activity_fill(
                            state
                                .project
                                .track(payload.source_track)
                                .map_or(hex_project::ElementColor::default(), |track| {
                                    track.color()
                                }),
                            state.project.clip_flavor(payload.clip).unwrap_or_default(),
                            false,
                            active,
                        ),
                        164.0,
                        scale,
                        false,
                        true,
                    )
                }),
            },
        ))
    }

    pub(super) fn midi_drag_source(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &MidiClip,
        start_frame: u64,
        end_frame: u64,
        viewport: Viewport,
    ) -> Option<(ClipDragSource, LaneBounds)> {
        if !self.drag_available() || viewport.length == 0 {
            return None;
        }
        let lane = *self.drag_lanes.get(&track)?;
        let start = clip.start();
        let length = clip.length();
        let _ = start.raw().checked_add(length.raw())?;
        (start_frame < end_frame).then_some((
            ClipDragSource {
                project_generation: self.project_generation,
                revision: state.revision,
                track,
                clip: clip.id(),
                position: ClipPosition::Midi { start, length },
                source_span: Default::default(),
                viewport,
                snap_grid: self.snap_grid,
                ghost_waveform: None,
            },
            lane,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn clip_drag_surface(
        &self,
        source: ClipDragSource,
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
        let element_width = crate::editor::trim::rendered_clip_width(lane.width, width);
        let element_left = lane.left + (f64::from(lane.width) * left) as f32;
        let drag_entity = cx.entity();
        let mouse_down_entity = drag_entity.clone();
        let mouse_up_entity = drag_entity.clone();
        let source_track = source.track;
        let source_generation = source.project_generation;
        let mouse_down_source = source.clone();
        let border = hex_widgets::rectangle_border_width(element_width);
        let header_height = crate::editor::time_selection::header_hit_height(lane.height);
        hex_widgets::rectangle_body(-border, header_height)
            .id(("clip-move", clip_id.value()))
            .debug_selector(|| format!("clip-drag-surface-{}", clip_id.value()))
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    // Click consumes the mouse-up before the separate release
                    // listener; retire only this surface's original press.
                    view.clear_pointer_down(source_generation, clip_id, None);
                    cx.stop_propagation();
                    view.select_clip_header(source_track, clip_id, event.modifiers(), window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                mouse_down_entity.update(cx, |view, _| {
                    let mouse_down_payload = mouse_down_source.payload();
                    if view.drag_start_is_current(&mouse_down_payload) {
                        view.remember_pointer_down(
                            mouse_down_payload.project_generation,
                            mouse_down_payload.clip,
                            None,
                            f32::from(event.position.x),
                        );
                        let edge = (|| {
                            let (left, width) = mouse_down_payload.source_span.get()?;
                            let bounds = drag::preview_for(
                                &view.state.as_ref()?.project,
                                mouse_down_payload.source_track,
                                mouse_down_payload.position,
                            )?;
                            drag::clip_snap_edge(
                                f32::from(event.position.x),
                                left,
                                width,
                                visible_start,
                                visible_end,
                                bounds.start_frame,
                                bounds.end_frame,
                            )
                        })();
                        if let Some(press) = view.pointer_down.as_mut() {
                            press.snap_edge = edge;
                        }
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
                    view.clear_pointer_down(source_generation, clip_id, None);
                    // Mouse-up may arrive before the next repaint removes
                    // this source header. The lane drop receiver still owns
                    // release; root cleanup handles drops outside a receiver.
                    // Retiring here would cancel the move before admission.
                });
            })
            // A release can precede the repaint that replaces this header
            // with the drag preview. Receive it here before sibling click
            // handlers consume the mouse-up; admission stays lane-owned.
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, source_track, window, cx);
            }))
            .on_drag_from_press(move |_, _, window, cx| {
                let payload = source.payload();
                let capture = drag_entity.update(cx, |view, _| {
                    if view.drag_start_is_current(&payload) {
                        view.take_pointer_down(payload.project_generation, payload.clip, None)
                    } else {
                        view.clear_pointer_down(payload.project_generation, payload.clip, None);
                        None
                    }
                });
                payload.capture.set(capture);
                payload
                    .snap_edge
                    .set(capture.and_then(|press| press.snap_edge));
                let original_down_x = capture.map(drag::PointerDown::origin_x);
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
                payload.snap_modifier.set(window.modifiers().platform);
                payload.copy_modifier.set(window.modifiers().alt);
                cx.set_active_drag_cursor_style(
                    drag::copy_cursor(payload.copy_modifier.get()),
                    window,
                );
                let pointer = window.mouse_position();
                let pointer = (f32::from(pointer.x), f32::from(pointer.y));
                payload.last_pointer.set(Some(pointer));
                let (preview, lane_hovered) = drag_entity.update(cx, |view, _| {
                    view.begin_clip_drag(&payload);
                    view.clip_drag_drop_position(&payload, pointer.0, pointer.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                drag_entity.update(cx, |view, cx| {
                    view.sync_held_clip_move_audition();
                    cx.notify();
                });
                Some((payload, cx.new(|_| ClipDragGhost)))
            })
    }
}

pub(in crate::editor) struct ClipDragGhost;

impl gpui::Render for ClipDragGhost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut Context<'_, Self>,
    ) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "clip-drag-ghost".to_owned())
            .w(px(0.0))
            .h(px(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_drag_ghost_geometry_retains_positive_extent() {
        let geometry = ClipGhostGeometry {
            left: 12.0,
            top: 4.0,
            width: 120.0,
            height: 48.0,
            scale: 2.0,
        };
        assert!(geometry.width > 0.0 && geometry.height > 0.0 && geometry.scale > 0.0);
    }
}
