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
