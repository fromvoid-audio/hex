//! Border assertions read completed GPUI scene quads, including retained scenes.
use crate::editor::{
    EditorView,
    clip_drag_ui_test::{audio_project, editor_window},
    viewport::Viewport,
};
use crate::theme;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipId, Command, Created, Editor};
use std::sync::Arc;
fn click(visual: &mut VisualTestContext, clip: ClipId, modifiers: Modifiers) {
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, modifiers);
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn selected_audio_midi_borders_cover_visible_bounds_and_restore_each_member(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, track, _, imported) = audio_project();
    let asset = project.clip(imported).unwrap().asset();
    let mut canonical = Editor::new(project, 16).unwrap();
    canonical.execute(Command::RemoveClip(imported)).unwrap();
    let mut clips = Vec::new();
    for i in 0..5 {
        let command = if i % 2 == 0 {
            Command::PlaceClip {
                track,
                asset,
                start: i * 36_000,
                duration: 24_000,
                source_offset: 0,
                source_length: 24_000,
                looping: false,
            }
        } else {
            Command::PlaceMidiClip {
                track,
                start: canonical
                    .project()
                    .tempo_map()
                    .frame_to_beat(i * 36_000)
                    .unwrap(),
                length: BeatTime::ONE,
            }
        };
        let Created::Clip(id) = canonical.execute(command).unwrap() else {
            panic!("clip")
        };
        clips.push((
            id,
            if i % 2 == 0 {
                "clip-visual"
            } else {
                "midi-clip-visual"
            },
        ));
    }
    let original = canonical.project().clone();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
    )]);
    let (_, preview) = hex_engine::compile::project_with_midi_preview(&original, &media)
        .expect("real prepared mixed-clip preview");
    let (handle, editor) = editor_window(cx, original.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(Viewport {
            start: 18_000,
            length: 144_000,
        });
        view.snap_grid = crate::editor::musical_time::SnapGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    // Inject after the editor loads its stored theme; a non-black role
    // proves every unselected outline reads the palette.
    let _outline = theme::inject_test_clip_outline(0x2a4b6c);
    editor.update(&mut visual, |_, cx| cx.notify());
    visual.run_until_parked();
    let selectors = clips
        .iter()
        .map(|(id, prefix)| {
            Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str()) as &'static str
        })
        .collect::<Vec<_>>();
    let bounds = selectors
        .iter()
        .map(|s| visual.debug_bounds(s).unwrap())
        .collect::<Vec<_>>();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    assert_eq!(
        bounds[0].left(),
        lane.left(),
        "left clipped clip starts at viewport"
    );
    // The outer row reserves its right divider; frame projection uses the
    // independently captured inner timeline width.
    let timeline = editor.read_with(&visual, |view, _| view.drag_lanes[&track]);
    assert!(
        (f32::from(bounds[4].right()) - (timeline.left + timeline.width)).abs() < 0.1,
        "right clipped clip ends at viewport"
    );
    click(&mut visual, clips[0].0, Modifiers::default());
    click(
        &mut visual,
        clips[4].0,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    let primary = editor.read_with(&visual, |view, _| view.selected_clip);
    for toggled in [false, true] {
        if toggled {
            click(
                &mut visual,
                clips[2].0,
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            );
            // Inspect deselection away from hover; unselected hover keeps its
            // existing separate affordance. The first pass also covers a
            // selected clip under the pointer.
            visual.simulate_mouse_move(
                gpui::point(lane.right() - gpui::px(4.0), lane.bottom() - gpui::px(4.0)),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
        }
        for (index, ((id, _), selector)) in clips.iter().zip(&selectors).enumerate() {
            let selected = !(toggled && index == 2);
            let (color, widths, mask) = visual
                .rendered_border(selector)
                .expect("actual painted border");
            assert_eq!(
                color,
                gpui::Hsla::from(gpui::rgb(if selected {
                    theme::color(theme::PaletteRole::Primary)
                } else {
                    theme::color(theme::PaletteRole::ClipOutline)
                })),
                "{id:?} selected border"
            );
            let zero = gpui::ScaledPixels::default();
            assert!(
                widths.top > zero
                    && widths.bottom > zero
                    && widths.left > zero
                    && widths.right == zero
            );
            let current = visual.debug_bounds(selector).unwrap();
            assert_eq!(
                current, bounds[index],
                "selection does not change clip geometry"
            );
            assert!(mask.size.width > zero && mask.size.height > zero);
            let edge_prefix = if clips[index].1 == "midi-clip-visual" {
                "midi-clip"
            } else {
                "clip"
            };
            let edge_name = Box::leak(
                format!(
                    "{edge_prefix}-{}trailing-{}",
                    if selected { "selected-" } else { "" },
                    id.value()
                )
                .into_boxed_str(),
            );
            let trailing = visual.debug_bounds(edge_name).unwrap();
            assert_eq!(trailing.left(), current.right());
            let scale = visual.update(|window, _| window.scale_factor());
            assert_eq!(f64::from(trailing.scale(scale).size.width), 1.0);
            if index != 4 {
                let expected = (current.top() + gpui::px(5.0)).scale(scale);
                let layers = visual.rendered_quad_paint_layers(edge_name, false);
                assert!(!layers.is_empty());
                assert!(
                    layers.iter().all(|(_, mask)| mask.top() >= expected),
                    "raised edge must leave the corner open"
                );
                let body_color = visual.rendered_background(selector).unwrap();
                let corner = gpui::point(
                    current.right() - gpui::px(0.25),
                    current.top() + gpui::px(0.25),
                )
                .scale(scale);
                assert!(
                    visual
                        .rendered_filled_quads(selector)
                        .iter()
                        .filter(|(_, fill)| *fill == body_color)
                        .all(|(bounds, _)| !bounds.contains(&corner)),
                    "the cut must be transparent, not a covering triangle"
                );
            }
            let fill = editor.read_with(&visual, |view: &EditorView, _| {
                let source = view.state.as_ref().unwrap().project.track(track).unwrap();
                let flavor = source
                    .clips()
                    .iter()
                    .find(|c| c.id() == *id)
                    .map(|c| c.flavor())
                    .unwrap_or_else(|| {
                        source
                            .midi_clips()
                            .iter()
                            .find(|c| c.id() == *id)
                            .unwrap()
                            .flavor()
                    });
                theme::arrangement_clip_body_fill(
                    theme::clip_fill(source.color(), flavor, false),
                    selected,
                )
            });
            assert_eq!(
                visual.rendered_background(selector),
                Some(gpui::rgb(fill).into())
            );
        }
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_clip, primary, "membership-only invalidation");
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
        assert!(view.last_enqueued_edit.is_none());
    });
    // The selected border also follows the real held edge preview, and escape
    // restores the original visible geometry without submitting a model edit.
    let audio = clips[2].0;
    click(&mut visual, audio, Modifiers::default());
    let edge = visual
        .debug_bounds(Box::leak(
            format!("clip-trim-right-{}", audio.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let moved = gpui::point(edge.x - gpui::px(8.0), edge.y);
    visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual
            .rendered_border("trim-live-preview")
            .expect("held edge preview")
            .0,
        gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Primary)))
    );
    let trim = visual.debug_bounds("trim-live-preview").unwrap();
    let trailing = visual.debug_bounds("trim-preview-trailing").unwrap();
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(trailing.left(), trim.right());
    assert_eq!(f64::from(trailing.scale(scale).size.width), 1.0);
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("trim-live-preview").is_none());
    assert_eq!(visual.debug_bounds(selectors[2]), Some(bounds[2]));
    editor.read_with(&visual, |view, _| {
        assert!(view.active_trim_drag.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
        assert!(view.last_enqueued_edit.is_none());
    });
}

type BorderFixture = (
    crate::editor::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    hex_project::TrackId,
    Vec<(ClipId, &'static str)>,
);

/// Private bounded startup: no ambient browser/catalog or shared fixture changes.
fn bottom_border_fixture(cx: &mut TestAppContext) -> BorderFixture {
    use gpui::AppContext;
    use hex_session::service::{Event, Request, Service};
    fn next(service: &Service) -> Event {
        let until = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < until,
                "next: hang guard expired"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    cx.update(crate::editor::bind_editor_keys);
    let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("border.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let source = &event.state.project.tracks()[0];
    let track = source.id();
    let asset = source.clips()[0].asset();
    service
        .submit(Request::Edit(Command::RemoveClip(source.clips()[0].id())))
        .unwrap();
    let mut event = next(&service);
    let mut clips = Vec::new();
    for i in 0..5 {
        let command = if i % 2 == 0 {
            Command::PlaceClip {
                track,
                asset,
                start: i * 36_000,
                duration: 24_000,
                source_offset: 0,
                source_length: 24_000,
                looping: false,
            }
        } else {
            Command::PlaceMidiClip {
                track,
                start: BeatTime::from_raw(i * 3 * (1 << 31)),
                length: BeatTime::ONE,
            }
        };
        service.submit(Request::Edit(command)).unwrap();
        event = next(&service);
        let Created::Clip(id) = event.created.unwrap() else {
            panic!("clip");
        };
        clips.push((
            id,
            if i % 2 == 0 {
                "clip-visual"
            } else {
                "midi-clip-visual"
            },
        ));
    }
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.viewport = Some(Viewport {
                    start: 18_000,
                    length: 144_000,
                });
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, track, clips)
}

#[gpui::test]
fn all_four_clip_edges_paint_above_actual_row_separator_without_geometry_changes(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, track, clips) = bottom_border_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    // Inject after the editor loads its stored theme; a non-black role
    // proves every unselected outline reads the palette.
    let _outline = theme::inject_test_clip_outline(0x2a4b6c);
    editor.update(&mut visual, |_, cx| cx.notify());
    visual.run_until_parked();
    let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let separator_selector =
        Box::leak(format!("lane-separator-{}", track.value()).into_boxed_str());
    for target_height in [44.0, 88.0, 23.0] {
        let current_height = f32::from(visual.debug_bounds(lane_selector).unwrap().size.height);
        if current_height != target_height {
            let selector =
                Box::leak(format!("track-height-resize-{}", track.value()).into_boxed_str());
            let down = visual.debug_bounds(selector).unwrap().center();
            let up = gpui::point(down.x, down.y + gpui::px(target_height - current_height));
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        }
        let lane = visual.debug_bounds(lane_selector).unwrap();
        assert!((f32::from(lane.size.height) - target_height).abs() < 0.1);
        let selectors = clips
            .iter()
            .map(|(id, prefix)| {
                Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str()) as &'static str
            })
            .collect::<Vec<_>>();
        let bounds = selectors
            .iter()
            .map(|s| visual.debug_bounds(s).unwrap())
            .collect::<Vec<_>>();
        click(&mut visual, clips[0].0, Modifiers::default());
        click(
            &mut visual,
            clips[4].0,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        for deselect in [false, true] {
            if deselect {
                click(
                    &mut visual,
                    clips[2].0,
                    Modifiers {
                        platform: true,
                        ..Modifiers::default()
                    },
                );
            }
            visual.simulate_mouse_move(
                gpui::point(lane.right() - gpui::px(3.0), lane.bottom() + gpui::px(3.0)),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
            let separator = visual.debug_bounds(separator_selector).unwrap();
            let separator_layers = visual.rendered_quad_paint_layers(separator_selector, false);
            assert!(!separator_layers.is_empty());
            let scale = visual.update(|window, _| window.scale_factor());
            for (i, selector) in selectors.iter().enumerate() {
                let current = visual.debug_bounds(selector).unwrap();
                assert_eq!(
                    current, bounds[i],
                    "selection leaves geometry/input bounds intact"
                );
                let (color, widths, mask) = visual.rendered_border(selector).unwrap();
                let selected = !(deselect && i == 2);
                assert_eq!(
                    color,
                    gpui::rgb(if selected {
                        theme::color(theme::PaletteRole::Primary)
                    } else {
                        theme::color(theme::PaletteRole::ClipOutline)
                    })
                    .into()
                );
                let zero = gpui::ScaledPixels::default();
                assert!(
                    widths.top > zero
                        && widths.bottom > zero
                        && widths.left > zero
                        && widths.right == zero
                );
                let scaled = current.scale(scale);
                let layers = visual.rendered_quad_paint_layers(selector, true);
                assert!(!layers.is_empty());
                // Retained rendering may replay one border as several masked
                // fragments. Check every pixel-center strip, not just the last
                // matching fragment's mask.
                let edges = [
                    (
                        scaled.left(),
                        scaled.top(),
                        scaled.right() - gpui::px(if i == 4 { 0.0 } else { 5.0 }).scale(scale),
                        scaled.top() + widths.top,
                    ),
                    (
                        scaled.left(),
                        scaled.bottom() - widths.bottom,
                        scaled.right(),
                        scaled.bottom(),
                    ),
                    (
                        scaled.left(),
                        scaled.top(),
                        scaled.left() + widths.left,
                        scaled.bottom(),
                    ),
                ];
                let prefix = if clips[i].1 == "midi-clip-visual" {
                    "midi-clip"
                } else {
                    "clip"
                };
                let edge_name = Box::leak(
                    format!(
                        "{prefix}-{}trailing-{}",
                        if selected { "selected-" } else { "" },
                        clips[i].0.value()
                    )
                    .into_boxed_str(),
                );
                let trailing = visual.debug_bounds(edge_name).unwrap();
                assert_eq!(trailing.left(), current.right());
                assert_eq!(f64::from(trailing.scale(scale).size.width), 1.0);
                if trailing.left() < lane.right() {
                    let trailing_layers = visual.rendered_quad_paint_layers(edge_name, false);
                    assert!(
                        !trailing_layers.is_empty(),
                        "visible trailing edge must paint"
                    );
                    let edge_order = trailing_layers
                        .iter()
                        .map(|(order, _)| *order)
                        .max()
                        .unwrap();
                    let separator_order = separator_layers
                        .iter()
                        .map(|(order, _)| *order)
                        .max()
                        .unwrap();
                    assert!(
                        edge_order > separator_order,
                        "trailing edge must paint above the row separator"
                    );
                }
                for (edge, (left, top, right, bottom)) in edges.iter().enumerate() {
                    let x0 = f64::from(*left);
                    let x1 = f64::from(*right);
                    let y0 = f64::from(*top);
                    let y1 = f64::from(*bottom);
                    for x in (x0.floor() as i32)..(x1.ceil() as i32) {
                        for y in (y0.floor() as i32)..(y1.ceil() as i32) {
                            let x = x as f64 + 0.5;
                            let y = y as f64 + 0.5;
                            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                                continue;
                            }
                            let contains = |mask: &gpui::Bounds<gpui::ScaledPixels>| {
                                x >= f64::from(mask.left())
                                    && x < f64::from(mask.right())
                                    && y >= f64::from(mask.top())
                                    && y < f64::from(mask.bottom())
                            };
                            let order = layers.iter().filter(|(_, mask)| contains(mask))
                                .map(|(order, _)| *order).max()
                                .unwrap_or_else(|| panic!("missing edge {edge} at {x},{y}: bounds={scaled:?} last_mask={mask:?} layers={layers:?}"));
                            let sep = separator.scale(scale);
                            if x >= f64::from(sep.left())
                                && x < f64::from(sep.right())
                                && y >= f64::from(sep.top())
                                && y < f64::from(sep.bottom())
                            {
                                for (separator_order, mask) in &separator_layers {
                                    if contains(mask) {
                                        assert!(
                                            order > *separator_order,
                                            "row separator covers edge {edge} at {x},{y}: clip={order:?} separator={separator_order:?}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                eprintln!(
                    "four edges clip={} height={} selected={selected} fragments={} separator_fragments={}",
                    clips[i].0.value(),
                    f32::from(lane.size.height),
                    layers.len(),
                    separator_layers.len()
                );
            }
            assert_eq!(
                bounds[0].left(),
                lane.left(),
                "left clipped selection edge visible"
            );
            let timeline = editor.read_with(&visual, |v, _| v.drag_lanes[&track]);
            assert!(
                (f32::from(bounds[4].right()) - (timeline.left + timeline.width)).abs() < 0.1,
                "right clipped selection edge visible"
            );
        }
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original);
        assert!(v.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn selected_track_header_has_no_selection_outline_and_color_strip_stays_distinct(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor) = crate::editor::clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let tracks = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|t| (t.id(), t.color()))
            .collect::<Vec<_>>()
    });
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for (id, color) in tracks.clone() {
        let header = Box::leak(format!("arrangement-track-header-{}", id.value()).into_boxed_str());
        let name = Box::leak(format!("track-name-{}", id.value()).into_boxed_str());
        let bounds = visual.debug_bounds(header).unwrap();
        let target = visual.debug_bounds(name).unwrap().center();
        visual.simulate_click(target, Modifiers::default());
        visual.run_until_parked();
        let outline = Box::leak(format!("track-selection-outline-{}", id.value()).into_boxed_str());
        assert!(visual.debug_bounds(outline).is_none());
        assert_eq!(
            visual.debug_bounds(header).unwrap(),
            bounds,
            "selection must not shift controls"
        );
        let strip = Box::leak(format!("track-color-strip-{}", id.value()).into_boxed_str());
        assert_eq!(visual.debug_bounds(strip).unwrap().size.width, gpui::px(4.));
        assert_eq!(
            visual.rendered_background(strip).unwrap(),
            gpui::rgb(theme::element_colors(color).accent).into()
        );
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track, Some(id));
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    for (id, color) in tracks {
        let strip = Box::leak(format!("session-track-color-strip-{}", id.value()).into_boxed_str());
        assert_eq!(
            visual.debug_bounds(strip).unwrap().size.height,
            gpui::px(4.)
        );
        assert_eq!(
            visual.rendered_background(strip).unwrap(),
            gpui::rgb(theme::element_colors(color).accent).into()
        );
    }
}

fn assert_clip_header_fill(visual: &mut VisualTestContext, prefix: &str, id: ClipId) {
    let parent = Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
    let header = Box::leak(format!("clip-name-{}", id.value()).into_boxed_str());
    let bounds = visual.debug_bounds(parent).unwrap();
    let fill = visual.debug_bounds(header).unwrap();
    let (_, mask) = visual.rendered_surface_geometry(header).unwrap();
    let scale = visual.update(|window, _| window.scale_factor());
    let physical = fill.scale(scale);
    let outer = bounds.scale(scale);
    let clip = |value: gpui::ScaledPixels| f64::from(value);
    let visible_left = clip(physical.left()).max(clip(mask.left()));
    let visible_right = clip(physical.right()).min(clip(mask.right()));
    let visible_top = clip(physical.top()).max(clip(mask.top()));
    let visible_bottom = clip(physical.bottom()).min(clip(mask.bottom()));
    let (_, widths, _) = visual.rendered_border(parent).unwrap();
    assert!(visible_left >= clip(outer.left()) + clip(widths.left));
    assert!(visible_right <= clip(outer.right()) - clip(widths.right));
    assert!(visible_top >= clip(outer.top()) + clip(widths.top));
    assert!(visible_bottom <= clip(outer.bottom()) - clip(widths.bottom));
}

#[gpui::test]
fn clip_name_fill_stays_inside_clip_border(cx: &mut TestAppContext) {
    let (project, _track, _, clip) = audio_project();
    let (handle, _editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_clip_header_fill(&mut visual, "clip-visual", clip);
}

#[gpui::test]
fn adjacent_clip_edges_share_one_grid_pixel(cx: &mut TestAppContext) {
    let (project, track, _, first) = audio_project();
    let mut canonical = Editor::new(project, 16).unwrap();
    let Created::Clip(second) = canonical
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    let asset = canonical.project().clip(first).unwrap().asset();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
    )]);
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(canonical.project(), &media).unwrap();
    let (handle, editor) = editor_window(cx, canonical.project().clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    let trailing = Box::leak(format!("clip-trailing-{}", first.value()).into_boxed_str());
    let following = Box::leak(format!("midi-clip-visual-{}", second.value()).into_boxed_str());
    for start in [0, 337] {
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start,
                length: 192_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let scale = visual.update(|window, _| window.scale_factor());
        let edge = visual.debug_bounds(trailing).unwrap().scale(scale);
        let next = visual.debug_bounds(following).unwrap().scale(scale);
        let grid = visual
            .debug_bounds("arrangement-grid-line-96000")
            .unwrap()
            .scale(scale);
        let inner_grid = visual
            .debug_bounds("clip-grid-line-48000")
            .unwrap()
            .scale(scale);
        let lane_grid = visual
            .debug_bounds("arrangement-grid-line-48000")
            .unwrap()
            .scale(scale);
        let (_, border, _) = visual.rendered_border(following).unwrap();
        assert_eq!(f64::from(edge.size.width), 1.0);
        assert_eq!(f64::from(grid.size.width), 1.0);
        assert_eq!(f64::from(border.left), 1.0);
        assert_eq!(edge.left(), grid.left());
        assert_eq!(edge.left(), next.left());
        assert_eq!(
            inner_grid.left(),
            lane_grid.left(),
            "internal clip grid must share the lane column"
        );
        assert_clip_header_fill(&mut visual, "clip-visual", first);
        assert_clip_header_fill(&mut visual, "midi-clip-visual", second);
    }
    click(&mut visual, first, Modifiers::default());
    assert_clip_header_fill(&mut visual, "clip-visual", first);
    assert_clip_header_fill(&mut visual, "midi-clip-visual", second);
    let selected_edge =
        Box::leak(format!("clip-selected-trailing-{}", first.value()).into_boxed_str());
    let selected = visual.debug_bounds(selected_edge).unwrap();
    let grid = visual.debug_bounds("arrangement-grid-line-96000").unwrap();
    assert_eq!(selected.left(), grid.left());
    assert_eq!(
        visual.rendered_background(selected_edge),
        Some(gpui::Background::from(gpui::rgb(theme::color(
            theme::PaletteRole::Primary
        ))))
    );
    let selected_order = visual
        .rendered_quad_paint_layers(selected_edge, false)
        .iter()
        .map(|(order, _)| *order)
        .max()
        .unwrap();
    let next_order = visual
        .rendered_quad_paint_layers(following, true)
        .iter()
        .map(|(order, _)| *order)
        .max()
        .unwrap();
    assert!(
        selected_order > next_order,
        "selected seam remains visible above an unselected neighbor"
    );
}

#[test]
fn corner_geometry_is_bounded_and_resolves_again_after_resize() {
    use super::{Notch, corner_extent};
    assert_eq!(corner_extent(100.0, 44.0), 5.0);
    assert_eq!(corner_extent(6.0, 44.0), 3.0);
    assert_eq!(corner_extent(100.0, 4.0), 2.0);
    assert_eq!(corner_extent(0.0, 44.0), 0.0);
    assert_eq!(corner_extent(-1.0, 44.0), 0.0);
    assert_eq!(corner_extent(f32::NAN, 44.0), 0.0);
    let notch = Notch::new(true);
    let edge = notch.clone();
    for width in [100.0, 6.0, 100.0] {
        notch.observe(gpui::Bounds::new(
            gpui::point(gpui::px(10.0), gpui::px(20.0)),
            gpui::size(gpui::px(width), gpui::px(44.0)),
        ));
        assert_eq!(edge.extent(), corner_extent(width, 44.0));
    }
    let cropped = Notch::default();
    cropped.observe(gpui::Bounds::new(
        gpui::point(gpui::px(0.0), gpui::px(0.0)),
        gpui::size(gpui::px(100.0), gpui::px(44.0)),
    ));
    assert_eq!(cropped.extent(), 0.0);
}
