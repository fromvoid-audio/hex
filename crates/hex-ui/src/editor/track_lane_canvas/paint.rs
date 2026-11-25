//! Batched resting-clip lane painting for `TrackLaneCanvas`.
use super::*;

impl TrackLaneCanvas {
    pub(super) fn paint_lane(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.is_empty() || self.viewport.length == 0 {
            return;
        }
        let Some(viewport_end) = self.viewport.end() else {
            return;
        };
        let lane_width = f32::from(bounds.size.width);
        let lane_height = f32::from(bounds.size.height);
        if !lane_width.is_finite()
            || lane_width <= 0.0
            || !lane_height.is_finite()
            || lane_height <= 0.0
        {
            return;
        }
        let scale = window.scale_factor().max(1.0);
        let stroke = 1.0 / scale;

        let view = self.editor.read(cx);
        let snapshot_matches = view
            .state
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &self.snapshot))
            || (self.is_committed_edit_view
                && view
                    .arrangement_duplicate_view()
                    .or_else(|| view.committed_section_copy_view())
                    .or_else(|| view.arrangement_paste_view())
                    .or_else(|| view.time_range_copy_view())
                    .is_some_and(|s| Arc::ptr_eq(s, &self.snapshot)));
        if !snapshot_matches {
            return;
        }
        view.arrangement_clip_index
            .borrow_mut()
            .ensure_current(view.project_generation, &self.snapshot);
        let Some(track) = self
            .snapshot
            .project
            .tracks()
            .get(self.track_index)
            .filter(|t| t.id() == self.track_id)
        else {
            return;
        };

        let index_ref = view.arrangement_clip_index.borrow();
        let Some(track_index_data) = index_ref.track(self.track_id) else {
            return;
        };
        let track_color = track.color();
        let track_height = view.track_height(self.track_id);
        let track_body_height = view.track_body_height(self.track_id);
        let hovered_clip = view
            .hovered_arrangement_clip
            .filter(|h| h.generation == view.project_generation && h.track == self.track_id)
            .map(|h| h.clip);
        #[cfg(feature = "ui-test-support")]
        let drag_available = view.drag_available();
        #[cfg(feature = "ui-test-support")]
        let trim_admitted = trim::drag_admitted(view);
        #[cfg(feature = "ui-test-support")]
        let header_hit_height = super::super::time_selection::header_hit_height(lane_height);
        #[cfg(feature = "ui-test-support")]
        let edge_trim_h = trim::edge_trim_height(lane_height);
        #[cfg(feature = "ui-test-support")]
        let fade_key_held = view.fade_key_held;
        #[cfg(feature = "ui-test-support")]
        let time_selection_key_held = view.time_selection_key_held;
        let rem_size = f32::from(window.rem_size());
        let font_size = px(rem_size * 0.75);
        let line_height = px(super::super::clip_name::CLIP_NAME_HEIGHT);

        let frames_per_dev_px =
            self.viewport.length as f64 / (f64::from(lane_width) * f64::from(scale));
        let mut narrow_spans: Vec<(f64, f64, NarrowInk)> = Vec::new();
        let mut narrow_fades: Vec<(usize, Bounds<Pixels>, u64, u64, bool)> = Vec::new();
        let mut wide_clips: Vec<WideClipPlan> = Vec::new();

        let is_excluded = |id: ClipId| self.excluded.contains(&id);

        // Ultra-dense LOD fast path: when an entire bin fits inside a sub-pixel
        // column and contains no selected/hovered/excluded/focus-filtered clips,
        // emit one narrow span for the whole bin in O(1).
        let use_lod = !view.arrangement_focus.projection.enabled
            && self.excluded.is_empty()
            && view.selected_clip().is_none()
            && view.selected_clip_ids().is_empty()
            && hovered_clip.is_none();

        if use_lod && let Some(lod_bins) = track_index_data.lod_level_for_pixel(frames_per_dev_px) {
            for bin in TrackClipIndex::candidate_bins(lod_bins, self.viewport.start, viewport_end) {
                if bin.end <= self.viewport.start || bin.start >= viewport_end {
                    continue;
                }
                let vis_start = bin.start.max(self.viewport.start);
                let vis_end = bin.end.min(viewport_end);
                if vis_start >= vis_end {
                    continue;
                }
                let left_frac =
                    (vis_start - self.viewport.start) as f64 / self.viewport.length as f64;
                let right_frac =
                    (vis_end - self.viewport.start) as f64 / self.viewport.length as f64;
                let rep = track_index_data.clips.get(bin.representative as usize);
                let (flavor, active) = rep
                    .map(|c| clip_flavor_active(view, track, *c))
                    .unwrap_or((hex_project::ClipFlavor::Muted, true));
                let header_fill = theme::clip_activity_fill(track_color, flavor, false, active);
                let fill_color =
                    theme::arrangement_clip_activity_body_fill(header_fill, false, active);
                narrow_spans.push((
                    left_frac,
                    right_frac,
                    NarrowInk {
                        color: fill_color,
                        selected: false,
                    },
                ));
            }
        } else {
            let candidates = track_index_data.candidate_slice(self.viewport.start, viewport_end);
            for &entry in candidates {
                if entry.end <= self.viewport.start || entry.start >= viewport_end {
                    continue;
                }
                if is_excluded(entry.id)
                    || (!self.is_committed_edit_view && !view.arrangement_clip_visible(entry.id))
                {
                    continue;
                }
                let vis_start = entry.start.max(self.viewport.start);
                let vis_end = entry.end.min(viewport_end);
                if vis_start >= vis_end {
                    continue;
                }
                let left_frac =
                    (vis_start - self.viewport.start) as f64 / self.viewport.length as f64;
                let right_frac =
                    (vis_end - self.viewport.start) as f64 / self.viewport.length as f64;
                let width_frac = (vis_end - vis_start) as f64 / self.viewport.length as f64;
                let pixel_width = trim::rendered_clip_width(lane_width, width_frac);
                let selected = view.clip_is_selected(entry.id);
                let (flavor, active, is_midi, idx, looping) = match entry.kind {
                    IndexedClipKind::Audio(i) => {
                        let Some(clip) = track.clips().get(i) else {
                            continue;
                        };
                        let active = view.displayed_clip_active(clip.id(), clip.active());
                        (
                            clip.flavor(),
                            active,
                            false,
                            i,
                            loop_handles::audio_repeats(clip),
                        )
                    }
                    IndexedClipKind::Midi(i) => {
                        let Some(clip) = track.midi_clips().get(i) else {
                            continue;
                        };
                        let active = view.displayed_clip_active(clip.id(), clip.active());
                        (clip.flavor(), active, true, i, clip.looping())
                    }
                };
                let header_fill = theme::clip_activity_fill(track_color, flavor, false, active);
                let body_fill =
                    theme::arrangement_clip_activity_body_fill(header_fill, selected, active);
                let narrow = narrow_clips::is_narrow(pixel_width, scale);
                let lane_left_f64 = f64::from(bounds.left());
                let raw_left = lane_left_f64 + left_frac * f64::from(lane_width);
                let raw_right = lane_left_f64 + right_frac * f64::from(lane_width);
                // Match Taffy's physical-pixel rounding so resting canvas clips
                // and promoted overlay clips share identical endpoints at every scale.
                let snapped_left = snap_px(raw_left, scale);
                let snapped_right = snap_px(raw_right, scale);
                let clip_bounds = Bounds::new(
                    point(px(snapped_left), bounds.top()),
                    size(
                        px((snapped_right - snapped_left).max(0.0)),
                        bounds.size.height,
                    ),
                );
                let resolved_width = f32::from(clip_bounds.size.width);

                if narrow {
                    narrow_spans.push((
                        left_frac,
                        right_frac,
                        NarrowInk {
                            color: body_fill,
                            selected,
                        },
                    ));
                    if !is_midi
                        && track_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                        && let Some(audio_clip) = track.clips().get(idx)
                    {
                        let fade = view
                            .fade_preview(audio_clip.id())
                            .unwrap_or(audio_clip.retained_fade());
                        if !fade.is_neutral() {
                            narrow_fades.push((idx, clip_bounds, vis_start, vis_end, active));
                        }
                    }
                    #[cfg(feature = "ui-test-support")]
                    {
                        record_narrow_clip_debug_bounds(
                            window,
                            entry.id,
                            is_midi,
                            clip_bounds,
                            header_hit_height,
                            drag_available,
                        );
                    }
                    continue;
                }

                let border = hex_widgets::rectangle_border_width(resolved_width) > 0.0;
                let notched = border && vis_end == entry.end;
                let outline = theme::clip_activity_outline(
                    active,
                    if selected {
                        theme::PaletteRole::Primary
                    } else {
                        theme::PaletteRole::ClipOutline
                    },
                );
                let hover_outline = if is_midi {
                    outline
                } else {
                    theme::clip_activity_outline(
                        active,
                        if selected {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Text
                        },
                    )
                };
                let waveform_ink = if is_midi {
                    0
                } else {
                    theme::clip_waveform_ink(
                        track_color,
                        theme::ClipWaveformBackground::Arrangement { flavor, selected },
                        active,
                    )
                };
                let height_allows_detail = if is_midi {
                    track_body_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                } else {
                    track_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                };
                let shows_detail = height_allows_detail
                    && narrow_clips::shows_body_detail(
                        lane_width,
                        entry.end - entry.start,
                        self.viewport.length,
                    );
                wide_clips.push(WideClipPlan {
                    id: entry.id,
                    is_midi,
                    index: idx,
                    #[cfg(feature = "ui-test-support")]
                    clip_start: entry.start,
                    #[cfg(feature = "ui-test-support")]
                    clip_end: entry.end,
                    visible_start: vis_start,
                    visible_end: vis_end,
                    bounds: clip_bounds,
                    pixel_width: resolved_width,
                    header_fill,
                    body_fill,
                    outline,
                    hover_outline,
                    waveform_ink,
                    selected,
                    hovered: hovered_clip == Some(entry.id),
                    active,
                    border,
                    notched,
                    shows_detail,
                    looping,
                });
            }
        }

        // Record narrow-clip-bars debug selector when narrow bars exist.
        #[cfg(feature = "ui-test-support")]
        if !narrow_spans.is_empty() {
            window.record_debug_bounds(
                format!("narrow-clip-bars-{}", self.track_id.value()),
                bounds,
            );
        }

        // PASS 1: Narrow bars + Wide clip backdrops (fills, headers, borders,
        // notches, header gradients) + unselected trailing edges in ONE paint_layer.
        with_lane_layer(bounds, window, |window| {
            paint_narrow_columns(&narrow_spans, bounds, window);
            for clip in &wide_clips {
                let outline_color = Hsla::from(rgb(if clip.hovered {
                    clip.hover_outline
                } else {
                    clip.outline
                }));
                if clip.border {
                    let cut = if clip.notched {
                        clip_boundary::corner_extent(clip.pixel_width, lane_height)
                    } else {
                        0.0
                    };
                    let header_gradient = if clip.is_midi {
                        track.midi_clips().get(clip.index).and_then(|midi_clip| {
                            if let Some(values) =
                                view.midi_header_loop_values(midi_clip.id(), self.snapshot.revision)
                            {
                                super::super::clip_header_loop::midi_source(
                                    midi_clip,
                                    values,
                                    self.snapshot.project.tempo_map(),
                                    clip.visible_start..clip.visible_end,
                                    clip.pixel_width * scale,
                                )
                            } else if clip.looping {
                                super::super::clip_header_loop::midi(
                                    midi_clip,
                                    None,
                                    self.snapshot.project.tempo_map(),
                                    clip.visible_start..clip.visible_end,
                                    clip.pixel_width * scale,
                                )
                            } else {
                                None
                            }
                        })
                    } else if clip.looping {
                        track.clips().get(clip.index).and_then(|audio_clip| {
                            super::super::clip_header_loop::audio(
                                audio_clip,
                                self.snapshot.project.tempo_map(),
                                clip.visible_start..clip.visible_end,
                                clip.pixel_width * scale,
                            )
                        })
                    } else {
                        None
                    };
                    clip_boundary::paint_backdrop_with_cut(
                        clip.bounds,
                        clip.body_fill,
                        clip.header_fill,
                        stroke,
                        super::super::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient.as_ref(),
                        cut,
                        outline_color,
                        window,
                    );
                    // Header bottom divider line (matches clip_name_surface border_b_1).
                    let text_rgb = theme::readable_text(
                        clip.header_fill,
                        theme::color(theme::PaletteRole::Text),
                    );
                    let header_h =
                        px(super::super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height);
                    let header_bounds = Bounds::from_corners(
                        point(clip.bounds.left() + px(stroke - 1.0), clip.bounds.top()),
                        point(clip.bounds.right() + px(1.0), clip.bounds.top() + header_h),
                    );
                    let inner_mask = Bounds::from_corners(
                        point(clip.bounds.left() + px(stroke), clip.bounds.top() + px(1.0)),
                        point(clip.bounds.right(), clip.bounds.bottom() - px(1.0)),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: inner_mask }), |window| {
                        window.paint_quad(quad(
                            header_bounds,
                            Corners::default(),
                            gpui::transparent_black(),
                            Edges {
                                top: px(0.0),
                                right: px(0.0),
                                bottom: px(1.0),
                                left: px(0.0),
                            },
                            rgba((text_rgb << 8) | 0x70),
                            Default::default(),
                        ));
                    });
                    if !clip.selected {
                        clip_boundary::paint_trailing_with_cut(
                            clip.bounds,
                            stroke,
                            clip.notched.then_some(cut),
                            outline_color,
                            window,
                        );
                    }
                } else {
                    // Sub-border wide clip (> 4 device px, <= rectangle border threshold):
                    // solid body + header + leading/trailing physical hairlines.
                    window.paint_quad(fill(clip.bounds, rgb(clip.body_fill)));
                    let header_rect = Bounds::new(
                        clip.bounds.origin,
                        size(
                            clip.bounds.size.width,
                            px(super::super::clip_name::CLIP_NAME_HEIGHT)
                                .min(clip.bounds.size.height),
                        ),
                    );
                    window.paint_quad(fill(header_rect, rgb(clip.header_fill)));
                    let leading = Bounds::new(
                        clip.bounds.origin,
                        size(px(stroke), clip.bounds.size.height),
                    );
                    window.paint_quad(fill(leading, outline_color));
                    if !clip.selected {
                        let trailing = Bounds::new(
                            point(clip.bounds.right(), clip.bounds.top()),
                            size(px(stroke), clip.bounds.size.height),
                        );
                        window.paint_quad(fill(trailing, outline_color));
                    }
                }
            }
        });

        // PASS 2: Clip-internal grid lines & loop repeat markers (single paint_layer).
        let any_detail_or_loop = wide_clips.iter().any(|c| c.shows_detail || c.looping);
        if any_detail_or_loop {
            with_lane_layer(bounds, window, |window| {
                let grid_color = Hsla::from(rgb(theme::color(theme::PaletteRole::Grid)));
                let meter_color = Hsla::from(rgb(theme::color(theme::PaletteRole::Primary)));
                for clip in &wide_clips {
                    let detail_opacity = theme::clip_detail_opacity(clip.active);
                    if clip.shows_detail {
                        let body_top =
                            clip.bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT);
                        let body_h = (clip.bounds.bottom() - body_top).max(px(0.0));
                        let span = clip.visible_end.saturating_sub(clip.visible_start);
                        if body_h > px(0.0) && span > 0 {
                            let first_line =
                                self.grid.partition_point(|l| l.frame <= clip.visible_start);
                            #[cfg(feature = "ui-test-support")]
                            let mut any_line = false;
                            for line in &self.grid[first_line..] {
                                if line.frame >= clip.visible_end {
                                    break;
                                }
                                #[cfg(feature = "ui-test-support")]
                                {
                                    any_line = true;
                                }
                                // Compute X directly in lane coordinates rounded to
                                // integer logical pixels so clip grid lines share the
                                // exact physical column as the lane's background grid.
                                let lane_frac = (line.frame.saturating_sub(self.viewport.start))
                                    as f64
                                    / self.viewport.length as f64;
                                let x = px(snap_px(
                                    f64::from(bounds.left()) + lane_frac * f64::from(lane_width),
                                    scale,
                                ));
                                let rect =
                                    Bounds::new(point(x, body_top), size(px(stroke), body_h));
                                let alpha = if line.meter_marker || line.bar_start {
                                    0.42
                                } else {
                                    0.24
                                } * detail_opacity;
                                let color = if line.meter_marker {
                                    meter_color.opacity(alpha)
                                } else {
                                    grid_color.opacity(alpha)
                                };
                                window.paint_quad(fill(rect, color));
                                #[cfg(feature = "ui-test-support")]
                                window.record_debug_bounds(
                                    format!("clip-grid-line-{}", line.frame),
                                    rect,
                                );
                            }
                            #[cfg(feature = "ui-test-support")]
                            if any_line {
                                let grid_bounds = Bounds::new(
                                    point(clip.bounds.left(), body_top),
                                    size(clip.bounds.size.width, body_h),
                                );
                                window.record_debug_bounds("clip-musical-grid", grid_bounds);
                            }
                        }
                    }
                    if clip.looping {
                        if clip.is_midi {
                            if let Some(midi_clip) = track.midi_clips().get(clip.index) {
                                loop_handles::paint_midi_markers(
                                    clip.bounds,
                                    midi_clip,
                                    &self.snapshot.project,
                                    clip.visible_start,
                                    clip.visible_end,
                                    detail_opacity,
                                    window,
                                );
                            }
                        } else if let Some(audio_clip) = track.clips().get(clip.index) {
                            loop_handles::paint_audio_markers(
                                clip.bounds,
                                audio_clip,
                                &self.snapshot.project,
                                clip.visible_start,
                                clip.visible_end,
                                detail_opacity,
                                window,
                            );
                        }
                    }
                }
            });
        }

        let title_cache_rc = Rc::clone(&view.clip_title_cache);
        let mut missing_labels: Vec<(&'static str, Bounds<Pixels>)> = Vec::new();
        let mut alias_flags: Vec<bool> = Vec::with_capacity(wide_clips.len());
        for clip in &wide_clips {
            alias_flags.push(view.clip_has_alias(clip.id));
        }

        // PASS 3: Audio waveforms, MIDI notes/tiles, and non-raised fade overlays.
        let waveform_grid = view.waveform_grid_for(self.track_id, self.viewport, scale);
        for clip in &wide_clips {
            if !clip.is_midi {
                let Some(audio_clip) = track.clips().get(clip.index) else {
                    continue;
                };
                if clip.shows_detail {
                    let y_border = if clip.border { 1.0 } else { 0.0 };
                    let body_top = clip.bounds.top()
                        + px(y_border + super::super::clip_name::CLIP_NAME_HEIGHT);
                    let body_bottom = (clip.bounds.bottom() - px(y_border)).max(body_top);
                    let body_bounds = Bounds::new(
                        point(
                            clip.bounds.left() + px(if clip.border { stroke } else { 0.0 }),
                            body_top,
                        ),
                        size(
                            px((clip.pixel_width - if clip.border { stroke } else { 0.0 })
                                .max(0.0)),
                            body_bottom - body_top,
                        ),
                    );
                    if let Some((waveform_data, mapping)) = view.resolve_audio_waveform_source(
                        &self.snapshot,
                        audio_clip,
                        clip.visible_start,
                        clip.visible_end,
                    ) {
                        waveform::paint_timeline_waveform_cached(
                            body_bounds,
                            &waveform_data,
                            &mapping,
                            clip.visible_start,
                            clip.visible_end,
                            &view.waveform_geometry_cache,
                            waveform_grid,
                            clip.waveform_ink,
                            clip.active,
                            window,
                        );
                    } else {
                        let label = if audio_clip.audio_edit().is_some() {
                            "prepared waveform unavailable"
                        } else {
                            "missing media"
                        };
                        missing_labels.push((label, body_bounds));
                    }
                    #[cfg(feature = "ui-test-support")]
                    window.record_debug_bounds("clip-waveform-body", body_bounds);
                }
                // Non-raised fade overlay.
                let fade = view
                    .fade_preview(audio_clip.id())
                    .unwrap_or(audio_clip.retained_fade());
                if track_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                    && clip.pixel_width > 0.0
                    && !fade.is_neutral()
                {
                    let samples = if audio_clip.audio_edit().is_some() {
                        projected_fade_envelope_samples(
                            audio_clip,
                            self.snapshot.project.tempo_map(),
                            clip.visible_start,
                            clip.visible_end,
                            None,
                        )
                        .unwrap_or_default()
                    } else {
                        fade_envelope_samples(
                            fade,
                            audio_clip.start(),
                            clip.visible_start,
                            clip.visible_end,
                        )
                    };
                    if samples.len() >= 2 {
                        paint_fade_overlay(clip.bounds, &samples, false, clip.active, window);
                        #[cfg(feature = "ui-test-support")]
                        {
                            let shade_bounds = Bounds::new(
                                point(
                                    clip.bounds.left(),
                                    clip.bounds.top()
                                        + px(super::super::clip_name::CLIP_NAME_HEIGHT),
                                ),
                                size(
                                    clip.bounds.size.width,
                                    (clip.bounds.size.height
                                        - px(super::super::clip_name::CLIP_NAME_HEIGHT))
                                    .max(px(0.0)),
                                ),
                            );
                            window.record_debug_bounds(
                                format!("clip-fade-shade-{}", audio_clip.id().value()),
                                shade_bounds,
                            );
                        }
                    }
                }
            } else if clip.shows_detail {
                let Some(midi_clip) = track.midi_clips().get(clip.index) else {
                    continue;
                };
                let y_border = if clip.border { 1.0 } else { 0.0 };
                let body_top =
                    clip.bounds.top() + px(y_border + super::super::clip_name::CLIP_NAME_HEIGHT);
                let body_bottom = (clip.bounds.bottom() - px(y_border)).max(body_top);
                let body_bounds = Bounds::new(
                    point(
                        clip.bounds.left() + px(if clip.border { stroke } else { 0.0 }),
                        body_top,
                    ),
                    size(
                        px((clip.pixel_width - if clip.border { stroke } else { 0.0 }).max(0.0)),
                        body_bottom - body_top,
                    ),
                );
                let merge_frames = super::super::midi_loop_notes::merge_frames(
                    clip.visible_end.saturating_sub(clip.visible_start),
                    clip.pixel_width,
                    scale,
                );
                let visible_notes = match view.midi_editor.visible_notes.try_borrow_mut() {
                    Ok(mut cache) => cache.get_tiled(
                        &self.snapshot.project,
                        midi_clip,
                        clip.visible_start,
                        clip.visible_end,
                        merge_frames,
                    ),
                    Err(_) => super::super::midi_loop_notes::visible_notes_tiled(
                        &self.snapshot.project,
                        midi_clip,
                        clip.visible_start,
                        clip.visible_end,
                        merge_frames,
                    )
                    .ok()
                    .map(Rc::new),
                };
                if let Some(visible_notes) = visible_notes {
                    let note_color = Hsla::from(rgb(theme::color(theme::PaletteRole::ClipText)))
                        .opacity(theme::clip_detail_opacity(clip.active));
                    let fit = visible_notes.fit;
                    let clipped_body = body_bounds.intersect(&window.content_mask().bounds);
                    if !clipped_body.is_empty() {
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: clipped_body,
                            }),
                            |window| {
                                if !visible_notes.intervals.is_empty() {
                                    with_lane_layer(body_bounds, window, |window| {
                                        for &(n_start, n_end, pitch) in &visible_notes.intervals {
                                            if let Some((x, w, pitch)) =
                                                super::super::midi_thumbnail::note_shape(
                                                    n_start,
                                                    n_end,
                                                    pitch,
                                                    clip.visible_start,
                                                    clip.visible_end,
                                                )
                                                && let Some(rect) =
                                                    super::super::midi_thumbnail::paint_bounds(
                                                        body_bounds,
                                                        x,
                                                        w,
                                                        pitch,
                                                        fit,
                                                        scale,
                                                    )
                                            {
                                                window.paint_quad(fill(rect, note_color));
                                            }
                                        }
                                    });
                                }
                                if visible_notes.tiles.is_some() {
                                    super::super::midi_loop_notes::paint_tiles(
                                        window,
                                        &view.midi_editor.tile_images,
                                        &self.snapshot.project,
                                        midi_clip.id(),
                                        &visible_notes,
                                        body_bounds,
                                        (clip.visible_start, clip.visible_end),
                                    );
                                }
                            },
                        );
                    }
                    #[cfg(feature = "ui-test-support")]
                    {
                        window.record_debug_bounds("clip-waveform-body", body_bounds);
                        if !visible_notes.intervals.is_empty() || visible_notes.tiles.is_some() {
                            window.record_debug_bounds(
                                format!("clip-notes-{}", midi_clip.id().value()),
                                body_bounds,
                            );
                        }
                    }
                }
            }
        }
        for &(idx, clip_bounds, vis_start, vis_end, active) in &narrow_fades {
            let Some(audio_clip) = track.clips().get(idx) else {
                continue;
            };
            let fade = view
                .fade_preview(audio_clip.id())
                .unwrap_or(audio_clip.retained_fade());
            let samples = if audio_clip.audio_edit().is_some() {
                projected_fade_envelope_samples(
                    audio_clip,
                    self.snapshot.project.tempo_map(),
                    vis_start,
                    vis_end,
                    None,
                )
                .unwrap_or_default()
            } else {
                fade_envelope_samples(fade, audio_clip.start(), vis_start, vis_end)
            };
            if samples.len() >= 2 {
                paint_fade_overlay(clip_bounds, &samples, false, active, window);
                #[cfg(feature = "ui-test-support")]
                {
                    let shade_bounds = Bounds::new(
                        point(
                            clip_bounds.left(),
                            clip_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT),
                        ),
                        size(
                            clip_bounds.size.width,
                            (clip_bounds.size.height
                                - px(super::super::clip_name::CLIP_NAME_HEIGHT))
                            .max(px(0.0)),
                        ),
                    );
                    window.record_debug_bounds(
                        format!("clip-fade-shade-{}", audio_clip.id().value()),
                        shade_bounds,
                    );
                }
            }
        }
        // Record loop handle debug bounds for wide clips while `view` is still in scope.
        #[cfg(feature = "ui-test-support")]
        if drag_available && trim_admitted {
            let lane_bounds = super::super::drag::LaneBounds {
                left: f32::from(bounds.left()),
                top: f32::from(bounds.top()),
                width: lane_width,
                height: lane_height,
            };
            for clip in &wide_clips {
                if !clip.looping || trim::handle_width(clip.pixel_width) <= 0.0 {
                    continue;
                }
                let controls = if clip.is_midi {
                    track.midi_clips().get(clip.index).and_then(|c| {
                        loop_handles::midi_controls(
                            c,
                            &self.snapshot.project,
                            lane_bounds,
                            self.viewport,
                        )
                    })
                } else {
                    track.clips().get(clip.index).and_then(|c| {
                        loop_handles::audio_controls(
                            c,
                            &self.snapshot.project,
                            lane_bounds,
                            self.viewport,
                        )
                    })
                };
                if let Some(controls) = controls {
                    for handle in controls.handles.into_iter().flatten() {
                        let kind_str = match handle.kind {
                            LoopHandleKind::FirstBoundary => "cycle",
                            LoopHandleKind::Outer => "outer",
                        };
                        let sel = format!(
                            "{}clip-loop-{kind_str}-{}",
                            if clip.is_midi { "midi-" } else { "" },
                            clip.id.value()
                        );
                        let rect = Bounds::new(
                            point(
                                clip.bounds.left() + px(handle.left),
                                clip.bounds.top() + px(handle.top),
                            ),
                            size(px(loop_handles::WIDTH), px(handle.height)),
                        );
                        window.record_debug_bounds(sel, rect);
                    }
                }
            }
        }
        // Release immutable borrow on `cx` before painting cached `ShapedLine` titles.
        drop(index_ref);
        let _ = view;

        // PASS 4: Cached shaped clip titles, missing-media notices & alias badges in ONE paint_layer.
        if !wide_clips.is_empty() || !missing_labels.is_empty() {
            with_lane_layer(bounds, window, |window| {
                let mut title_cache = title_cache_rc.try_borrow_mut().ok();
                if let Some(cache) = title_cache.as_mut() {
                    let muted_color = theme::color(theme::PaletteRole::Muted);
                    for (label, body_bounds) in &missing_labels {
                        cache.paint_clipped(
                            label,
                            *body_bounds,
                            super::super::clip_title_cache::TitlePaintStyle {
                                font_size,
                                line_height,
                                color: muted_color,
                            },
                            window,
                            cx,
                        );
                    }
                }
                for (clip, &has_alias) in wide_clips.iter().zip(alias_flags.iter()) {
                    if has_alias && clip.pixel_width >= 14.0 {
                        paint_alias_badge(
                            clip.id,
                            clip.bounds,
                            clip.header_fill,
                            clip.notched,
                            window,
                        );
                    }
                    let omit_label = clip.notched
                        && super::super::clip_name::notched_label_is_clipped(
                            clip.pixel_width,
                            rem_size,
                            scale,
                        );
                    if omit_label {
                        continue;
                    }
                    let name = if clip.is_midi {
                        track.midi_clips().get(clip.index).map(MidiClip::name)
                    } else {
                        track.clips().get(clip.index).map(Clip::name)
                    };
                    let Some(name) = name else {
                        continue;
                    };
                    let pad_left = 4.0 + if clip.border { stroke } else { 0.0 };
                    let pad_right = if has_alias {
                        if clip.notched { 20.0 } else { 15.0 }
                    } else if clip.notched {
                        6.0
                    } else {
                        4.0
                    };
                    let text_width = clip.pixel_width - pad_left - pad_right;
                    if text_width <= 1.0 {
                        continue;
                    }
                    let text_bounds = Bounds::new(
                        point(clip.bounds.left() + px(pad_left), clip.bounds.top()),
                        size(
                            px(text_width),
                            px(super::super::clip_name::CLIP_NAME_HEIGHT)
                                .min(clip.bounds.size.height),
                        ),
                    );
                    let text_color = theme::readable_text(
                        clip.header_fill,
                        theme::color(theme::PaletteRole::Text),
                    );
                    if let Some(cache) = title_cache.as_mut() {
                        cache.paint_clipped(
                            name,
                            text_bounds,
                            super::super::clip_title_cache::TitlePaintStyle {
                                font_size,
                                line_height,
                                color: text_color,
                            },
                            window,
                            cx,
                        );
                    }
                    #[cfg(feature = "ui-test-support")]
                    window.record_debug_bounds(
                        format!("clip-name-label-{}", clip.id.value()),
                        text_bounds,
                    );
                }
            });
        }

        // PASS 5: Selected clip borders & trailing edges painted LAST so the
        // Primary selection stroke always wins over adjacent clips.
        let any_selected = wide_clips.iter().any(|c| c.selected);
        if any_selected {
            with_lane_layer(bounds, window, |window| {
                for clip in wide_clips.iter().filter(|c| c.selected) {
                    let outline_color = Hsla::from(rgb(clip.outline));
                    if clip.border {
                        let cut = if clip.notched {
                            clip_boundary::corner_extent(clip.pixel_width, lane_height)
                        } else {
                            0.0
                        };
                        clip_boundary::paint_trailing_with_cut(
                            clip.bounds,
                            stroke,
                            clip.notched.then_some(cut),
                            outline_color,
                            window,
                        );
                        #[cfg(feature = "ui-test-support")]
                        {
                            let edge = Bounds::new(
                                point(clip.bounds.right(), clip.bounds.top()),
                                size(px(stroke), clip.bounds.size.height),
                            );
                            let sel = if clip.is_midi {
                                format!("midi-clip-selected-trailing-{}", clip.id.value())
                            } else {
                                format!("clip-selected-trailing-{}", clip.id.value())
                            };
                            window.record_debug_bounds(sel, edge);
                        }
                    } else {
                        let trailing = Bounds::new(
                            point(clip.bounds.right(), clip.bounds.top()),
                            size(px(stroke), clip.bounds.size.height),
                        );
                        window.paint_quad(fill(trailing, outline_color));
                    }
                }
            });
        }

        // Record test-support debug bounds for wide clips (compiled out in release!).
        #[cfg(feature = "ui-test-support")]
        for clip in &wide_clips {
            let visual_sel = if clip.is_midi {
                format!("midi-clip-visual-{}", clip.id.value())
            } else {
                format!("clip-visual-{}", clip.id.value())
            };
            window.record_debug_bounds(visual_sel, clip.bounds);
            let trailing_sel = if clip.is_midi {
                format!("midi-clip-trailing-{}", clip.id.value())
            } else {
                format!("clip-trailing-{}", clip.id.value())
            };
            let trailing_edge = Bounds::new(
                point(clip.bounds.right(), clip.bounds.top()),
                size(px(stroke), clip.bounds.size.height),
            );
            window.record_debug_bounds(trailing_sel, trailing_edge);
            let header_bounds = if clip.border {
                Bounds::from_corners(
                    point(clip.bounds.left() + px(stroke - 1.0), clip.bounds.top()),
                    point(
                        clip.bounds.right() + px(1.0),
                        clip.bounds.top()
                            + px(super::super::clip_name::CLIP_NAME_HEIGHT)
                                .min(clip.bounds.size.height),
                    ),
                )
            } else {
                Bounds::new(
                    clip.bounds.origin,
                    size(
                        clip.bounds.size.width,
                        px(super::super::clip_name::CLIP_NAME_HEIGHT).min(clip.bounds.size.height),
                    ),
                )
            };
            window.record_debug_bounds(format!("clip-name-{}", clip.id.value()), header_bounds);
            if drag_available {
                let hw = if trim_admitted {
                    trim::handle_width(clip.pixel_width)
                } else {
                    0.0
                };
                let left_inset = if hw > 0.0 && clip.visible_start == clip.clip_start {
                    hw
                } else {
                    0.0
                };
                let right_inset = if hw > 0.0 && clip.visible_end == clip.clip_end {
                    hw
                } else {
                    0.0
                };
                let move_bounds = Bounds::new(
                    point(clip.bounds.left() + px(left_inset), clip.bounds.top()),
                    size(
                        px((clip.pixel_width - left_inset - right_inset).max(0.0)),
                        px(header_hit_height).min(clip.bounds.size.height),
                    ),
                );
                window.record_debug_bounds(
                    format!("clip-drag-surface-{}", clip.id.value()),
                    move_bounds,
                );
                if hw > 0.0 {
                    if clip.visible_start == clip.clip_start {
                        let left_rect = Bounds::new(
                            clip.bounds.origin,
                            size(px(hw), px(edge_trim_h).min(clip.bounds.size.height)),
                        );
                        let sel = if clip.is_midi {
                            format!("midi-clip-trim-left-{}", clip.id.value())
                        } else {
                            format!("clip-trim-left-{}", clip.id.value())
                        };
                        window.record_debug_bounds(sel, left_rect);
                    }
                    if clip.visible_end == clip.clip_end {
                        let right_rect = Bounds::new(
                            point(clip.bounds.right() - px(hw), clip.bounds.top()),
                            size(px(hw), px(edge_trim_h).min(clip.bounds.size.height)),
                        );
                        let sel = if clip.is_midi {
                            format!("midi-clip-trim-right-{}", clip.id.value())
                        } else {
                            format!("clip-trim-right-{}", clip.id.value())
                        };
                        window.record_debug_bounds(sel, right_rect);
                    }
                }
            }
            if !clip.is_midi
                && clip.pixel_width >= 28.0
                && track_height > super::super::track_layout::HEADER_ONLY_HEIGHT
                && !fade_key_held
                && !time_selection_key_held
            {
                let gain_rect = Bounds::new(
                    point(clip.bounds.left() + px(9.0), clip.bounds.top()),
                    size(
                        px((clip.pixel_width - 18.0).max(0.0)),
                        px(4.0).min(clip.bounds.size.height),
                    ),
                );
                window.record_debug_bounds(
                    format!("clip-gain-handle-{}", clip.id.value()),
                    gain_rect,
                );
            }
        }
    }
}
