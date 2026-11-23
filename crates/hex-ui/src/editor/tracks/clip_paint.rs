//! Audio/MIDI clip body painting, fade/grid layers, waveform source resolution and clip views.
use super::*;

pub(super) fn midi_note_body(
    clip: hex_project::ClipId,
    shapes: impl IntoIterator<Item = (f32, f32, u8)>,
    fit: crate::editor::midi_thumbnail::PitchFit,
    dragging: bool,
) -> (Div, usize) {
    midi_note_canvas(
        move || {
            format!(
                "clip-{}notes-{}",
                if dragging { "drag-" } else { "" },
                clip.value()
            )
        },
        shapes,
        fit,
    )
}

/// Paint display-only note shapes in one canvas, so every visible note is
/// kept without asking layout and interactivity to traverse one Div per
/// rectangle. `selector` names the note layer for rendered inspection.
pub(in crate::editor) fn midi_note_canvas(
    selector: impl Fn() -> String + 'static,
    shapes: impl IntoIterator<Item = (f32, f32, u8)>,
    fit: crate::editor::midi_thumbnail::PitchFit,
) -> (Div, usize) {
    let shapes: Vec<_> = shapes.into_iter().collect();
    let count = shapes.len();
    if count == 0 {
        // Empty clips have no rectangles to submit.
        return (div().absolute().inset_0(), 0);
    }
    let notes = div().absolute().inset_0().child(
        div().debug_selector(selector).absolute().inset_0().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    for &(x, width, pitch) in &shapes {
                        if let Some(note) = crate::editor::midi_thumbnail::paint_bounds(
                            bounds,
                            x,
                            width,
                            pitch,
                            fit,
                            window.scale_factor(),
                        ) {
                            window.paint_quad(gpui::fill(note, rgb(0x000000)));
                        }
                    }
                },
            )
            .absolute()
            .inset_0(),
        ),
    );
    (notes, count)
}

pub(super) fn clip_fade_layer(
    clip: hex_project::ClipId,
    samples: Vec<(f32, f32)>,
    editing: bool,
    clip_active: bool,
) -> impl IntoElement + use<> {
    div()
        .debug_selector(move || format!("clip-fade-shade-{}", clip.value()))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let left = f32::from(bounds.origin.x);
                    let top = f32::from(bounds.origin.y);
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let mut shade = PathBuilder::fill();
                    if let Some((x, _)) = samples.first() {
                        shade.move_to(point(px(left + width * x), px(top)));
                    }
                    let mut path = editing.then(|| PathBuilder::stroke(px(1.25)));
                    for (index, (x, gain)) in samples.iter().copied().enumerate() {
                        let point = point(px(left + width * x), px(top + height * (1.0 - gain)));
                        if let Some(path) = path.as_mut() {
                            if index == 0 {
                                path.move_to(point);
                            } else {
                                path.line_to(point);
                            }
                        }
                        shade.line_to(point);
                    }
                    if let Some((x, _)) = samples.last() {
                        shade.line_to(point(px(left + width * x), px(top)));
                    }
                    shade.close();
                    if let Ok(shade) = shade.build() {
                        let ink = gpui::Hsla::from(rgba(theme::CLIP_FADE_SHADE))
                            .opacity(theme::clip_detail_opacity(clip_active));
                        window.paint_path(shade, ink);
                    }
                    if let Some(path) = path
                        && let Ok(path) = path.build()
                    {
                        window.paint_path(path, rgba(0x000000e0));
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .absolute()
        .top(px(crate::editor::clip_name::CLIP_NAME_HEIGHT))
        .left_0()
        .right_0()
        .bottom_0()
}

pub(super) enum ClipPaintSource<'a> {
    Audio(&'a Clip),
    Midi(&'a MidiClip),
}

pub(in crate::editor) fn midi_trim_note_shapes(
    project: &hex_project::Project,
    clip: &MidiClip,
    trim: impl Into<crate::editor::trim::MidiEdgePreview>,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Vec<(f32, f32, u8)> {
    let trim = trim.into();
    let Ok(playback) = trim.source_playback(clip) else {
        return Vec::new();
    };
    let plans = clip.notes().iter().filter_map(|note| {
        let (start, duration) = trim.note_interval(note)?;
        playback
            .note_occurrences(start, duration)
            .ok()
            .map(|plan| (plan, note.pitch()))
    });
    crate::editor::midi_loop_notes::visible_occurrences(
        project.tempo_map(),
        trim.start(),
        plans,
        visible_start,
        visible_end,
        merge_frames,
        false,
    )
    .unwrap_or_default()
    .into_iter()
    .filter_map(|(start, end, pitch)| {
        crate::editor::midi_thumbnail::note_shape(start, end, pitch, visible_start, visible_end)
    })
    .collect()
}

pub(in crate::editor) fn clip_grid_layer(
    lines: &[crate::editor::musical_time::GridLine],
    start: u64,
    end: u64,
) -> Option<Div> {
    clip_grid_layer_impl(lines, start, end, None)
}

/// Place the grid against the outer clip interval rather than its inset
/// content box. This shares the lane's physical-pixel columns.
pub(in crate::editor) fn aligned_clip_grid_layer(
    lines: &[crate::editor::musical_time::GridLine],
    start: u64,
    end: u64,
    stroke: f32,
    has_border: bool,
) -> Option<Div> {
    clip_grid_layer_impl(lines, start, end, Some(stroke)).map(|grid| {
        // The title content is transparent on a shared backdrop. Restrict
        // timeline strokes to the body instead of relying on an opaque title.
        grid.top(px(crate::editor::clip_name::CLIP_NAME_HEIGHT))
            .left(px(if has_border { -stroke } else { 0.0 }))
            .right_0()
    })
}

pub(super) fn clip_grid_layer_impl(
    lines: &[crate::editor::musical_time::GridLine],
    start: u64,
    end: u64,
    stroke: Option<f32>,
) -> Option<Div> {
    if start >= end {
        return None;
    }
    let width = end - start;
    let children = lines
        .iter()
        .filter_map(|line| {
            let offset = line.frame.checked_sub(start)?;
            if offset == 0 || offset >= width {
                return None;
            }
            Some(
                div()
                    .debug_selector(|| format!("clip-grid-line-{}", line.frame))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative((offset as f64 / width as f64) as f32))
                    .w(px(stroke.unwrap_or(if line.meter_marker {
                        1.5
                    } else {
                        1.0
                    })))
                    .bg(rgb(if line.meter_marker {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::Grid)
                    }))
                    .opacity(if line.meter_marker || line.bar_start {
                        0.42
                    } else {
                        0.24
                    }),
            )
        })
        .collect::<Vec<_>>();
    (!children.is_empty()).then(|| {
        div()
            .debug_selector(|| "clip-musical-grid".to_owned())
            .absolute()
            .inset_0()
            .children(children)
    })
}

impl EditorView {
    pub(in crate::editor) fn build_clip_contents(
        &self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        #[cfg(all(test, feature = "ui-test-support"))]
        let _navigation_timer = crate::editor::navigation_metrics::Timer::start(
            crate::editor::navigation_metrics::Stage::ClipModel,
        );
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
            caption_rem_size,
        } = timeline;
        let Some(viewport_end) = viewport.end() else {
            return div().size_full();
        };
        let track_id = track.id();
        let mut clip_area = div().size_full().relative();
        let mut fade_foreground = Vec::new();
        let mut selected_fade_foreground = Vec::new();
        let mut selected_edges = Vec::new();
        let mut bars = narrow_clips::Bars::default();
        let move_display = self
            .time_range_move_display()
            .filter(|_| self.time_range_copy_view().is_none());
        let exclusions = move_exclusions(move_display.as_ref(), track_id, track_index);
        let group_owner = self
            .displayed_clip_drag()
            .filter(|d| !d.copy_modifier.get());
        let group = group_owner.map(|d| d.group.borrow());
        let hides_group_member = |id| {
            group.as_ref().is_some_and(|g| {
                ((g.ids.len() > 1 && !g.previews().is_empty()) || self.pending_clip_move.is_some())
                    && g.ids.binary_search(&id).is_ok()
            })
        };
        // Section focus shows every clip in each visible row. The canonical
        // ID cache cannot yet contain new IDs from a committed copy or paste.
        let is_committed_edit_view = self
            .arrangement_duplicate_view()
            .or_else(|| self.committed_section_copy_view())
            .or_else(|| self.arrangement_paste_view())
            .or_else(|| self.time_range_copy_view())
            .is_some_and(|view| std::ptr::eq(view.as_ref(), state));

        if let Some(snapshot) = self
            .state
            .as_ref()
            .filter(|snapshot| std::ptr::eq(snapshot.as_ref(), state))
            .or_else(|| {
                self.arrangement_duplicate_view()
                    .or_else(|| self.committed_section_copy_view())
                    .or_else(|| self.arrangement_paste_view())
                    .or_else(|| self.time_range_copy_view())
                    .filter(|snapshot| std::ptr::eq(snapshot.as_ref(), state))
            })
            .cloned()
        {
            self.arrangement_clip_index
                .borrow_mut()
                .ensure_current(self.project_generation, &snapshot);

            let has_potential_overlays = exclusions.iter().any(Option::is_some)
                || group.is_some()
                || self.clip_name_edit.is_some()
                || self.pending_trim.is_some()
                || self.active_trim_drag.is_some()
                || self.active_midi_trim_drag.is_some()
                || self.fade_key_held
                || self.hovered_fade_clip.is_some()
                || self.hovered_fade_handle.is_some()
                || self.active_fade_drag.is_some()
                || self.pending_fade.is_some()
                || self.clip_gain.has_displayed()
                || self.audio_clip_controls.gain_preview(self).is_some()
                || self.audio_clip_controls.has_play_start_preview()
                || self.shows_audio_clip_loop_preview()
                || self.shows_midi_clip_loop_preview()
                || self.audio_event_editor.gesture.is_some()
                || self.held_arrangement_event_clip().is_some();

            if !has_potential_overlays {
                let canvas = crate::editor::track_lane_canvas::TrackLaneCanvas::new(
                    cx.entity(),
                    snapshot,
                    track_id,
                    track_index,
                    viewport,
                    std::rc::Rc::clone(grid),
                    std::rc::Rc::from([]),
                    is_committed_edit_view,
                );
                return self.wrap_track_lane_canvas(
                    track_id,
                    viewport,
                    scale_factor,
                    canvas,
                    Vec::new(),
                    cx,
                );
            }

            let mut excluded: Vec<hex_project::ClipId> = Vec::new();
            let mut overlays: Vec<gpui::AnyElement> = Vec::new();
            let mut overlay_fade_foreground = Vec::new();
            let mut overlay_selected_fade_foreground = Vec::new();
            let mut overlay_selected_edges = Vec::new();
            let mut overlay_bars = narrow_clips::Bars::default();

            let visible_entries: Vec<_> = self
                .arrangement_clip_index
                .borrow()
                .track(track_id)
                .map(|idx| idx.candidate_slice(viewport.start, viewport_end).to_vec())
                .unwrap_or_default();

            for entry in visible_entries {
                if entry.end <= viewport.start || entry.start >= viewport_end {
                    continue;
                }
                if hides_group_member(entry.id) {
                    excluded.push(entry.id);
                    continue;
                }
                if !is_committed_edit_view && !self.arrangement_clip_visible(entry.id) {
                    continue;
                }
                match entry.kind {
                    crate::editor::arrangement_clip_index::IndexedClipKind::Audio(i) => {
                        let Some(canonical_clip) = track.clips().get(i) else {
                            continue;
                        };
                        let scope_preview = self.audio_scope_display_clip(canonical_clip, state);
                        let clip = scope_preview.as_ref();
                        let has_scope = !std::ptr::eq(clip, canonical_clip);
                        let has_trim = self
                            .audio_trim_display(track_id, Some(clip.id()))
                            .is_some_and(|(orig, _)| orig.id() == clip.id());
                        let has_exclusions = clip_intersects_exclusions(clip, exclusions);
                        let has_rename = self.clip_name_is_editing(clip.id());
                        let has_fade_overlay = self.fade_key_held
                            || self.fade_curve_active(clip.id())
                            || self.hovered_fade_clip == Some((self.project_generation, clip.id()))
                            || self
                                .hovered_fade_handle
                                .and_then(|h| h.in_track(self.project_generation, track_id))
                                .is_some_and(|(id, _)| id == clip.id())
                            || self.fade_preview(clip.id()).is_some();
                        let has_gain_overlay =
                            self.displayed_clip_gain(clip.id()).is_some() || self.clip_gain.held();

                        if !(has_scope
                            || has_trim
                            || has_exclusions
                            || has_rename
                            || has_fade_overlay
                            || has_gain_overlay)
                        {
                            continue;
                        }
                        excluded.push(clip.id());
                        if has_exclusions {
                            for fragment in
                                clip_fragments_outside(clip, exclusions, state.project.tempo_map())
                                    .into_iter()
                                    .flatten()
                            {
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
                                        "time-move-audio-residual-{}-{}",
                                        track_id.value(),
                                        fragment.start()
                                    ),
                                    self.track_body_height(track_id)
                                        <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                                ) {
                                    overlays.push(fragment_view.into_any_element());
                                }
                            }
                            continue;
                        }
                        let selected_clip = self.clip_is_selected(clip.id());
                        let (body, foreground, notch) = self.clip_view(
                            state,
                            track_id,
                            clip,
                            ClipViewTimeline {
                                viewport,
                                grid,
                                scale_factor,
                                caption_rem_size,
                            },
                            &mut overlay_bars,
                            cx,
                        );
                        overlays.push(body);
                        if selected_clip
                            && let Some(notch) = notch
                            && let Some(end) = clip.start().checked_add(clip.duration())
                        {
                            let end = end.min(viewport_end);
                            let id = clip.id();
                            let active = self.displayed_clip_active(id, clip.active());
                            let outline =
                                theme::clip_activity_outline(active, theme::PaletteRole::Primary);
                            overlay_selected_edges.push(
                                crate::editor::clip_boundary::trailing_edge(
                                    notch, outline, outline,
                                )
                                .debug_selector(move || {
                                    format!("clip-selected-trailing-{}", id.value())
                                })
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(relative(
                                    (end - viewport.start) as f32 / viewport.length as f32,
                                ))
                                .w(px(1.0 / scale_factor.max(1.0)))
                                .into_any_element(),
                            );
                        }
                        if let Some(foreground) = foreground {
                            if self.selected_clip == Some(clip.id())
                                || self.fade_curve_active(clip.id())
                            {
                                overlay_selected_fade_foreground
                                    .push(foreground.into_any_element());
                            } else {
                                overlay_fade_foreground.push(foreground.into_any_element());
                            }
                        }
                    }
                    crate::editor::arrangement_clip_index::IndexedClipKind::Midi(i) => {
                        let Some(canonical_clip) = track.midi_clips().get(i) else {
                            continue;
                        };
                        let scope_preview = self.midi_scope_display_clip(canonical_clip, state);
                        let clip = scope_preview.as_ref();
                        let has_scope = !std::ptr::eq(clip, canonical_clip);
                        let has_trim = self
                            .midi_trim_display(track_id, Some(clip.id()))
                            .is_some_and(|(orig, _)| orig.id() == clip.id());
                        let has_exclusions = exclusions
                            .into_iter()
                            .flatten()
                            .any(|(es, ee)| es < entry.end && ee > entry.start);
                        let has_rename = self.clip_name_is_editing(clip.id());
                        if !(has_scope || has_trim || has_exclusions || has_rename) {
                            continue;
                        }
                        excluded.push(clip.id());
                        if has_exclusions {
                            let active = self.displayed_clip_active(clip.id(), clip.active());
                            for (fragment_index, interval) in
                                time_fragments_outside(entry.start, entry.end, exclusions)
                                    .into_iter()
                                    .enumerate()
                            {
                                let Some(interval) = interval else {
                                    continue;
                                };
                                if let Some(fragment) = passive_midi_fragment(
                                    self.drag_lanes
                                        .get(&track_id)
                                        .map_or(0.0, |lane| lane.width),
                                    scale_factor,
                                    theme::clip_activity_fill(
                                        track.color(),
                                        clip.flavor(),
                                        false,
                                        active,
                                    ),
                                    clip.id(),
                                    clip.name(),
                                    active,
                                    |start, end, merge| {
                                        self.loop_note_shapes(
                                            &state.project,
                                            clip,
                                            start,
                                            end,
                                            merge,
                                        )
                                    },
                                    interval,
                                    interval,
                                    viewport,
                                    viewport_end,
                                    grid,
                                    format!("time-move-midi-residual-{fragment_index}"),
                                    self.track_body_height(track_id)
                                        <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                                ) {
                                    overlays.push(fragment.into_any_element());
                                }
                            }
                            continue;
                        }
                        // For active MIDI rename, scope or trim preview, mount just that one
                        // clip's live header/shell overlay.
                        if has_rename {
                            let vis_start = entry.start.max(viewport.start);
                            let vis_end = entry.end.min(viewport_end);
                            let left = vis_start.saturating_sub(viewport.start) as f64
                                / viewport.length as f64;
                            let width =
                                vis_end.saturating_sub(vis_start) as f64 / viewport.length as f64;
                            let active = self.displayed_clip_active(clip.id(), clip.active());
                            let header_fill = theme::clip_activity_fill(
                                track.color(),
                                clip.flavor(),
                                false,
                                active,
                            );
                            overlays.push(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(relative(left as f32))
                                    .w(relative(width as f32))
                                    .child(self.clip_name_header(
                                        track_id,
                                        clip.id(),
                                        clip.name(),
                                        header_fill,
                                        cx,
                                    ))
                                    .into_any_element(),
                            );
                        } else if has_scope {
                            let tempo = state.project.tempo_map();
                            if let (Ok(start), Some(end)) = (
                                tempo.beat_to_frame(clip.start()),
                                clip.start()
                                    .raw()
                                    .checked_add(clip.length().raw())
                                    .and_then(|raw| {
                                        tempo.beat_to_frame(BeatTime::from_raw(raw)).ok()
                                    }),
                            ) && end > start
                            {
                                let active = self.displayed_clip_active(clip.id(), clip.active());
                                let selected_clip = self.clip_is_selected(clip.id());
                                if let Some(fragment) = passive_midi_fragment(
                                    self.drag_lanes
                                        .get(&track_id)
                                        .map_or(0.0, |lane| lane.width),
                                    scale_factor,
                                    theme::clip_activity_fill(
                                        track.color(),
                                        clip.flavor(),
                                        selected_clip,
                                        active,
                                    ),
                                    clip.id(),
                                    clip.name(),
                                    active,
                                    |start, end, merge| {
                                        self.loop_note_shapes(
                                            &state.project,
                                            clip,
                                            start,
                                            end,
                                            merge,
                                        )
                                    },
                                    (start, end),
                                    (start, end),
                                    viewport,
                                    viewport_end,
                                    grid,
                                    "midi-clip-visual".into(),
                                    self.track_body_height(track_id)
                                        <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                                ) {
                                    overlays.push(fragment.into_any_element());
                                }
                            }
                        } else if has_trim {
                            let vis_start = entry.start.max(viewport.start);
                            let vis_end = entry.end.min(viewport_end);
                            let left = vis_start.saturating_sub(viewport.start) as f64
                                / viewport.length as f64;
                            let width =
                                vis_end.saturating_sub(vis_start) as f64 / viewport.length as f64;
                            let clip_id = clip.id();
                            overlays.push(
                                div()
                                    .id(("midi-trim-source-anchor", clip_id.value()))
                                    .debug_selector(move || {
                                        format!("midi-clip-visual-{}", clip_id.value())
                                    })
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(relative(left as f32))
                                    .w(relative(width as f32))
                                    .into_any_element(),
                            );
                        }
                    }
                }
            }

            if let Some(bars) = overlay_bars.view(track_id) {
                overlays.insert(0, bars.into_any_element());
            }
            overlays.extend(overlay_fade_foreground);
            overlays.extend(overlay_selected_fade_foreground);
            overlays.extend(overlay_selected_edges);

            let canvas = crate::editor::track_lane_canvas::TrackLaneCanvas::new(
                cx.entity(),
                snapshot,
                track_id,
                track_index,
                viewport,
                std::rc::Rc::clone(grid),
                std::rc::Rc::from(excluded),
                is_committed_edit_view,
            );
            return self.wrap_track_lane_canvas(
                track_id,
                viewport,
                scale_factor,
                canvas,
                overlays,
                cx,
            );
        }
        for canonical_clip in track.clips() {
            let scope_preview = self.audio_scope_display_clip(canonical_clip, state);
            let clip = scope_preview.as_ref();
            if hides_group_member(clip.id())
                || (!is_committed_edit_view
                    && std::ptr::eq(clip, canonical_clip)
                    && !self.arrangement_clip_visible(clip.id()))
            {
                continue;
            }
            let Some(end) = clip.start().checked_add(clip.duration()) else {
                continue;
            };
            let display = self
                .pending_trim
                .as_ref()
                .and_then(|_| self.audio_trim_display(track_id, Some(clip.id())));
            let display_clip = display.as_ref().map_or(clip, |(_, preview)| preview);
            let Some(display_end) = display_clip.start().checked_add(display_clip.duration())
            else {
                continue;
            };
            if display_clip.start() >= viewport_end || display_end <= viewport.start {
                continue;
            }
            let selected_clip = self.clip_is_selected(clip.id());
            if clip_intersects_exclusions(clip, exclusions) {
                for fragment in clip_fragments_outside(clip, exclusions, state.project.tempo_map())
                    .into_iter()
                    .flatten()
                {
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
                            "time-move-audio-residual-{}-{}",
                            track_id.value(),
                            fragment.start()
                        ),
                        self.track_body_height(track_id)
                            <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                    ) {
                        clip_area = clip_area.children([fragment_view]);
                    }
                }
                continue;
            }
            let (body, foreground, notch) = self.clip_view(
                state,
                track_id,
                clip,
                ClipViewTimeline {
                    viewport,
                    grid,
                    scale_factor,
                    caption_rem_size,
                },
                &mut bars,
                cx,
            );
            clip_area = clip_area.child(body);
            if selected_clip && let Some(notch) = notch {
                let end = end.min(viewport_end);
                let id = clip.id();
                let active = self.displayed_clip_active(id, clip.active());
                let outline = theme::clip_activity_outline(active, theme::PaletteRole::Primary);
                selected_edges.push(
                    crate::editor::clip_boundary::trailing_edge(notch, outline, outline)
                        .debug_selector(move || format!("clip-selected-trailing-{}", id.value()))
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(
                            (end - viewport.start) as f32 / viewport.length as f32,
                        ))
                        .w(px(1.0 / scale_factor.max(1.0))),
                );
            }
            if let Some(foreground) = foreground {
                if self.selected_clip == Some(clip.id()) || self.fade_curve_active(clip.id()) {
                    selected_fade_foreground.push(foreground);
                } else {
                    fade_foreground.push(foreground);
                }
            }
        }
        for canonical_clip in track.midi_clips() {
            let scope_preview = self.midi_scope_display_clip(canonical_clip, state);
            let clip = scope_preview.as_ref();
            let has_scope = !std::ptr::eq(clip, canonical_clip);
            if hides_group_member(clip.id())
                || (!is_committed_edit_view && !self.arrangement_clip_visible(clip.id()))
            {
                continue;
            }
            let (start, end) = if has_scope {
                let tempo = state.project.tempo_map();
                let Ok(start) = tempo.beat_to_frame(clip.start()) else {
                    continue;
                };
                let Some(end) = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .and_then(|raw| tempo.beat_to_frame(BeatTime::from_raw(raw)).ok())
                else {
                    continue;
                };
                (start, end)
            } else {
                let Some(preview) = state.midi_preview.get(clip.id()) else {
                    continue;
                };
                (preview.frame_start, preview.frame_end)
            };
            let active = self.displayed_clip_active(clip.id(), clip.active());
            if end <= start {
                continue;
            }
            if start >= viewport_end || end <= viewport.start {
                continue;
            }
            if exclusions
                .into_iter()
                .flatten()
                .any(|(exclude_start, exclude_end)| exclude_start < end && exclude_end > start)
            {
                for (fragment_index, interval) in time_fragments_outside(start, end, exclusions)
                    .into_iter()
                    .enumerate()
                {
                    let Some(interval) = interval else {
                        continue;
                    };
                    let fragment = passive_midi_fragment(
                        self.drag_lanes
                            .get(&track_id)
                            .map_or(0.0, |lane| lane.width),
                        scale_factor,
                        theme::clip_activity_fill(track.color(), clip.flavor(), false, active),
                        clip.id(),
                        clip.name(),
                        active,
                        |start, end, merge| {
                            self.loop_note_shapes(&state.project, clip, start, end, merge)
                        },
                        interval,
                        interval,
                        viewport,
                        viewport_end,
                        grid,
                        format!("time-move-midi-residual-{fragment_index}"),
                        self.track_body_height(track_id)
                            <= crate::editor::track_layout::HEADER_ONLY_HEIGHT,
                    );
                    if let Some(fragment) = fragment {
                        clip_area = clip_area.children([fragment]);
                    }
                }
                continue;
            }
            let selected_clip = self.clip_is_selected(clip.id());
            let left = start.max(viewport.start).saturating_sub(viewport.start) as f64
                / viewport.length as f64;
            let width = end
                .min(viewport_end)
                .saturating_sub(start.max(viewport.start)) as f64
                / viewport.length as f64;
            let midi_clip_id = clip.id();
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport_end);
            let trim_preview = self
                .midi_trim_display(track_id, Some(clip.id()))
                .filter(|(original, _)| original.id() == clip.id())
                .map(|(_, preview)| preview);
            let lane_width = self
                .drag_lanes
                .get(&track_id)
                .map_or(0.0, |lane| lane.width);
            let pixel_width = crate::editor::trim::rendered_clip_width(lane_width, width);
            let trim_handles = self.midi_trim_handles(
                state,
                track_id,
                clip,
                start,
                end,
                visible_start,
                visible_end,
                pixel_width,
                viewport,
                scale_factor,
                cx,
            );
            let midi_drag_source =
                self.midi_drag_source(state, track_id, clip, start, end, viewport);
            let header_fill =
                theme::clip_activity_fill(track.color(), clip.flavor(), false, active);
            let fill =
                theme::arrangement_clip_activity_body_fill(header_fill, selected_clip, active);
            let color = theme::clip_activity_outline(
                active,
                if selected_clip {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::ClipOutline
                },
            );
            let detail_opacity = theme::clip_detail_opacity(active);
            let stroke = 1.0 / scale_factor.max(1.0);
            let narrow = trim_preview.is_none()
                && !self.clip_name_is_editing(midi_clip_id)
                && narrow_clips::is_narrow(pixel_width, scale_factor);
            if narrow {
                bars.push(left..left + width, fill, selected_clip);
            }
            let surface_width = if narrow { 0.0 } else { pixel_width };
            let border = hex_widgets::rectangle_border_width(surface_width) > 0.0;
            let notch = crate::editor::clip_boundary::Notch::new(border && visible_end == end);
            let mut midi = hex_widgets::rectangle_surface(left as f32, width as f32, surface_width)
                .id(("midi-clip", clip.id().value()))
                .debug_selector(|| format!("midi-clip-visual-{}", clip.id().value()))
                .cursor(gpui::CursorStyle::Arrow)
                .opacity(if trim_preview.is_some() { 0.0 } else { 1.0 })
                .left_0()
                .w_full()
                .border_l(px(if border { stroke } else { 0.0 }))
                .border_r(px(0.0))
                .bg(rgb(fill))
                .border_color(rgb(color))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        view.open_clip_context_menu(
                            event.position,
                            track_id,
                            midi_clip_id,
                            window,
                            cx,
                        );
                    }),
                );
            if border || narrow {
                midi = midi
                    .bg(gpui::transparent_black())
                    .border_color(gpui::transparent_black());
            }
            if !narrow
                && narrow_clips::shows_body_detail(lane_width, end - start, viewport.length)
                && self.track_body_height(track_id)
                    > crate::editor::track_layout::HEADER_ONLY_HEIGHT
            {
                midi = midi.children(
                    aligned_clip_grid_layer(
                        grid,
                        visible_start,
                        visible_end,
                        1.0 / scale_factor.max(1.0),
                        hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                    )
                    .map(|grid| grid.opacity(detail_opacity)),
                );
                // Every visible repeat's notes come from the playback plan,
                // unsampled; same-pitch notes within one device pixel merge.
                let merge_frames = crate::editor::midi_loop_notes::merge_frames(
                    visible_end.saturating_sub(visible_start),
                    pixel_width,
                    scale_factor,
                );
                let visible = match self.midi_visible_notes.try_borrow_mut() {
                    Ok(mut cache) => cache.get_tiled(
                        &state.project,
                        clip,
                        visible_start,
                        visible_end,
                        merge_frames,
                    ),
                    // Re-entrant build: compute without the cache.
                    Err(_) => crate::editor::midi_loop_notes::visible_notes_tiled(
                        &state.project,
                        clip,
                        visible_start,
                        visible_end,
                        merge_frames,
                    )
                    .ok()
                    .map(std::rc::Rc::new),
                };
                let fit = visible.as_ref().map_or_else(
                    || crate::editor::midi_thumbnail::PitchFit::from_pitches([]),
                    |visible| visible.fit,
                );
                let note_shapes = visible
                    .iter()
                    .flat_map(|visible| visible.intervals.iter())
                    .filter_map(|&(start, end, pitch)| {
                        crate::editor::midi_thumbnail::note_shape(
                            start,
                            end,
                            pitch,
                            visible_start,
                            visible_end,
                        )
                    });
                let (mut notes, _) = midi_note_body(midi_clip_id, note_shapes, fit, false);
                // Tileable repeats paint one rasterized period per repeat.
                if let Some(visible) = visible.filter(|visible| visible.tiles.is_some()) {
                    let images = std::rc::Rc::clone(&self.midi_tile_images);
                    let project = std::sync::Arc::clone(&state.project);
                    notes = notes.child(
                        div()
                            .debug_selector(move || {
                                format!("clip-note-tiles-{}", midi_clip_id.value())
                            })
                            .absolute()
                            .inset_0()
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        crate::editor::midi_loop_notes::paint_tiles(
                                            window,
                                            &images,
                                            &project,
                                            midi_clip_id,
                                            &visible,
                                            bounds,
                                            (visible_start, visible_end),
                                        );
                                    },
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    );
                }
                midi = midi.child(clip_waveform_body(notes.opacity(detail_opacity)));
            }
            let header = if narrow {
                self.narrow_clip_header(track_id, midi_clip_id, cx)
                    .into_any_element()
            } else if border {
                self.clip_name_header_on_backdrop(
                    track_id,
                    clip.id(),
                    clip.name(),
                    header_fill,
                    notch.enabled(),
                    caption_rem_size.is_some_and(|rem| {
                        crate::editor::clip_name::notched_label_is_clipped(
                            pixel_width,
                            rem,
                            scale_factor,
                        )
                    }),
                    cx,
                )
                .into_any_element()
            } else {
                self.clip_name_header(track_id, clip.id(), clip.name(), header_fill, cx)
                    .into_any_element()
            };
            midi = midi.child(header);
            midi = midi.when(!narrow && clip.looping(), |midi| {
                midi.child(
                    loop_handles::midi_markers(
                        clip,
                        None,
                        &state.project,
                        visible_start,
                        visible_end,
                    )
                    .opacity(detail_opacity),
                )
            });
            if let Some((payload, lane)) = midi_drag_source {
                let left_border = if border { stroke } else { 0.0 };
                midi = midi.child(clip_drag_span(&payload, left_border));
                let move_surface = self
                    .clip_drag_surface(
                        payload,
                        lane,
                        visible_start,
                        visible_end,
                        start,
                        Some(clip.start()),
                        viewport,
                        clip.id(),
                        cx,
                    )
                    .left(px(-left_border
                        + if !trim_handles.is_empty() && visible_start == start {
                            crate::editor::trim::handle_width(pixel_width)
                        } else {
                            0.0
                        }))
                    .right(px(if !trim_handles.is_empty() && visible_end == end {
                        crate::editor::trim::handle_width(pixel_width)
                    } else {
                        0.0
                    }));
                midi = midi.children([move_surface]).children(trim_handles);
            }
            let framed_clip = if narrow {
                midi.left(relative(left as f32))
                    .w(relative(width as f32))
                    .into_any_element()
            } else if border {
                crate::editor::clip_boundary::backdrop_shell(
                    midi,
                    left as f32,
                    width as f32,
                    crate::editor::clip_boundary::Backdrop {
                        body: fill,
                        header: header_fill,
                        outline: color,
                        hover_outline: color,
                        stroke,
                        header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient: if trim_preview.is_some() {
                            None
                        } else if let Some(values) =
                            self.midi_header_loop_values(clip.id(), state.revision)
                        {
                            crate::editor::clip_header_loop::midi_source(
                                clip,
                                values,
                                state.project.tempo_map(),
                                visible_start..visible_end,
                                pixel_width * scale_factor,
                            )
                        } else {
                            crate::editor::clip_header_loop::midi(
                                clip,
                                None,
                                state.project.tempo_map(),
                                visible_start..visible_end,
                                pixel_width * scale_factor,
                            )
                        },
                        notch: notch.clone(),
                    },
                    trim_preview.is_none(),
                    move || format!("midi-clip-trailing-{}", midi_clip_id.value()),
                )
                .into_any_element()
            } else {
                clip_boundary_shell(
                    midi,
                    left as f32,
                    width as f32,
                    stroke,
                    color,
                    color,
                    trim_preview.is_none(),
                    false,
                    0.0,
                    move || format!("midi-clip-trailing-{}", midi_clip_id.value()),
                )
                .into_any_element()
            };
            clip_area = clip_area.child(framed_clip);
            if selected_clip && !narrow && trim_preview.is_none() {
                let id = clip.id();
                selected_edges.push(
                    crate::editor::clip_boundary::trailing_edge(notch, color, color)
                        .debug_selector(move || {
                            format!("midi-clip-selected-trailing-{}", id.value())
                        })
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(
                            (visible_end - viewport.start) as f32 / viewport.length as f32,
                        ))
                        .w(px(stroke)),
                );
            }
        }
        let clips = clip_area
            .children(fade_foreground)
            .children(selected_fade_foreground)
            .children(selected_edges);
        if let Some(bars) = bars.view(track_id) {
            div().size_full().relative().child(bars).child(clips)
        } else {
            clips
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn clip_drag_content(
        &self,
        state: &ViewState,
        source: ClipPaintSource<'_>,
        destination: (u64, u64),
        visible: (u64, u64),
        fill: u32,
        pixel_width: f32,
        scale: f32,
        header_only: bool,
        on_backdrop: bool,
    ) -> Option<Div> {
        let (id, name, source_range) = match source {
            ClipPaintSource::Audio(clip) => (
                clip.id(),
                clip.name(),
                (clip.start(), clip.start().checked_add(clip.duration())?),
            ),
            ClipPaintSource::Midi(clip) => {
                let preview = state.midi_preview.get(clip.id())?;
                (
                    clip.id(),
                    clip.name(),
                    (preview.frame_start, preview.frame_end),
                )
            }
        };
        let (start, end) = clone_source_window(source_range, destination, visible)?;
        let mut content = div().absolute().inset_0().overflow_hidden();
        if header_only {
            return Some(content.child(self.clip_name_drag_header(id, name, fill, on_backdrop)));
        }
        match source {
            ClipPaintSource::Audio(clip) => {
                content = content.child(clip_waveform_body(self.audio_clip_body(
                    state,
                    clip,
                    start,
                    end,
                    None,
                    theme::waveform_ink_for_fill(fill),
                )));
                let fade = if clip.audio_edit().is_some() {
                    crate::editor::waveform::projected_fade_envelope_samples(
                        clip,
                        state.project.tempo_map(),
                        start,
                        end,
                        None,
                    )
                    .unwrap_or_default()
                } else {
                    fade_envelope_samples(clip.retained_fade(), clip.start(), start, end)
                };
                if !clip.retained_fade().is_neutral() && fade.len() >= 2 {
                    content = content.child(clip_fade_layer(
                        clip.id(),
                        fade,
                        self.fade_curve_active(clip.id()),
                        clip.active(),
                    ));
                }
            }
            ClipPaintSource::Midi(clip) => {
                // A group member is a projected copy; its notes are drawn
                // from the canonical clip it was captured from.
                let canonical = state.project.midi_clip(clip.id())?;
                let (shapes, fit) = self.loop_note_shapes(
                    &state.project,
                    canonical,
                    start,
                    end,
                    crate::editor::midi_loop_notes::merge_frames(end - start, pixel_width, scale),
                );
                let (notes, _) = midi_note_body(id, shapes, fit, true);
                content = content.child(clip_waveform_body(
                    notes.opacity(theme::clip_detail_opacity(canonical.active())),
                ));
            }
        }
        Some(content.child(self.clip_name_drag_header(id, name, fill, on_backdrop)))
    }

    pub(in crate::editor) fn resolve_audio_waveform_source(
        &self,
        state: &ViewState,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
    ) -> Option<(
        std::sync::Arc<hex_session::CachedWaveform>,
        crate::editor::waveform_geometry::WaveformMapping,
    )> {
        use crate::editor::waveform_geometry::{WaveformMapping, WaveformWindow};

        let waveform = state.waveforms.get(&clip.asset());
        let gain = self
            .waveform_clip_gain(clip)
            .filter(|gain| *gain != clip.gain());
        let fade = self.fade_preview(clip.id());
        let trim = self.fade_trim_preview(clip.id());
        let prepared = state.audio_clip_waveforms.get(&clip.id());
        let fallback = if let Some(prepared) = prepared {
            let root_offset = trim
                .and_then(Clip::audio_edit)
                .and_then(|edit| edit.projected_domain(state.project.tempo_map()).ok())
                .map_or(prepared.offset, |(_, _, offset, _)| offset);
            Some((
                std::sync::Arc::clone(&prepared.waveform),
                WaveformMapping::Prepared {
                    timeline_origin: clip.start(),
                    root_offset,
                    gain: gain.unwrap_or(clip.gain()),
                },
            ))
        } else {
            waveform
                .filter(|_| clip.audio_edit().is_none())
                .map(|waveform| {
                    (
                        std::sync::Arc::clone(waveform),
                        WaveformMapping::Clip {
                            clip: clip.clone(),
                            timeline_origin: clip.start(),
                            gain: gain.map(hex_project::ClipGain::linear),
                            fade: Some(fade.unwrap_or(clip.retained_fade())),
                        },
                    )
                })
        };
        let events = self.arrangement_event_amplitude(clip);
        let event_source = prepared.and_then(|prepared| prepared.event_source.as_ref());
        let musical_source = if event_source.is_some_and(|source| source.musical_clock.is_some()) {
            crate::editor::waveform_musical_event::Projection::for_display(
                &state.project,
                clip,
                prepared,
                events.clone(),
                gain,
                fade,
            )
        } else {
            None
        };
        let source = if musical_source.is_none() {
            crate::editor::waveform_frame::Projection::for_display(
                &state.project,
                clip,
                waveform,
                prepared,
                fade,
                events,
                gain,
            )
        } else {
            None
        };
        if let Some((waveform, projection)) = musical_source {
            Some((
                waveform,
                WaveformMapping::MusicalEventFade {
                    projection,
                    clock: crate::editor::waveform_musical_event::Clock::Arrangement,
                    fallback: fallback.map(|(waveform, mapping)| {
                        std::sync::Arc::new(WaveformWindow {
                            waveform,
                            mapping,
                            visible: visible_start..visible_end,
                        })
                    }),
                },
            ))
        } else {
            match source {
                Some((waveform, projection)) => Some((
                    waveform,
                    WaveformMapping::Frame {
                        projection,
                        clock: crate::editor::waveform_frame::Clock::Arrangement,
                        fallback: fallback.map(|(waveform, mapping)| {
                            std::sync::Arc::new(WaveformWindow {
                                waveform,
                                mapping,
                                visible: visible_start..visible_end,
                            })
                        }),
                    },
                )),
                None => prepared
                    .and_then(|prepared| {
                        if let Some((waveform, view)) =
                            crate::editor::waveform_prepared_fade::PreparedMusicalFade::for_clip(
                                &state.project,
                                clip,
                                prepared,
                                fade,
                                gain.unwrap_or(clip.gain()),
                            )
                        {
                            return Some((waveform, WaveformMapping::PreparedMusicalFade(view)));
                        }
                        let view = crate::editor::waveform_prepared_fade::PreparedFade::for_clip(
                            &state.project,
                            clip,
                            prepared,
                            fade,
                            gain.unwrap_or(clip.gain()),
                        )?;
                        Some((
                            std::sync::Arc::clone(&prepared.waveform),
                            WaveformMapping::PreparedFade(view),
                        ))
                    })
                    .or(fallback),
            }
        }
    }

    pub(in crate::editor) fn waveform_grid_for(
        &self,
        track: hex_project::TrackId,
        viewport: Viewport,
        scale: f32,
    ) -> Option<crate::editor::waveform::WaveformGrid> {
        self.waveform_grid(track, viewport, scale)
    }

    pub(super) fn audio_clip_body(
        &self,
        state: &ViewState,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
        grid: Option<crate::editor::waveform::WaveformGrid>,
        ink: u32,
    ) -> Div {
        if self.audio_scope_preview_active(clip.id(), state)
            && let Some(original) = state
                .project
                .clip(clip.id())
                .filter(|original| !std::ptr::eq(*original, clip))
        {
            return crate::editor::waveform::preview_waveform_view_cached(
                crate::editor::waveform::WaveformPreview {
                    original,
                    preview: clip,
                    tempo: state.project.tempo_map(),
                    asset: state.waveforms.get(&clip.asset()),
                    prepared: state.audio_clip_waveforms.get(&clip.id()),
                    ink,
                },
                visible_start,
                visible_end,
                std::rc::Rc::clone(&self.waveform_geometry_cache),
                grid,
            )
            .unwrap_or_else(|| div().absolute().inset_0());
        }
        use crate::editor::waveform_geometry::{WaveformMapping, WaveformWindow};

        let waveform = state.waveforms.get(&clip.asset());
        let gain = self
            .waveform_clip_gain(clip)
            .filter(|gain| *gain != clip.gain());
        let fade = self.fade_preview(clip.id());
        let trim = self.fade_trim_preview(clip.id());
        let prepared = state.audio_clip_waveforms.get(&clip.id());
        let fallback = if let Some(prepared) = prepared {
            // Cropping changes the visible offset in the immutable prepared root.
            let root_offset = trim
                .and_then(Clip::audio_edit)
                .and_then(|edit| edit.projected_domain(state.project.tempo_map()).ok())
                .map_or(prepared.offset, |(_, _, offset, _)| offset);
            Some((
                std::sync::Arc::clone(&prepared.waveform),
                WaveformMapping::Prepared {
                    timeline_origin: clip.start(),
                    root_offset,
                    gain: gain.unwrap_or(clip.gain()),
                },
            ))
        } else {
            waveform
                .filter(|_| clip.audio_edit().is_none())
                .map(|waveform| {
                    (
                        std::sync::Arc::clone(waveform),
                        WaveformMapping::Clip {
                            clip: clip.clone(),
                            timeline_origin: clip.start(),
                            gain: gain.map(hex_project::ClipGain::linear),
                            fade: Some(fade.unwrap_or(clip.retained_fade())),
                        },
                    )
                })
        };
        let events = self.arrangement_event_amplitude(clip);
        let event_source = prepared.and_then(|prepared| prepared.event_source.as_ref());
        let musical_source = if event_source.is_some_and(|source| source.musical_clock.is_some()) {
            crate::editor::waveform_musical_event::Projection::for_display(
                &state.project,
                clip,
                prepared,
                events.clone(),
                gain,
                fade,
            )
        } else {
            None
        };
        let source = if musical_source.is_none() {
            crate::editor::waveform_frame::Projection::for_display(
                &state.project,
                clip,
                waveform,
                prepared,
                fade,
                events,
                gain,
            )
        } else {
            None
        };
        let displayed = if let Some((waveform, projection)) = musical_source {
            Some((
                waveform,
                WaveformMapping::MusicalEventFade {
                    projection,
                    clock: crate::editor::waveform_musical_event::Clock::Arrangement,
                    fallback: fallback.map(|(waveform, mapping)| {
                        std::sync::Arc::new(WaveformWindow {
                            waveform,
                            mapping,
                            visible: visible_start..visible_end,
                        })
                    }),
                },
            ))
        } else {
            match source {
                Some((waveform, projection)) => Some((
                    waveform,
                    WaveformMapping::Frame {
                        projection,
                        clock: crate::editor::waveform_frame::Clock::Arrangement,
                        fallback: fallback.map(|(waveform, mapping)| {
                            std::sync::Arc::new(WaveformWindow {
                                waveform,
                                mapping,
                                visible: visible_start..visible_end,
                            })
                        }),
                    },
                )),
                None => prepared
                    .and_then(|prepared| {
                        if let Some((waveform, view)) =
                            crate::editor::waveform_prepared_fade::PreparedMusicalFade::for_clip(
                                &state.project,
                                clip,
                                prepared,
                                fade,
                                gain.unwrap_or(clip.gain()),
                            )
                        {
                            return Some((waveform, WaveformMapping::PreparedMusicalFade(view)));
                        }
                        let view = crate::editor::waveform_prepared_fade::PreparedFade::for_clip(
                            &state.project,
                            clip,
                            prepared,
                            fade,
                            gain.unwrap_or(clip.gain()),
                        )?;
                        Some((
                            std::sync::Arc::clone(&prepared.waveform),
                            WaveformMapping::PreparedFade(view),
                        ))
                    })
                    .or(fallback),
            }
        };
        let body = match displayed {
            Some((waveform, mapping)) => crate::editor::waveform::timeline_waveform_view_cached(
                waveform,
                mapping,
                visible_start,
                visible_end,
                std::rc::Rc::clone(&self.waveform_geometry_cache),
                grid,
                ink,
            ),
            None if clip.audio_edit().is_some() => div()
                .absolute()
                .inset_0()
                .text_xs()
                .child(theme::text("prepared waveform unavailable")),
            None => div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .px_1()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("missing media")),
        };
        crate::editor::waveform::clip_waveform_activity(
            body,
            self.displayed_clip_active(clip.id(), clip.active()),
        )
    }

    pub(super) fn waveform_grid(
        &self,
        track: hex_project::TrackId,
        viewport: Viewport,
        scale: f32,
    ) -> Option<crate::editor::waveform::WaveformGrid> {
        let lane = self.drag_lanes.get(&track)?;
        crate::editor::waveform::WaveformGrid::arrangement(
            viewport,
            lane.left,
            lane.width,
            scale,
            self.state.as_ref()?.project.sample_rate(),
        )
    }

    pub(super) fn clip_view(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &Clip,
        timeline: ClipViewTimeline<'_>,
        bars: &mut narrow_clips::Bars,
        cx: &mut Context<Self>,
    ) -> (
        gpui::AnyElement,
        Option<Div>,
        Option<crate::editor::clip_boundary::Notch>,
    ) {
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
            caption_rem_size,
        } = timeline;
        let clip = self.fade_trim_preview(clip.id()).unwrap_or(clip);
        let clip_id = clip.id();
        let selected = self.clip_is_selected(clip_id);
        let trim_preview_clip = self
            .audio_trim_display(track, Some(clip_id))
            .filter(|(original, _)| original.id() == clip_id)
            .map(|(_, preview)| preview);
        let active_trim_preview = trim_preview_clip.is_some();
        let Some(viewport_end) = viewport.end() else {
            return (
                div().id(("clip", clip_id.value())).into_any_element(),
                None,
                None,
            );
        };
        // Keep the interactive surface in the same place as the retained trim
        // preview while its canonical edit is still in flight.
        let display_clip = if self.pending_trim.is_some() {
            trim_preview_clip.as_ref().unwrap_or(clip)
        } else {
            clip
        };
        let Some(clip_end) = display_clip.start().checked_add(display_clip.duration()) else {
            return (
                div().id(("clip", clip_id.value())).into_any_element(),
                None,
                None,
            );
        };
        let visible_start = display_clip.start().max(viewport.start);
        let visible_end = clip_end.min(viewport_end);
        if visible_start >= visible_end || viewport.length == 0 {
            return (
                div().id(("clip", clip_id.value())).into_any_element(),
                None,
                None,
            );
        }
        let visible_frames = visible_end - visible_start;
        let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
        let width = visible_frames as f64 / viewport.length as f64;
        let drag_payload = self
            .drag_available()
            .then(|| self.drag_lanes.get(&track).copied())
            .flatten()
            .map(|lane| {
                (
                    ClipDragSource {
                        project_generation: self.project_generation,
                        revision: state.revision,
                        track,
                        clip: clip_id,
                        position: ClipPosition::Audio {
                            start: clip.start(),
                            duration: clip.duration(),
                        },
                        source_span: Default::default(),
                        viewport,
                        snap_grid: self.snap_grid,
                        ghost_waveform: if clip.audio_edit().is_none() {
                            state.waveforms.get(&clip.asset()).cloned()
                        } else {
                            None
                        },
                    },
                    lane,
                )
            });
        let identity = state
            .project
            .track(track)
            .map_or(hex_project::ElementColor::default(), |t| t.color());
        let active = self.displayed_clip_active(clip.id(), clip.active());
        let header_fill = theme::clip_activity_fill(identity, clip.flavor(), false, active);
        let waveform_ink = theme::clip_waveform_ink(
            identity,
            theme::ClipWaveformBackground::Arrangement {
                flavor: clip.flavor(),
                selected,
            },
            active,
        );
        let fill = theme::arrangement_clip_activity_body_fill(header_fill, selected, active);
        let color = theme::clip_activity_outline(
            active,
            if selected {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::ClipOutline
            },
        );
        let hover = theme::clip_activity_outline(
            active,
            if selected {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Text
            },
        );
        let detail_opacity = theme::clip_detail_opacity(active);
        let lane_width = self.drag_lanes.get(&track).map_or(0.0, |lane| lane.width);
        let clip_pixel_width = crate::editor::trim::rendered_clip_width(lane_width, width);
        let stroke = 1.0 / scale_factor.max(1.0);
        let narrow = !active_trim_preview
            && !self.fade_curve_active(clip_id)
            && !self.clip_name_is_editing(clip_id)
            && narrow_clips::is_narrow(clip_pixel_width, scale_factor);
        if narrow {
            bars.push(left..left + width, fill, selected);
        }
        let surface_width = if narrow { 0.0 } else { clip_pixel_width };
        let border = hex_widgets::rectangle_border_width(surface_width) > 0.0;
        let notch = crate::editor::clip_boundary::Notch::new(border && visible_end == clip_end);
        let mut content = hex_widgets::rectangle_surface(left as f32, width as f32, surface_width)
            .cursor(gpui::CursorStyle::Arrow)
            .opacity(if active_trim_preview { 0.0 } else { 1.0 })
            .left_0()
            .w_full()
            .border_l(px(if border { stroke } else { 0.0 }))
            .border_r(px(0.0))
            .bg(rgb(fill))
            .hover(move |style| {
                style.border_color(if border {
                    gpui::transparent_black()
                } else {
                    rgb(hover).into()
                })
            });
        content = content.border_color(rgb(color));
        if border || narrow {
            content = content
                .bg(gpui::transparent_black())
                .border_color(gpui::transparent_black());
        }
        let looping = loop_handles::audio_repeats(clip);
        if !narrow
            && !active_trim_preview
            && narrow_clips::shows_body_detail(lane_width, display_clip.duration(), viewport.length)
            && self.track_height(track) > crate::editor::track_layout::HEADER_ONLY_HEIGHT
        {
            content = content.children(
                aligned_clip_grid_layer(grid, visible_start, visible_end, stroke, border)
                    .map(|grid| grid.opacity(detail_opacity)),
            );
            content = content.child(clip_waveform_body(self.audio_clip_body(
                state,
                clip,
                visible_start,
                visible_end,
                self.waveform_grid(track, viewport, scale_factor),
                waveform_ink,
            )));
        }
        // Held/acknowledged fade editing paints after every clip body. Keep the
        // overlay in this lane so automation opacity and clipping still apply.
        let raise_fade = !narrow
            && !self.automation_view.enabled
            && (self.fade_key_held || self.fade_curve_active(clip_id));
        let mut foreground = div().absolute().inset_0();
        let fade_preview = self.fade_preview(clip.id());
        let fade = fade_preview.unwrap_or(clip.retained_fade());
        if !active_trim_preview
            && self.track_height(track) > crate::editor::track_layout::HEADER_ONLY_HEIGHT
            && clip_pixel_width > 0.0
            && !fade.is_neutral()
        {
            let samples = if clip.audio_edit().is_some() {
                crate::editor::waveform::projected_fade_envelope_samples(
                    clip,
                    state.project.tempo_map(),
                    visible_start,
                    visible_end,
                    fade_preview
                        .map(|fade| (fade, state.project.arrangement_fade_edit_uses_outer(clip))),
                )
                .unwrap_or_default()
            } else {
                fade_envelope_samples(fade, clip.start(), visible_start, visible_end)
            };
            if samples.len() >= 2 {
                let layer = div().absolute().inset_0().child(clip_fade_layer(
                    clip.id(),
                    samples,
                    self.fade_key_held || self.fade_curve_active(clip_id),
                    active,
                ));
                if raise_fade {
                    foreground = foreground.children([layer]);
                } else {
                    content = content.children([layer]);
                }
            }
        }
        let trim_handles = self.trim_handles(
            state,
            track,
            clip,
            visible_start,
            visible_end,
            clip_pixel_width,
            viewport,
            scale_factor,
            cx,
        );
        let hover_generation = self.project_generation;
        let header = if narrow {
            self.narrow_clip_header(track, clip_id, cx)
                .into_any_element()
        } else if border {
            self.clip_name_header_on_backdrop(
                track,
                clip.id(),
                clip.name(),
                header_fill,
                notch.enabled(),
                caption_rem_size.is_some_and(|rem| {
                    crate::editor::clip_name::notched_label_is_clipped(
                        clip_pixel_width,
                        rem,
                        scale_factor,
                    )
                }),
                cx,
            )
            .into_any_element()
        } else {
            self.clip_name_header(track, clip.id(), clip.name(), header_fill, cx)
                .into_any_element()
        };
        let mut content = content
            .child(header)
            .when(!narrow && !active_trim_preview && looping, |content| {
                content.child(
                    loop_handles::audio_markers(clip, &state.project, visible_start, visible_end)
                        .opacity(detail_opacity),
                )
            })
            .id(("clip", clip_id.value()))
            .debug_selector(|| format!("clip-visual-{}", clip_id.value()))
            .on_mouse_move(
                cx.listener(move |view, event: &gpui::MouseMoveEvent, _, cx| {
                    if view.update_fade_hover(
                        track,
                        clip_id,
                        hover_generation,
                        Some(f32::from(event.position.y)),
                    ) {
                        cx.notify();
                    }
                }),
            )
            .on_hover(cx.listener(move |view, hovered: &bool, window, cx| {
                let pointer_y = hovered.then(|| f32::from(window.mouse_position().y));
                if view.update_fade_hover(track, clip_id, hover_generation, pointer_y) {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_clip_context_menu(event.position, track, clip_id, window, cx);
                }),
            );
        if let Some((payload, lane)) = drag_payload {
            let left_border = if border { stroke } else { 0.0 };
            content = content.child(clip_drag_span(&payload, left_border));
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
                // Edge strips retain trimming beside the upper-third move header.
                .left(px(-left_border
                    + if !trim_handles.is_empty() && visible_start == clip.start() {
                        crate::editor::trim::handle_width(clip_pixel_width)
                    } else {
                        0.0
                    }))
                .right(px(if !trim_handles.is_empty() && visible_end == clip_end {
                    crate::editor::trim::handle_width(clip_pixel_width)
                } else {
                    0.0
                }));
            content = content.children([move_surface]).children(trim_handles);
        }
        if !narrow && !active_trim_preview {
            content = content.children(self.clip_gain_handle(
                state,
                track,
                clip,
                clip_pixel_width,
                viewport,
                cx,
            ));
        }
        if !narrow
            && !active_trim_preview
            && (self.track_height(track) > crate::editor::track_layout::HEADER_ONLY_HEIGHT
                || self.fade_curve_active(clip_id))
        {
            let handles = self.fade_handles(
                state,
                track,
                clip,
                (visible_start, visible_end),
                viewport,
                cx,
            );
            if raise_fade {
                foreground = foreground.children(handles);
            } else {
                content = content.children(handles);
            }
        }
        let foreground = raise_fade.then(|| {
            let border = hex_widgets::rectangle_border_width(clip_pixel_width);
            div()
                .absolute()
                .left(relative(left as f32))
                .w(relative(width as f32))
                .top_0()
                .bottom_0()
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        // Match the clip body's physical-pixel leading border
                        // and separately painted trailing edge.
                        .left(px(if border > 0.0 { stroke } else { 0.0 }))
                        .right_0()
                        .top(px(border))
                        .bottom(px(border))
                        .children([foreground]),
                )
        });
        (
            if narrow {
                content
                    .left(relative(left as f32))
                    .w(relative(width as f32))
                    .into_any_element()
            } else if border {
                crate::editor::clip_boundary::backdrop_shell(
                    content,
                    left as f32,
                    width as f32,
                    crate::editor::clip_boundary::Backdrop {
                        body: fill,
                        header: header_fill,
                        outline: color,
                        hover_outline: hover,
                        stroke,
                        header_height: crate::editor::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient: if !active_trim_preview {
                            crate::editor::clip_header_loop::audio(
                                clip,
                                state.project.tempo_map(),
                                visible_start..visible_end,
                                clip_pixel_width * scale_factor,
                            )
                        } else {
                            None
                        },
                        notch: notch.clone(),
                    },
                    !active_trim_preview,
                    move || format!("clip-trailing-{}", clip_id.value()),
                )
                .into_any_element()
            } else {
                clip_boundary_shell(
                    content,
                    left as f32,
                    width as f32,
                    stroke,
                    color,
                    hover,
                    !active_trim_preview,
                    false,
                    0.0,
                    move || format!("clip-trailing-{}", clip_id.value()),
                )
                .into_any_element()
            },
            foreground,
            (!narrow && !active_trim_preview).then_some(notch),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_trim_note_shapes_returns_empty_for_empty_clip() {
        let project = hex_project::Project::new(48_000).unwrap();
        let mut editor = hex_project::Editor::new(project, 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let hex_project::Created::Clip(clip_id) = editor
            .execute(hex_project::Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(0).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        let clip = editor.project().midi_clip(clip_id).unwrap();
        let trim = clip
            .preview_edge_trim(
                hex_project::TrimEdge::Right,
                BeatTime::from_quarters(4).unwrap(),
            )
            .unwrap();
        assert!(midi_trim_note_shapes(editor.project(), clip, trim, 0, 100, 1).is_empty());
    }
}
