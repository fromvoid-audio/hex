//! Compact Arrangement rows use the real actor, imported media and rendered inputs.
use super::{EditorView, clip_drag_ui_test, test_environment::Environment};
use crate::theme;
use gpui::{
    Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px,
    size,
};
use hex_project::{BeatTime, ClipId, Command, TrackId};
use hex_session::service::Request;
use std::{rc::Rc, sync::Arc};

struct Fixture {
    _environment: Arc<Environment>,
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    tracks: [TrackId; 2],
    clips: [ClipId; 2],
}

fn selector(prefix: &str, id: u64) -> &'static str {
    Box::leak(format!("{prefix}-{id}").into_boxed_str())
}

fn bounds(visual: &mut VisualTestContext, prefix: &str, id: u64) -> Bounds<Pixels> {
    visual
        .debug_bounds(selector(prefix, id))
        .unwrap_or_else(|| panic!("missing {prefix}-{id}"))
}

fn edit(editor: &Entity<EditorView>, visual: &mut VisualTestContext, command: Command) {
    editor.update(&mut *visual, |view, _| {
        assert!(view.enqueue(Request::Edit(command)))
    });
    clip_drag_ui_test::pump_move(editor, visual);
}

fn fixture(cx: &mut TestAppContext) -> Fixture {
    // This existing fixture creates one Environment before importing its actual
    // WAV and constructing the editor; discovery and saves retain that same Arc.
    let (environment, handle, editor) = clip_drag_ui_test::upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (tracks, audio) = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        let project = &view.state.as_ref().unwrap().project;
        (
            [project.tracks()[0].id(), project.tracks()[1].id()],
            project.tracks()[0].clips()[0].id(),
        )
    });
    edit(
        &editor,
        &mut visual,
        Command::PlaceMidiClip {
            track: tracks[1],
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        },
    );
    let midi = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(tracks[1])
            .unwrap()
            .midi_clips()[0]
            .id()
    });
    edit(
        &editor,
        &mut visual,
        Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    // The cache intentionally caps samples at32;33 real notes also mount its
    // ordinary count badge, proving compact mode removes an existing detail.
    for pitch in 61..=92 {
        edit(
            &editor,
            &mut visual,
            Command::AddMidiNote {
                clip: midi,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 100,
            },
        );
    }
    for (clip, name) in [(audio, "audio compact"), (midi, "midi compact")] {
        edit(
            &editor,
            &mut visual,
            Command::RenameClip {
                clip,
                name: name.into(),
            },
        );
    }
    edit(
        &editor,
        &mut visual,
        Command::SetClipFades {
            clip: audio,
            fade_in: 12_000,
            fade_out: 12_000,
        },
    );
    editor.update(&mut visual, |view, cx| {
        view.snap_grid = super::musical_time::SnapGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    Fixture {
        _environment: environment,
        visual,
        editor,
        tracks,
        clips: [audio, midi],
    }
}

fn resize_all(f: &mut Fixture, delta: f32) {
    let edge = bounds(&mut f.visual, "track-height-resize", f.tracks[0].value()).center();
    let end = point(edge.x, edge.y + px(delta));
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    f.visual
        .simulate_mouse_down(edge, MouseButton::Left, option);
    f.visual.simulate_mouse_move(end, MouseButton::Left, option);
    // A real held repaint may cache a compact body without editing controls.
    // Release must invalidate that admission state without a project mutation.
    f.visual.run_until_parked();
    f.editor
        .read_with(&f.visual, |v, _| assert!(v.track_resize_active()));
    f.visual.simulate_mouse_up(end, MouseButton::Left, option);
    for _ in 0..3 {
        f.visual.run_until_parked();
        f.editor.update(&mut f.visual, |_, cx| cx.notify());
    }
    f.editor
        .read_with(&f.visual, |v, _| assert!(!v.track_resize_active()));
}

fn compact(f: &mut Fixture) {
    resize_all(f, -300.0);
    for track in f.tracks {
        assert_eq!(
            bounds(&mut f.visual, "arrangement-lane", track.value())
                .size
                .height,
            px(23.0)
        );
        assert_eq!(
            bounds(&mut f.visual, "arrangement-track-header", track.value())
                .size
                .height,
            px(23.0)
        );
    }
}

fn assert_no_bodies(f: &mut Fixture) {
    assert!(
        f.visual.debug_bounds("clip-waveform-body").is_none(),
        "compact rows must not mount cached waveform or note bodies"
    );
    assert!(
        f.visual
            .debug_bounds(selector("midi-preview-count", f.clips[1].value()))
            .is_none()
    );
    for dragging in [false, true] {
        assert!(
            super::midi_thumbnail::rendered_notes(&mut f.visual, f.clips[1], dragging).is_empty()
        );
    }
    assert!(
        super::midi_thumbnail::rendered_note_layer(&mut f.visual, "midi-trim-preview-notes")
            .is_empty()
    );
    for prefix in [
        "fade-in-top",
        "fade-in-bottom",
        "fade-in-curve",
        "fade-out-top",
        "fade-out-bottom",
        "fade-out-curve",
    ] {
        assert!(
            f.visual
                .debug_bounds(selector(prefix, f.clips[0].value()))
                .is_none(),
            "compact rows must not mount {prefix}"
        );
    }
}

fn assert_title(
    visual: &mut VisualTestContext,
    prefix: &str,
    clip: ClipId,
    parent: Bounds<Pixels>,
) {
    let title = bounds(visual, prefix, clip.value());
    assert!(
        title.size.width > px(30.0),
        "{prefix} title is too narrow: {title:?}, parent={parent:?}"
    );
    assert_title_rail_height(title, prefix);
    assert!(
        title.top() >= parent.top() - px(1.0),
        "{prefix} title starts above its parent: title={title:?}, parent={parent:?}"
    );
    assert!(
        title.bottom() <= parent.bottom(),
        "{prefix} title is vertically clipped: title={title:?}, parent={parent:?}"
    );
    assert!(
        visual
            .rendered_background(selector(prefix, clip.value()))
            .is_some(),
        "title strip must actually paint"
    );
}

fn assert_title_rail_height(bounds: Bounds<Pixels>, label: &str) {
    // clip_name_surface uses a 14px text line in a 44/3px strip. Native layout
    // snaps the fractional strip to physical pixels, so requiring its unsnapped
    // nominal height rejects a complete rendered 14px line. Keep that actual
    // line-height minimum for both the label and its move/selection hit rail.
    assert!(
        bounds.size.height >= px(14.0),
        "{label} cannot fit the configured 14px title line: bounds={bounds:?}, nominal strip={}px",
        super::clip_name::CLIP_NAME_HEIGHT
    );
}

#[gpui::test]
fn compact_rows_preserve_canonical_titles_selection_and_restore_body_on_expansion(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx);
    assert!(f.visual.debug_bounds("clip-waveform-body").is_some());
    assert!(!super::midi_thumbnail::rendered_notes(&mut f.visual, f.clips[1], false).is_empty());
    assert!(
        f.visual
            .debug_bounds(selector("midi-preview-count", f.clips[1].value()))
            .is_none(),
        "expanded rows draw every note and never a sample-count badge"
    );
    let before = f.editor.read_with(&f.visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    compact(&mut f);
    for (i, clip) in f.clips.into_iter().enumerate() {
        let title = bounds(&mut f.visual, "clip-name", clip.value());
        f.visual.simulate_click(
            title.center(),
            Modifiers {
                platform: i != 0,
                ..Default::default()
            },
        );
        f.visual.run_until_parked();
    }
    for (clip, prefix) in [
        (f.clips[0], "clip-visual"),
        (f.clips[1], "midi-clip-visual"),
    ] {
        let surface = bounds(&mut f.visual, prefix, clip.value());
        assert_eq!(surface.size.height, px(23.0));
        assert_title(&mut f.visual, "clip-name", clip, surface);
        let (_, divider, _) = f
            .visual
            .rendered_border(selector("clip-name", clip.value()))
            .unwrap();
        assert!(
            divider.bottom > gpui::ScaledPixels::default(),
            "compact clip title retains its divider"
        );
        let (color, widths, _) = f
            .visual
            .rendered_border(selector(prefix, clip.value()))
            .unwrap();
        assert_eq!(
            color,
            gpui::Hsla::from(gpui::rgb(theme::color(theme::PaletteRole::Primary)))
        );
        let zero = gpui::ScaledPixels::default();
        assert!(
            widths.top > zero && widths.bottom > zero && widths.left > zero && widths.right == zero
        );
        // The right edge is a separately painted physical pixel, not a layout
        // border. Keep its scene coverage alongside the three native edges.
        let edge_prefix = if prefix == "midi-clip-visual" {
            "midi-clip-selected-trailing"
        } else {
            "clip-selected-trailing"
        };
        let edge_selector = selector(edge_prefix, clip.value());
        let edge = f.visual.debug_bounds(edge_selector).unwrap();
        let scale = f.visual.update(|window, _| window.scale_factor());
        assert_eq!(edge.left(), surface.right());
        assert_eq!(f64::from(edge.scale(scale).size.width), 1.0);
        assert_eq!(
            f.visual.rendered_background(edge_selector),
            Some(gpui::rgb(theme::color(theme::PaletteRole::Primary)).into())
        );
        assert!(
            !f.visual
                .rendered_quad_paint_layers(edge_selector, false)
                .is_empty()
        );
        let move_hit = bounds(&mut f.visual, "clip-drag-surface", clip.value());
        assert_title_rail_height(move_hit, "clip-drag-surface");
    }
    let audio = bounds(&mut f.visual, "clip-visual", f.clips[0].value());
    f.visual
        .simulate_mouse_move(audio.center(), None, Modifiers::default());
    f.visual.simulate_keystrokes("f");
    f.visual.run_until_parked();
    assert_no_bodies(&mut f);
    f.editor.read_with(&f.visual, |view, _| {
        assert!(!view.pointer_hits_priority_clip_control(
            f32::from(audio.center().x),
            f32::from(audio.center().y),
            false,
        ));
        assert_eq!(view.selected_clip_ids(), f.clips);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!view.track_resize_active());
    });
    resize_all(&mut f, 50.0);
    assert!(f.visual.debug_bounds("clip-waveform-body").is_some());
    assert!(!super::midi_thumbnail::rendered_notes(&mut f.visual, f.clips[1], false).is_empty());
    f.editor.read_with(&f.visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn compact_audio_and_midi_trim_edges_keep_title_and_exact_preview_until_cancel(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx);
    compact(&mut f);
    let before = f.editor.read_with(&f.visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for (index, prefix, preview_selector) in [
        (0, "clip-trim-right", "trim-live-preview"),
        (1, "midi-clip-trim-right", "midi-trim-live-preview"),
    ] {
        let clip = f.clips[index];
        let edge = bounds(&mut f.visual, prefix, clip.value());
        assert_eq!(edge.size.height, px(23.0));
        let down = edge.center();
        let end = point(down.x - px(32.0), down.y);
        f.visual
            .simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        f.visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        f.visual
            .simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        let preview = f
            .visual
            .debug_bounds(preview_selector)
            .expect("real compact trim preview");
        assert_eq!(preview.size.height, px(23.0));
        assert_title(&mut f.visual, "clip-name", clip, preview);
        let fill = f.editor.read_with(&f.visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            theme::clip_activity_fill(
                project.track(f.tracks[index]).unwrap().color(),
                project.clip_flavor(clip).unwrap(),
                false,
                project.clip_active(clip).unwrap(),
            )
        });
        let background = gpui::Background::from(gpui::rgb(fill));
        assert_eq!(
            f.visual
                .rendered_background(selector("clip-name", clip.value())),
            Some(background),
            "held title inherits its clip color"
        );
        assert_eq!(
            f.visual.rendered_background(preview_selector),
            Some(background)
        );
        let scale = f.visual.update(|window, _| window.scale_factor());
        let corner = point(preview.right() - px(0.25), preview.top() + px(0.25)).scale(scale);
        assert!(
            f.visual
                .rendered_filled_quads(preview_selector)
                .iter()
                .filter(|(_, color)| *color == background)
                .all(|(bounds, _)| !bounds.contains(&corner)),
            "held preview leaves the real upper-right cut open"
        );
        let trailing = if index == 0 {
            "trim-preview-trailing"
        } else {
            "midi-trim-preview-trailing"
        };
        let edge = f.visual.debug_bounds(trailing).unwrap();
        assert_eq!(edge.left(), preview.right());
        assert_eq!(f64::from(edge.scale(scale).size.width), 1.0);
        let layers = f.visual.rendered_quad_paint_layers(trailing, false);
        assert!(!layers.is_empty());
        assert!(
            layers
                .iter()
                .all(|(_, mask)| mask.top() >= (preview.top() + px(5.0)).scale(scale))
        );
        assert_no_bodies(&mut f);
        f.editor.update(&mut f.visual, |view, cx| {
            if index == 0 {
                let drag = view.active_trim_drag.as_ref().unwrap();
                assert_eq!(drag.clip.id(), clip);
                assert!(drag.preview.borrow().as_ref().unwrap().clip.duration() < 96_000);
            } else {
                let drag = view.active_midi_trim_drag.as_ref().unwrap();
                assert_eq!(drag.clip.id(), clip);
                assert!(
                    drag.preview.borrow().unwrap().length() < BeatTime::from_quarters(4).unwrap()
                );
            }
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            cx.notify();
        });
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.debug_bounds(preview_selector).unwrap(),
            preview,
            "repaint keeps captured geometry"
        );
        f.visual.simulate_keystrokes("escape");
        f.visual
            .simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        // Start with a cropped endpoint: changing the viewport during a held
        // trim intentionally cancels that capture rather than reinterpreting it.
        let viewport = f.editor.update(&mut f.visual, |view, cx| {
            let project = &view.state.as_ref().unwrap().project;
            let stop = if index == 0 {
                let source = project.clip(clip).unwrap();
                source.start() + source.duration()
            } else {
                let source = project.midi_clip(clip).unwrap();
                project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(
                        source.start().raw() + source.length().raw(),
                    ))
                    .unwrap()
            };
            let old = view.viewport.replace(super::viewport::Viewport {
                start: 0,
                length: stop - 1_000,
            });
            cx.notify();
            old
        });
        f.visual.run_until_parked();
        let left = if index == 0 {
            "clip-trim-left"
        } else {
            "midi-clip-trim-left"
        };
        let down = bounds(&mut f.visual, left, clip.value()).center();
        let end = point(down.x + px(20.0), down.y);
        f.visual
            .simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        f.visual
            .simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        let cropped = f
            .visual
            .debug_bounds(preview_selector)
            .expect("held left trim with cropped right endpoint");
        let corner = point(cropped.right() - px(0.25), cropped.top() + px(2.0)).scale(scale);
        assert!(
            f.visual
                .rendered_filled_quads(preview_selector)
                .iter()
                .any(|(bounds, color)| *color == background && bounds.contains(&corner)),
            "viewport crop keeps its square filled edge, not a false clip ending"
        );
        f.visual.simulate_keystrokes("escape");
        f.visual
            .simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        f.editor.update(&mut f.visual, |view, cx| {
            view.viewport = viewport;
            cx.notify();
        });
        f.visual.run_until_parked();
        f.editor.read_with(&f.visual, |view, _| {
            assert!(view.active_trim_drag.is_none() && view.active_midi_trim_drag.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert!(view.pending_requests.is_empty() && view.worker_requests_pending == 0);
        });
    }
}

#[gpui::test]
fn compact_mixed_group_drag_has_titles_without_bodies_and_preserves_capture(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx);
    compact(&mut f);
    for (index, clip) in f.clips.into_iter().enumerate() {
        let title = bounds(&mut f.visual, "clip-name", clip.value());
        f.visual.simulate_click(
            title.center(),
            Modifiers {
                platform: index != 0,
                ..Default::default()
            },
        );
        f.visual.run_until_parked();
    }
    let before = f.editor.read_with(&f.visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let down = bounds(&mut f.visual, "clip-drag-surface", f.clips[0].value()).center();
    f.visual
        .simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    f.visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let end = point(down.x + px(100.0), down.y);
    f.visual
        .simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    f.visual.run_until_parked();
    let capture = f.editor.read_with(&f.visual, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("mixed group drag");
        assert_eq!(drag.group.borrow().ids, f.clips);
        assert_eq!(drag.group.borrow().previews().len(), 2);
        Rc::clone(&drag.group)
    });
    let previews = f.clips.map(|clip| {
        let preview = bounds(&mut f.visual, "group-clip-preview", clip.value());
        assert_title(&mut f.visual, "clip-drag-name", clip, preview);
        preview
    });
    assert_no_bodies(&mut f);
    f.editor.update(&mut f.visual, |view, cx| {
        cx.notify();
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
    f.visual.run_until_parked();
    for (clip, preview) in f.clips.into_iter().zip(previews) {
        assert_eq!(
            bounds(&mut f.visual, "group-clip-preview", clip.value()),
            preview
        );
    }
    f.editor.read_with(&f.visual, |view, _| {
        assert!(Rc::ptr_eq(
            &capture,
            &view.active_clip_drag.as_ref().unwrap().group
        ))
    });
    f.visual.simulate_keystrokes("escape");
    f.visual
        .simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    f.visual.run_until_parked();
    f.editor.read_with(&f.visual, |view, _| {
        assert!(view.active_clip_drag.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn compact_time_range_copy_projects_named_audio_and_midi_fragments_without_mutation(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx);
    compact(&mut f);
    let before = f.editor.read_with(&f.visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    // Selection ownership is set directly; the move itself starts on its real
    // rendered handle and uses normal Option-copy routing.
    f.editor.update(&mut f.visual, |view, cx| {
        view.time_selection = Some(super::time_selection::TimeSelection {
            anchor_track: f.tracks[0],
            end_track: f.tracks[1],
            anchor_frame: 0,
            end_frame: 120_000,
        });
        cx.notify();
    });
    f.visual.run_until_parked();
    let handle = bounds(&mut f.visual, "time-selection-handle", f.tracks[0].value());
    assert_title_rail_height(handle, "time-selection-handle");
    let down = handle.center();
    let end = point(down.x + px(180.0), down.y);
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    f.visual
        .simulate_mouse_down(down, MouseButton::Left, option);
    f.visual.simulate_mouse_move(end, MouseButton::Left, option);
    f.visual.run_until_parked();
    f.editor.read_with(&f.visual, |view, _| {
        let display = view
            .time_range_move_display()
            .expect("real compact time-copy preview");
        assert!(display.copy_modifier);
        assert_eq!(display.source.tracks(), f.tracks);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.active_clip_drag.is_none());
    });
    for index in 0..2 {
        let name = if index == 0 {
            Box::leak(
                format!(
                    "time-move-audio-fragment-{}-{}",
                    f.tracks[index].value(),
                    f.clips[index].value()
                )
                .into_boxed_str(),
            )
        } else {
            selector("time-move-midi-fragment", f.clips[index].value())
        };
        let fragment = f
            .visual
            .debug_bounds(name)
            .expect("named compact time fragment");
        assert_eq!(fragment.size.height, px(23.0));
        assert_title(&mut f.visual, "clip-drag-name", f.clips[index], fragment);
    }
    assert_no_bodies(&mut f);
    f.visual.simulate_keystrokes("escape");
    f.visual.simulate_mouse_up(end, MouseButton::Left, option);
    f.visual.run_until_parked();
    f.editor.read_with(&f.visual, |view, _| {
        assert!(view.time_range_move_gesture.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.pending_requests.is_empty() && view.worker_requests_pending == 0);
    });
}

#[path = "track_header_name_ui_tests.rs"]
mod header_name_tests;
