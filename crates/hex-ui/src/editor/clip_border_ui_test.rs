//! Border assertions read completed GPUI scene quads, including retained scenes.
use super::{
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
    cx.update(super::bind_editor_keys);
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
        view.snap_grid = super::musical_time::SnapGrid::Off;
        cx.notify();
    });
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
                    0x000000
                })),
                "{id:?} selected border"
            );
            let zero = gpui::ScaledPixels::default();
            assert!(
                widths.top > zero
                    && widths.bottom > zero
                    && widths.left > zero
                    && widths.right > zero
            );
            let current = visual.debug_bounds(selector).unwrap();
            assert_eq!(
                current, bounds[index],
                "selection does not change clip geometry"
            );
            assert!(mask.size.width > zero && mask.size.height > zero);
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
                theme::clip_fill(source.color(), flavor, selected)
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

/// Private bounded startup: no ambient browser/catalog or shared fixture changes.
fn bottom_border_fixture(
    cx: &mut TestAppContext,
) -> (
    super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    hex_project::TrackId,
    Vec<(ClipId, &'static str)>,
) {
    use gpui::AppContext;
    use hex_session::service::{Event, Request, Service};
    fn next(service: &Service) -> Event {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
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
    let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let lane_selector = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let separator_selector =
        Box::leak(format!("lane-separator-{}", track.value()).into_boxed_str());
    for enlarged in [false, true] {
        if enlarged {
            let selector =
                Box::leak(format!("track-height-resize-{}", track.value()).into_boxed_str());
            let down = visual.debug_bounds(selector).unwrap().center();
            let up = gpui::point(down.x, down.y + gpui::px(44.0));
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
        }
        let lane = visual.debug_bounds(lane_selector).unwrap();
        assert!((f32::from(lane.size.height) - if enlarged { 88.0 } else { 44.0 }).abs() < 0.1);
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
            let separator_order = visual
                .rendered_quad_paint_order(separator_selector)
                .unwrap();
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
                        0x000000
                    })
                    .into()
                );
                let zero = gpui::ScaledPixels::default();
                assert!(
                    widths.top > zero
                        && widths.bottom > zero
                        && widths.left > zero
                        && widths.right > zero
                );
                let scaled = current.scale(scale);
                assert!(
                    scaled.left() >= mask.left()
                        && scaled.right() <= mask.right()
                        && scaled.top() >= mask.top()
                        && scaled.bottom() <= mask.bottom(),
                    "all four painted strips inside actual ancestor mask"
                );
                assert!(
                    current.bottom() > separator.top() && current.bottom() <= separator.bottom(),
                    "actual bottom stroke shares the separator's occluding strip"
                );
                let order = visual.rendered_quad_paint_order(selector).unwrap();
                eprintln!(
                    "border edge clip={} height={} selected={selected} clip_order={order:?} separator_order={separator_order:?} bounds={current:?} separator={separator:?}",
                    clips[i].0.value(),
                    f32::from(lane.size.height)
                );
                assert!(
                    order > separator_order,
                    "later row separator must not cover the completed clip border"
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
