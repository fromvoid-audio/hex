#[path = "tracks/clone_source_window.rs"]
mod clone_source_window;
#[path = "tracks/group_overlap.rs"]
mod group_overlap;

pub(super) use group_overlap::group_overlap_spans;

#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "tracks/midi_trim_unit_tests.rs"]
mod midi_trim_unit_tests;

use super::clip_boundary::clip_boundary_shell;
use super::drag::{self, ClipDrag, ClipPosition, LaneBounds};
use super::time_move_preview::{
    clip_fragments_outside, clip_intersection, clip_intersects_exclusions, move_exclusions,
    passive_audio_fragment, passive_midi_fragment, time_fragments_outside,
};
use super::trim::{TrimDrag, loop_handles};
use super::viewport::Viewport;
use super::waveform::{
    ClipTooltip, button, fade_envelope_samples, toggle_button_with_style,
    waveform_source_segment_view, waveform_summary_view,
};
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb, rgba,
};
use hex_project::{BeatTime, Clip, MidiClip, Track};

use clone_source_window::clone_source_window;

/// Observe the actual pixel-rounded resting clip rectangle. A clip's move hit
/// strip excludes handles and cannot define its horizontal thirds. The canvas
/// includes the parent's leading stroke; the trailing stroke belongs to the
/// separate boundary shell, so there is no trailing layout border to expand.
fn clip_drag_span(payload: &ClipDrag, leading_border: f32) -> impl IntoElement + use<> {
    let span = std::rc::Rc::clone(&payload.source_span);
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

struct ClipGhostGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale: f32,
}

struct ClipGhostDecor {
    fill: u32,
    looping: bool,
    notched: bool,
    selector: &'static str,
    grid: Option<(Vec<super::musical_time::GridLine>, u64, u64)>,
    contents: Option<Div>,
}

/// Body-only clipping leaves the title and outer selection/rounded mask intact.
pub(super) fn clip_waveform_body(content: Div) -> Div {
    waveform_body_below(content, super::clip_name::CLIP_NAME_HEIGHT)
}
fn waveform_body_below(content: Div, header_height: f32) -> Div {
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

pub(super) fn alias_drag_badge() -> Div {
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

fn clip_repeats(project: &hex_project::Project, clip: hex_project::ClipId) -> bool {
    project
        .clip(clip)
        .map(loop_handles::audio_repeats)
        .or_else(|| project.midi_clip(clip).map(|clip| clip.looping()))
        .unwrap_or(false)
}

fn clip_drag_ghost(payload: &ClipDrag, geometry: ClipGhostGeometry, decor: ClipGhostDecor) -> Div {
    let ClipGhostGeometry {
        left,
        top,
        width,
        height,
        scale,
    } = geometry;
    let ClipGhostDecor {
        fill,
        looping,
        notched,
        selector,
        grid,
        contents,
    } = decor;
    let stroke = 1.0 / scale.max(1.0);
    let has_border = hex_widgets::rectangle_border_width(width) > 0.0;
    let bounds = gpui::Bounds::new(
        point(px(left), px(top)),
        gpui::size(px(width), px(height.max(3.0))),
    );
    let backdrop = super::clip_boundary::Backdrop {
        body: fill,
        header: fill,
        outline: theme::color(theme::PaletteRole::Primary),
        hover_outline: theme::color(theme::PaletteRole::Primary),
        stroke,
        bottom_radius: if looping {
            loop_handles::BOTTOM_RADIUS
        } else {
            0.0
        },
        header_height: super::clip_name::CLIP_NAME_HEIGHT,
        header_gradient: None,
        notch: super::clip_boundary::Notch::new(has_border && notched),
    };
    let mut ghost = hex_widgets::rectangle_surface_pixels(left, top, width, height)
        .left_0()
        .top_0()
        .w_full()
        .h_full()
        .border_l(px(if has_border { stroke } else { 0.0 }))
        .border_r(px(0.0))
        .debug_selector(move || selector.to_owned())
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
        .bg(rgb(fill));
    if let Some((lines, start, end)) = grid {
        ghost = ghost.children(
            aligned_clip_grid_layer(&lines, start, end, stroke, has_border)
                .map(|grid| loop_handles::body(grid, looping)),
        );
    }
    if let Some(content) = contents {
        ghost = ghost.child(loop_handles::body(content, looping));
        if payload.alias_copy() {
            ghost = ghost.children([alias_drag_badge()]);
        }
        return super::clip_boundary::pixel_backdrop_shell(
            ghost,
            bounds,
            backdrop,
            has_border,
            move || format!("{selector}-trailing"),
        );
    } else if let Some(waveform) = &payload.ghost_waveform {
        ghost = ghost.children([loop_handles::body(
            clip_waveform_body(waveform_summary_view(
                std::sync::Arc::clone(waveform),
                theme::waveform_ink_for_fill(fill),
            )),
            looping,
        )]);
    } else {
        ghost = ghost.child(clip_waveform_body(
            div().child(
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
    super::clip_boundary::pixel_backdrop_shell(ghost, bounds, backdrop, has_border, move || {
        format!("{selector}-trailing")
    })
}

fn midi_note_body(
    clip: hex_project::ClipId,
    shapes: impl IntoIterator<Item = (f32, f32, u8)>,
    fit: super::midi_thumbnail::PitchFit,
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
pub(super) fn midi_note_canvas(
    selector: impl Fn() -> String + 'static,
    shapes: impl IntoIterator<Item = (f32, f32, u8)>,
    fit: super::midi_thumbnail::PitchFit,
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
                        if let Some(note) = super::midi_thumbnail::paint_bounds(
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
                window.paint_path(shade, rgba(theme::CLIP_FADE_SHADE));
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

enum ClipPaintSource<'a> {
    Audio(&'a Clip),
    Midi(&'a MidiClip),
}

pub(super) fn midi_trim_note_shapes(
    project: &hex_project::Project,
    clip: &MidiClip,
    trim: impl Into<super::trim::MidiEdgePreview>,
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
    super::midi_loop_notes::visible_occurrences(
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
        super::midi_thumbnail::note_shape(start, end, pitch, visible_start, visible_end)
    })
    .collect()
}

pub(super) const TRACK_ROW_HEIGHT: f32 = 44.0;
/// Reserve a readable label beside the compact mixer and meter, even at the
/// minimum header width. The remaining workspace stays available to the lane.
pub(super) const TRACK_HEADER_WIDTH: f32 = 256.0;
pub(super) const DEFAULT_TRACK_HEADER_WIDTH: f32 = TRACK_HEADER_WIDTH;

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
) -> Option<Div> {
    clip_grid_layer_impl(lines, start, end, None)
}

/// Place the grid against the outer clip interval rather than its inset
/// content box. This shares the lane's physical-pixel columns.
pub(super) fn aligned_clip_grid_layer(
    lines: &[super::musical_time::GridLine],
    start: u64,
    end: u64,
    stroke: f32,
    has_border: bool,
) -> Option<Div> {
    clip_grid_layer_impl(lines, start, end, Some(stroke)).map(|grid| {
        // The title content is transparent on a shared backdrop. Restrict
        // timeline strokes to the body instead of relying on an opaque title.
        grid.top(px(super::clip_name::CLIP_NAME_HEIGHT))
            .left(px(if has_border { -stroke } else { 0.0 }))
            .right_0()
    })
}

fn clip_grid_layer_impl(
    lines: &[super::musical_time::GridLine],
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
        let _navigation_timer =
            super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::Row);
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
            clip_area =
                clip_area.children([super::group_overview::view(self, state, track, viewport)]);
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
        let move_display = self.time_range_move_display();
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
                        super::trim::rendered_clip_width(lane.width, width)
                    });
                    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                    let fill = theme::clip_activity_fill(
                        track.color(),
                        clip.flavor(),
                        false,
                        clip.active(),
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
                            .border_color(rgb(if self.clip_is_selected(clip.id()) {
                                theme::color(theme::PaletteRole::Primary)
                            } else {
                                theme::color(theme::PaletteRole::ClipOutline)
                            }))
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
                                .map(|grid| loop_handles::body(grid, preview.looping(&clip))),
                            );
                    let looping = preview.looping(&clip);
                    overlay = loop_handles::surface(overlay, looping);
                    if self.track_body_height(track_id) > super::track_layout::HEADER_ONLY_HEIGHT {
                        let shapes = midi_trim_note_shapes(
                            &state.project,
                            &clip,
                            preview,
                            visible_start,
                            visible_end,
                            super::midi_loop_notes::merge_frames(
                                visible_end - visible_start,
                                pixel_width,
                                scale_factor,
                            ),
                        );
                        // Keep the resting clip's zoom so the drag doesn't
                        // change note thickness.
                        let fit = super::midi_loop_notes::sounding_fit(&clip);
                        let (notes, _) =
                            midi_note_canvas(|| "midi-trim-preview-notes".to_owned(), shapes, fit);
                        overlay =
                            overlay.child(loop_handles::body(clip_waveform_body(notes), looping));
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
                        overlay.child(loop_handles::midi_markers(
                            &clip,
                            Some(preview),
                            &state.project,
                            visible_start,
                            visible_end,
                        ))
                    });
                    let color = if self.clip_is_selected(clip.id()) {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::ClipOutline)
                    };
                    let framed_clip = if has_border {
                        super::clip_boundary::backdrop_shell(
                            overlay,
                            left as f32,
                            width as f32,
                            super::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                bottom_radius: if looping {
                                    loop_handles::BOTTOM_RADIUS
                                } else {
                                    0.0
                                },
                                header_height: super::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: super::clip_header_loop::midi(
                                    &clip,
                                    Some(preview),
                                    state.project.tempo_map(),
                                    visible_start..visible_end,
                                    pixel_width * scale_factor,
                                ),
                                notch: super::clip_boundary::Notch::new(notched),
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
                            if looping {
                                loop_handles::BOTTOM_RADIUS
                            } else {
                                0.0
                            },
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
                        super::trim::rendered_clip_width(lane.width, width)
                    });
                    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                    let fill = theme::clip_activity_fill(
                        track.color(),
                        preview_clip.flavor(),
                        false,
                        preview_clip.active(),
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
                            .border_color(rgb(if self.clip_is_selected(original_clip.id()) {
                                theme::color(theme::PaletteRole::Primary)
                            } else {
                                theme::color(theme::PaletteRole::ClipOutline)
                            }))
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
                                .map(|grid| {
                                    loop_handles::body(
                                        grid,
                                        loop_handles::audio_repeats(&preview_clip),
                                    )
                                }),
                            );
                    let looping = loop_handles::audio_repeats(&preview_clip);
                    preview = loop_handles::surface(preview, looping);
                    if self.track_body_height(track_id) > super::track_layout::HEADER_ONLY_HEIGHT
                        && let Some(waveform) = super::waveform::preview_waveform_view_cached(
                            super::waveform::WaveformPreview {
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
                        preview = preview
                            .child(loop_handles::body(clip_waveform_body(waveform), looping));
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
                        preview.child(loop_handles::audio_markers(
                            &preview_clip,
                            &state.project,
                            visible_start,
                            visible_end,
                        ))
                    });
                    let color = if self.clip_is_selected(original_clip.id()) {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::ClipOutline)
                    };
                    let framed_clip = if has_border {
                        super::clip_boundary::backdrop_shell(
                            preview,
                            left as f32,
                            width as f32,
                            super::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                bottom_radius: if looping {
                                    loop_handles::BOTTOM_RADIUS
                                } else {
                                    0.0
                                },
                                header_height: super::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: super::clip_header_loop::audio(
                                    &preview_clip,
                                    state.project.tempo_map(),
                                    visible_start..visible_end,
                                    pixel_width * scale_factor,
                                ),
                                notch: super::clip_boundary::Notch::new(notched),
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
                            if looping {
                                loop_handles::BOTTOM_RADIUS
                            } else {
                                0.0
                            },
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
                    let (flavor, active, looping) = match &member.content {
                        ClipGroupContent::Audio(c) => {
                            (c.flavor(), c.active(), loop_handles::audio_repeats(c))
                        }
                        ClipGroupContent::Midi(c) => (c.flavor(), c.active(), c.looping()),
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
                        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
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
                            .map(|grid| loop_handles::body(grid, looping)),
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
                                <= super::track_layout::HEADER_ONLY_HEIGHT,
                            has_border,
                        )
                    {
                        overlay = overlay.child(loop_handles::body(content, looping));
                    }
                    if drag.alias_copy() {
                        overlay = overlay.children([alias_drag_badge()]);
                    }
                    let color = theme::color(theme::PaletteRole::Primary);
                    let framed_clip = if has_border {
                        super::clip_boundary::backdrop_shell(
                            overlay,
                            left,
                            width,
                            super::clip_boundary::Backdrop {
                                body: fill,
                                header: fill,
                                outline: color,
                                hover_outline: color,
                                stroke: 1.0 / scale_factor.max(1.0),
                                bottom_radius: if looping {
                                    loop_handles::BOTTOM_RADIUS
                                } else {
                                    0.0
                                },
                                header_height: super::clip_name::CLIP_NAME_HEIGHT,
                                header_gradient: None,
                                notch: super::clip_boundary::Notch::new(end == bounds.end_frame),
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
                    super::trim::rendered_clip_width(lane.width, width)
                });
                let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
                let fill = theme::clip_activity_fill(
                    track.color(),
                    self.displayed_clip_drag()
                        .and_then(|d| state.project.clip_flavor(d.clip))
                        .unwrap_or_default(),
                    false,
                    self.displayed_clip_drag()
                        .and_then(|d| state.project.clip_active(d.clip))
                        .unwrap_or(true),
                );
                let looping = self
                    .displayed_clip_drag()
                    .filter(|_| clip_preview.is_some())
                    .and_then(|drag| {
                        state
                            .project
                            .clip(drag.clip)
                            .map(loop_handles::audio_repeats)
                            .or_else(|| {
                                state
                                    .project
                                    .midi_clip(drag.clip)
                                    .map(|clip| clip.looping())
                            })
                    })
                    .unwrap_or(false);
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
                        .border_color(rgb(if clip_preview.is_some() {
                            theme::color(theme::PaletteRole::Primary)
                        } else {
                            theme::color(theme::PaletteRole::ClipOutline)
                        }))
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
                            .map(|grid| loop_handles::body(grid, looping)),
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
                                <= super::track_layout::HEADER_ONLY_HEIGHT,
                            has_border,
                        ) {
                            drop_preview = drop_preview.child(loop_handles::body(content, looping));
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
                    drop_preview =
                        drop_preview.children([clip_waveform_body(waveform_source_segment_view(
                            waveform,
                            preview.start_frame,
                            visible_start,
                            visible_end,
                            crate::theme::color(crate::theme::PaletteRole::Waveform),
                        ))]);
                }
                if clip_preview.is_some()
                    && self
                        .displayed_clip_drag()
                        .is_some_and(|drag| drag.alias_copy())
                {
                    drop_preview = drop_preview.children([alias_drag_badge()]);
                }
                let color = if clip_preview.is_some() {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    theme::color(theme::PaletteRole::ClipOutline)
                };
                let framed_clip = (if has_border {
                    super::clip_boundary::backdrop_shell(
                        drop_preview,
                        left as f32,
                        width as f32,
                        super::clip_boundary::Backdrop {
                            body: fill,
                            header: fill,
                            outline: color,
                            hover_outline: color,
                            stroke: 1.0 / scale_factor.max(1.0),
                            bottom_radius: if looping {
                                loop_handles::BOTTOM_RADIUS
                            } else {
                                0.0
                            },
                            header_height: super::clip_name::CLIP_NAME_HEIGHT,
                            header_gradient: None,
                            notch: super::clip_boundary::Notch::new(
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
        if let Some(selection) = self.time_selection_overlay(track_id, viewport, scale_factor, cx) {
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
                    self.track_body_height(track_id) <= super::track_layout::HEADER_ONLY_HEIGHT,
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
                    clip.looping(),
                    |start, end, merge| {
                        self.loop_note_shapes(&state.project, clip, start, end, merge)
                    },
                    (selected_start, selected_end),
                    (destination_start, destination_end),
                    viewport,
                    viewport_end,
                    grid,
                    "time-move-midi-fragment".to_owned(),
                    self.track_body_height(track_id) <= super::track_layout::HEADER_ONLY_HEIGHT,
                );
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
        let name_colors = theme::element_colors(track.color());
        let (name_fill, name_text) = if track_selected {
            (name_colors.accent, name_colors.accent_text)
        } else {
            (name_colors.muted, name_colors.muted_text)
        };
        let hierarchy_indent = self.track_hierarchy_indent.pixels(track_id);
        let track_name_view = super::track_header::name()
            .text_color(rgb(name_text))
            .flex()
            .items_center()
            .children(track_selected.then(|| {
                super::track_header::selection_marker(format!(
                    "selected-track-marker-{}",
                    track_id.value()
                ))
            }))
            .children((hierarchy_indent > 0.0).then(|| {
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
            .children(track.is_group().then(|| {
                let folded = self.arrangement_focus.projection.folded(track_id);
                div()
                    .id(("group-fold", track_id.value()))
                    .debug_selector(move || format!("group-fold-{}", track_id.value()))
                    .w(px(14.0))
                    .flex_shrink_0()
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
                        cx.new(|_| super::transport::MiniTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            description: if folded { "expand group" } else { "fold group" }.into(),
                        })
                        .into()
                    })
                    .child(theme::text(if folded { ">" } else { "v" }))
            }))
            .child(self.track_name_label(
                super::track_name::Target::Track(track.id()),
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
        let group_plate = track.is_group()
            && self.document_audio_mode == hex_session::DocumentAudioMode::Resident;
        let group_expanded = group_plate && self.track_body_height(track_id) > TRACK_ROW_HEIGHT;
        let name_area = div()
            .id(("track-name-area", track_id.value()))
            .cursor_pointer()
            .debug_selector(move || format!("track-name-area-{}", track_id.value()))
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .px(px(2.))
            .children([super::track_header::name_plate(
                format!("track-name-plate-{}", track_id.value()),
                name_fill,
                track.is_group(),
            )])
            .text_color(rgb(name_text))
            .when(group_plate, |plate| plate.h_full().justify_center())
            .when(group_expanded, |plate| plate.justify_start());
        let sample_source =
            super::sample_recorder::Arm::Output(hex_engine::OutputCaptureId::Track(track_id));
        let resident_layout = self.document_audio_mode == hex_session::DocumentAudioMode::Resident;
        // Empty header gutters bubble to the shared selection/focus owner.
        // Only actual control hit regions stop clicks; row-wide stops make the
        // lower header background unreachable for ordinary/modifier selection.
        let header_content = if self.track_body_height(track_id) < TRACK_ROW_HEIGHT {
            // A compact header must not mount hidden numeric controls or their
            // hit targets beneath its readable title and resize gutter.
            div()
                .id(("compact-track-title", track_id.value()))
                .w_full()
                .flex()
                .items_center()
                .children([name_area.child(track_name_view).flex_1().min_w_0()])
                .child(self.sample_arm_button(sample_source, cx))
        } else if resident_layout {
            let (active_background, active_text) =
                super::track_gain::activator_colors(!track.muted());
            let (solo_background, solo_text) =
                super::track_gain::activator_colors(self.displayed_solo(track_id, track.soloed()));
            let utilities = div()
                .id(("track-header-utilities", track_id.value()))
                .w_full()
                .min_w_0()
                .flex()
                .gap(px(2.0))
                .child(
                    button(
                        "a",
                        "show track gain automation",
                        ("gain-lane", track_id.value()),
                    )
                    .h(px(18.0))
                    .flex_shrink()
                    .min_w(px(14.0))
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
                    .flex_shrink()
                    .min_w(px(14.0))
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
            let activator = toggle_button_with_style(
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
            let arm = div()
                .id(("track-arm-unavailable", track_id.value()))
                .debug_selector(move || format!("track-arm-unavailable-{}", track_id.value()))
                .w(px(16.0))
                .h(px(14.0))
                .rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
                .border_1()
                .border_color(rgb(theme::control_style().border))
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
                        kind: crate::tooltips::Kind::Identification,
                        name: "recording unavailable · planned for hex 0.2".into(),
                    })
                    .into()
                })
                .child(theme::text("○"));
            div()
                .id(("arrangement-track-controls", track_id.value()))
                .when(group_plate, |controls| controls.h_full())
                .debug_selector(move || format!("track-controls-{}", track_id.value()))
                .w_full()
                .flex()
                .items_center()
                .when(group_expanded, |controls| controls.items_start())
                .gap(px(2.0))
                .children([name_area.flex_1().child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .when(group_plate, |content| {
                            content
                                .h(px(TRACK_ROW_HEIGHT - 4.0))
                                .flex_shrink_0()
                                .justify_center()
                        })
                        .child(track_name_view)
                        .child(utilities),
                )])
                .child(
                    div()
                        .id(("track-header-mixer", track_id.value()))
                        .when(group_plate, |mixer| {
                            mixer.h(px(TRACK_ROW_HEIGHT - 4.0)).justify_center()
                        })
                        // Two existing 60-point numeric minima and their gap.
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
                                .child(self.sample_arm_button(sample_source, cx))
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
                .w_full()
                .gap(px(1.0))
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
                .child(self.track_volume(track, cx))
                .child(
                    toggle_button_with_style(
                        "",
                        if track.muted() {
                            "unmute track"
                        } else {
                            "mute track"
                        },
                        ("track-mute", track_id.value()),
                        {
                            let (background, text) =
                                super::track_gain::activator_colors(!track.muted());
                            theme::action_style().with_state_colors(background, text)
                        },
                    )
                    .w(px(20.0))
                    .h(px(14.0))
                    .text_size(px(10.0))
                    .border_1()
                    .border_color(rgb(theme::control_style().border))
                    .child(theme::text((track_index + 1).to_string()))
                    .debug_selector(move || format!("track-mute-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        view.toggle_mute(track_id);
                        cx.notify();
                    })),
                )
                .child(
                    toggle_button_with_style(
                        "s",
                        "solo track · s · cmd+click additive",
                        ("track-solo", track_id.value()),
                        {
                            let (background, text) = super::track_gain::activator_colors(
                                self.displayed_solo(track_id, track.soloed()),
                            );
                            theme::action_style().with_state_colors(background, text)
                        },
                    )
                    .w(px(16.0))
                    .h(px(14.0))
                    .text_size(px(10.0))
                    .border_1()
                    .border_color(rgb(theme::control_style().border))
                    .debug_selector(move || format!("track-solo-{}", track_id.value()))
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
                )
                .child(self.sample_arm_button(sample_source, cx));
            div()
                .id(("streaming-track-controls", track_id.value()))
                .w_full()
                .flex()
                .flex_col()
                .children([name_area.child(track_name_view).w_full()])
                .child(track_controls)
        };
        let header_focus = self.track_header_focus(track_id, cx);
        let click_focus = header_focus.clone();
        let header_height = self.track_body_height(track_id);
        let expanded_header = header_height > TRACK_ROW_HEIGHT;
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
                    .left(px(depth as f32 * super::track_hierarchy_indent::RAIL_WIDTH))
                    .top_0()
                    .bottom_0()
                    .w(px(super::track_hierarchy_indent::RAIL_WIDTH))
                    .debug_selector(move || {
                        format!("track-group-rail-{}-{}", track_id.value(), ancestor.value())
                    })
                    .when(branch_ends(ancestor), |rail| {
                        rail.rounded_bl(px(super::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                    })
                    .bg(rgb(fill))
            })
            .collect::<Vec<_>>();
        let header = super::track_header::surface(
            ("arrangement-track-header", track_id.value()),
            self.track_header_width(),
            header_height,
            selected,
            None,
        )
        .when(expanded_header, |header| header.justify_start())
        .when(track.is_group() || hierarchy_indent > 0.0, |header| {
            header.border_b_0()
        })
        .when(group_plate, |header| header.pb_0())
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
                // Keep the compact content slot, excluding its bottom inset
                // and border, anchored to the top when the row grows.
                .when(expanded_header, |row| row.h(px(TRACK_ROW_HEIGHT - 4.0)))
                .when(group_plate, |row| row.h_full())
                .flex()
                .items_center()
                .when(group_expanded, |row| row.items_start())
                .gap(px(2.0))
                .children([header_content.flex_1().min_w_0()])
                .children(
                    self.track_performance_indicator(hex_engine::OutputCaptureId::Track(track_id)),
                )
                .children(
                    self.track_latency_indicator(super::track_latency::Owner::Track(track_id)),
                )
                .children([super::track_meters::render(
                    track_id,
                    self.track_meters.level(track_id),
                    header_height.min(TRACK_ROW_HEIGHT),
                )]),
        )
        .children(self.track_resize_handles(track_id, cx))
        .relative()
        .children(group_rails)
        .child(
            div()
                .absolute()
                .left(px(hierarchy_indent))
                .top(px(if track.is_group() || hierarchy_indent > 0.0 {
                    0.0
                } else {
                    1.0
                }))
                .bottom_0()
                .when(track.is_group(), |rail| {
                    rail.rounded_tl(px(super::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                })
                .when(track.is_group() && own_branch_ends, |rail| {
                    rail.rounded_bl(px(super::track_hierarchy_indent::RAIL_WIDTH / 2.0))
                })
                .w(px(4.0))
                .debug_selector(move || format!("track-color-strip-{}", track_id.value()))
                .bg(rgb(if track.is_group() {
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
        if self.automation_view.enabled {
            let width = self
                .drag_lanes
                .get(&track_id)
                .map_or(0.0, |lane| lane.width);
            if self.gain_lane.track == Some(track_id)
                && !self
                    .automation_view
                    .devices
                    .has_main(super::device_panel::DevicePanelOwner::Track(track_id))
            {
                clip_area = clip_area.children(super::gain_lane::inline(
                    self, state, track_id, viewport, width, cx,
                ));
            }
            clip_area = clip_area.children(super::device_automation::overlay(
                self,
                state,
                super::device_panel::DevicePanelOwner::Track(track_id),
                viewport,
                cx,
            ));
        }
        let row = div()
            .w_full()
            .h(px(self.track_body_height(track_id)))
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
            );
        let rows = super::device_automation::rows(
            self,
            state,
            super::device_panel::DevicePanelOwner::Track(track_id),
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

    pub(super) fn build_clip_contents(
        &self,
        state: &ViewState,
        track: &Track,
        track_index: usize,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        #[cfg(all(test, feature = "ui-test-support"))]
        let _navigation_timer =
            super::navigation_metrics::Timer::start(super::navigation_metrics::Stage::ClipModel);
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
        let mut fade_foreground = Vec::new();
        let mut selected_fade_foreground = Vec::new();
        let mut selected_edges = Vec::new();
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
            if hides_group_member(clip.id()) || !self.arrangement_clip_visible(clip.id()) {
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
                        self.track_body_height(track_id) <= super::track_layout::HEADER_ONLY_HEIGHT,
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
                selected_clip,
                ClipViewTimeline {
                    viewport,
                    grid,
                    scale_factor,
                },
                cx,
            );
            clip_area = clip_area.child(body);
            if selected_clip && self.audio_trim_display(track_id, Some(clip.id())).is_none() {
                let end = end.min(viewport_end);
                let id = clip.id();
                selected_edges.push(
                    super::clip_boundary::trailing_edge(
                        notch,
                        theme::color(theme::PaletteRole::Primary),
                        theme::color(theme::PaletteRole::Primary),
                    )
                    .debug_selector(move || format!("clip-selected-trailing-{}", id.value()))
                    .absolute()
                    .top_0()
                    .bottom(px(if loop_handles::audio_repeats(clip) {
                        loop_handles::BOTTOM_RADIUS
                    } else {
                        0.0
                    }))
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
        for clip in track.midi_clips() {
            if hides_group_member(clip.id()) || !self.arrangement_clip_visible(clip.id()) {
                continue;
            }
            let Some(preview) = state.midi_preview.get(clip.id()) else {
                continue;
            };
            let active = self.displayed_clip_active(clip.id(), clip.active());
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
                    let fragment = passive_midi_fragment(
                        self.drag_lanes
                            .get(&track_id)
                            .map_or(0.0, |lane| lane.width),
                        scale_factor,
                        theme::clip_activity_fill(track.color(), clip.flavor(), false, active),
                        clip.id(),
                        clip.name(),
                        clip.looping(),
                        |start, end, merge| {
                            self.loop_note_shapes(&state.project, clip, start, end, merge)
                        },
                        interval,
                        interval,
                        viewport,
                        viewport_end,
                        grid,
                        format!("time-move-midi-residual-{fragment_index}"),
                        self.track_body_height(track_id) <= super::track_layout::HEADER_ONLY_HEIGHT,
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
            let pixel_width = self
                .drag_lanes
                .get(&track_id)
                .map(|lane| super::trim::rendered_clip_width(lane.width, width))
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
                scale_factor,
                cx,
            );
            let midi_drag_payload =
                self.midi_drag_payload(state, track_id, clip, start, end, viewport);
            let header_fill =
                theme::clip_activity_fill(track.color(), clip.flavor(), false, active);
            let fill = theme::arrangement_clip_body_fill(header_fill, selected_clip);
            let stroke = 1.0 / scale_factor.max(1.0);
            let border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
            let notch = super::clip_boundary::Notch::new(border && visible_end == end);
            let mut midi = hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
                .id(("midi-clip", clip.id().value()))
                .debug_selector(|| format!("midi-clip-visual-{}", clip.id().value()))
                .cursor(gpui::CursorStyle::Arrow)
                .opacity(if trim_preview.is_some() { 0.0 } else { 1.0 })
                .left_0()
                .w_full()
                .border_l(px(if border { stroke } else { 0.0 }))
                .border_r(px(0.0))
                .bg(rgb(fill))
                .border_color(rgb(if selected_clip {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    theme::color(theme::PaletteRole::ClipOutline)
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
            if border {
                midi = midi
                    .bg(gpui::transparent_black())
                    .border_color(gpui::transparent_black());
            }
            midi = loop_handles::surface(midi, clip.looping());
            if self.track_body_height(track_id) > super::track_layout::HEADER_ONLY_HEIGHT {
                midi = midi.children(
                    aligned_clip_grid_layer(
                        grid,
                        visible_start,
                        visible_end,
                        1.0 / scale_factor.max(1.0),
                        hex_widgets::rectangle_border_width(pixel_width) > 0.0,
                    )
                    .map(|grid| loop_handles::body(grid, clip.looping())),
                );
                // Every visible repeat's notes come from the playback plan,
                // unsampled; same-pitch notes within one device pixel merge.
                let merge_frames = super::midi_loop_notes::merge_frames(
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
                    Err(_) => super::midi_loop_notes::visible_notes_tiled(
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
                    || super::midi_thumbnail::PitchFit::from_pitches([]),
                    |visible| visible.fit,
                );
                let note_shapes = visible
                    .iter()
                    .flat_map(|visible| visible.intervals.iter())
                    .filter_map(|&(start, end, pitch)| {
                        super::midi_thumbnail::note_shape(
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
                                        super::midi_loop_notes::paint_tiles(
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
                midi = midi.child(loop_handles::body(
                    clip_waveform_body(notes),
                    clip.looping(),
                ));
            }
            let header = if border {
                self.clip_name_header_on_backdrop(
                    track_id,
                    clip.id(),
                    clip.name(),
                    header_fill,
                    notch.enabled(),
                    cx,
                )
                .into_any_element()
            } else {
                self.clip_name_header(track_id, clip.id(), clip.name(), header_fill, cx)
                    .into_any_element()
            };
            midi = midi.child(header);
            midi = midi.when(clip.looping(), |midi| {
                midi.child(loop_handles::midi_markers(
                    clip,
                    None,
                    &state.project,
                    visible_start,
                    visible_end,
                ))
            });
            if let Some((payload, lane)) = midi_drag_payload {
                let border = hex_widgets::rectangle_border_width(pixel_width);
                let left_border = if border > 0.0 { stroke } else { 0.0 };
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
            let color = if selected_clip {
                theme::color(theme::PaletteRole::Primary)
            } else {
                theme::color(theme::PaletteRole::ClipOutline)
            };
            let bottom_radius = if clip.looping() {
                loop_handles::BOTTOM_RADIUS
            } else {
                0.0
            };
            let framed_clip = if border {
                super::clip_boundary::backdrop_shell(
                    midi,
                    left as f32,
                    width as f32,
                    super::clip_boundary::Backdrop {
                        body: fill,
                        header: header_fill,
                        outline: color,
                        hover_outline: color,
                        stroke,
                        bottom_radius,
                        header_height: super::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient: if trim_preview.is_some() {
                            None
                        } else if let Some(values) =
                            self.midi_header_loop_values(clip.id(), state.revision)
                        {
                            super::clip_header_loop::midi_source(
                                clip,
                                values,
                                state.project.tempo_map(),
                                visible_start..visible_end,
                                pixel_width * scale_factor,
                            )
                        } else {
                            super::clip_header_loop::midi(
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
                    bottom_radius,
                    move || format!("midi-clip-trailing-{}", midi_clip_id.value()),
                )
            };
            clip_area = clip_area.child(framed_clip);
            if selected_clip && trim_preview.is_none() {
                let id = clip.id();
                selected_edges.push(
                    super::clip_boundary::trailing_edge(
                        notch,
                        theme::color(theme::PaletteRole::Primary),
                        theme::color(theme::PaletteRole::Primary),
                    )
                    .debug_selector(move || format!("midi-clip-selected-trailing-{}", id.value()))
                    .absolute()
                    .top_0()
                    .bottom(px(if clip.looping() {
                        loop_handles::BOTTOM_RADIUS
                    } else {
                        0.0
                    }))
                    .left(relative(
                        (visible_end - viewport.start) as f32 / viewport.length as f32,
                    ))
                    .w(px(stroke)),
                );
            }
        }
        clip_area
            .children(fade_foreground)
            .children(selected_fade_foreground)
            .children(selected_edges)
    }

    #[allow(clippy::too_many_arguments)]
    fn clip_drag_content(
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
                // A group member is a projected copy; its notes are drawn
                // from the canonical clip it was captured from.
                let canonical = state.project.midi_clip(clip.id())?;
                let (shapes, fit) = self.loop_note_shapes(
                    &state.project,
                    canonical,
                    start,
                    end,
                    super::midi_loop_notes::merge_frames(end - start, pixel_width, scale),
                );
                let (notes, _) = midi_note_body(id, shapes, fit, true);
                content = content.child(clip_waveform_body(notes));
            }
        }
        Some(content.child(self.clip_name_drag_header(id, name, fill, on_backdrop)))
    }

    fn audio_clip_body(
        &self,
        state: &ViewState,
        clip: &Clip,
        visible_start: u64,
        visible_end: u64,
        grid: Option<super::waveform::WaveformGrid>,
        ink: u32,
    ) -> Div {
        let waveform = state.waveforms.get(&clip.asset());
        let body = if let Some(prepared) = state.audio_clip_waveforms.get(&clip.id()) {
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
            super::waveform::timeline_waveform_view_cached(
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
                ink,
            )
        } else if clip.audio_edit().is_some() {
            div()
                .absolute()
                .inset_0()
                .text_xs()
                .child(theme::text("prepared waveform unavailable"))
        } else {
            match waveform {
                Some(waveform) => super::waveform::timeline_waveform_view_cached(
                    std::sync::Arc::clone(waveform),
                    super::waveform_geometry::WaveformMapping::Clip {
                        clip: clip.clone(),
                        timeline_origin: clip.start(),
                        fade: Some(self.fade_preview(clip.id()).unwrap_or(clip.retained_fade())),
                    },
                    visible_start,
                    visible_end,
                    std::rc::Rc::clone(&self.waveform_geometry_cache),
                    grid,
                    ink,
                ),
                None => div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text("missing media")),
            }
        };
        super::waveform::clip_waveform_activity(
            body,
            self.displayed_clip_active(clip.id(), clip.active()),
        )
    }

    fn waveform_grid(
        &self,
        track: hex_project::TrackId,
        viewport: Viewport,
        scale: f32,
    ) -> Option<super::waveform::WaveformGrid> {
        let lane = self.drag_lanes.get(&track)?;
        super::waveform::WaveformGrid::arrangement(
            viewport,
            lane.left,
            lane.width,
            scale,
            self.state.as_ref()?.project.sample_rate(),
        )
    }

    fn clip_view(
        &self,
        state: &ViewState,
        track: hex_project::TrackId,
        clip: &Clip,
        selected: bool,
        timeline: ClipViewTimeline<'_>,
        cx: &mut Context<Self>,
    ) -> (gpui::AnyElement, Option<Div>, super::clip_boundary::Notch) {
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
            return (
                div().id(("clip", clip_id.value())).into_any_element(),
                None,
                Default::default(),
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
                Default::default(),
            );
        };
        let visible_start = display_clip.start().max(viewport.start);
        let visible_end = clip_end.min(viewport_end);
        if visible_start >= visible_end || viewport.length == 0 {
            return (
                div().id(("clip", clip_id.value())).into_any_element(),
                None,
                Default::default(),
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
                        snap_edge: std::rc::Rc::new(std::cell::Cell::new(None)),
                        source_span: Default::default(),
                        capture: std::rc::Rc::new(std::cell::Cell::new(None)),
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
        let fill = theme::arrangement_clip_body_fill(header_fill, selected);
        let clip_pixel_width = self.drag_lanes.get(&track).map_or(0.0, |lane| {
            super::trim::rendered_clip_width(lane.width, width)
        });
        let stroke = 1.0 / scale_factor.max(1.0);
        let border = hex_widgets::rectangle_border_width(clip_pixel_width) > 0.0;
        let notch = super::clip_boundary::Notch::new(border && visible_end == clip_end);
        let mut content =
            hex_widgets::rectangle_surface(left as f32, width as f32, clip_pixel_width)
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
                        rgb(theme::color(if selected {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Text
                        }))
                        .into()
                    })
                });
        content = content.border_color(rgb(if selected {
            theme::color(theme::PaletteRole::Primary)
        } else {
            theme::color(theme::PaletteRole::ClipOutline)
        }));
        if border {
            content = content
                .bg(gpui::transparent_black())
                .border_color(gpui::transparent_black());
        }
        let looping = loop_handles::audio_repeats(clip);
        content = loop_handles::surface(content, looping);
        if !active_trim_preview
            && self.track_height(track) > super::track_layout::HEADER_ONLY_HEIGHT
        {
            content = content.children(
                aligned_clip_grid_layer(grid, visible_start, visible_end, stroke, border)
                    .map(|grid| loop_handles::body(grid, looping)),
            );
            content = content.child(loop_handles::body(
                clip_waveform_body(self.audio_clip_body(
                    state,
                    clip,
                    visible_start,
                    visible_end,
                    self.waveform_grid(track, viewport, scale_factor),
                    waveform_ink,
                )),
                looping,
            ));
        }
        // Held/acknowledged fade editing paints after every clip body. Keep the
        // overlay in this lane so automation opacity and clipping still apply.
        let raise_fade = !self.automation_view.enabled
            && (self.fade_key_held || self.fade_curve_active(clip_id));
        let mut foreground = div().absolute().inset_0();
        let fade_preview = self.fade_preview(clip.id());
        let fade = fade_preview.unwrap_or(clip.retained_fade());
        let envelope_width = self
            .drag_lanes
            .get(&track)
            .map(|lane| super::trim::rendered_clip_width(lane.width, width))
            .unwrap_or(0.0);
        if !active_trim_preview
            && self.track_height(track) > super::track_layout::HEADER_ONLY_HEIGHT
            && envelope_width > 0.0
            && !fade.is_neutral()
        {
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
                let layer = loop_handles::body(
                    div().absolute().inset_0().child(clip_fade_layer(
                        samples,
                        self.fade_key_held || self.fade_curve_active(clip_id),
                    )),
                    looping,
                );
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
        let header = if border {
            self.clip_name_header_on_backdrop(
                track,
                clip.id(),
                clip.name(),
                header_fill,
                notch.enabled(),
                cx,
            )
            .into_any_element()
        } else {
            self.clip_name_header(track, clip.id(), clip.name(), header_fill, cx)
                .into_any_element()
        };
        let mut content = content
            .child(header)
            .when(!active_trim_preview && looping, |content| {
                content.child(loop_handles::audio_markers(
                    clip,
                    &state.project,
                    visible_start,
                    visible_end,
                ))
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
            let border = hex_widgets::rectangle_border_width(clip_pixel_width);
            let left_border = if border > 0.0 { stroke } else { 0.0 };
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
                        super::trim::handle_width(clip_pixel_width)
                    } else {
                        0.0
                    }))
                .right(px(if !trim_handles.is_empty() && visible_end == clip_end {
                    super::trim::handle_width(clip_pixel_width)
                } else {
                    0.0
                }));
            content = content.children([move_surface]).children(trim_handles);
        }
        if !active_trim_preview {
            content = content.children(self.clip_gain_handle(
                state,
                track,
                clip,
                clip_pixel_width,
                viewport,
                cx,
            ));
        }
        if !active_trim_preview
            && (self.track_height(track) > super::track_layout::HEADER_ONLY_HEIGHT
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
        let color = if selected {
            theme::color(theme::PaletteRole::Primary)
        } else {
            theme::color(theme::PaletteRole::ClipOutline)
        };
        let hover = theme::color(if selected {
            theme::PaletteRole::Primary
        } else {
            theme::PaletteRole::Text
        });
        (
            if border {
                super::clip_boundary::backdrop_shell(
                    content,
                    left as f32,
                    width as f32,
                    super::clip_boundary::Backdrop {
                        body: fill,
                        header: header_fill,
                        outline: color,
                        hover_outline: hover,
                        stroke,
                        bottom_radius: if looping {
                            loop_handles::BOTTOM_RADIUS
                        } else {
                            0.0
                        },
                        header_height: super::clip_name::CLIP_NAME_HEIGHT,
                        header_gradient: if !active_trim_preview {
                            super::clip_header_loop::audio(
                                &self.audio_header_loop_clip(clip, state),
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
                    if looping {
                        loop_handles::BOTTOM_RADIUS
                    } else {
                        0.0
                    },
                    move || format!("clip-trailing-{}", clip_id.value()),
                )
            }
            .into_any_element(),
            foreground,
            notch,
        )
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

    pub(super) fn floating_clip_drag_ghost(&self, scale: f32) -> Option<Div> {
        let payload = self.displayed_clip_drag()?;
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
                    looping: clip_repeats(&state.project, payload.clip),
                    notched: visible_end == preview.end_frame,
                    fill: theme::clip_activity_fill(
                        hex_project::ElementColor::default(),
                        state.project.clip_flavor(payload.clip).unwrap_or_default(),
                        false,
                        state.project.clip_active(payload.clip).unwrap_or(true),
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
                                    state.project.clip_active(payload.clip).unwrap_or(true),
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
                looping: self
                    .state
                    .as_ref()
                    .is_some_and(|state| clip_repeats(&state.project, payload.clip)),
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
                    self.state
                        .as_ref()
                        .and_then(|state| state.project.clip_active(payload.clip))
                        .unwrap_or(true),
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
                            state.project.clip_active(payload.clip).unwrap_or(true),
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
                snap_edge: std::rc::Rc::new(std::cell::Cell::new(None)),
                source_span: Default::default(),
                capture: std::rc::Rc::new(std::cell::Cell::new(None)),
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
        let element_width = super::trim::rendered_clip_width(lane.width, width);
        let element_left = lane.left + (f64::from(lane.width) * left) as f32;
        let drag_entity = cx.entity();
        let mouse_down_entity = drag_entity.clone();
        let mouse_up_entity = drag_entity.clone();
        let move_entity = drag_entity.clone();
        let source_track = payload.source_track;
        let source_generation = payload.project_generation;
        let mouse_down_payload = payload.clone();
        let mouse_up_payload = payload.clone();
        let border = hex_widgets::rectangle_border_width(element_width);
        let header_height = super::time_selection::header_hit_height(lane.height);
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
                    view.clear_pointer_down(
                        mouse_up_payload.project_generation,
                        mouse_up_payload.clip,
                        None,
                    );
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
            .on_drag(payload, move |payload, _, window, cx| {
                let capture = drag_entity.update(cx, |view, _| {
                    if view.drag_start_is_current(payload) {
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

    #[gpui::test]
    fn audio_and_midi_grid_paint_stays_below_titles_on_shared_lane_columns(
        cx: &mut gpui::TestAppContext,
    ) {
        let (project, track, _, audio) = audio_project();
        let asset = project.clip(audio).unwrap().asset();
        let mut canonical = Editor::new(project, 16).unwrap();
        let hex_project::Created::Clip(midi) = canonical
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(4).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        let media = std::collections::BTreeMap::from([(
            asset,
            hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
        )]);
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(canonical.project(), &media).unwrap();
        let (handle, editor) = editor_window(
            cx,
            canonical.project().clone(),
            std::sync::Arc::new(preview),
        );
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(Viewport {
                start: 0,
                length: 192_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let scale = visual.update(|window, _| window.scale_factor());
        for (clip, frame) in [(audio, 48_000), (midi, 144_000)] {
            let selector =
                |prefix| Box::leak(format!("{prefix}-{frame}").into_boxed_str()) as &'static str;
            let header = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
            let header = visual.debug_bounds(header).unwrap().scale(scale);
            let grid_selector = selector("clip-grid-line");
            let grid = visual.debug_bounds(grid_selector).unwrap().scale(scale);
            let lane = visual
                .debug_bounds(selector("arrangement-grid-line"))
                .unwrap()
                .scale(scale);
            assert!(
                grid.top() >= header.bottom(),
                "grid paint must not cross the transparent title or its corner cut"
            );
            assert_eq!(
                grid.left(),
                lane.left(),
                "body restriction preserves physical timeline columns"
            );
            assert_eq!(f64::from(grid.size.width), 1.0);
            assert!(
                visual.rendered_surface_geometry(grid_selector).is_some(),
                "actual completed scene contains the grid stroke"
            );
        }
    }

    struct ThumbnailOracle(hex_project::ClipId);
    impl gpui::Render for ThumbnailOracle {
        fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
            // A five-row fitted range: thick rows, top and bottom at the body edges.
            let shapes = [
                (0.0, 0.1, 64),
                (0.13, 0.31, 60),
                (0.57, 0.005, 62),
                (0.9, 0.1, 60),
            ];
            let fit = super::super::midi_thumbnail::PitchFit::from_pitches([64, 60, 62]);
            let mut legacy = div().absolute().inset_0();
            for (x, width, pitch) in shapes {
                let (top, height) =
                    super::super::midi_thumbnail::note_rows(fit, pitch, 73.25).unwrap();
                legacy = legacy.child(
                    div()
                        .absolute()
                        .left(relative(x))
                        .top(px(top))
                        .w(relative(width))
                        .h(px(height))
                        .bg(rgb(0x00ff00)),
                );
            }
            div().size_full().child(
                div()
                    .debug_selector(|| "thumbnail-oracle".into())
                    .absolute()
                    .left(px(13.25))
                    .top(px(37.25))
                    .w(px(193.25))
                    .h(px(73.25))
                    .overflow_hidden()
                    .children([legacy, midi_note_body(self.0, shapes, fit, false).0]),
            )
        }
    }

    #[gpui::test]
    fn batched_note_quads_match_fitted_layout_rows_and_body_clipping(
        cx: &mut gpui::TestAppContext,
    ) {
        let clip = audio_project().3;
        let handle = cx.add_window(|_, _| ThumbnailOracle(clip));
        let mut visual = gpui::VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let quads = visual.rendered_filled_quads("thumbnail-oracle");
        let rectangles = |color| {
            let mut found: Vec<_> = quads
                .iter()
                .filter(|(_, fill)| *fill == gpui::Background::from(rgb(color)))
                .map(|(bounds, _)| *bounds)
                .collect();
            found.sort_by_key(|a| a.left());
            found
        };
        let old = rectangles(0x00ff00);
        let new = rectangles(0x000000);
        assert_eq!(old.len(), 4);
        assert_eq!(new.len(), old.len());
        for (old, new) in old.iter().zip(new) {
            // Taffy rounds using its unrounded ancestor offsets; the canvas
            // receives the rounded body. They may differ by one physical pixel.
            for (old, new) in [
                (old.left(), new.left()),
                (old.right(), new.right()),
                (old.top(), new.top()),
                (old.bottom(), new.bottom()),
            ] {
                assert!(
                    (f64::from(old - new)).abs() <= 1.0,
                    "old={old:?}, new={new:?}"
                );
            }
        }
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
        let deadline = crate::test_wait::hang_deadline();
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
        let (original, original_fill) =
            super::super::midi_thumbnail::rendered_notes(&mut visual, clip, false)
                .into_iter()
                .next()
                .expect("normal production note quad");
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
        super::super::clip_boundary::tests::assert_open_upper_right(
            &mut visual,
            "clip-drop-preview",
            "clip-drop-preview-trailing",
            Some(Box::leak(
                format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
            )),
        );
        let (actual, fill) = super::super::midi_thumbnail::rendered_notes(&mut visual, clip, true)
            .into_iter()
            .next()
            .expect("held target production MIDI note quad");
        assert_eq!(fill, original_fill);
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
        let deadline = crate::test_wait::hang_deadline();
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
            if selector == "clip-drag-floating-preview" {
                super::super::clip_boundary::tests::assert_open_upper_right(
                    &mut visual,
                    selector,
                    "clip-drag-floating-preview-trailing",
                    Some(Box::leak(
                        format!("clip-drag-name-{}", clip.value()).into_boxed_str(),
                    )),
                );
                let (corners, _) = visual.rendered_surface_geometry(selector).unwrap();
                assert_eq!(
                    corners.bottom_left > gpui::ScaledPixels::default(),
                    loop_handles::audio_repeats(&source),
                    "floating source keeps its loop-bottom tier"
                );
            }

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
            let (note, _) = super::super::midi_thumbnail::rendered_notes(&mut visual, midi, true)
                .into_iter()
                .next()
                .expect("actual mixed-group note quad");
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
