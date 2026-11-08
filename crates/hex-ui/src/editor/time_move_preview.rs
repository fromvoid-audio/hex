//! Passive time-chunk fragments retain their timeline bounds and clip shell.
//! Passive titles and contours share the canonical clip paint owner. Compact
//! rows skip body construction; other rows use cached waveform/note summaries.
//! Inputs remain prepared display data, never edits.
use super::tracks::aligned_clip_grid_layer;
use super::trim::loop_handles;
use super::viewport::Viewport;
use crate::theme;
use gpui::{Div, div, prelude::*, px, rgb};
use hex_project::Clip;

pub(super) fn move_exclusions(
    display: Option<&super::time_selection::TimeRangeMoveDisplay<'_>>,
    track: hex_project::TrackId,
    track_index: usize,
) -> [Option<(u64, u64)>; 2] {
    let Some(display) = display else {
        return [None, None];
    };
    let source = (!display.copy_modifier && display.source.tracks().contains(&track))
        .then_some((display.source.start(), display.source.end()));
    let destination = (track_index >= display.destination_first_index
        && track_index <= display.destination_last_index)
        .then_some((display.destination_start, display.destination_end));
    [source, destination]
}

pub(super) fn clip_intersects_exclusions(clip: &Clip, exclusions: [Option<(u64, u64)>; 2]) -> bool {
    let Some(end) = clip.start().checked_add(clip.duration()) else {
        return false;
    };
    exclusions
        .into_iter()
        .flatten()
        .any(|(start, stop)| start < end && stop > clip.start())
}

pub(super) fn clip_fragments_outside(
    clip: &Clip,
    exclusions: [Option<(u64, u64)>; 2],
    tempo: &hex_project::TempoMap,
) -> [Option<Clip>; 3] {
    let Some(clip_end) = clip.start().checked_add(clip.duration()) else {
        return [None, None, None];
    };
    let intervals = time_fragments_outside(clip.start(), clip_end, exclusions);
    let mut result = [None, None, None];
    for (index, interval) in intervals.into_iter().enumerate() {
        if let Some((start, end)) = interval {
            result[index] =
                hex_project::preview_clip_fragment_with_tempo(clip, start, end, start, tempo).ok();
        }
    }
    result
}

pub(super) fn time_fragments_outside(
    start: u64,
    end: u64,
    exclusions: [Option<(u64, u64)>; 2],
) -> [Option<(u64, u64)>; 3] {
    let mut intervals = [None, None];
    let mut interval_count = 0;
    for (exclude_start, exclude_end) in exclusions.into_iter().flatten() {
        let exclude_start = exclude_start.max(start);
        let exclude_end = exclude_end.min(end);
        if exclude_start < exclude_end && interval_count < intervals.len() {
            intervals[interval_count] = Some((exclude_start, exclude_end));
            interval_count += 1;
        }
    }
    if interval_count == 2
        && intervals[0].is_some_and(|left| intervals[1].is_some_and(|right| right.0 < left.0))
    {
        intervals.swap(0, 1);
    }
    let mut result = [None, None, None];
    let mut count = 0;
    let mut cursor = start;
    for (from, to) in intervals.into_iter().flatten() {
        if to <= cursor {
            continue;
        }
        if from > cursor && count < result.len() {
            result[count] = Some((cursor, from));
            count += 1;
        }
        cursor = cursor.max(to);
    }
    if cursor < end && count < result.len() {
        result[count] = Some((cursor, end));
    }
    result
}

pub(super) fn clip_intersection(clip: &Clip, interval: (u64, u64)) -> Option<(u64, u64)> {
    let end = clip.start().checked_add(clip.duration())?;
    let start = clip.start().max(interval.0);
    let stop = end.min(interval.1);
    (start < stop).then_some((start, stop))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn passive_audio_fragment(
    lane_width: f32,
    scale: f32,
    fill: u32,
    clip: &Clip,
    waveform: Option<&std::sync::Arc<hex_session::CachedWaveform>>,
    viewport: Viewport,
    viewport_end: u64,
    grid: &[super::musical_time::GridLine],
    cache: std::rc::Rc<std::cell::RefCell<super::waveform_geometry::WaveformGeometryCache>>,
    selector: String,
    header_only: bool,
) -> Option<Div> {
    let end = clip.start().checked_add(clip.duration())?;
    let start = clip.start().max(viewport.start);
    let stop = end.min(viewport_end);
    if start >= stop || viewport.length == 0 {
        return None;
    }
    let left = (start - viewport.start) as f64 / viewport.length as f64;
    let width = (stop - start) as f64 / viewport.length as f64;
    let pixel_width = super::trim::rendered_clip_width(lane_width, width);
    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
    let stroke = 1.0 / scale.max(1.0);
    let looping = loop_handles::audio_repeats(clip);
    let header_selector = format!("{selector}-name");
    let mut content = hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
        .left_0()
        .w_full()
        .border_l(px(if has_border { stroke } else { 0.0 }))
        .border_r(px(0.0))
        .debug_selector(move || selector.clone())
        .border_color(rgb(theme::color(theme::PaletteRole::ClipOutline)))
        .bg(rgb(fill))
        .when(has_border, |surface| {
            surface
                .bg(gpui::transparent_black())
                .border_color(gpui::transparent_black())
        });
    if !header_only {
        content = content.children(
            aligned_clip_grid_layer(grid, start, stop, stroke, has_border)
                .map(|grid| loop_handles::body(grid, looping)),
        );
        let body = if let Some(waveform) = waveform {
            super::waveform::waveform_view_cached(
                waveform,
                clip,
                start,
                stop,
                cache,
                theme::waveform_ink_for_fill(fill),
            )
        } else {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .px_1()
                .text_xs()
                .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .child(theme::text("missing media"))
        };
        content = content.child(loop_handles::body(
            super::tracks::clip_waveform_body(body),
            looping,
        ));
    }
    content = content.child(
        super::clip_name::passive_name_header(clip.name(), fill, has_border)
            .debug_selector(move || header_selector.clone()),
    );
    let color = theme::color(theme::PaletteRole::ClipOutline);
    Some(if has_border {
        super::clip_boundary::backdrop_shell(
            content,
            left as f32,
            width as f32,
            super::clip_boundary::Backdrop {
                body: fill,
                header: fill,
                outline: color,
                hover_outline: color,
                stroke,
                bottom_radius: if looping {
                    loop_handles::BOTTOM_RADIUS
                } else {
                    0.0
                },
                header_height: super::clip_name::CLIP_NAME_HEIGHT,
                notch: super::clip_boundary::Notch::new(stop == end),
            },
            true,
            || "passive-audio-trailing".into(),
        )
    } else {
        super::clip_boundary::clip_boundary_shell(
            content,
            left as f32,
            width as f32,
            stroke,
            color,
            color,
            true,
            false,
            0.0,
            || "passive-audio-trailing".into(),
        )
    })
}

/// A passive MIDI fragment showing canonical `source_interval` frames at
/// `timeline_interval`. `notes(start, end, merge_frames)` returns the shapes
/// and row fit of the canonical source frames `[start, end)` that land in the
/// visible part of this fragment, so every visible repeat stays inside it.
#[allow(clippy::too_many_arguments)]
pub(super) fn passive_midi_fragment(
    lane_width: f32,
    scale: f32,
    fill: u32,
    clip_id: hex_project::ClipId,
    name: &str,
    looping: bool,
    notes: impl FnOnce(u64, u64, u64) -> (Vec<(f32, f32, u8)>, super::midi_thumbnail::PitchFit),
    source_interval: (u64, u64),
    timeline_interval: (u64, u64),
    viewport: Viewport,
    viewport_end: u64,
    grid: &[super::musical_time::GridLine],
    selector: String,
    header_only: bool,
) -> Option<Div> {
    let visible_start = timeline_interval.0.max(viewport.start);
    let visible_end = timeline_interval.1.min(viewport_end);
    if visible_start >= visible_end || viewport.length == 0 {
        return None;
    }
    let left = (visible_start - viewport.start) as f64 / viewport.length as f64;
    let width = (visible_end - visible_start) as f64 / viewport.length as f64;
    let pixel_width = super::trim::rendered_clip_width(lane_width, width);
    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
    let stroke = 1.0 / scale.max(1.0);
    let notes_selector = format!("{selector}-notes-{}", clip_id.value());
    let header_selector = format!("{selector}-name-{}", clip_id.value());
    let mut content = hex_widgets::rectangle_surface(left as f32, width as f32, pixel_width)
        .left_0()
        .w_full()
        .border_l(px(if has_border { stroke } else { 0.0 }))
        .border_r(px(0.0))
        .debug_selector(move || format!("{selector}-{}", clip_id.value()))
        .bg(rgb(fill))
        .border_color(rgb(theme::color(theme::PaletteRole::ClipOutline)))
        .when(has_border, |surface| {
            surface
                .bg(gpui::transparent_black())
                .border_color(gpui::transparent_black())
        });
    if !header_only {
        content = content.children(
            aligned_clip_grid_layer(grid, visible_start, visible_end, stroke, has_border)
                .map(|grid| loop_handles::body(grid, looping)),
        );
        // The visible destination maps frame-for-frame onto canonical source
        // frames; shapes are fractions of that window, so they land unchanged.
        let source_start = source_interval
            .0
            .checked_add(visible_start - timeline_interval.0);
        let source_end = source_start
            .and_then(|start| start.checked_add(visible_end - visible_start))
            .map(|end| end.min(source_interval.1));
        if let (Some(start), Some(end)) = (source_start, source_end)
            && start < end
        {
            let (shapes, fit) = notes(
                start,
                end,
                super::midi_loop_notes::merge_frames(end - start, pixel_width, scale),
            );
            let (notes, _) =
                super::tracks::midi_note_canvas(move || notes_selector.clone(), shapes, fit);
            content = content.child(loop_handles::body(
                super::tracks::clip_waveform_body(notes),
                looping,
            ));
        }
    }
    content = content.child(
        super::clip_name::passive_name_header(name, fill, has_border)
            .debug_selector(move || header_selector.clone()),
    );
    let color = theme::color(theme::PaletteRole::ClipOutline);
    Some(if has_border {
        super::clip_boundary::backdrop_shell(
            content,
            left as f32,
            width as f32,
            super::clip_boundary::Backdrop {
                body: fill,
                header: fill,
                outline: color,
                hover_outline: color,
                stroke,
                bottom_radius: if looping {
                    loop_handles::BOTTOM_RADIUS
                } else {
                    0.0
                },
                header_height: super::clip_name::CLIP_NAME_HEIGHT,
                notch: super::clip_boundary::Notch::new(visible_end == timeline_interval.1),
            },
            true,
            || "passive-midi-trailing".into(),
        )
    } else {
        super::clip_boundary::clip_boundary_shell(
            content,
            left as f32,
            width as f32,
            stroke,
            color,
            color,
            true,
            false,
            0.0,
            || "passive-midi-trailing".into(),
        )
    })
}

#[cfg(test)]
mod time_move_interval_tests {
    use super::time_fragments_outside;

    #[test]
    fn overlapping_source_and_destination_exclusions_leave_only_the_union_tail() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [Some((0, 48_000)), Some((24_000, 72_000))]),
            [Some((72_000, 96_000)), None, None]
        );
    }

    #[test]
    fn destination_replacement_preserves_both_outside_tails() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [None, Some((24_000, 72_000))]),
            [Some((0, 24_000)), Some((72_000, 96_000)), None]
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod visual_tests {
    use super::*;
    use crate::editor::clip_drag_ui_test::audio_project;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, Window};
    use std::{cell::Cell, rc::Rc};

    struct Fragments {
        audio: Clip,
        midi: hex_project::ClipId,
        name: String,
        compact: bool,
        cropped: bool,
        mapped: Rc<Cell<Option<(u64, u64)>>>,
    }
    impl Render for Fragments {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let length = if self.cropped { 48_000 } else { 192_000 };
            let viewport = Viewport { start: 0, length };
            let scale = window.scale_factor();
            let mapped = Rc::clone(&self.mapped);
            div()
                .flex()
                .flex_col()
                .w(px(300.0))
                .h(px(88.0))
                .child(
                    div()
                        .relative()
                        .w(px(300.0))
                        .h(px(44.0))
                        .children(passive_audio_fragment(
                            300.0,
                            scale,
                            0x856050,
                            &self.audio,
                            None,
                            viewport,
                            length,
                            &[],
                            Rc::new(std::cell::RefCell::new(Default::default())),
                            "fragment-audio".into(),
                            self.compact,
                        )),
                )
                .child(
                    div()
                        .relative()
                        .w(px(300.0))
                        .h(px(44.0))
                        .children(passive_midi_fragment(
                            300.0,
                            scale,
                            0x856050,
                            self.midi,
                            &self.name,
                            false,
                            move |start, end, _| {
                                mapped.set(Some((start, end)));
                                (
                                    vec![(0.0, 0.5, 60)],
                                    super::super::midi_thumbnail::PitchFit::from_pitches([60]),
                                )
                            },
                            (0, 96_000),
                            (0, 96_000),
                            viewport,
                            length,
                            &[],
                            "fragment-midi".into(),
                            self.compact,
                        )),
                )
        }
    }
    #[gpui::test]
    fn fragment_titles_cuts_crops_and_compact_body_preserve_source_mapping(
        cx: &mut TestAppContext,
    ) {
        use hex_project::{BeatTime, Command, Created, Editor};
        let (project, _, track, audio) = audio_project();
        let mut canonical = Editor::new(project, 16).unwrap();
        let Created::Clip(midi) = canonical
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        let audio = canonical.project().clip(audio).unwrap().clone();
        let name = canonical
            .project()
            .midi_clip(midi)
            .unwrap()
            .name()
            .to_owned();
        for compact in [false, true] {
            for cropped in [false, true] {
                let mapped = Rc::new(Cell::new(None));
                let oracle = Fragments {
                    audio: audio.clone(),
                    midi,
                    name: name.clone(),
                    compact,
                    cropped,
                    mapped: Rc::clone(&mapped),
                };
                let handle = *cx.add_window(move |_, _| oracle);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let midi_root =
                    Box::leak(format!("fragment-midi-{}", midi.value()).into_boxed_str());
                let midi_name =
                    Box::leak(format!("fragment-midi-name-{}", midi.value()).into_boxed_str());
                let scale = visual.update(|window, _| window.scale_factor());
                for (root, header, tail) in [
                    (
                        "fragment-audio",
                        "fragment-audio-name",
                        "passive-audio-trailing",
                    ),
                    (midi_root, midi_name, "passive-midi-trailing"),
                ] {
                    let bounds = visual.debug_bounds(root).unwrap();
                    let title = visual.debug_bounds(header).unwrap();
                    assert_eq!(
                        title.size.height,
                        px(super::super::clip_name::CLIP_NAME_HEIGHT)
                    );
                    if cropped {
                        let corner = gpui::point(bounds.right() - px(0.25), bounds.top() + px(2.0))
                            .scale(scale);
                        assert!(
                            visual
                                .rendered_quad_paint_layers(root, false)
                                .iter()
                                .any(|(_, mask)| mask.contains(&corner)),
                            "viewport crop remains square and filled"
                        );
                    } else {
                        super::super::clip_boundary::tests::assert_open_upper_right(
                            &mut visual,
                            root,
                            tail,
                            Some(header),
                        );
                    }
                }
                assert_eq!(
                    mapped.get(),
                    (!compact).then_some((0, if cropped { 48_000 } else { 96_000 }))
                );
                let notes =
                    Box::leak(format!("fragment-midi-notes-{}", midi.value()).into_boxed_str());
                if compact {
                    assert!(visual.debug_bounds(notes).is_none());
                } else {
                    let notes = visual.debug_bounds(notes).unwrap();
                    assert!(
                        notes.top() >= visual.debug_bounds(midi_name).unwrap().bottom(),
                        "notes remain in the body below the title"
                    );
                }
            }
        }
    }
}
