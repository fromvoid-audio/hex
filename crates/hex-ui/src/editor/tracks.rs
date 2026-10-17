use super::drag::{self, ClipDrag, ClipPosition, LaneBounds};
use super::time_move_preview::{
    clip_fragments_outside, clip_intersection, clip_intersects_exclusions, move_exclusions,
    passive_audio_fragment, passive_midi_fragment, time_fragments_outside,
};
use super::trim::TrimDrag;
use super::viewport::Viewport;
use super::waveform::{
    ClipTooltip, button, fade_envelope_samples, waveform_source_segment_view,
    waveform_summary_view, waveform_view_at,
};
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb,
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
    waveform_visual: Option<Div>,
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
        waveform_visual,
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
        .border_color(rgb(0x000000))
        .bg(rgb(fill))
        .opacity(0.88);
    if let Some((lines, start, end)) = grid {
        ghost = ghost.children([clip_grid_layer(&lines, start, end)]);
    }
    if let Some(waveform) = waveform_visual {
        ghost = ghost.children([waveform]);
    } else if let Some(waveform) = &payload.ghost_waveform {
        ghost = ghost.children([waveform_summary_view(std::sync::Arc::clone(waveform))]);
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

fn midi_trim_note_shapes(
    project: &hex_project::Project,
    clip: &MidiClip,
    trim: MidiClipTrim,
    visible_start: u64,
    visible_end: u64,
) -> Vec<(f32, f32, u8)> {
    let playback = MidiSourcePlayback::new(
        clip.source_length(),
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
                .note_occurrences(note.start(), note.duration())
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
        clip_area = clip_area.child(content);
        let move_display = self.time_range_move_display();
        let mut move_preview_shape_budget = super::midi_thumbnail::ROW_NOTE_SHAPE_LIMIT;
        let live_midi_trim_preview = self
            .active_midi_trim_drag
            .as_ref()
            .filter(|drag| drag.source_track == track_id && self.midi_trim_drag_current(drag))
            .and_then(|drag| {
                (*drag.preview.borrow()).map(|preview| (std::rc::Rc::clone(&drag.clip), preview))
            });
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
                        .border_color(rgb(0x000000))
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
        let live_trim_preview = self
            .active_trim_drag
            .as_ref()
            .filter(|drag| drag.source_track == track_id && self.trim_drag_current(drag))
            .and_then(|drag| {
                drag.preview
                    .borrow()
                    .as_ref()
                    .map(|preview| (drag.clip.clone(), preview.clip.clone()))
            });
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
                        .border_color(rgb(0x000000))
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
                        preview = preview.child(waveform);
