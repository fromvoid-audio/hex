use super::*;
use crate::editor::{EditorView, clip_drag_ui_test::editor_window};
use gpui::{
    Entity, KeyDownEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px, size,
};
use hex_project::{Command, Created, LoopRegion};
use std::sync::Arc;

#[path = "fixture.rs"]
mod fixture;
use fixture::{Fixture, model};

fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<EditorView>, Fixture) {
    cx.update(crate::editor::bind_editor_keys);
    let f = model();
    let audio = hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap();
    let media = std::collections::BTreeMap::from([(f.asset, audio)]);
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(f.model.project(), &media).unwrap();
    let (handle, editor) = editor_window(cx, f.model.project().clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.update(|window, cx| {
        editor.update(cx, |v, _| {
            // This fixture tests section selection; autoplay is a separate interaction.
            v.section_autoplay = false;
            v.focus.focus(window);
        })
    });
    visual.run_until_parked();
    (visual, editor, f)
}
fn selector(prefix: &str, id: u64) -> &'static str {
    Box::leak(format!("{prefix}-{id}").into_boxed_str())
}
fn replace(visual: &mut VisualTestContext, editor: &Entity<EditorView>, project: Project) {
    editor.update(visual, |v, cx| {
        let old = v.state.as_ref().unwrap();
        v.state = Some(Arc::new(hex_session::service::ViewState {
            canonical_audio_source: old.canonical_audio_source.clone(),
            comparison_audio: None,
            project: Arc::new(project),
            waveforms: old.waveforms.clone(),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            midi_preview: old.midi_preview.clone(),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision + 1,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        cx.notify();
    });
    visual.run_until_parked();
}

// This fixture selects Q's driving section IDs; Shift alone selects time.
fn select_section(visual: &mut VisualTestContext, index: u64, toggle_membership: bool) {
    let bounds = visual
        .debug_bounds(selector("arrangement-section-header", index))
        .unwrap();
    // Avoid the separate launch triangle at the left of a narrow header.
    let at = point(bounds.left() + bounds.size.width * 0.75, bounds.center().y);
    visual.simulate_mouse_down(
        at,
        MouseButton::Left,
        Modifiers {
            shift: toggle_membership,
            platform: toggle_membership,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    visual.simulate_mouse_up(
        at,
        MouseButton::Left,
        Modifiers {
            shift: toggle_membership,
            platform: toggle_membership,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
}

#[gpui::test]
fn section_focus_ignores_muted_audio_and_midi_clips(cx: &mut TestAppContext) {
    let (mut visual, editor, mut f) = open(cx);
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    let visible = |visual: &VisualTestContext, expected: &[usize]| {
        editor.read_with(visual, |view, _| {
            assert_eq!(view.arrangement_track_indices(), expected);
        });
    };
    visible(&visual, &[1, 3]);

    // The audio track also has active clips outside the chosen section.
    for (clip, active, expected) in [
        (f.clips[0], false, &[3][..]),
        (f.clips[2], false, &[][..]),
        (f.clips[0], true, &[1][..]),
    ] {
        f.model
            .execute(Command::SetClipActive { clip, active })
            .unwrap();
        replace(&mut visual, &editor, f.model.project().clone());
        visible(&visual, expected);
    }
    let midi = selector("midi-clip-visual", f.clips[2].value());
    assert!(visual.debug_bounds(midi).is_none());
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    visible(&visual, &[0, 1, 2, 3, 4]);
    assert!(visual.debug_bounds(midi).is_some());
}

#[gpui::test]
fn section_focus_renders_visible_child_with_nested_group_headers(cx: &mut TestAppContext) {
    let (mut visual, editor, mut f) = open(cx);
    let mut child = f.tracks[3];
    let mut groups = Vec::new();
    for _ in 0..2 {
        let Created::Track(group) = f
            .model
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[child]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        groups.push(group);
        child = group;
    }
    for index in 0..8 {
        f.model
            .execute(Command::AddTrack {
                name: format!("extra {index}"),
            })
            .unwrap();
    }
    replace(&mut visual, &editor, f.model.project().clone());
    // A folded subtree removes two rows before the last track. Q must save
    // the visible row's canonical identity rather than its filtered index.
    let outer_fold = selector("group-fold", groups[1].value());
    let at = visual.debug_bounds(outer_fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_resize(size(px(1280.0), px(420.0)));
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 4,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        let top = view.track_scroll.list.logical_scroll_top();
        let canonical = view.arrangement_track_index(top.item_ix).unwrap();
        assert!(
            canonical > top.item_ix,
            "top row must follow the collapsed subtree"
        );
        let expected = view.state.as_ref().unwrap().project.tracks()[canonical].id();
        view.toggle_arrangement_focus();
        assert_eq!(
            view.arrangement_focus.restore_scroll.map(|(id, _)| id),
            Some(expected)
        );
        view.toggle_arrangement_focus();
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let at = visual.debug_bounds(outer_fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    for id in [groups[1], groups[0], f.tracks[3]] {
        assert!(
            visual
                .debug_bounds(selector("arrangement-track-header", id.value()))
                .is_some()
        );
    }
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", f.tracks[2].value()))
            .is_none()
    );
    let fold = selector("group-fold", groups[0].value());
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", groups[0].value()))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", f.tracks[3].value()))
            .is_none()
    );
    let at = visual.debug_bounds(fold).unwrap().center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(selector("arrangement-track-header", f.tracks[3].value()))
            .is_some()
    );
}

#[gpui::test]
fn q_filters_tracks_but_keeps_outside_clips_and_hits_without_editing_history(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, f) = open(cx);
    // Hidden rows keep their own authored view height, independently of H.
    let grip = visual
        .debug_bounds(selector("track-height-resize", f.tracks[0].value()))
        .unwrap()
        .center();
    visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
    let taller = point(grip.x, grip.y + px(32.0));
    visual.simulate_mouse_move(taller, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(taller, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.run_until_parked();
    let outside = selector("midi-clip-visual", f.clips[4].value());
    let before = visual
        .debug_bounds(outside)
        .expect("outside midi in retained audio row");
    let empty = selector("arrangement-track-header", f.tracks[0].value());
    let original_first = visual.debug_bounds(empty).unwrap();
    let toggle = visual.debug_bounds("arrangement-focus-toggle").unwrap();
    let toolbar = visual.debug_bounds("editor-toolbar").unwrap();
    assert!(toggle.top() >= toolbar.top() && toggle.bottom() <= toolbar.bottom());
    let main = visual.debug_bounds("main-track-header").unwrap();
    assert!(toggle.bottom() < main.top());
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    editor.update(&mut visual, |v, _| {
        v.select_lane_track(f.tracks[2]);
        v.clip_selection.only(f.tracks[2], f.clips[1]);
        v.selected_clip = Some(f.clips[1]);
    });
    let selected = editor.read_with(&visual, |v, _| {
        (
            v.selected_track,
            v.selected_clip,
            v.selected_track_ids(),
            (v.selected_clip_ids(), v.clip_selection.epoch),
        )
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(empty).is_none());
    assert!(visual.debug_bounds(outside).is_some());
    assert!(
        visual
            .debug_bounds(selector("clip-visual", f.clips[5].value()))
            .is_some()
    );
    let first = visual
        .debug_bounds(selector("arrangement-track-header", f.tracks[1].value()))
        .unwrap();
    assert_eq!(
        first.top(),
        original_first.top(),
        "filtered list rows pack at the body origin"
    );
    assert!(
        visual
            .debug_bounds(selector("midi-clip-visual", f.clips[2].value()))
            .is_some()
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_focus.projection.indices, [1, 3]);
        assert_eq!(
            (
                v.selected_track,
                v.selected_clip,
                v.selected_track_ids(),
                (v.selected_clip_ids(), v.clip_selection.epoch)
            ),
            selected
        );
        assert_eq!(
            v.state.as_ref().unwrap().project.as_ref(),
            f.model.project()
        );
        assert!(v.last_enqueued_edit.is_none());
        assert!(!v.state.as_ref().unwrap().can_undo);
        let lane = v.drag_lanes[&f.tracks[1]];
        let x = f32::from(before.center().x);
        let viewport = v.visible_viewport(v.state.as_ref().unwrap().timeline_frames);
        assert!(
            v.clip_handle_at_pointer(f.tracks[1], x, lane.top + 3.0, viewport, false),
            "outside-section clip on a retained track stays interactive"
        );
    });
    // H only changes row presentation. Leaving focus retains that independent mode.
    visual.simulate_keystrokes("h q");
    visual.run_until_parked();
    assert!(visual.debug_bounds(empty).is_some());
    let compact = visual.debug_bounds(empty).unwrap();
    assert_eq!(
        compact.size.height,
        px(crate::editor::track_layout::HEADER_ONLY_HEIGHT)
    );
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds(empty).unwrap().size.height,
        original_first.size.height
    );
    assert!(visual.debug_bounds(outside).is_some());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.as_ref(),
            f.model.project()
        )
    });
}
#[gpui::test]
fn toggle_tracks_section_selection_and_ignores_loop_changes(cx: &mut TestAppContext) {
    let (mut visual, editor, mut f) = open(cx);
    let button = visual
        .debug_bounds("arrangement-focus-toggle")
        .unwrap()
        .center();
    visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert_eq!(v.arrangement_focus.projection.indices, [0, 1, 2, 3, 4]);
    });
    select_section(&mut visual, 0, false);
    select_section(&mut visual, 2, true);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_focus.projection.indices, [1, 3]);
        assert!(v.arrangement_clip_visible(f.clips[4]));
        assert!(!v.arrangement_clip_visible(f.clips[3]));
    });
    let first = visual
        .debug_bounds(selector("arrangement-lane", f.tracks[1].value()))
        .unwrap();
    let last = visual
        .debug_bounds(selector("arrangement-lane", f.tracks[3].value()))
        .unwrap();
    let start = point(
        first.left() + first.size.width * 0.8,
        first.top() + first.size.height * 0.75,
    );
    let end = point(
        last.left() + last.size.width * 0.9,
        last.top() + last.size.height * 0.75,
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.selected_track_ids(),
            [f.tracks[1], f.tracks[3]],
            "painting selects crossed visible rows without adding filtered tracks"
        );
    });
    f.model
        .execute(Command::SetLoopRegion {
            start: 300_000,
            end: 350_000,
            enabled: true,
        })
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_focus.projection.indices, [1, 3])
    });
    let header = visual
        .debug_bounds(selector("track-name", f.tracks[3].value()))
        .unwrap()
        .center();
    visual.simulate_click(header, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track, Some(f.tracks[3]));
        assert_eq!(v.sections.selected_ids(), &[f.sections[0], f.sections[2]]);
        assert_eq!(v.arrangement_focus.projection.indices, [1, 3]);
    });
    select_section(&mut visual, 1, false);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert!(v.arrangement_focus.projection.indices.is_empty());
        assert!(v.focused_neighbor_track(true).is_none());
    });
    select_section(&mut visual, 1, true);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert_eq!(v.arrangement_focus.projection.indices, [0, 1, 2, 3, 4]);
        assert!(v.last_enqueued_edit.is_none());
    });
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.arrangement_focus.projection.enabled)
    });
    select_section(&mut visual, 0, false);
    let button = visual
        .debug_bounds("arrangement-focus-toggle")
        .unwrap()
        .center();
    visual.simulate_click(button, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert_eq!(v.sections.selected_ids(), &f.sections[..1]);
        assert_eq!(v.arrangement_focus.projection.indices, [1, 3]);
    });
    f.model
        .execute(Command::RemoveArrangementSection(f.sections[0]))
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert!(v.sections.selected_ids().is_empty());
        assert_eq!(v.arrangement_focus.projection.indices, [0, 1, 2, 3, 4]);
    });
}
#[gpui::test]
fn q_respects_text_repeat_modifiers_and_filtered_drag_geometry(cx: &mut TestAppContext) {
    let (mut visual, editor, f) = open(cx);
    for key in ["ctrl-q", "alt-q", "shift-q"] {
        visual.simulate_keystrokes(key);
    }
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("q").unwrap(),
        is_held: true,
    });
    editor.read_with(&visual, |v, _| {
        assert!(!v.arrangement_focus.projection.enabled)
    });
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("q");
    visual.run_until_parked();
    editor.read_with(&visual, |v, cx| {
        assert_eq!(v.browser.search.read(cx).text(), "q");
        assert!(!v.arrangement_focus.projection.enabled);
    });
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.set_mode(crate::editor::EditorMode::Session);
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("q");
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled)
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.set_mode(crate::editor::EditorMode::Arrangement);
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    let source = visual
        .debug_bounds(selector("track-name", f.tracks[3].value()))
        .unwrap();
    let target = visual
        .debug_bounds(selector("arrangement-track-header", f.tracks[1].value()))
        .unwrap();
    let to = point(source.center().x, target.top() + px(1.0));
    visual.simulate_mouse_down(source.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let line = visual
        .debug_bounds("track-insertion-line")
        .expect("filtered gap is a real insertion target");
    assert!((line.top() - target.top()).abs() <= px(1.0));
    visual.simulate_keystrokes("q");
    editor.read_with(&visual, |v, _| {
        assert!(
            v.arrangement_focus.projection.enabled,
            "capture must keep its row projection"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| assert!(v.last_enqueued_edit.is_none()));
}

#[gpui::test]
fn focused_hidden_runs_are_marked_without_adding_row_space(cx: &mut TestAppContext) {
    let (mut visual, editor, f) = open(cx);
    let lane = |id| selector("arrangement-lane", id);
    let before = |id| selector("section-hidden-before", id);
    let after = |id| selector("section-hidden-after", id);
    let height = visual
        .debug_bounds(lane(f.tracks[1].value()))
        .unwrap()
        .size
        .height;
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.arrangement_track_indices(),
            &[1, 3],
            "enabled {}, selected {:?}, status {}, error {:?}",
            view.arrangement_focus.projection.enabled,
            view.sections.selected_ids(),
            view.status,
            view.error.as_ref().map(|e| e.to_string())
        )
    });
    let pixel = visual.update(|window, _| px(1.0 / window.scale_factor().max(1.0)));
    let scale = visual.update(|window, _| window.scale_factor());
    let line_color: gpui::Background = hidden_tracks::line_color().into();
    let secondary_color: gpui::Background =
        gpui::rgb(theme::color(theme::PaletteRole::Secondary)).into();
    let assert_dashed_lane_cue = |visual: &mut VisualTestContext, name: &'static str| {
        assert_eq!(
            visual.rendered_background(name),
            None,
            "gap cue must never paint a solid full-width bar"
        );
        let quads = visual.rendered_filled_quads(name);
        let dashes: Vec<_> = quads
            .iter()
            .filter(|(bounds, bg)| {
                *bg == line_color
                    && bounds.size.height == pixel.scale(scale)
                    && bounds.size.width <= px(4.0).scale(scale)
            })
            .collect();
        assert!(
            dashes.len() >= 2,
            "gap cue must render multiple dashed neutral segments"
        );
        assert!(
            quads.iter().all(|(_, bg)| *bg != secondary_color),
            "gap cue must never use PaletteRole::Secondary"
        );
    };
    for (name, track, bottom) in [
        (before(f.tracks[1].value()), f.tracks[1], false),
        (before(f.tracks[3].value()), f.tracks[3], false),
        (after(f.tracks[3].value()), f.tracks[3], true),
    ] {
        let mark = visual.debug_bounds(name).expect("hidden track gap marker");
        let row = visual.debug_bounds(lane(track.value())).unwrap();
        let divider = visual
            .debug_bounds(selector("lane-right-divider", track.value()))
            .unwrap();
        let header = visual
            .debug_bounds(selector("arrangement-track-header", track.value()))
            .unwrap();
        assert_eq!(mark.size.height, pixel);
        assert_eq!(row.size.height, height, "markers never add row height");
        if bottom {
            assert_eq!(mark.bottom(), row.bottom());
        } else {
            assert_eq!(mark.top(), row.top());
        }
        assert_eq!(mark.left(), row.left());
        assert!(
            mark.right() <= divider.left() && mark.right() <= header.left(),
            "gap cue must stay inside timeline lane before lane-right-divider and header"
        );
        assert_dashed_lane_cue(&mut visual, name);
    }
    assert!(
        visual.debug_bounds(after(f.tracks[1].value())).is_none(),
        "interior run has one marker, above the next visible row"
    );
    select_section(&mut visual, 1, false);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.arrangement_track_indices().is_empty())
    });
    let empty = visual
        .debug_bounds("section-hidden-all")
        .expect("one all-hidden run");
    assert_eq!(empty.size.height, pixel);
    let main_header = visual.debug_bounds("main-track-header").unwrap();
    assert!(
        empty.right() < main_header.left(),
        "empty-state gap cue must stay bounded to the timeline lane"
    );
    assert_dashed_lane_cue(&mut visual, "section-hidden-all");
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    for track in &f.tracks {
        assert!(visual.debug_bounds(before(track.value())).is_none());
        assert!(visual.debug_bounds(after(track.value())).is_none());
    }
    assert!(visual.debug_bounds("section-hidden-all").is_none());
    assert_eq!(
        visual
            .debug_bounds(lane(f.tracks[1].value()))
            .unwrap()
            .size
            .height,
        height
    );
}

#[gpui::test]
fn only_selected_sections_in_q_focus_receive_the_pattern(cx: &mut TestAppContext) {
    let (mut visual, editor, f) = open(cx);
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(super::super::viewport::Viewport {
            start: 0,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_none()
    );
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    assert_eq!(
        editor.read_with(&visual, |view, _| view.sections.selected),
        Some(f.sections[0])
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_none()
    );
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-1")
            .is_none()
    );
    select_section(&mut visual, 1, true);
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-1")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-2")
            .is_none()
    );
    select_section(&mut visual, 1, false);
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-1")
            .is_some()
    );
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-0")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("arrangement-section-focus-hatch-1")
            .is_none()
    );
}

#[gpui::test]
fn hidden_track_cues_ignore_folds_tags_and_clear_across_edits_scroll_and_exit(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, mut f) = open(cx);
    // Group tracks[2] so folding its parent creates a non-contiguous visible track index gap.
    let Created::Track(group) = f
        .model
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[f.tracks[2]]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    // Add extra tracks with clips in section 0 so we can scroll offscreen and back.
    let mut extras = Vec::new();
    for index in 0..10 {
        let Created::Track(track) = f
            .model
            .execute(Command::AddTrack {
                name: format!("extra {index}"),
            })
            .unwrap()
        else {
            panic!("extra track")
        };
        f.model
            .execute(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::from_quarters(2).unwrap(),
                length: hex_project::BeatTime::ONE,
            })
            .unwrap();
        extras.push(track);
    }
    // Put a clip in section 0 on tracks[0] and tracks[4] too, and tag tracks[1], tracks[3], extras.
    // Then fold `group` and filter by tag `keep`: tracks[0], group, tracks[2], tracks[4] are
    // excluded from console_indices by fold/tag, while tracks[1], tracks[3], and extras remain.
    let tag = hex_project::TrackTag::new("keep").unwrap();
    for track in [f.tracks[1], f.tracks[3]]
        .into_iter()
        .chain(extras.iter().copied())
    {
        f.model
            .execute(Command::SetTrackTags {
                track,
                tags: vec![tag.clone()],
            })
            .unwrap();
    }
    // Also place a clip in section 0 on tracks[2] inside `group`, and fold `group`.
    f.model
        .execute(Command::PlaceMidiClip {
            track: f.tracks[2],
            start: hex_project::BeatTime::from_quarters(2).unwrap(),
            length: hex_project::BeatTime::ONE,
        })
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());

    let before = |id: TrackId| selector("section-hidden-before", id.value());
    let after = |id: TrackId| selector("section-hidden-after", id.value());
    let assert_no_gap_cues = |visual: &mut VisualTestContext, ids: &[TrackId]| {
        for &id in ids {
            assert!(
                visual.debug_bounds(before(id)).is_none(),
                "unexpected before cue on track {}",
                id.value()
            );
            assert!(
                visual.debug_bounds(after(id)).is_none(),
                "unexpected after cue on track {}",
                id.value()
            );
        }
    };

    let all_ids: Vec<_> = f
        .tracks
        .iter()
        .copied()
        .chain([group])
        .chain(extras.iter().copied())
        .collect();

    // 1. Fold `group` and enable section focus on section 0 where tracks[0] & tracks[4] have no clips:
    // Verify folded child f.tracks[2] produces no gap cue when tracks[3] (tagged `keep`) is shown,
    // and when tag filter `keep` is active (excluding tracks[0], group, tracks[2], tracks[4])
    // ALL remaining tracks intersect section 0 -> non-contiguous canonical indices, ZERO cues!
    editor.update(&mut visual, |v, cx| {
        v.arrangement_focus.projection.toggle_fold(group);
        v.arrangement_focus.projection.set_tag_filter(Some(tag));
        v.reconcile_arrangement_focus();
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        // Tracks 0, group, 2, 4 are filtered out by tag/fold -> non-contiguous canonical indices!
        assert!(
            v.arrangement_track_indices()
                .windows(2)
                .any(|w| w[1] - w[0] > 1)
        );
        assert_eq!(
            v.arrangement_track_indices(),
            v.arrangement_focus.projection.console_indices.as_slice()
        );
    });
    assert_no_gap_cues(&mut visual, &all_ids);

    // 2. Clear tag filter while keeping `group` folded: `group` remains visible because its folded
    // child `tracks[2]` intersects section 0, while `tracks[0]` and `tracks[4]` are hidden by
    // section focus and folded child `tracks[2]` is omitted by folding (not section focus).
    editor.update(&mut visual, |v, cx| {
        v.arrangement_focus.projection.set_tag_filter(None);
        v.reconcile_arrangement_focus();
        cx.notify();
    });
    visual.run_until_parked();
    // Deactivate clips[0] on tracks[1] so tracks[1] is also hidden by section focus, leaving
    // `group`, `tracks[3]`, and `extras` visible: `before(group)` marks hidden `tracks[0..=1]`,
    // `before(tracks[3])` stays None because `tracks[2]` is hidden by folding, and
    // `before(extras[0])` marks hidden `tracks[4]`.
    f.model
        .execute(Command::SetClipActive {
            clip: f.clips[0],
            active: false,
        })
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());
    assert!(visual.debug_bounds(before(group)).is_some());
    assert!(
        visual.debug_bounds(before(f.tracks[3])).is_none(),
        "folded child between group and tracks[3] must not emit a gap cue"
    );
    assert!(visual.debug_bounds(before(extras[0])).is_some());
    assert_no_gap_cues(&mut visual, &extras[1..]);

    // 3. Distinction from `track-insertion-line`: start track reorder drag while section focus is active.
    let source = visual
        .debug_bounds(selector("track-name", extras[0].value()))
        .unwrap();
    let target = visual
        .debug_bounds(selector("arrangement-track-header", f.tracks[3].value()))
        .unwrap();
    let to = point(source.center().x, target.top() + px(1.0));
    visual.simulate_mouse_down(source.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let insertion = visual
        .debug_bounds("track-insertion-line")
        .expect("reorder insertion line must appear");
    assert_eq!(
        visual.rendered_background("track-insertion-line"),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Secondary)).into())
    );
    let cue_bounds = visual.debug_bounds(before(extras[0])).unwrap();
    assert!(
        insertion.size.width > cue_bounds.size.width,
        "full-row Secondary reorder line spans header + lane, distinct from lane-bounded gap cue"
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());

    // 4. Edit (re-activate clips[0]) + selection change + scroll offscreen and back.
    f.model
        .execute(Command::SetClipActive {
            clip: f.clips[0],
            active: true,
        })
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());
    // Re-activating clips[0] reveals f.tracks[1] above the anchored top row (`group`), so scroll
    // to item 0 to inspect the newly revealed row before scrolling offscreen and back.
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds(before(f.tracks[1])).is_some());
    assert!(visual.debug_bounds(before(extras[0])).is_some());
    // Scroll down in a shorter window so top rows go offscreen, then scroll back to top.
    visual.simulate_resize(size(px(1280.0), px(380.0)));
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 5,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    visual.run_until_parked();
    // Click an extra track header to test selection change while scrolled.
    let extra_header = visual
        .debug_bounds(selector("track-name", extras[5].value()))
        .unwrap()
        .center();
    visual.simulate_click(extra_header, Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.track_scroll.list.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    assert!(visual.debug_bounds(before(f.tracks[1])).is_some());

    // 5. Exit section focus with Q: every section-hidden-* cue clears cleanly.
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    assert_no_gap_cues(&mut visual, &all_ids);
}

#[gpui::test]
fn section_focus_lifecycle_guards_empty_state_project_replace_and_mode_switch(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, f) = open(cx);
    // 1. Toggling arrangement focus with state == None must remain a no-op.
    editor.update(&mut visual, |v, cx| {
        let saved = v.state.take();
        v.toggle_arrangement_focus();
        assert!(!v.arrangement_focus.projection.enabled);
        v.state = saved;
        cx.notify();
    });
    visual.run_until_parked();

    // 2. Active section focus resets eagerly on project_replaced.
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    select_section(&mut visual, 0, false);
    assert!(
        visual
            .debug_bounds(selector("section-hidden-before", f.tracks[1].value()))
            .is_some()
    );
    editor.update(&mut visual, |v, _| {
        v.service
            .submit(hex_session::service::Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            let replaced = event.project_replaced;
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            if replaced {
                break;
            }
        } else {
            assert!(
                std::time::Instant::now() < deadline,
                "replacement event before deadline"
            );
            std::thread::yield_now();
        }
    }
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.arrangement_focus.projection.enabled);
        assert!(v.arrangement_focus.pending_restore.is_none());
    });
    for &track in &f.tracks {
        assert!(
            visual
                .debug_bounds(selector("section-hidden-before", track.value()))
                .is_none()
        );
        assert!(
            visual
                .debug_bounds(selector("section-hidden-after", track.value()))
                .is_none()
        );
    }

    // 3. Switching Arrangement -> Session mid-loop-ACK cancels pending loop focus and
    // loop_state_installed in Session mode must not enable arrangement focus.
    let empty_project = Project::new(48_000).unwrap();
    replace(&mut visual, &editor, empty_project);
    // Empty project with Q enabled and no sections selected must not show "no clips in selected sections".
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert!(!v.arrangement_focus.projection.all_tracks_hidden());
    });
    visual.simulate_keystrokes("q");
    visual.run_until_parked();

    // Restore fixture project with sections for pending-loop mode switch check.
    let mut model = f.model;
    replace(&mut visual, &editor, model.project().clone());
    select_section(&mut visual, 1, false);
    let region = editor.read_with(&visual, |v, _| {
        let (start, end) = v.section_loop_bounds().unwrap().unwrap();
        LoopRegion::new(start, end, true).unwrap()
    });
    editor.update(&mut visual, |v, cx| {
        v.submit_selection_loop_preview(region, true);
        assert!(v.pending_loop_has_focus_intent());
        v.set_mode(crate::editor::EditorMode::Session);
        assert!(
            !v.pending_loop_has_focus_intent(),
            "leaving Arrangement cancels pending loop focus intent"
        );
        cx.notify();
    });
    visual.run_until_parked();
    model
        .execute(Command::SetLoopRegion {
            start: region.start(),
            end: region.end(),
            enabled: region.enabled(),
        })
        .unwrap();
    replace(&mut visual, &editor, model.project().clone());
    editor.update(&mut visual, |v, cx| {
        v.loop_state_installed(true);
        assert!(
            !v.arrangement_focus.projection.enabled,
            "loop ACK in Session mode must not arm Arrangement focus"
        );
        v.set_mode(crate::editor::EditorMode::Arrangement);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(!v.arrangement_focus.projection.enabled);
    });
}
