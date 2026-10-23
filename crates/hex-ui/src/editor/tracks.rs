use super::drag::{self, ClipDrag, ClipPosition, LaneBounds};
use super::time_move_preview::{
    clip_fragments_outside, clip_intersection, clip_intersects_exclusions, move_exclusions,
    passive_audio_fragment, passive_midi_fragment, time_fragments_outside,
};
use super::trim::TrimDrag;
use super::viewport::Viewport;
use super::waveform::{
    ClipTooltip, button, button_with_style, fade_envelope_samples, waveform_source_segment_view,
    waveform_summary_view,
};
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb, rgba,
};
use hex_project::{BeatTime, Clip, MidiClip, MidiClipTrim, MidiSourcePlayback, Track};

struct ClipGhostGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

struct ClipGhostDecor {
    fill: u32,
    selector: &'static str,
    grid: Option<(Vec<super::musical_time::GridLine>, u64, u64)>,
    contents: Option<Div>,
}

/// Body-only clipping leaves the title and outer selection/rounded mask intact.
pub(super) fn clip_waveform_body(content: Div) -> Div {
    waveform_body_below(content, super::clip_name::CLIP_NAME_HEIGHT)
}
fn waveform_body_below(content: Div, header_height: f32) -> Div {
    div()
        .absolute()
        .top(px(header_height))
        .left_0()
        .right_0()
        .bottom_0()
        .min_h_0()
        .overflow_hidden()
        .child(content)
}

fn clip_drag_ghost(payload: &ClipDrag, geometry: ClipGhostGeometry, decor: ClipGhostDecor) -> Div {
    let ClipGhostGeometry {
        left,
        top,
        width,
        height,
    } = geometry;
    let ClipGhostDecor {
        fill,
        selector,
        grid,
        contents,
    } = decor;
    let mut ghost = div()
        .debug_selector(move || selector.to_owned())
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width.max(3.0)))
        .h(px(height.max(3.0)))
        .overflow_hidden()
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
        .bg(rgb(fill));
    if let Some((lines, start, end)) = grid {
        ghost = ghost.children([clip_grid_layer(&lines, start, end)]);
    }
    if let Some(content) = contents {
        return ghost.child(content);
    } else if let Some(waveform) = &payload.ghost_waveform {
        ghost = ghost.children([waveform_body_below(
            waveform_summary_view(std::sync::Arc::clone(waveform)),
            10.0,
        )]);
    } else {
        ghost = ghost.child(
            div()
                .absolute()
                .top(relative(0.5))
                .left(px(4.0))
                .right(px(4.0))
                .h(px(1.0))
                .bg(rgb(theme::color(theme::PaletteRole::ClipText))),
        );
    }
    ghost.child(
        div()
            .absolute()
            .left(px(4.0))
            .top_0()
            .right(px(3.0))
            .h(px(10.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::ClipText)))
            .line_clamp(1)
            .child(theme::text(payload.ghost_label.clone())),
    )
}

fn midi_note_body(
    clip: hex_project::ClipId,
    shapes: impl IntoIterator<Item = (f32, f32, u8)>,
    dragging: bool,
) -> (Div, usize) {
    let mut notes = div().absolute().inset_0();
    let mut count = 0;
    for (index, (x, width, pitch)) in shapes.into_iter().enumerate() {
        count += 1;
        notes = notes.child(
            div()
                .debug_selector(move || {
                    format!(
                        "{}-{}-{index}",
                        if dragging {
                            "clip-drag-note"
                        } else {
                            "clip-note"
                        },
                        clip.value()
                    )
                })
                .absolute()
                .left(relative(x))
                .top(relative(f32::from(127_u8.saturating_sub(pitch)) / 128.0))
                .w(relative(width))
                .h(px(2.0))
                .bg(rgb(0x000000)),
        );
    }
    (notes, count)
}

fn clip_fade_layer(samples: Vec<(f32, f32)>, active: bool) -> impl IntoElement + use<> {
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
            let mut path = active.then(|| PathBuilder::stroke(px(1.25)));
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
                window.paint_path(shade, rgba(0x00000038));
            }
            if let Some(path) = path
                && let Ok(path) = path.build()
            {
                window.paint_path(path, rgba(0x000000e0));
            }
        },
    )
    .absolute()
    .top(px(super::clip_name::CLIP_NAME_HEIGHT))
    .left_0()
    .right_0()
    .bottom_0()
}

/// Map only display coordinates into the known original crop. This is a visual
/// clone of accepted data, never a newly compiled destination-audio projection.
fn clone_source_window(
    source: (u64, u64),
    destination: (u64, u64),
    visible: (u64, u64),
) -> Option<(u64, u64)> {
    let source_length = source.1.checked_sub(source.0).filter(|n| *n > 0)?;
    let destination_length = destination
        .1
        .checked_sub(destination.0)
        .filter(|n| *n > 0)?;
    let left = visible.0.max(destination.0);
    let right = visible.1.min(destination.1);
    if left >= right {
        return None;
    }
    let start = u128::from(left.checked_sub(destination.0)?) * u128::from(source_length)
        / u128::from(destination_length);
    let end = (u128::from(right.checked_sub(destination.0)?) * u128::from(source_length))
        .div_ceil(u128::from(destination_length));
    Some((
        source.0.checked_add(u64::try_from(start).ok()?)?,
        source
            .0
            .checked_add(u64::try_from(end).ok()?)?
            .min(source.1),
    ))
}

enum ClipPaintSource<'a> {
    Audio(&'a Clip),
    Midi(&'a MidiClip),
}

pub(super) fn midi_trim_note_shapes(
    project: &hex_project::Project,
    clip: &MidiClip,
    trim: MidiClipTrim,
    visible_start: u64,
    visible_end: u64,
) -> Vec<(f32, f32, u8)> {
    let shifted = |value: BeatTime| {
        value
            .raw()
            .checked_add(trim.source_prefix().raw())
            .map(BeatTime::from_raw)
    };
    let Some(source_length) = shifted(clip.source_length()) else {
        return Vec::new();
    };
    let playback = MidiSourcePlayback::new(
        source_length,
        trim.source_start(),
        clip.looping()
            .then_some((clip.loop_start(), clip.loop_end())),
        trim.length(),
    );
    let Ok(playback) = playback else {
        return Vec::new();
    };
    let plans: Vec<_> = clip
        .notes()
        .iter()
        .take(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
        .filter_map(|note| {
            playback
                .note_occurrences(shifted(note.start())?, note.duration())
                .ok()
                .map(|plan| (note.pitch(), plan))
        })
        .collect();
    let mut shapes = Vec::with_capacity(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP);
    for ordinal in 0..hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP {
        for (pitch, plan) in &plans {
            let Some(occurrence) = plan.iter().nth(ordinal) else {
                continue;
            };
            let Some(start_raw) = trim
                .start()
                .raw()
                .checked_add(occurrence.timeline_start().raw())
            else {
                continue;
            };
            let Some(end_raw) = start_raw.checked_add(occurrence.gate_duration().raw()) else {
                continue;
            };
            let (Ok(start), Ok(end)) = (
                project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(start_raw)),
                project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(end_raw)),
            ) else {
                continue;
            };
            if let Some(shape) =
                super::midi_thumbnail::note_shape(start, end, *pitch, visible_start, visible_end)
            {
                shapes.push(shape);
                if shapes.len() >= hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP {
                    return shapes;
                }
            }
        }
    }
    shapes
}

pub(super) const TRACK_ROW_HEIGHT: f32 = 44.0;
pub(super) const TRACK_HEADER_WIDTH: f32 = 176.0;

/// Visible occupied spans under a landing preview. Merge overlapping victims so
/// the translucent accent has the same intensity everywhere.
fn drop_overlap_spans(
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

/// Paint-only scalar sweep: intersect unions without member × victim work.
/// Two events per visible member/victim, one sort; no capture/update storage.
fn group_overlap_spans(mut events: Vec<(u64, i32, i32)>) -> Vec<(u64, u64)> {
    events.sort_unstable();
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let (mut cuts, mut victims, mut previous) = (0, 0, 0);
    for (position, cut, victim) in events {
        if previous < position && cuts > 0 && victims > 0 {
            if let Some(last) = spans.last_mut()
                && last.1 == previous
            {
                last.1 = position;
            } else {
                spans.push((previous, position));
            }
        }
        cuts += cut;
        victims += victim;
        previous = position;
    }
    spans
}

pub(super) fn clip_grid_layer(
    lines: &[super::musical_time::GridLine],
    start: u64,
    end: u64,
) -> Div {
    if start >= end {
        return div().absolute().inset_0();
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
                    .w(px(if line.meter_marker { 1.5 } else { 1.0 }))
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
    div()
        .debug_selector(|| "clip-musical-grid".to_owned())
        .absolute()
        .inset_0()
        .children(children)
}

pub(super) struct ClipViewTimeline<'a> {
    pub(super) viewport: Viewport,
    pub(super) grid: &'a [super::musical_time::GridLine],
    pub(super) scale_factor: f32,
}

impl EditorView {
    pub(super) fn track_row(
        &mut self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        selected: bool,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.track_row_builds = self.track_row_builds.saturating_add(1);
        }
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
        } = timeline;
        let track_id = track.id();
        let Some(viewport_end) = viewport.end() else {
            return div().w_full().h(px(self.track_height(track_id)));
        };
        let mut clip_area = div()
            .id(("track-lane", track_id.value()))
            .debug_selector(|| format!("arrangement-lane-{}", track_id.value()))
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(self.track_height(track_id)))
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
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
                    .bg(
                        gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Secondary)))
                            .alpha(theme::TRACK_SELECTION_OPACITY),
                    ),
            );
        }
        let lane_entity = cx.entity();
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
        if track_selected {
            // Paint beneath clip content so every outer selection stroke stays
            // intact, including where vertical clip edges cross these accents.
            let edge = gpui::Hsla::from(rgb(theme::color(theme::PaletteRole::Secondary)))
                .alpha(theme::TRACK_SELECTION_EDGE_OPACITY);
            let stroke = 1.0 / scale_factor.max(1.0);
            clip_area = clip_area
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(1.0))
                        .h(px(stroke))
                        .debug_selector(move || {
                            format!("selected-track-lane-top-{}", track_id.value())
                        })
                        .bg(edge),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(1.0))
                        .h(px(stroke))
                        .debug_selector(move || {
                            format!("selected-track-lane-bottom-{}", track_id.value())
                        })
                        .bg(edge),
                );
        }
        clip_area = clip_area.child(content);
        let move_display = self.time_range_move_display();
        let mut move_preview_shape_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
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
                    let mut overlay = div()
                        .id(("midi-trim-live-preview", track_id.value()))
                        .debug_selector(|| "midi-trim-live-preview".to_owned())
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(left as f32))
                        .w(relative(width as f32))
                        .min_w(px(4.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(if self.clip_is_selected(clip.id()) {
                            theme::color(theme::PaletteRole::Primary)
                        } else {
                            0x000000
                        }))
                        .bg(rgb(theme::clip_fill(track.color(), clip.flavor(), false)))
                        .children([clip_grid_layer(grid, visible_start, visible_end)]);
                    let shapes = midi_trim_note_shapes(
                        &state.project,
                        &clip,
                        preview,
                        visible_start,
                        visible_end,
                    );
                    for (x, shape_width, pitch) in shapes {
                        let top = f32::from(127_u8.saturating_sub(pitch)) / 128.0;
                        overlay = overlay.child(
                            div()
                                .debug_selector(|| format!("midi-trim-preview-note-{pitch}"))
                                .absolute()
                                .left(relative(x))
                                .top(relative(top))
                                .w(relative(shape_width))
                                .h(px(2.0))
                                .bg(rgb(0x000000)),
                        );
                    }
                    clip_area = clip_area.children([overlay]);
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
                    let mut preview = div()
                        .id(("trim-live-preview", track_id.value()))
                        .debug_selector(|| "trim-live-preview".to_owned())
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(left as f32))
                        .w(relative(width as f32))
                        .min_w(px(3.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(if self.clip_is_selected(original_clip.id()) {
                            theme::color(theme::PaletteRole::Primary)
                        } else {
                            0x000000
                        }))
                        .bg(rgb(theme::clip_fill(
                            track.color(),
                            preview_clip.flavor(),
                            false,
                        )))
                        .children([clip_grid_layer(grid, visible_start, visible_end)]);
                    if let Some(waveform) = super::waveform::preview_waveform_view_cached(
                        super::waveform::WaveformPreview {
                            original: &original_clip,
                            preview: &preview_clip,
                            tempo: state.project.tempo_map(),
                            asset: state.waveforms.get(&original_clip.asset()),
                            prepared: state.audio_clip_waveforms.get(&original_clip.id()),
                        },
                        visible_start,
                        visible_end,
                        std::rc::Rc::clone(&self.waveform_geometry_cache),
                        self.waveform_grid(track_id, viewport, scale_factor),
                    ) {
                        preview = preview.child(clip_waveform_body(waveform));
                    }
                    preview = preview.child(self.clip_name_preview_header(
                        track_id,
                        original_clip.id(),
                        original_clip.name(),
                        cx,
                    ));
                    clip_area = clip_area.children([preview]);
                }
            }
        }
        // Group metadata is projected once per pointer/modifier/timer update.
        // Render only the members owned by this row; no project/PCM rebuild.
        if let Some(drag) = self.displayed_clip_drag() {
            let group = drag.group.borrow();
            if group.ids.len() > 1 {
                let mut shape_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
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
                    let flavor = match &member.content {
                        ClipGroupContent::Audio(c) => c.flavor(),
                        ClipGroupContent::Midi(c) => c.flavor(),
                    };
                    let id = member.id();
                    let mut overlay = div()
                        .debug_selector(move || format!("group-clip-preview-{}", id.value()))
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(
                            (start - viewport.start) as f32 / viewport.length as f32,
                        ))
                        .w(relative((end - start) as f32 / viewport.length as f32))
                        .min_w(px(3.0))
                        .overflow_hidden()
                        .border_1()
                        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
                        .bg(rgb(theme::clip_fill(track.color(), flavor, false)))
                        .children([clip_grid_layer(grid, start, end)]);
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
                            theme::clip_fill(track.color(), flavor, false),
                            &mut shape_budget,
                        )
                    {
                        overlay = overlay.child(content);
                    }
                    clip_area = clip_area.children([overlay]);
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
                let mut drop_preview = div()
                    .id(("clip-drop-preview", track_id.value()))
                    .debug_selector(|| "clip-drop-preview".to_owned())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(left as f32))
                    .w(relative(width as f32))
                    .min_w(px(3.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(if clip_preview.is_some() {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        0x000000
                    }))
                    .bg(rgb(theme::clip_fill(
                        track.color(),
                        self.displayed_clip_drag()
                            .and_then(|d| state.project.clip_flavor(d.clip))
                            .unwrap_or_default(),
                        false,
                    )))
                    .opacity(if clip_preview.is_some() { 1.0 } else { 0.88 })
                    .children([clip_grid_layer(grid, visible_start, visible_end)]);
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
                        let fill = theme::clip_fill(
                            track.color(),
                            state.project.clip_flavor(drag.clip).unwrap_or_default(),
                            false,
                        );
                        let mut note_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
                        if let Some(content) = self.clip_drag_content(
                            state,
                            source,
                            (preview.start_frame, preview.end_frame),
                            (visible_start, visible_end),
                            fill,
                            &mut note_budget,
                        ) {
                            drop_preview = drop_preview.child(content);
                        }
                    }
                } else if let Some(drag) = self
                    .active_browser_audio_drag
                    .as_ref()
                    .filter(|drag| self.browser_audio_drag_is_current(drag))
                    && let Some(waveform) = drag
                        .metadata
                        .borrow()
                        .as_ref()
                        .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                        .map(|metadata| std::sync::Arc::clone(&metadata.waveform))
                {
                    drop_preview = drop_preview.children([waveform_source_segment_view(
                        waveform,
                        preview.start_frame,
                        visible_start,
                        visible_end,
                    )]);
                }
                clip_area = clip_area.children([drop_preview]);
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
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| drag.lane_hovered.get())
            .and_then(|drag| match drag.destination.get() {
                Some(drag::BrowserDropDestination::ExistingTrack { track, start }) => {
                    Some((track, start))
                }
                Some(
                    drag::BrowserDropDestination::NewTrack { .. }
                    | drag::BrowserDropDestination::Console(_),
                )
                | None => None,
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
        if let Some(selection) = self.time_selection_overlay(track_id, viewport, scale_factor) {
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
                    theme::clip_fill(track.color(), fragment.flavor(), false),
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
                let (fragment, shown) = passive_midi_fragment(
                    theme::clip_fill(track.color(), clip.flavor(), false),
                    clip.id(),
                    &preview.samples,
                    (selected_start, selected_end),
                    (destination_start, destination_end),
                    viewport,
                    viewport_end,
                    grid,
                    "time-move-midi-fragment".to_owned(),
                    move_preview_shape_budget,
                );
                move_preview_shape_budget = move_preview_shape_budget.saturating_sub(shown);
                if let Some(fragment) = fragment {
                    clip_area = clip_area.children([fragment]);
                }
            }
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
            .on_drop(
                cx.listener(move |view, drag: &super::trim::MidiTrimDrag, _, cx| {
                    view.drop_midi_trim(drag, track_id, cx);
                }),
            );
        let track_name = track.name().to_owned();
        let track_name_view = super::track_header::name()
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
            .children(self.track_selection.contains(track_id).then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .debug_selector(move || format!("selected-track-{}", track_id.value()))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
                    .opacity(0.2)
            }))
            .debug_selector(|| format!("track-name-{}", track_id.value()))
            .child(self.track_name_label(
                super::track_name::Target::Track(track.id()),
                track.name(),
                cx,
            ))
            .tooltip(move |_, cx| {
                cx.new(|_| ClipTooltip {
                    name: track_name.clone(),
                })
                .into()
            });
        let live_volume = self.document_audio_mode == hex_session::DocumentAudioMode::Resident;
        // Empty header gutters bubble to the shared selection/focus owner.
        // Only actual control hit regions stop clicks; row-wide stops make the
        // lower header background unreachable for ordinary/modifier selection.
        let header_content = if live_volume {
            let (active_background, active_text) =
                super::track_gain::activator_colors(!track.muted());
            let (solo_background, solo_text) = super::track_gain::activator_colors(track.soloed());
            let utilities = div()
                .id(("track-header-utilities", track_id.value()))
                .flex()
                .gap(px(2.0))
                .child(
                    button(
                        "a",
                        "show track gain automation",
                        ("gain-lane", track_id.value()),
                    )
                    .h(px(18.0))
                    .debug_selector(move || format!("gain-lane-show-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        view.show_gain_lane(track_id, window, cx)
                    })),
                )
                .child(
                    button(
                        "↗",
                        "edit return sends",
                        ("track-return-sends", track_id.value()),
                    )
                    .h(px(18.0))
                    .debug_selector(move || format!("track-return-sends-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        let next = if view.send_menu_track == Some(track_id) {
                            None
                        } else {
                            Some(track_id)
                        };
                        if next != view.send_menu_track
                            && !super::device_panel::finalize_controls(view, cx)
                        {
                            cx.notify();
                            return;
                        }
                        view.send_menu_track = next;
                        cx.notify();
                    })),
                );
            let activator = button_with_style(
                "",
                if track.muted() {
                    "activate track"
                } else {
                    "deactivate track"
                },
                ("track-mute", track_id.value()),
                theme::action_style().with_state_colors(active_background, active_text),
            )
            .debug_selector(move || format!("track-mute-{}", track_id.value()))
            .flex_1()
            .min_w_0()
            .h(px(18.0))
            .child(theme::text((track_index + 1).to_string()))
            .on_click(cx.listener(move |view, _, _, cx| {
                cx.stop_propagation();
                view.toggle_mute(track_id);
                cx.notify();
            }));
            let solo = button_with_style(
                "s",
                "solo track · s · cmd+click additive",
                ("track-solo", track_id.value()),
                theme::action_style().with_state_colors(solo_background, solo_text),
            )
            .debug_selector(move || format!("track-solo-{}", track_id.value()))
            .w(px(28.0))
            .h(px(18.0))
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                cx.stop_propagation();
                view.toggle_solo(track_id, !event.modifiers().platform);
                cx.notify();
            }));
            let arm = div()
                .id(("track-arm-unavailable", track_id.value()))
                .debug_selector(move || format!("track-arm-unavailable-{}", track_id.value()))
                .w(px(28.0))
                .h(px(18.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                .cursor(gpui::CursorStyle::Arrow)
                .opacity(0.45)
                .tooltip(|_, cx| {
                    cx.new(|_| ClipTooltip {
                        name: "recording unavailable · planned for hex 0.2".into(),
                    })
                    .into()
                })
                .child(theme::text("○"));
            div()
                .id(("arrangement-track-controls", track_id.value()))
                .debug_selector(move || format!("track-controls-{}", track_id.value()))
                .w_full()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(track_name_view)
                        .child(utilities),
                )
                .child(
                    div()
                        .id(("track-header-mixer", track_id.value()))
                        .w(px(122.0))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .gap(px(2.0))
                                .child(activator)
                                .child(solo)
                                .child(arm),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .gap(px(2.0))
                                .child(
                                    self.track_volume(track, cx)
                                        .id(("track-volume-hit", track_id.value()))
                                        .on_click(|_, _, cx| cx.stop_propagation()),
                                )
                                .child(
                                    self.track_pan(track, cx)
                                        .id(("track-pan-hit", track_id.value()))
                                        .on_click(|_, _, cx| cx.stop_propagation()),
                                ),
                        ),
                )
        } else {
            let track_controls = div()
                .id(("arrangement-track-controls", track_id.value()))
                .debug_selector(move || format!("track-controls-{}", track_id.value()))
                .flex()
                .items_center()
                .gap_1()
                .when(live_volume, |row| row.w_full().gap(px(1.0)))
                .child(
                    button(
                        "a",
                        "show track gain automation",
                        ("gain-lane", track_id.value()),
                    )
                    .debug_selector(move || format!("gain-lane-show-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        view.show_gain_lane(track_id, window, cx);
                    })),
                )
                .when(live_volume, |row| row.child(self.track_volume(track, cx)))
                // VerifiedStreaming keeps its functional explicit controls until
                // the typed transient-source route can make held drags audible.
                .when(!live_volume, |row| {
                    row.child(
                        button("+", "increase track gain", ("gain-up", track_id.value()))
                            .debug_selector(move || format!("track-gain-up-{}", track_id.value()))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                cx.stop_propagation();
                                if view
                                    .state
                                    .as_ref()
                                    .and_then(|state| state.project.track(track_id))
                                    .is_some_and(|track| {
                                        !track.gain_envelope().is_empty()
                                            || !track.gain_lane().spans().is_empty()
                                    })
                                {
                                    view.show_gain_lane(track_id, window, cx);
                                }
                                view.gain_by(track_id, 0.1);
                                cx.notify();
                            })),
                    )
                    .child(
                        button("−", "decrease track gain", ("gain-down", track_id.value()))
                            .debug_selector(move || format!("track-gain-down-{}", track_id.value()))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                cx.stop_propagation();
                                if view
                                    .state
                                    .as_ref()
                                    .and_then(|state| state.project.track(track_id))
                                    .is_some_and(|track| {
                                        !track.gain_envelope().is_empty()
                                            || !track.gain_lane().spans().is_empty()
                                    })
                                {
                                    view.show_gain_lane(track_id, window, cx);
                                }
                                view.gain_by(track_id, -0.1);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .w(px(34.0))
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text(
                                if track.gain_envelope().is_empty()
                                    && track.gain_lane().spans().is_empty()
                                {
                                    format!("{:.0}%", track.gain() * 100.0)
                                } else {
                                    "auto".into()
                                },
                            )),
                    )
                })
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
                    .debug_selector(move || format!("track-mute-{}", track_id.value()))
                    .text_color(rgb(if track.muted() {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::Muted)
                    }))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
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
                    .debug_selector(move || format!("track-solo-{}", track_id.value()))
                    .text_color(rgb(if track.soloed() {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::Muted)
                    }))
                    .on_click(cx.listener(
                        move |view, event: &gpui::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            view.toggle_solo(track_id, !event.modifiers().platform);
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button(
                        "↗",
                        "edit return sends",
                        ("track-return-sends", track_id.value()),
                    )
                    .debug_selector(move || format!("track-return-sends-{}", track_id.value()))
                    .text_color(rgb(theme::color(
                        if self.send_menu_track == Some(track_id) {
                            theme::PaletteRole::Secondary
                        } else {
                            theme::PaletteRole::Muted
                        },
                    )))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        let next = if view.send_menu_track == Some(track_id) {
                            None
                        } else {
                            Some(track_id)
                        };
                        if next != view.send_menu_track
                            && !super::device_panel::finalize_controls(view, cx)
                        {
                            cx.notify();
                            return;
                        }
                        view.send_menu_track = next;
                        cx.notify();
                    })),
                );
            div()
                .id(("streaming-track-controls", track_id.value()))
                .w_full()
                .flex()
                .flex_col()
                .child(track_name_view)
                .child(track_controls)
        };
        let header_focus = self.track_header_focus(track_id, cx);
        let click_focus = header_focus.clone();
        let header = super::track_header::surface(
            ("arrangement-track-header", track_id.value()),
            self.track_header_width(),
            self.track_height(track_id),
            selected,
        )
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
        .child(header_content)
        .children(self.track_resize_handles(track_id, cx))
        .relative()
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(1.0))
                .bottom_0()
                .w(px(4.0))
                .debug_selector(move || format!("track-color-strip-{}", track_id.value()))
                .bg(rgb(theme::element_colors(track.color()).accent)),
        )
        // Paint selection after opaque header content and its assigned-color
        // strip. Keep layout/input bounds unchanged and resize feedback local.
        .children(
            (selected && self.active_trim_drag.is_none() && self.active_midi_trim_drag.is_none())
                .then(|| {
                    super::track_header::selection_outline(format!(
                        "track-selection-outline-{}",
                        track_id.value()
                    ))
                }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                view.open_track_color_context_menu(event.position, track_id, window, cx);
            }),
        );
        div()
            .w_full()
            .h(px(self.track_height(track_id)))
            .flex_shrink_0()
            .flex()
            .child(clip_area)
            .child(self.device_drop_header(track_id, header, cx))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    view.open_track_context_menu(event.position, Some(track_id), window, cx);
                }),
            )
    }

    pub(super) fn build_clip_contents(
        &self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
        } = timeline;
        let Some(viewport_end) = viewport.end() else {
            return div().size_full();
        };
        let track_id = track.id();
        let mut clip_area = div().size_full().relative();
        let move_display = self.time_range_move_display();
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
        for clip in track.clips() {
            if hides_group_member(clip.id()) {
                continue;
            }
            let Some(end) = clip.start().checked_add(clip.duration()) else {
                continue;
            };
            if clip.start() >= viewport_end || end <= viewport.start {
                continue;
            }
            let selected_clip = self.clip_is_selected(clip.id());
            if clip_intersects_exclusions(clip, exclusions) {
                for fragment in clip_fragments_outside(clip, exclusions, state.project.tempo_map())
                    .into_iter()
                    .flatten()
                {
                    if let Some(fragment_view) = passive_audio_fragment(
                        theme::clip_fill(track.color(), fragment.flavor(), false),
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
                    ) {
                        clip_area = clip_area.children([fragment_view]);
                    }
                }
                continue;
            }
            clip_area = clip_area.child(self.clip_view(
                state,
                track_id,
                clip,
                selected_clip,
                ClipViewTimeline {
                    viewport,
                    grid,
                    scale_factor,
                },
                cx,
            ));
        }
        let mut midi_shape_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
        for clip in track.midi_clips() {
            if hides_group_member(clip.id()) {
                continue;
            }
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
                    let (fragment, shown) = passive_midi_fragment(
                        theme::clip_fill(track.color(), clip.flavor(), false),
                        clip.id(),
                        &preview.samples,
                        interval,
                        interval,
                        viewport,
                        viewport_end,
                        grid,
                        format!("time-move-midi-residual-{fragment_index}"),
                        midi_shape_budget,
                    );
                    midi_shape_budget = midi_shape_budget.saturating_sub(shown);
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
            let pixel_width = self
                .drag_lanes
                .get(&track_id)
                .map(|lane| super::trim::rendered_clip_width(lane.width, width, 4.0))
                .unwrap_or(0.0);
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
                cx,
            );
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
            let fill = theme::clip_fill(track.color(), clip.flavor(), selected_clip);
            let mut midi = div()
                .id(("midi-clip", clip.id().value()))
                .debug_selector(|| format!("midi-clip-visual-{}", clip.id().value()))
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(left as f32))
                .w(relative(width as f32))
                .min_w(px(4.0))
                .overflow_hidden()
                .cursor(gpui::CursorStyle::Arrow)
                .opacity(if trim_preview.is_some() { 0.0 } else { 1.0 })
                .bg(rgb(fill))
                .border_1()
                .border_color(rgb(if selected_clip {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    0x000000
                }))
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
            midi = midi.children([clip_grid_layer(grid, visible_start, visible_end)]);
            let (notes, shown) = midi_note_body(midi_clip_id, note_shapes, false);
            midi = midi.child(clip_waveform_body(notes));
            midi_shape_budget = midi_shape_budget.saturating_sub(shown);
            if preview.occurrence_count > shown {
                midi = midi.child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                        .child(theme::text(format!("{shown}/{}", preview.occurrence_count))),
                );
            }
            midi = midi.child(self.clip_name_header(track_id, clip.id(), clip.name(), fill, cx));
            if let Some((payload, lane)) = midi_drag_payload {
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
                    .left(px(if !trim_handles.is_empty() && visible_start == start {
                        super::trim::handle_width(pixel_width)
                    } else {
                        0.0
                    }))
                    .right(px(if !trim_handles.is_empty() && visible_end == end {
                        super::trim::handle_width(pixel_width)
                    } else {
                        0.0
                    }));
                midi = midi.children([move_surface]).children(trim_handles);
            }
            clip_area = clip_area.children([midi]);
        }
        clip_area
    }

    fn clip_drag_content(
        &self,
        state: &ViewState,
        source: ClipPaintSource<'_>,
        destination: (u64, u64),
        visible: (u64, u64),
        fill: u32,
        note_budget: &mut usize,
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
        match source {
            ClipPaintSource::Audio(clip) => {
                content = content.child(clip_waveform_body(
                    self.audio_clip_body(state, clip, start, end, None),
                ));
                let fade = if clip.audio_edit().is_some() {
                    super::waveform::projected_fade_envelope_samples(
                        clip,
                        state.project.tempo_map(),
                        start,
                        end,
                    )
                    .unwrap_or_default()
                } else {
                    fade_envelope_samples(clip.retained_fade(), clip.start(), start, end)
                };
                if !clip.retained_fade().is_neutral() && fade.len() >= 2 {
                    content =
                        content.child(clip_fade_layer(fade, self.fade_curve_active(clip.id())));
                }
            }
            ClipPaintSource::Midi(clip) => {
                let preview = state.midi_preview.get(clip.id())?;
                let shapes = preview
                    .samples
                    .iter()
                    .take(hex_engine::MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP)
                    .filter_map(|sample| {
                        super::midi_thumbnail::note_shape(
                            sample.frame_start,
                            sample.frame_gate_end,
                            sample.pitch,
                            start,
                            end,
                        )
                    })
                    .take(*note_budget);
                let (notes, shown) = midi_note_body(id, shapes, true);
                *note_budget = note_budget.saturating_sub(shown);
                content = content.child(clip_waveform_body(notes));
            }
        }
        Some(content.child(self.clip_name_drag_header(id, name, fill)))
    }

    fn audio_clip_body(
        &self,
        state: &ViewState,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
        grid: Option<super::waveform::WaveformGrid>,
    ) -> Div {
        let waveform = state.waveforms.get(&clip.asset());
        let mut body = div().absolute().inset_0();
        if let Some(prepared) = state.audio_clip_waveforms.get(&clip.id()) {
            // The immutable prepared peaks cover the retained root. Cropping
            // changes its visible offset, never its processing/source origin.
            let preview = self.fade_trim_preview(clip.id()).and_then(|clip| {
                let (_, _, offset, _) = clip
                    .audio_edit()?
                    .projected_domain(state.project.tempo_map())
                    .ok()?;
                let mut preview = prepared.clone();
                preview.offset = offset;
                preview.length = clip.duration();
                Some(preview)
            });
            let prepared = preview.as_ref().unwrap_or(prepared);
            body = body.child(super::waveform::timeline_waveform_view_cached(
                std::sync::Arc::clone(&prepared.waveform),
                super::waveform_geometry::WaveformMapping::Prepared {
                    timeline_origin: clip.start(),
                    root_offset: prepared.offset,
                    gain: clip.gain(),
                },
                visible_start,
                visible_end,
                std::rc::Rc::clone(&self.waveform_geometry_cache),
                grid,
            ));
        } else if clip.audio_edit().is_some() {
            body = body.child(
                div()
                    .text_xs()
                    .child(theme::text("prepared waveform unavailable")),
            );
        } else {
            match waveform {
                Some(waveform) => {
                    body = body.child(super::waveform::timeline_waveform_view_cached(
                        std::sync::Arc::clone(waveform),
                        super::waveform_geometry::WaveformMapping::Clip {
                            clip: clip.clone(),
                            timeline_origin: clip.start(),
                        },
                        visible_start,
                        visible_end,
                        std::rc::Rc::clone(&self.waveform_geometry_cache),
                        grid,
                    ));
                }
                None => {
                    body = body.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .px_1()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("missing media")),
                    );
                }
            }
        }
        body
    }

    fn waveform_grid(
        &self,
        track: hex_project::TrackId,
        viewport: Viewport,
        scale: f32,
    ) -> Option<super::waveform::WaveformGrid> {
        let lane = self.drag_lanes.get(&track)?;
        Some(super::waveform::WaveformGrid {
            start: viewport.start,
            end: viewport.end()?,
            left: lane.left,
            width: lane.width,
            columns: super::waveform::waveform_column_count(lane.width, scale),
        })
    }

    fn clip_view(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &Clip,
        selected: bool,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let ClipViewTimeline {
            viewport,
            grid,
            scale_factor,
        } = timeline;
        let clip = self.fade_trim_preview(clip.id()).unwrap_or(clip);
        let clip_id = clip.id();
        let trim_preview_clip = self
            .audio_trim_display(track, Some(clip_id))
            .filter(|(original, _)| original.id() == clip_id)
            .map(|(_, preview)| preview);
        let active_trim_preview = trim_preview_clip.is_some();
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
        let drag_payload = self
            .drag_available()
            .then(|| self.drag_lanes.get(&track).copied())
            .flatten()
            .map(|lane| {
                (
                    ClipDrag {
                        group: Default::default(),
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
                        scroll: Default::default(),
                        snap_grid: self.snap_grid,
                        snap_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
                        copy_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
                        preview: std::rc::Rc::new(std::cell::Cell::new(None)),
                        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
                        insertion_line_y: std::rc::Rc::new(std::cell::Cell::new(None)),
                        ghost_label: format!("audio clip {}", clip_id.value()),
                        ghost_waveform: if clip.audio_edit().is_none() {
                            state.waveforms.get(&clip.asset()).cloned()
                        } else {
                            None
                        },
                        lane_hovered: std::rc::Rc::new(std::cell::Cell::new(false)),
                        last_pointer: std::rc::Rc::new(std::cell::Cell::new(None)),
                    },
                    lane,
                )
            });
        let fill = theme::clip_fill(
            state
                .project
                .track(track)
                .map_or(hex_project::ElementColor::default(), |t| t.color()),
            clip.flavor(),
            selected,
        );
        let mut content = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(left as f32))
            .w(relative(width as f32))
            .min_w(px(3.0))
            .overflow_hidden()
            .cursor(gpui::CursorStyle::Arrow)
            .opacity(if active_trim_preview { 0.0 } else { 1.0 })
            .bg(rgb(fill))
            .hover(move |style| {
                style.border_color(rgb(theme::color(if selected {
                    theme::PaletteRole::Primary
                } else {
                    theme::PaletteRole::Text
                })))
            });
        content = content.border_1().border_color(rgb(if selected {
            theme::color(theme::PaletteRole::Primary)
        } else {
            0x000000
        }));
        if !active_trim_preview {
            content = content.children([clip_grid_layer(grid, visible_start, visible_end)]);
            content = content.child(clip_waveform_body(self.audio_clip_body(
                state,
                clip,
                visible_start,
                visible_end,
                self.waveform_grid(track, viewport, scale_factor),
            )));
        }
        let fade_preview = self.fade_preview(clip.id());
        let fade = fade_preview.unwrap_or(clip.retained_fade());
        let envelope_width = self
            .drag_lanes
            .get(&track)
            .map(|lane| (lane.width * width as f32).max(3.0))
            .unwrap_or(0.0);
        if !active_trim_preview && envelope_width > 0.0 && !fade.is_neutral() {
            let samples = if clip.audio_edit().is_some() && fade_preview.is_none() {
                super::waveform::projected_fade_envelope_samples(
                    clip,
                    state.project.tempo_map(),
                    visible_start,
                    visible_end,
                )
                .unwrap_or_default()
            } else {
                fade_envelope_samples(fade, clip.start(), visible_start, visible_end)
            };
            if samples.len() >= 2 {
                content =
                    content.child(clip_fade_layer(samples, self.fade_curve_active(clip.id())));
            }
        }
        let clip_pixel_width = self
            .drag_lanes
            .get(&track)
            .map(|lane| super::trim::rendered_clip_width(lane.width, width, 3.0))
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
        let hover_generation = self.project_generation;
        let mut content = content
            .child(self.clip_name_header(track, clip.id(), clip.name(), fill, cx))
            .id(("clip", clip_id.value()))
            .debug_selector(|| format!("clip-visual-{}", clip_id.value()))
            .on_hover(cx.listener(move |view, hovered, _, cx| {
                if *hovered && view.project_generation != hover_generation {
                    return;
                }
                let hovered_clip = (hover_generation, clip_id);
                let next = if *hovered {
                    Some(hovered_clip)
                } else if view.hovered_fade_clip == Some(hovered_clip) {
                    None
                } else {
                    view.hovered_fade_clip
                };
                if view.hovered_fade_clip != next {
                    view.hovered_fade_clip = next;
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
                .left(px(
                    if !trim_handles.is_empty() && visible_start == clip.start() {
                        super::trim::handle_width(clip_pixel_width)
                    } else {
                        0.0
                    },
                ))
                .right(px(if !trim_handles.is_empty() && visible_end == clip_end {
                    super::trim::handle_width(clip_pixel_width)
                } else {
                    0.0
                }));
            content = content.children([move_surface]).children(trim_handles);
        }
        if !active_trim_preview {
            content = content.children(self.fade_handles(
                state,
                track,
                clip,
                (visible_start, visible_end),
                viewport,
                cx,
            ));
        }
        content
    }

    pub(super) fn drag_available(&self) -> bool {
        !self.track_resize_active()
            && !self.track_reorder_active()
            && self.clip_name_edit.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
    }

    pub(super) fn floating_clip_drag_ghost(&self) -> Option<Div> {
        let payload = self.displayed_clip_drag()?;
        if let Some(drag::ClipDropDestination::NewTrack {
            before_track: None,
            position,
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
                },
                ClipGhostDecor {
                    fill: theme::clip_fill(
                        hex_project::ElementColor::default(),
                        state.project.clip_flavor(payload.clip).unwrap_or_default(),
                        false,
                    ),
                    selector: "clip-drag-append-preview",
                    grid: Some((
                        super::musical_time::grid_lines(
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
                        let mut note_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
                        source.and_then(|source| {
                            self.clip_drag_content(
                                state,
                                source,
                                (preview.start_frame, preview.end_frame),
                                (visible_start, visible_end),
                                theme::clip_fill(
                                    hex_project::ElementColor::default(),
                                    state.project.clip_flavor(payload.clip).unwrap_or_default(),
                                    false,
                                ),
                                &mut note_budget,
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
            },
            ClipGhostDecor {
                fill: theme::clip_fill(
                    self.state
                        .as_ref()
                        .and_then(|s| s.project.track(payload.source_track))
                        .map_or(hex_project::ElementColor::default(), |t| t.color()),
                    self.state
                        .as_ref()
                        .and_then(|s| s.project.clip_flavor(payload.clip))
                        .unwrap_or_default(),
                    false,
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
                    let mut note_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
                    self.clip_drag_content(
                        state,
                        source,
                        (bounds.start_frame, bounds.end_frame),
                        (bounds.start_frame, bounds.end_frame),
                        theme::clip_fill(
                            state
                                .project
                                .track(payload.source_track)
                                .map_or(hex_project::ElementColor::default(), |track| {
                                    track.color()
                                }),
                            state.project.clip_flavor(payload.clip).unwrap_or_default(),
                            false,
                        ),
                        &mut note_budget,
                    )
                }),
            },
        ))
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
                group: Default::default(),
                project_generation: self.project_generation,
                revision: state.revision,
                source_track: track,
                clip: clip.id(),
                position: ClipPosition::Midi { start, length },
                grab_offset: std::rc::Rc::new(std::cell::Cell::new(None)),
                origin_pointer_x: std::rc::Rc::new(std::cell::Cell::new(None)),
                viewport_start: viewport.start,
                viewport_length: viewport.length,
                scroll: Default::default(),
                snap_grid: self.snap_grid,
                snap_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
                copy_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
                preview: std::rc::Rc::new(std::cell::Cell::new(None)),
                destination: std::rc::Rc::new(std::cell::Cell::new(None)),
                insertion_line_y: std::rc::Rc::new(std::cell::Cell::new(None)),
                ghost_label: format!("midi clip {}", clip.id().value()),
                ghost_waveform: None,
                lane_hovered: std::rc::Rc::new(std::cell::Cell::new(false)),
                last_pointer: std::rc::Rc::new(std::cell::Cell::new(None)),
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
        let source_track = payload.source_track;
        let source_generation = payload.project_generation;
        let mouse_down_payload = payload.clone();
        let mouse_up_payload = payload.clone();
        div()
            .id(("clip-move", clip_id.value()))
            .debug_selector(|| format!("clip-drag-surface-{}", clip_id.value()))
            .absolute()
            .top(px(-1.0))
            .left_0()
            .right_0()
            .h(px(
                lane.height * super::time_selection::SELECTION_HANDLE_FRACTION
            ))
            .cursor(gpui::CursorStyle::OpenHand)
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
                    view.begin_clip_drag(payload);
                    view.clip_drag_drop_position(payload, pointer.0, pointer.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                drag_entity.update(cx, |_, cx| cx.notify());
                cx.new(|_| ClipDragGhost)
            })
            .on_drag_move::<ClipDrag>(move |event, window, cx| {
                let Some(payload) = event.dragged_item().downcast_ref::<ClipDrag>() else {
                    return;
                };
                #[cfg(test)]
                move_entity.update(cx, |view, _| {
                    view.drag_move_event_count = view.drag_move_event_count.saturating_add(1);
                });
                let pointer = event.event.position;
                let pointer = (f32::from(pointer.x), f32::from(pointer.y));
                payload.last_pointer.set(Some(pointer));
                payload.snap_modifier.set(event.event.modifiers.platform);
                payload.copy_modifier.set(event.event.modifiers.alt);
                cx.set_active_drag_cursor_style(
                    drag::copy_cursor(payload.copy_modifier.get()),
                    window,
                );
                let (preview, lane_hovered) = move_entity.update(cx, |view, _| {
                    view.clip_drag_drop_position(payload, pointer.0, pointer.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                move_entity.update(cx, |_, cx| cx.notify());
            })
    }
}

struct ClipDragGhost;

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

#[cfg(all(test, feature = "ui-test-support"))]
mod overlap_tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
    use hex_project::{Command, Editor};

    #[test]
    fn original_crop_clone_mapping_is_checked_and_crops_both_edges() {
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1000, 1800)),
            Some((100, 500))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1200, 1600)),
            Some((200, 400))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (0, 1201)),
            Some((100, 201))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1601, u64::MAX)),
            Some((400, 500))
        );
        assert_eq!(
            clone_source_window((u64::MAX - 4, u64::MAX), (0, u64::MAX), (0, u64::MAX)),
            Some((u64::MAX - 4, u64::MAX))
        );
        for (source, destination, visible) in [
            ((1, 1), (1, 2), (1, 2)),
            ((2, 1), (1, 2), (1, 2)),
            ((1, 2), (2, 1), (1, 2)),
            ((1, 2), (1, 2), (2, 3)),
            ((1, 2), (1, 2), (2, 1)),
        ] {
            assert_eq!(clone_source_window(source, destination, visible), None);
        }
    }

    #[test]
    fn group_overlap_sweep_matches_independent_half_open_occupancy() {
        let mut intervals = vec![(0, 0)];
        for a in 0..4 {
            for b in a + 1..=4 {
                intervals.push((a, b));
            }
        }
        for &a in &intervals {
            for &b in &intervals {
                for &c in &intervals {
                    for &d in &intervals {
                        let mut events = Vec::new();
                        for (start, end) in [a, b] {
                            if start < end {
                                events.push((start, 1, 0));
                                events.push((end, -1, 0));
                            }
                        }
                        for (start, end) in [c, d] {
                            if start < end {
                                events.push((start, 0, 1));
                                events.push((end, 0, -1));
                            }
                        }
                        let actual = group_overlap_spans(events);
                        assert!(actual.iter().all(|(start, end)| start < end));
                        assert!(actual.windows(2).all(|pair| pair[0].1 < pair[1].0));
                        for frame in 0..4 {
                            let has = |ranges: [(u64, u64); 2]| {
                                ranges
                                    .iter()
                                    .any(|(start, end)| *start <= frame && frame < *end)
                            };
                            assert_eq!(
                                actual
                                    .iter()
                                    .any(|(start, end)| *start <= frame && frame < *end),
                                has([a, b]) && has([c, d])
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn group_overlap_sweep_max_model_shape_reports_paint_storage_separately() {
        let pairs = hex_project::MAX_CLIPS / 2;
        let mut events = Vec::with_capacity(pairs * 4);
        for index in (0..pairs).rev() {
            let start = index as u64 * 8;
            events.extend([
                (start, 1, 0),
                (start + 6, -1, 0),
                (start + 3, 0, 1),
                (start + 8, 0, -1),
            ]);
        }
        let event_bytes = events.capacity() * std::mem::size_of::<(u64, i32, i32)>();
        let start = std::time::Instant::now();
        let spans = group_overlap_spans(events);
        let elapsed = start.elapsed();
        assert_eq!(spans.len(), pairs);
        assert_eq!(spans[0], (3, 6));
        assert_eq!(
            spans[pairs - 1],
            ((pairs as u64 - 1) * 8 + 3, (pairs as u64 - 1) * 8 + 6)
        );
        eprintln!(
            "group_paint_sweep events={} event_capacity_bytes={event_bytes} output_capacity_bytes={} elapsed_ns={} debug_only/no_native_deadline; two paint-local Vecs, capture/update allocation unchanged",
            pairs * 4,
            spans.capacity() * std::mem::size_of::<(u64, u64)>(),
            elapsed.as_nanos()
        );
    }

    #[gpui::test]
    fn drop_overlap_spans_clip_merge_and_ignore_exact_touches(cx: &mut gpui::TestAppContext) {
        let (project, source, _, clip) = audio_project();
        let mut model = Editor::new(project, 8).unwrap();
        model
            .execute(Command::CopyClipTo {
                clip,
                track: source,
                start: 48_000,
            })
            .unwrap();
        let (_handle, editor) = editor_window(cx, model.project().clone(), Default::default());
        editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            let track = state.project.track(source).unwrap();
            assert_eq!(
                drop_overlap_spans(state, track, (24_000, 120_000), None),
                vec![(24_000, 120_000)]
            );
            assert_eq!(
                drop_overlap_spans(state, track, (24_000, 120_000), Some(clip)),
                vec![(48_000, 120_000)]
            );
            assert!(drop_overlap_spans(state, track, (144_000, 192_000), None).is_empty());
            assert!(drop_overlap_spans(state, track, (12_000, 12_000), None).is_empty());
        });
    }
    fn drag_content_next(service: &hex_session::service::Service) -> hex_session::service::Event {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual Service fixture response missing"
            );
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    fn held_midi_drag_keeps_actual_note_paint_and_colored_name_below_header(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{AppContext, Modifiers, MouseButton, VisualTestContext};
        use hex_project::{BeatTime, Created};
        use hex_session::service::{Request, Service};
        cx.update(crate::editor::bind_editor_keys);
        let environment = crate::editor::test_environment::Environment::new().unwrap();
        std::fs::create_dir_all(environment.root().join("samples")).unwrap();
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        drag_content_next(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "source".into(),
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let source = e.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "target".into(),
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let target = e.state.project.tracks()[1].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(4).unwrap(),
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let Created::Clip(clip) = e.created.unwrap() else {
            panic!("actual MIDI clip");
        };
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        let e = drag_content_next(&service);
        assert!(!e.state.midi_preview.get(clip).unwrap().samples.is_empty());
        let before = e.state.project.clone();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_environment(
                        window,
                        cx,
                        service,
                        environment.clone(),
                    )
                });
                editor.update(cx, |view, cx| {
                    assert!(std::sync::Arc::ptr_eq(&view.test_environment, &environment));
                    view.state = Some(e.state);
                    view.snap_grid = view.snap_grid.with_platform_modifier(true);
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let normal = Box::leak(format!("clip-note-{}-0", clip.value()).into_boxed_str());
        let original = visual
            .debug_bounds(normal)
            .expect("normal production note primitive");
        let original_fill = visual
            .rendered_background(normal)
            .expect("actual normal note quad");
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let start = header.center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(start.x + px(12.0), start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&target]);
        let end = point(start.x + px(72.0), px(lane.top + lane.height * 0.5));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let payload = view.active_clip_drag.as_ref().expect("real captured drag");
            assert_eq!(payload.preview.get().unwrap().target_track, target);
            assert_eq!(view.state.as_ref().unwrap().project, before);
        });
        let ghost = visual
            .debug_bounds("clip-drop-preview")
            .expect("actual target preview");
        let note = Box::leak(format!("clip-drag-note-{}-0", clip.value()).into_boxed_str());
        let actual = visual
            .debug_bounds(note)
            .expect("held target must retain actual production MIDI note primitive");
        assert_eq!(visual.rendered_background(note), Some(original_fill));
        assert!(actual.size.width > px(0.0) && actual.size.height == original.size.height);
        assert!(actual.top() >= ghost.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT));
        assert!(actual.bottom() <= ghost.bottom());
        let title = Box::leak(format!("clip-drag-name-{}", clip.value()).into_boxed_str());
        assert!(
            visual.rendered_background(title).is_some(),
            "actual colored clone title"
        );
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-drop-preview").is_none());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
    }

    #[gpui::test]
    fn held_processed_audio_drag_reuses_ready_root_on_lane_and_floating(
        cx: &mut gpui::TestAppContext,
    ) {
        assert_audio_drag_content(cx, false);
    }

    #[gpui::test]
    fn held_looped_warp_drag_keeps_last_ready_content_in_mixed_copy_group(
        cx: &mut gpui::TestAppContext,
    ) {
        assert_audio_drag_content(cx, true);
    }

    fn assert_audio_drag_content(cx: &mut gpui::TestAppContext, warped_loop: bool) {
        use gpui::{AppContext, Modifiers, MouseButton, VisualTestContext};
        use hex_session::service::{Request, Service};
        use std::sync::Arc;
        cx.update(crate::editor::bind_editor_keys);
        let environment = crate::editor::test_environment::Environment::new().unwrap();
        std::fs::create_dir_all(environment.root().join("samples")).unwrap();
        let path = environment.root().join("ready-root.wav");
        let frames = 96_000_u32;
        let data_bytes = frames * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..frames {
            wav.extend_from_slice(&(((i % 120) as i16 - 60) * 256).to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        drag_content_next(&service);
        // On a setup failure, retain the path until the real import ACK proves
        // the worker has completed its filesystem access.
        service.submit(Request::Import(path.clone())).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let e = loop {
            if let Ok(event) = service.try_event() {
                if event.error.is_some() {
                    environment.retain_for_unacknowledged_setup_failure();
                    panic!("actual import failed: {:?}", event.error);
                }
                break event;
            }
            if std::time::Instant::now() >= deadline {
                environment.retain_for_unacknowledged_setup_failure();
                panic!("actual import ACK required before releasing fixture paths");
            }
            std::thread::yield_now();
        };
        let clip = e.state.project.tracks()[0].clips()[0].id();
        service
            .submit(Request::Edit(Command::TrimClip {
                clip,
                start: 48_000,
                duration: 48_000,
                source_offset: 12_000,
                source_length: 48_000,
            }))
            .unwrap();
        drag_content_next(&service);
        service
            .submit(Request::Edit(Command::SetClipFades {
                clip,
                fade_in: 4_000,
                fade_out: 6_000,
            }))
            .unwrap();
        drag_content_next(&service);
        service
            .submit(Request::Edit(Command::SetAudioClipPitch {
                clip,
                cents: 300,
            }))
            .unwrap();
        let e = drag_content_next(&service);
        if warped_loop {
            service
                .submit(Request::Edit(Command::SetClipLooping {
                    clip,
                    looping: true,
                }))
                .unwrap();
            let e = drag_content_next(&service);
            let settings = hex_project::default_audio_warp(
                e.state.project.clip(clip).unwrap(),
                e.state.project.tempo_map(),
                hex_project::WarpMode::Beats {
                    preserve: hex_project::BeatsPreserve::Transients,
                    tail: hex_project::BeatsTail::Forward,
                    decay: 100,
                },
            )
            .unwrap();
            service
                .submit(Request::Edit(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                }))
                .unwrap();
            drag_content_next(&service);
        } else {
            assert!(e.state.project.clip(clip).unwrap().audio_edit().is_some());
        }
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "target".into(),
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let target = e.state.project.tracks()[1].id();
        let source_track = e.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: source_track,
                start: hex_project::BeatTime::from_quarters(5).unwrap(),
                length: hex_project::BeatTime::ONE,
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let hex_project::Created::Clip(midi) = e.created.unwrap() else {
            panic!("actual mixed-group MIDI member");
        };
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip: midi,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 64,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        let e = drag_content_next(&service);
        let source = e.state.project.clip(clip).unwrap().clone();
        assert!(source.audio_edit().is_some());
        let ready = e.state.audio_clip_waveforms.get(&clip).unwrap().clone();
        let before = e.state.project.clone();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_environment(
                        window,
                        cx,
                        service,
                        environment.clone(),
                    )
                });
                editor.update(cx, |view, cx| {
                    assert!(Arc::ptr_eq(&view.test_environment, &environment));
                    view.state = Some(e.state);
                    view.snap_grid = view.snap_grid.with_platform_modifier(true);
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let start = header.center();
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(start.x + px(12.0), start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        let lane = editor.read_with(&visual, |v, _| v.drag_lanes[&target]);
        let landing = point(start.x + px(90.0), px(lane.top + lane.height * 0.5));
        let floating = point(px(60.0), px(100.0));
        for (pointer, selector) in [
            (landing, "clip-drop-preview"),
            (floating, "clip-drag-floating-preview"),
        ] {
            visual.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::default());
            editor.update(&mut visual, |v, cx| {
                v.clip_content_cache = Default::default();
                v.waveform_geometry_cache.borrow_mut().clear();
                cx.notify();
            });
            visual.run_until_parked();
            let bounds = visual.debug_bounds(selector).expect("real clone surface");
            let title = visual
                .debug_bounds(Box::leak(
                    format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
                ))
                .unwrap();
            assert!(
                visual.rendered_border(selector).is_some(),
                "actual highlighted border"
            );
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, before);
                assert_eq!(v.worker_requests_pending, 0);
                let cache = v.waveform_geometry_cache.borrow();
                let paints: Vec<_> = cache
                    .painted_waveforms()
                    .iter()
                    .filter(|p| {
                        p.bounds.top() >= bounds.top()
                            && p.bounds.top() < bounds.bottom()
                            && p.bounds.left() >= bounds.left()
                            && p.bounds.left() < bounds.right()
                    })
                    .collect();
                assert!(
                    !paints.is_empty(),
                    "ready processed waveform must actually paint in {selector}"
                );
                for paint in paints {
                    assert!(paint.bounds.top() >= title.bottom());
                    assert!(paint.mask.top() >= title.bottom());
                    assert!(paint.mask.bottom() <= bounds.bottom());
                }
                assert!(
                    cache.rendered_waveforms().iter().any(|sample| {
                        Arc::ptr_eq(&sample.source, &ready.waveform)
                            && sample.prepared == Some((source.start(), ready.offset))
                            && sample.spans[0] > 0.0
                    }),
                    "clone must use original ready processed root and source offset"
                );
            });
        }
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(floating, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-drag-floating-preview").is_none());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before)
        });
        // Actual additive header selection, then drag a non-primary member.
        for (index, id) in [clip, midi].into_iter().enumerate() {
            let header = visual
                .debug_bounds(Box::leak(
                    format!("clip-name-{}", id.value()).into_boxed_str(),
                ))
                .unwrap();
            visual.simulate_click(
                header.center(),
                Modifiers {
                    platform: index != 0,
                    ..Modifiers::default()
                },
            );
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert_eq!(v.selected_clip_ids().len(), 2));
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let down = header.center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        for duplicate in [false, true] {
            visual.simulate_modifiers_change(Modifiers {
                alt: duplicate,
                ..Modifiers::default()
            });
            let pointer = point(
                down.x + px(if duplicate { 110.0 } else { 80.0 }),
                px(lane.top + lane.height * 0.5),
            );
            visual.simulate_mouse_move(
                pointer,
                Some(MouseButton::Left),
                Modifiers {
                    alt: duplicate,
                    ..Modifiers::default()
                },
            );
            editor.update(&mut visual, |v, cx| {
                v.clip_content_cache = Default::default();
                v.waveform_geometry_cache.borrow_mut().clear();
                cx.notify();
            });
            visual.run_until_parked();
            let audio_bounds = visual
                .debug_bounds(Box::leak(
                    format!("group-clip-preview-{}", clip.value()).into_boxed_str(),
                ))
                .unwrap();
            let midi_bounds = visual
                .debug_bounds(Box::leak(
                    format!("group-clip-preview-{}", midi.value()).into_boxed_str(),
                ))
                .unwrap();
            let note = visual
                .debug_bounds(Box::leak(
                    format!("clip-drag-note-{}-0", midi.value()).into_boxed_str(),
                ))
                .expect("actual mixed-group note primitive");
            assert!(
                note.top() >= midi_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT)
            );
            assert!(note.bottom() <= midi_bounds.bottom());
            for id in [clip, midi] {
                let title = Box::leak(format!("clip-drag-name-{}", id.value()).into_boxed_str());
                assert!(visual.rendered_background(title).is_some());
            }
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, before);
                let cache = v.waveform_geometry_cache.borrow();
                assert!(
                    cache.painted_waveforms().iter().any(|p| {
                        p.bounds.top()
                            >= audio_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT)
                            && p.bounds.top() < audio_bounds.bottom()
                            && p.bounds.left() >= audio_bounds.left()
                            && p.bounds.left() < audio_bounds.right()
                    }),
                    "mixed-group audio must actually paint in both move/copy modes"
                );
                assert!(
                    cache
                        .rendered_waveforms()
                        .iter()
                        .any(|p| Arc::ptr_eq(&p.source, &ready.waveform))
                );
                assert_eq!(
                    v.active_clip_drag.as_ref().unwrap().copy_modifier.get(),
                    duplicate
                );
                assert_eq!(v.worker_requests_pending, 0);
            });
        }
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.active_clip_drag.is_none());
            assert_eq!(v.state.as_ref().unwrap().project, before);
        });
    }
}
