//! Arrangement track row layout, track header controls, arm button and mixer strip.
use super::*;

impl EditorView {
    pub(super) fn audio_arm_button(
        &self,
        track: hex_project::TrackId,
        eligible: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let generation = self.project_generation;
        let armed = self
            .audio_recorder
            .is_audio_armed(self.project_generation, track);
        let (background, text) = if armed {
            (
                theme::color(theme::PaletteRole::Recording),
                theme::color(theme::PaletteRole::Background),
            )
        } else {
            crate::editor::track_gain::activator_colors(false)
        };
        toggle_button_with_style(
            "○",
            if !eligible {
                "audio recording unavailable on this track"
            } else if armed {
                "disarm track for audio recording"
            } else {
                "arm track for audio recording"
            },
            ("track-audio-arm", track.value()),
            theme::action_style().with_state_colors(background, text),
        )
        .debug_selector(move || format!("track-audio-arm-{}", track.value()))
        .w(px(16.0))
        .h(px(14.0))
        .text_size(px(10.0))
        .border_1()
        .border_color(rgb(theme::control_style().border))
        .flex_shrink_0()
        .when(!eligible, |button| {
            button.opacity(0.45).cursor(gpui::CursorStyle::Arrow)
        })
        .on_click(cx.listener(move |view, _, _, cx| {
            if eligible {
                cx.stop_propagation();
                view.toggle_audio_arm(track, generation);
                cx.notify();
            }
        }))
    }

    pub(in crate::editor) fn track_row(
        &mut self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        selected_rows: Option<(usize, usize)>,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        #[cfg(all(test, feature = "ui-test-support"))]
        let _navigation_timer = crate::editor::navigation_metrics::Timer::start(
            crate::editor::navigation_metrics::Stage::Row,
        );
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.track_row_builds = self.track_row_builds.saturating_add(1);
        }
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
            caption_rem_size,
        } = timeline;
        let track_id = track.id();
        let Some(viewport_end) = viewport.end() else {
            return div().w_full().h(px(self.track_body_height(track_id)));
        };
        let mut clip_area = div()
            .id(("track-lane", track_id.value()))
            .debug_selector(|| format!("arrangement-lane-{}", track_id.value()))
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(self.track_body_height(track_id)))
            .overflow_hidden()
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    if let Some(position) = event.mouse_position() {
                        view.place_edit_cursor(
                            track_id,
                            f32::from(position.x),
                            event.modifiers().platform,
                            window,
                            cx,
                        );
                    }
                }),
            );
        let track_selected = self.track_selection.contains(track_id);
        if track_selected {
            clip_area = clip_area.child(
                div()
                    .absolute()
                    .inset_0()
                    .debug_selector(move || format!("selected-track-lane-{}", track_id.value()))
                    .bg(gpui::Hsla::from(rgb(theme::track_selection_fill()))
                        .alpha(theme::TRACK_SELECTION_OPACITY)),
            );
        }
        let lane_entity = cx.entity();
        let track_drop_height = self.track_height(track_id);
        clip_area = clip_area.children([canvas(
            move |bounds, _, cx| {
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
                    view.track_drop_lanes.insert(
                        track_id,
                        LaneBounds {
                            height: track_drop_height,
                            ..lane
                        },
                    );
                    view.drag_lane_epochs
                        .insert(track_id, view.drag_layout_generation);
                    if changed {
                        view_cx.notify();
                    }
                });
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0()]);
        let content = self.clip_content_element(
            state,
            track,
            track_index,
            ClipViewTimeline {
                viewport,
                grid,
                scale_factor,
                caption_rem_size,
            },
            cx,
        );
        // The row separator is a background surface. Painting it after the
        // clips would cover their bottom selection stroke at the row boundary.
        clip_area = clip_area.child(
            div()
                .debug_selector(|| format!("lane-separator-{}", track_id.value()))
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Border))),
        );
        if track.is_group() {
            clip_area = clip_area.children([crate::editor::group_overview::view(
                self, state, track, viewport,
            )]);
        }
        // Keep the divider out of layout: clips, pointer projection and the Main
        // lane must all use the complete timeline width. Paint beneath clips to
        // preserve their selection stroke at the viewport edge.
        clip_area = clip_area.child(
            div()
                .debug_selector(|| format!("lane-right-divider-{}", track_id.value()))
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::Border))),
        );
        // Keep the group cue anchored to the header edge, below clips and their
        // interaction overlays, while the timeline scrolls independently.
        clip_area = clip_area.children(
            self.track_hierarchy_indent
                .ancestors(track_id)
                .iter()
                .enumerate()
                .map(|(depth, &(ancestor, color))| {
                    div()
                        .absolute()
                        .right(px(1.0 + depth as f32 * 2.0))
                        .top_0()
                        .bottom_0()
                        .w(px(2.0))
                        .debug_selector(move || {
                            format!("lane-group-rail-{}-{}", track_id.value(), ancestor.value())
                        })
                        .bg(gpui::Hsla::from(rgb(theme::element_colors(color).accent)).alpha(0.35))
                }),
        );
        clip_area = clip_area.child(
            div()
                .absolute()
                .inset_0()
                .debug_selector(move || format!("arrangement-clip-layer-{}", track_id.value()))
                .opacity(if self.automation_view.enabled {
                    0.22
                } else {
                    1.0
                })
                .child(content),
        );
        let move_display = self
            .time_range_move_display()
            .filter(|_| self.time_range_copy_view().is_none());
        let live_midi_trim_preview = self.midi_trim_display(track_id, None);
        if let Some((clip, preview)) = live_midi_trim_preview {
            let start_beat = preview.start();
            if let Some(end_beat) = start_beat
                .raw()
                .checked_add(preview.length().raw())
                .map(BeatTime::from_raw)
                && let (Ok(start), Ok(end)) = (
                    state.project.tempo_map().beat_to_frame(start_beat),
                    state.project.tempo_map().beat_to_frame(end_beat),
                )
            {
                let visible_start = start.max(viewport.start);
                let visible_end = end.min(viewport_end);
                if visible_start < visible_end {
                    let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
                    let width = (visible_end - visible_start) as f64 / viewport.length as f64;
                    let pixel_width = self.drag_lanes.get(&track_id).map_or(0.0, |lane| {
                        crate::editor::trim::rendered_clip_width(lane.width, width)
                    });
                    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                    let active = clip.active();
                    let detail_opacity = theme::clip_detail_opacity(active);
                    let fill =
                        theme::clip_activity_fill(track.color(), clip.flavor(), false, active);
                    let color = theme::clip_activity_outline(
                        active,
                        if self.clip_is_selected(clip.id()) {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::ClipOutline
                        },
                    );
                    let notched = has_border && visible_end == end;
                    let mut overlay =
                        hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
                            .left_0()
                            .w_full()
                            .border_l(px(
                                if hex_widgets::rectangle_border_width(pixel_width) > 0.0 {
                                    1.0 / scale_factor.max(1.0)
                                } else {
                                    0.0
                                },
                            ))
                            .border_r(px(0.0))
                            .id(("midi-trim-live-preview", track_id.value()))
                            .debug_selector(|| "midi-trim-live-preview".to_owned())
                            .border_color(rgb(color))
                            .bg(rgb(fill))
                            .when(has_border, |overlay| {
                                overlay
                                    .bg(gpui::transparent_black())
                                    .border_color(gpui::transparent_black())
                            })
                            .children(
                                aligned_clip_grid_layer(
                                    grid,
                                    visible_start,
                                    visible_end,
                                    1.0 / scale_factor.max(1.0),
                                    hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                                )
                                .map(|grid| grid.opacity(detail_opacity)),
                            );
                    let looping = preview.looping(&clip);
                    if self.track_body_height(track_id)
                        > crate::editor::track_layout::HEADER_ONLY_HEIGHT
                    {
                        let shapes = midi_trim_note_shapes(
                            &state.project,
                            &clip,
                            preview,
                            visible_start,
                            visible_end,
                            crate::editor::midi_loop_notes::merge_frames(
                                visible_end - visible_start,
                                pixel_width,
                                scale_factor,
                            ),
                        );
                        // Keep the resting clip's zoom so the drag doesn't
                        // change note thickness.
                        let fit = crate::editor::midi_loop_notes::sounding_fit(&clip);
                        let (notes, _) =
                            midi_note_canvas(|| "midi-trim-preview-notes".to_owned(), shapes, fit);
                        overlay = overlay.child(clip_waveform_body(notes.opacity(detail_opacity)));
                    }
                    overlay = overlay.child(self.clip_name_preview_header(
                        track_id,
                        clip.id(),
                        clip.name(),
                        fill,
                        has_border,
                        notched,
                        cx,
                    ));
                    overlay = overlay.when(looping, |overlay| {
                        overlay.child(
                            loop_handles::midi_markers(
                                &clip,
                                Some(preview),
                                &state.project,
                                visible_start,
                                visible_end,
                            )
                            .opacity(detail_opacity),
                        )
                    });
                    let framed_clip = if has_border {
                        crate::editor::clip_boundary::backdrop_shell(
                            overlay,
                            left as f32,
                            width as f32,
                            crate::editor::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: crate::editor::clip_header_loop::midi(
                                    &clip,
                                    Some(preview),
                                    state.project.tempo_map(),
                                    visible_start..visible_end,
                                    pixel_width * scale_factor,
                                ),
                                notch: crate::editor::clip_boundary::Notch::new(notched),
                            },
                            true,
                            || "midi-trim-preview-trailing".into(),
                        )
                    } else {
                        clip_boundary_shell(
                            overlay,
                            left as f32,
                            width as f32,
                            1.0 / scale_factor.max(1.0),
                            color,
                            color,
                            true,
                            hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                            0.0,
                            || "midi-trim-preview-trailing".into(),
                        )
                    };
                    clip_area = clip_area.child(framed_clip);
                }
            }
        }
        let live_trim_preview = self.audio_trim_display(track_id, None);
        if let Some((original_clip, preview_clip)) = live_trim_preview {
            let preview_start = preview_clip.start();
            if let Some(preview_end) = preview_start.checked_add(preview_clip.duration()) {
                let visible_start = preview_start.max(viewport.start);
                let visible_end = preview_end.min(viewport_end);
                if visible_start < visible_end {
                    let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
                    let width = (visible_end - visible_start) as f64 / viewport.length as f64;
                    let pixel_width = self.drag_lanes.get(&track_id).map_or(0.0, |lane| {
                        crate::editor::trim::rendered_clip_width(lane.width, width)
                    });
                    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                    let active = preview_clip.active();
                    let detail_opacity = theme::clip_detail_opacity(active);
                    let fill = theme::clip_activity_fill(
                        track.color(),
                        preview_clip.flavor(),
                        false,
                        active,
                    );
                    let color = theme::clip_activity_outline(
                        active,
                        if self.clip_is_selected(original_clip.id()) {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::ClipOutline
                        },
                    );
                    let notched = has_border && visible_end == preview_end;
                    let mut preview =
                        hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
                            .left_0()
                            .w_full()
                            .border_l(px(
                                if hex_widgets::rectangle_border_width(pixel_width) > 0.0 {
                                    1.0 / scale_factor.max(1.0)
                                } else {
                                    0.0
                                },
                            ))
                            .border_r(px(0.0))
                            .id(("trim-live-preview", track_id.value()))
                            .debug_selector(|| "trim-live-preview".to_owned())
                            .border_color(rgb(color))
                            .bg(rgb(fill))
                            .when(has_border, |preview| {
                                preview
                                    .bg(gpui::transparent_black())
                                    .border_color(gpui::transparent_black())
                            })
                            .children(
                                aligned_clip_grid_layer(
                                    grid,
                                    visible_start,
                                    visible_end,
                                    1.0 / scale_factor.max(1.0),
                                    hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                                )
                                .map(|grid| grid.opacity(detail_opacity)),
                            );
                    let looping = loop_handles::audio_repeats(&preview_clip);
                    if self.track_body_height(track_id)
                        > crate::editor::track_layout::HEADER_ONLY_HEIGHT
                        && let Some(waveform) =
                            crate::editor::waveform::preview_waveform_view_cached(
                                crate::editor::waveform::WaveformPreview {
                                    original: &original_clip,
                                    preview: &preview_clip,
                                    tempo: state.project.tempo_map(),
                                    asset: state.waveforms.get(&original_clip.asset()),
                                    prepared: state.audio_clip_waveforms.get(&original_clip.id()),
                                    ink: theme::waveform_ink_for_fill(fill),
                                },
                                visible_start,
                                visible_end,
                                std::rc::Rc::clone(&self.waveform_geometry_cache),
                                self.waveform_grid(track_id, viewport, scale_factor),
                            )
                    {
                        preview = preview.child(clip_waveform_body(waveform));
                    }
                    let fade = preview_clip.retained_fade();
                    if self.track_body_height(track_id)
                        > crate::editor::track_layout::HEADER_ONLY_HEIGHT
                        && !fade.is_neutral()
                    {
                        let samples = if preview_clip.audio_edit().is_some() {
                            crate::editor::waveform::projected_fade_envelope_samples(
                                &preview_clip,
                                state.project.tempo_map(),
                                visible_start,
                                visible_end,
                                None,
                            )
                            .unwrap_or_default()
                        } else {
                            fade_envelope_samples(fade, preview_start, visible_start, visible_end)
                        };
                        if samples.len() >= 2 {
                            preview = preview.child(clip_fade_layer(
                                preview_clip.id(),
                                samples,
                                self.fade_key_held,
                                active,
                            ));
                        }
                    }
                    preview = preview.child(self.clip_name_preview_header(
                        track_id,
                        original_clip.id(),
                        original_clip.name(),
                        fill,
                        has_border,
                        notched,
                        cx,
                    ));
                    preview = preview.when(looping, |preview| {
                        preview.child(
                            loop_handles::audio_markers(
                                &preview_clip,
                                &state.project,
                                visible_start,
                                visible_end,
                            )
                            .opacity(detail_opacity),
                        )
                    });
                    let framed_clip = if has_border {
                        crate::editor::clip_boundary::backdrop_shell(
                            preview,
                            left as f32,
                            width as f32,
                            crate::editor::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: crate::editor::clip_header_loop::audio(
                                    &preview_clip,
                                    state.project.tempo_map(),
                                    visible_start..visible_end,
                                    pixel_width * scale_factor,
                                ),
                                notch: crate::editor::clip_boundary::Notch::new(notched),
                            },
                            true,
                            || "trim-preview-trailing".into(),
                        )
                    } else {
                        clip_boundary_shell(
                            preview,
                            left as f32,
                            width as f32,
                            1.0 / scale_factor.max(1.0),
                            color,
                            color,
                            true,
                            hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                            0.0,
                            || "trim-preview-trailing".into(),
                        )
                    };
                    clip_area = clip_area.child(framed_clip);
                }
            }
        }
        // Group metadata is projected once per pointer/modifier/timer update.
        // Render only the members owned by this row; no project/PCM rebuild.
        if let Some(drag) = self.displayed_clip_drag() {
            let group = drag.group.borrow();
            if group.ids.len() > 1 {
                let mut overlap_events = Vec::new();
                for (member_index, member) in group
                    .previews()
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.track == track_id)
                {
                    use hex_project::ClipGroupContent;
                    let position = match &member.content {
                        ClipGroupContent::Audio(c) => ClipPosition::Audio {
                            start: c.start(),
                            duration: c.duration(),
                        },
                        ClipGroupContent::Midi(c) => ClipPosition::Midi {
                            start: c.start(),
                            length: c.length(),
                        },
                    };
                    let Some(bounds) = drag::preview_for(&state.project, track_id, position) else {
                        continue;
                    };
                    let start = bounds.start_frame.max(viewport.start);
                    let end = bounds.end_frame.min(viewport_end);
                    if start >= end {
                        continue;
                    }
                    overlap_events.push((start, 1, 0));
                    overlap_events.push((end, -1, 0));
                    let (flavor, active) = match &member.content {
                        ClipGroupContent::Audio(c) => (c.flavor(), c.active()),
                        ClipGroupContent::Midi(c) => (c.flavor(), c.active()),
                    };
                    let id = member.id();
                    let left = (start - viewport.start) as f32 / viewport.length as f32;
                    let width = (end - start) as f32 / viewport.length as f32;
                    let pixel_width = self
                        .drag_lanes
                        .get(&track_id)
                        .map_or(0.0, |lane| lane.width * width);
                    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                    let fill = theme::clip_activity_fill(track.color(), flavor, false, active);
                    let color = theme::clip_activity_outline(active, theme::PaletteRole::Primary);
                    let mut overlay = hex_widgets::rectangle_surface(left, width, pixel_width)
                        .left_0()
                        .w_full()
                        .border_l(px(
                            if hex_widgets::rectangle_border_width(pixel_width) > 0.0 {
                                1.0 / scale_factor.max(1.0)
                            } else {
                                0.0
                            },
                        ))
                        .border_r(px(0.0))
                        .debug_selector(move || format!("group-clip-preview-{}", id.value()))
                        .border_color(rgb(color))
                        .bg(rgb(fill))
                        .when(has_border, |overlay| {
                            overlay
                                .bg(gpui::transparent_black())
                                .border_color(gpui::transparent_black())
                        })
                        .children(
                            aligned_clip_grid_layer(
                                grid,
                                start,
                                end,
                                1.0 / scale_factor.max(1.0),
                                hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                            )
                            .map(|grid| grid.opacity(theme::clip_detail_opacity(active))),
                        );
                    let source = match &member.content {
                        ClipGroupContent::Audio(_) => group
                            .original_audio(&state.project, member_index)
                            .map(ClipPaintSource::Audio),
                        ClipGroupContent::Midi(clip) => Some(ClipPaintSource::Midi(clip)),
                    };
                    if let Some(source) = source
                        && let Some(content) = self.clip_drag_content(
                            state,
                            source,
                            (bounds.start_frame, bounds.end_frame),
                            (start, end),
                            theme::clip_activity_fill(track.color(), flavor, false, active),
                            pixel_width,
                            scale_factor,
                            self.track_body_height(track_id)
                                <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                            has_border,
                        )
                    {
                        overlay = overlay.child(content);
                    }
                    if drag.alias_copy() {
                        overlay = overlay.children([alias_drag_badge()]);
                    }
                    let framed_clip = if has_border {
                        crate::editor::clip_boundary::backdrop_shell(
                            overlay,
                            left,
                            width,
                            crate::editor::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: None,
                                notch: crate::editor::clip_boundary::Notch::new(
                                    end == bounds.end_frame,
                                ),
                            },
                            true,
                            move || format!("group-clip-preview-trailing-{}", id.value()),
                        )
                    } else {
                        clip_boundary_shell(
                            overlay,
                            left,
                            width,
                            1.0 / scale_factor.max(1.0),
                            color,
                            color,
                            true,
                            hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                            0.0,
                            move || format!("group-clip-preview-trailing-{}", id.value()),
                        )
                    };
                    clip_area = clip_area.child(framed_clip);
                }
                if self.active_clip_drag.is_some() && !overlap_events.is_empty() {
                    // Captured member IDs are sorted by the existing selection
                    // owner. Originals are protected in BOTH copy/move modes.
                    let occupied = track
                        .clips()
                        .iter()
                        .filter_map(|c| {
                            if group.ids.binary_search(&c.id()).is_ok() {
                                return None;
                            }
                            clip_intersection(c, (viewport.start, viewport_end))
                        })
                        .chain(track.midi_clips().iter().filter_map(|c| {
                            if group.ids.binary_search(&c.id()).is_ok() {
                                return None;
                            }
                            let p = state.midi_preview.get(c.id())?;
                            let (start, end) = (
                                p.frame_start.max(viewport.start),
                                p.frame_end.min(viewport_end),
                            );
                            (start < end).then_some((start, end))
                        }));
                    for (start, end) in occupied {
                        overlap_events.push((start, 0, 1));
                        overlap_events.push((end, 0, -1));
                    }
                    // Passive accents come AFTER all ghosts, with no hit handler.
                    for (index, (start, end)) in
                        group_overlap_spans(overlap_events).into_iter().enumerate()
                    {
                        clip_area = clip_area.child(
                            div()
                                .debug_selector(move || format!("clip-drop-overlap-{index}"))
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(relative(
                                    (start - viewport.start) as f32 / viewport.length as f32,
                                ))
                                .w(relative((end - start) as f32 / viewport.length as f32))
                                .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                                .opacity(0.72),
                        );
                    }
                }
            }
        }
        let clip_preview = self
            .displayed_clip_drag()
            .filter(|drag| drag.group.borrow().ids.len() <= 1)
            .and_then(|drag| drag.preview.get())
            .filter(|preview| preview.target_track == track_id);
        let browser_preview = self
            .pending_sample_drop
            .as_ref()
            .and_then(|drop| drop.preview)
            .or_else(|| self.displayed_browser_audio_drag()?.preview.get())
            .filter(|preview| preview.target_track == track_id);
        if let Some(preview) = clip_preview.or(browser_preview) {
            let visible_start = preview.start_frame.max(viewport.start);
            let visible_end = preview.end_frame.min(viewport_end);
            if visible_start < visible_end {
                let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
                let width = (visible_end - visible_start) as f64 / viewport.length as f64;
                let pixel_width = self.drag_lanes.get(&track_id).map_or(0.0, |lane| {
                    crate::editor::trim::rendered_clip_width(lane.width, width)
                });
                let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                let active = self
                    .displayed_clip_drag()
                    .and_then(|drag| state.project.clip_active(drag.clip))
                    .unwrap_or(true);
                let color = theme::clip_activity_outline(
                    active,
                    if clip_preview.is_some() {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::ClipOutline
                    },
                );
                let fill = theme::clip_activity_fill(
                    track.color(),
                    self.displayed_clip_drag()
                        .and_then(|d| state.project.clip_flavor(d.clip))
                        .unwrap_or_default(),
                    false,
                    active,
                );
                let mut drop_preview =
                    hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
                        .left_0()
                        .w_full()
                        .border_l(px(
                            if hex_widgets::rectangle_border_width(pixel_width) > 0.0 {
                                1.0 / scale_factor.max(1.0)
                            } else {
                                0.0
                            },
                        ))
                        .border_r(px(0.0))
                        .id(("clip-drop-preview", track_id.value()))
                        .debug_selector(|| "clip-drop-preview".to_owned())
                        .border_color(rgb(color))
                        .bg(rgb(fill))
                        .when(has_border, |preview| {
                            preview
                                .bg(gpui::transparent_black())
                                .border_color(gpui::transparent_black())
                        })
                        .children(
                            aligned_clip_grid_layer(
                                grid,
                                visible_start,
                                visible_end,
                                1.0 / scale_factor.max(1.0),
                                hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                            )
                            .map(|grid| grid.opacity(theme::clip_detail_opacity(active))),
                        );
                if let Some(drag) = self
                    .displayed_clip_drag()
                    .filter(|_| clip_preview.is_some())
                {
                    let source = state
                        .project
                        .clip(drag.clip)
                        .map(ClipPaintSource::Audio)
                        .or_else(|| {
                            state
                                .project
                                .midi_clip(drag.clip)
                                .map(ClipPaintSource::Midi)
                        });
                    if let Some(source) = source {
                        let fill = theme::clip_activity_fill(
                            track.color(),
                            state.project.clip_flavor(drag.clip).unwrap_or_default(),
                            false,
                            state.project.clip_active(drag.clip).unwrap_or(true),
                        );
                        if let Some(content) = self.clip_drag_content(
                            state,
                            source,
                            (preview.start_frame, preview.end_frame),
                            (visible_start, visible_end),
                            fill,
                            pixel_width,
                            scale_factor,
                            self.track_body_height(track_id)
                                <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                            has_border,
                        ) {
                            drop_preview = drop_preview.child(content);
                        }
                    }
                } else if let Some(waveform) = self
                    .pending_sample_drop
                    .as_ref()
                    .and_then(|drop| drop.metadata.clone())
                    .or_else(|| {
                        self.displayed_browser_audio_drag()?
                            .metadata
                            .borrow()
                            .clone()
                    })
                    .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                    .map(|metadata| std::sync::Arc::clone(&metadata.waveform))
                {
                    drop_preview = drop_preview.children([clip_waveform_body(
                        waveform_source_segment_view_cached(
                            waveform,
                            preview.start_frame,
                            visible_start,
                            visible_end,
                            std::rc::Rc::clone(&self.waveform_geometry_cache),
                            crate::theme::color(crate::theme::PaletteRole::Waveform),
                        ),
                    )]);
                }
                if clip_preview.is_some()
                    && self
                        .displayed_clip_drag()
                        .is_some_and(|drag| drag.alias_copy())
                {
                    drop_preview = drop_preview.children([alias_drag_badge()]);
                }
                let framed_clip = (if has_border {
                    crate::editor::clip_boundary::backdrop_shell(
                        drop_preview,
                        left as f32,
                        width as f32,
                        crate::editor::clip_boundary::Backdrop {
                            body: fill,
                            header: fill,
                            outline: color,
                            hover_outline: color,
                            stroke: 1.0 / scale_factor.max(1.0),
                            header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                            header_gradient: None,
                            notch: crate::editor::clip_boundary::Notch::new(
                                visible_end == preview.end_frame,
                            ),
                        },
                        true,
                        || "clip-drop-preview-trailing".into(),
                    )
                } else {
                    clip_boundary_shell(
                        drop_preview,
                        left as f32,
                        width as f32,
                        1.0 / scale_factor.max(1.0),
                        color,
                        color,
                        true,
                        hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                        0.0,
                        || "clip-drop-preview-trailing".into(),
                    )
                })
                .opacity(if clip_preview.is_some() { 1.0 } else { 0.88 });
                clip_area = clip_area.child(framed_clip);
                let excluded_clip = self
                    .displayed_clip_drag()
                    .filter(|drag| clip_preview.is_some() && !drag.copy_modifier.get())
                    .map(|drag| drag.clip);
                for (index, (start, end)) in
                    drop_overlap_spans(state, track, (visible_start, visible_end), excluded_clip)
                        .into_iter()
                        .filter(|_| clip_preview.is_none() || self.active_clip_drag.is_some())
                        .enumerate()
                {
                    clip_area = clip_area.child(
                        div()
                            .debug_selector(move || format!("clip-drop-overlap-{index}"))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(
                                ((start - viewport.start) as f64 / viewport.length as f64) as f32,
                            ))
                            .w(relative(
                                ((end - start) as f64 / viewport.length as f64) as f32,
                            ))
                            .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                            .opacity(0.72),
                    );
                }
            }
        } else if let Some((pending_track, start)) = self
            .sample_drop_target()
            .and_then(|destination| match destination {
                drag::BrowserDropDestination::ExistingTrack { track, start } => {
                    Some((track, start))
                }
                _ => None,
            })
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
                        .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
                );
            }
        }
        if self.selected_track == Some(track_id) && self.time_selection.is_some() {
            clip_area = clip_area.child(
                div()
                    .debug_selector(|| format!("time-selection-focus-{}", track_id.value()))
                    .absolute()
                    .inset_0()
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(0.035),
            );
        }
        if let Some(preview) = self.clip_paint_overlay(track_id, viewport) {
            clip_area = clip_area.child(preview);
        }
        if let Some(selected_rows) = selected_rows
            && let Some(selection) = self.time_selection_overlay(
                track_id,
                track_index,
                selected_rows,
                viewport,
                scale_factor,
                cx,
            )
        {
            clip_area = clip_area.children([selection]);
        }
        if let Some(preview) = self.time_range_move_preview_overlay(track_id, track_index, viewport)
        {
            clip_area = clip_area.children([preview]);
        }
        if let Some(display) = move_display.as_ref()
            && let Some(destination_offset) = track_index
                .checked_sub(display.destination_first_index)
                .filter(|_| track_index <= display.destination_last_index)
            && let Some(source_track_id) = display.source.tracks().get(destination_offset)
            && let Some(source_track) = state.project.track(*source_track_id)
        {
            for clip in source_track.clips() {
                let Some((selected_start, selected_end)) =
                    clip_intersection(clip, (display.source.start(), display.source.end()))
                else {
                    continue;
                };
                let Some(destination_start) = display
                    .destination_start
                    .checked_add(selected_start - display.source.start())
                else {
                    continue;
                };
                let Ok(fragment) = hex_project::preview_clip_fragment_with_tempo(
                    clip,
                    selected_start,
                    selected_end,
                    destination_start,
                    state.project.tempo_map(),
                ) else {
                    continue;
                };
                if let Some(fragment_view) = passive_audio_fragment(
                    self.drag_lanes
                        .get(&track_id)
                        .map_or(0.0, |lane| lane.width),
                    scale_factor,
                    theme::clip_activity_fill(
                        track.color(),
                        fragment.flavor(),
                        false,
                        fragment.active(),
                    ),
                    &fragment,
                    state.waveforms.get(&fragment.asset()),
                    viewport,
                    viewport_end,
                    grid,
                    std::rc::Rc::clone(&self.waveform_geometry_cache),
                    format!(
                        "time-move-audio-fragment-{}-{}",
                        track_id.value(),
                        clip.id().value()
                    ),
                    self.track_body_height(track_id)
                        <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                ) {
                    clip_area = clip_area.children([fragment_view]);
                }
            }
            for clip in source_track.midi_clips() {
                let Some(preview) = state.midi_preview.get(clip.id()) else {
                    continue;
                };
                let selected_start = preview.frame_start.max(display.source.start());
                let selected_end = preview.frame_end.min(display.source.end());
                if selected_start >= selected_end {
                    continue;
                }
                let Some(destination_start) = display
                    .destination_start
                    .checked_add(selected_start - display.source.start())
                else {
                    continue;
                };
                let Some(destination_end) =
                    destination_start.checked_add(selected_end - selected_start)
                else {
                    continue;
                };
                let fragment = passive_midi_fragment(
                    self.drag_lanes
                        .get(&track_id)
                        .map_or(0.0, |lane| lane.width),
                    scale_factor,
                    theme::clip_activity_fill(track.color(), clip.flavor(), false, clip.active()),
                    clip.id(),
                    clip.name(),
                    clip.active(),
                    |start, end, merge| {
                        self.loop_note_shapes(&state.project, clip, start, end, merge)
                    },
                    (selected_start, selected_end),
                    (destination_start, destination_end),
                    viewport,
                    viewport_end,
                    grid,
                    "time-move-midi-fragment".to_owned(),
                    self.track_body_height(track_id)
                        <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                );
                if let Some(fragment) = fragment {
                    clip_area = clip_area.children([fragment]);
                }
            }
        }
        if self
            .applied_clip_preview
            .is_some_and(|(_, applied)| applied.destination.existing_track() == Some(track_id))
            && let Some((_, frame)) = self.audible_clip_move_anchor()
            && let Some(position) = viewport.fraction(frame)
        {
            clip_area = clip_area.child(
                div()
                    .debug_selector(|| "clip-audition-audible-marker".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(position))
                    .w(px(1.0 / scale_factor.max(1.0)))
                    .bg(rgb(theme::color(theme::PaletteRole::Primary)))
                    .child(
                        div()
                            .debug_selector(|| "clip-audition-audible-label".into())
                            .absolute()
                            .left(px(2.0))
                            .top_0()
                            .px_1()
                            .bg(rgb(theme::color(theme::PaletteRole::Primary)))
                            .text_color(rgb(theme::color(theme::PaletteRole::Background)))
                            .text_xs()
                            .child(theme::text("audible")),
                    ),
            );
        }
        clip_area = clip_area
            .on_drop(cx.listener(move |view, drag: &ClipDrag, window, cx| {
                view.drop_clip(drag, track_id, window, cx);
            }))
            .on_drop(
                cx.listener(move |view, drag: &drag::BrowserAudioDrag, window, cx| {
                    view.drop_browser_audio(drag, window, cx);
                }),
            )
            .on_drop(cx.listener(move |view, drag: &TrimDrag, window, cx| {
                view.drop_trim(drag, track_id, window, cx);
            }))
            .on_drop(cx.listener(
                move |view, drag: &crate::editor::trim::MidiTrimDrag, _, cx| {
                    view.drop_midi_trim(drag, track_id, cx);
                },
            ));
        if self.automation_view.enabled {
            let width = self
                .drag_lanes
                .get(&track_id)
                .map_or(0.0, |lane| lane.width);
            if self.gain_lane.track == Some(track_id)
                && !self.automation_view.devices.has_main(
                    crate::editor::device_panel::DevicePanelOwner::Track(track_id),
                )
            {
                clip_area = clip_area.children(crate::editor::gain_lane::inline(
                    self, state, track_id, viewport, width, cx,
                ));
            }
            clip_area = clip_area.children(crate::editor::device_automation::overlay(
                self,
                state,
                crate::editor::device_panel::DevicePanelOwner::Track(track_id),
                viewport,
                cx,
            ));
        }
        let header = self.track_header_element(state, track, track_index, scale_factor, cx);
        let row = div()
            .w_full()
            .h(px(self.track_body_height(track_id)))
            .flex_shrink_0()
            .flex()
            .child(clip_area)
            .child(header)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_track_context_menu(event.position, Some(track_id), window, cx);
                }),
            );
        let rows = crate::editor::device_automation::rows(
            self,
            state,
            crate::editor::device_panel::DevicePanelOwner::Track(track_id),
            viewport,
            cx,
        );
        let row = div()
            .on_scroll_wheel(
                cx.listener(move |view, event: &gpui::ScrollWheelEvent, _, cx| {
                    view.resize_row_wheel(Some(track_id), event, cx);
                }),
            )
            .w_full()
            .h(px(self.track_height(track_id)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(row)
            .child(rows);
        self.decorate_focused_track_row(track_index, track_id, scale_factor, row)
    }

    pub(in crate::editor) fn build_track_header(
        &mut self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let track_id = track.id();
        let selected = self.selected_track() == Some(track_id);
        let track_selected = self.track_selection.contains(track_id);
        let track_name = track.name().to_owned();
        let name_colors = theme::element_colors(track.color());
        let (name_fill, name_text) = if track_selected {
            (name_colors.accent, name_colors.accent_text)
        } else {
            (name_colors.muted, name_colors.muted_text)
        };
        let hierarchy_indent = self.track_hierarchy_indent.pixels(track_id);
        let is_group = track.is_group();
        let track_name_view = crate::editor::track_header::name()
            .text_color(rgb(name_text))
            .flex()
            .items_center()
            .children((track_selected || is_group).then(|| {
                crate::editor::track_header::selection_marker(
                    format!("selected-track-marker-{}", track_id.value()),
                    track_selected,
                )
            }))
            .children((!is_group && hierarchy_indent > 0.0).then(|| {
                div()
                    .w(px(hierarchy_indent))
                    .flex_shrink_0()
                    .debug_selector(move || format!("track-hierarchy-indent-{}", track_id.value()))
            }))
            .id(("track-name", track_id.value()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                    if event.click_count == 1 {
                        view.begin_track_reorder(track_id, event.position, window, cx);
                    }
                }),
            )
            .relative()
            .debug_selector(|| format!("track-name-{}", track_id.value()))
            .children(is_group.then(|| {
                let folded = self.arrangement_focus.projection.folded(track_id);
                div()
                    .id(("group-fold", track_id.value()))
                    .debug_selector(move || format!("group-fold-{}", track_id.value()))
                    .w(px(14.0))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        if view.arrangement_view_change_available(cx)
                            && view
                                .state
                                .as_ref()
                                .and_then(|state| state.project.track(track_id))
                                .is_some_and(Track::is_group)
                        {
                            view.arrangement_focus.projection.toggle_fold(track_id);
                            view.reconcile_arrangement_focus();
                            cx.notify();
                        }
                    }))
                    .tooltip(move |_, cx| {
                        cx.new(|_| crate::editor::transport::MiniTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            description: if folded { "expand group" } else { "fold group" }.into(),
                        })
                        .into()
                    })
                    .child(theme::text(if folded { "▸" } else { "▾" }))
            }))
            .child(self.track_name_label(
                crate::editor::track_name::Target::Track(track.id()),
                track.name(),
                cx,
            ))
            .children(self.track_tags_header(track, cx))
            .tooltip(move |_, cx| {
                cx.new(|_| ClipTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    name: track_name.clone(),
                })
                .into()
            });
        let header_height = self.track_body_height(track_id);
        let name_area = div()
            .id(("track-name-area", track_id.value()))
            .cursor_pointer()
            .debug_selector(move || format!("track-name-area-{}", track_id.value()))
            .h_full()
            .min_w_0()
            .flex_1()
            .relative()
            .px(px(2.0))
            .when(is_group, |plate| plate.ml(px(hierarchy_indent)))
            .children([crate::editor::track_header::name_plate(
                format!("track-name-plate-{}", track_id.value()),
                name_fill,
            )])
            .child(track_name_view);
        let sample_source = crate::editor::sample_recorder::Arm::Output(
            hex_engine::OutputCaptureId::Track(track_id),
        );
        let resident_layout = self.document_audio_mode == hex_session::DocumentAudioMode::Resident;
        let (muted, soloed) = self.displayed_track_switches(track);
        let (active_background, active_text) = crate::editor::track_gain::activator_colors(!muted);
        let (solo_background, solo_text) = crate::editor::track_gain::activator_colors(soloed);
        let activator = toggle_button_with_style(
            "",
            if muted {
                "activate track"
            } else {
                "deactivate track"
            },
            ("track-mute", track_id.value()),
            theme::action_style().with_state_colors(active_background, active_text),
        )
        .debug_selector(move || format!("track-mute-{}", track_id.value()))
        .w(px(34.0))
        .flex_shrink_0()
        .h(px(14.0))
        .text_size(px(10.0))
        .border_1()
        .border_color(rgb(theme::control_style().border))
        .child(theme::text((track_index + 1).to_string()))
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            view.toggle_mute(track_id);
            cx.notify();
        }));
        let solo = toggle_button_with_style(
            "s",
            "solo track · s · cmd+click additive",
            ("track-solo", track_id.value()),
            theme::action_style().with_state_colors(solo_background, solo_text),
        )
        .debug_selector(move || format!("track-solo-{}", track_id.value()))
        .w(px(16.0))
        .h(px(14.0))
        .text_size(px(10.0))
        .border_1()
        .border_color(rgb(theme::control_style().border))
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            cx.stop_propagation();
            view.toggle_solo(track_id, !event.modifiers().platform);
            cx.notify();
        }));
        let gain_lane = button(
            "a",
            "show track gain automation",
            ("gain-lane", track_id.value()),
        )
        .w(px(14.0))
        .h(px(14.0))
        .px_0()
        .debug_selector(move || format!("gain-lane-show-{}", track_id.value()))
        .on_click(cx.listener(move |view, _, window, cx| {
            cx.stop_propagation();
            view.show_gain_lane(track_id, window, cx);
        }));
        let sends = button(
            "↗",
            "edit return sends",
            ("track-return-sends", track_id.value()),
        )
        .w(px(14.0))
        .h(px(14.0))
        .px_0()
        .debug_selector(move || format!("track-return-sends-{}", track_id.value()))
        .on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            let next = if view.send_menu_track == Some(track_id) {
                None
            } else {
                Some(track_id)
            };
            if next != view.send_menu_track
                && !crate::editor::device_panel::finalize_controls(view, cx)
            {
                cx.notify();
                return;
            }
            view.send_menu_track = next;
            cx.notify();
        }));
        // Keep the working switches above the resize gutter, with numeric
        // controls taking the second line only when they fit.
        let header_controls = div()
            .id(("arrangement-track-controls", track_id.value()))
            .debug_selector(move || format!("track-controls-{}", track_id.value()))
            .w(px(122.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .w_full()
                    .h(px(crate::editor::track_header::NAME_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(activator)
                    .children([gain_lane, sends])
                    .child(div().flex_1())
                    .child(self.sample_arm_button(sample_source, cx))
                    .child(solo)
                    .children(
                        resident_layout.then(|| self.audio_arm_button(track_id, !is_group, cx)),
                    ),
            )
            .children((header_height >= TRACK_ROW_HEIGHT).then(|| {
                div()
                    .w_full()
                    .flex()
                    .gap(px(2.0))
                    .child(
                        self.track_volume(track, cx)
                            .id(("track-volume-hit", track_id.value()))
                            .on_click(|_, _, cx| cx.stop_propagation()),
                    )
                    .children(resident_layout.then(|| {
                        self.track_pan(track, cx)
                            .id(("track-pan-hit", track_id.value()))
                            .on_click(|_, _, cx| cx.stop_propagation())
                    }))
            }));
        let header_focus = self.track_header_focus(track_id, cx);
        let click_focus = header_focus.clone();
        let next_ancestors = self
            .arrangement_row_index(track_index)
            .and_then(|row| self.arrangement_track_index(row + 1))
            .and_then(|index| state.project.tracks().get(index))
            .map_or(&[][..], |next| {
                self.track_hierarchy_indent.ancestors(next.id())
            });
        let branch_ends = |ancestor| !next_ancestors.iter().any(|(id, _)| *id == ancestor);
        let own_branch_ends = branch_ends(track_id);
        let group_rails = self
            .track_hierarchy_indent
            .ancestors(track_id)
            .iter()
            .enumerate()
            .map(|(depth, &(ancestor, color))| {
                let identity = theme::element_colors(color);
                let fill = if self.track_selection.contains(ancestor) {
                    identity.accent
                } else {
                    identity.muted
                };
                div()
                    .absolute()
                    .left(px(
                        depth as f32 * crate::editor::track_hierarchy_indent::RAIL_WIDTH
                    ))
                    .top_0()
                    .bottom_0()
                    .w(px(crate::editor::track_hierarchy_indent::RAIL_WIDTH))
                    .debug_selector(move || {
                        format!("track-group-rail-{}-{}", track_id.value(), ancestor.value())
                    })
                    .when(branch_ends(ancestor), |rail| {
                        rail.rounded_bl(px(crate::editor::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                    })
                    .bg(rgb(fill))
            })
            .collect::<Vec<_>>();
        let header = crate::editor::track_header::surface(
            ("arrangement-track-header", track_id.value()),
            self.track_header_width(),
            header_height,
            selected,
            None,
        )
        .pb_0()
        .when(is_group || hierarchy_indent > 0.0, |header| {
            header.border_b_0()
        })
        .debug_selector(move || format!("arrangement-track-header-{}", track_id.value()))
        .track_focus(&header_focus)
        .key_context("hex_track_header")
        .on_click(
            cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                view.select_track_with_modifiers(track_id, event.modifiers(), window);
                click_focus.focus(window);
                cx.notify();
            }),
        )
        .child(
            div()
                .w_full()
                .h_full()
                .flex()
                .items_start()
                .gap(px(2.0))
                .children([name_area, header_controls])
                .children(
                    self.track_performance_indicator(hex_engine::OutputCaptureId::Track(track_id)),
                )
                .children(
                    self.track_latency_indicator(crate::editor::track_latency::Owner::Track(
                        track_id,
                    )),
                )
                .children([crate::editor::track_meters::render_arrangement(
                    track_id,
                    self.track_meters.level(track_id),
                    header_height,
                )]),
        )
        .children(self.track_resize_handles(track_id, cx))
        .relative()
        .children(group_rails)
        .child(
            div()
                .absolute()
                .left(px(hierarchy_indent))
                .top(px(if is_group || hierarchy_indent > 0.0 {
                    0.0
                } else {
                    1.0
                }))
                .bottom_0()
                .when(is_group, |rail| {
                    rail.rounded_tl(px(crate::editor::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                })
                .when(is_group && own_branch_ends, |rail| {
                    rail.rounded_bl(px(crate::editor::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                })
                .w(px(crate::editor::track_hierarchy_indent::RAIL_WIDTH))
                .debug_selector(move || format!("track-color-strip-{}", track_id.value()))
                .bg(rgb(if is_group {
                    name_fill
                } else {
                    theme::element_colors(track.color()).accent
                })),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                view.open_track_color_context_menu(event.position, track_id, window, cx);
            }),
        );
        self.device_drop_header(track_id, header, cx)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_header_and_row_dimensions_remain_positive() {
        const { assert!(TRACK_ROW_HEIGHT > 0.0) };
        const { assert!(TRACK_HEADER_WIDTH >= 200.0) };
        assert_eq!(DEFAULT_TRACK_HEADER_WIDTH, TRACK_HEADER_WIDTH);
    }
}
