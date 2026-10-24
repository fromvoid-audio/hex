use super::*;
use crate::editor::{EditorView, clip_drag_ui_test::editor_window};
use gpui::{
    Entity, KeyDownEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px, size,
};
use hex_project::{BeatTime, Command, Created, Editor};
use std::sync::Arc;

struct Fixture {
    model: Editor,
    tracks: Vec<TrackId>,
    clips: Vec<ClipId>,
    asset: hex_project::AssetId,
    sections: Vec<SectionId>,
}
fn model() -> Fixture {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let add = |model: &mut Editor, name: &str| {
        let Created::Track(id) = model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        id
    };
    let empty = add(&mut model, "empty");
    let Created::Imported {
        track: audio,
        clip,
        asset,
    } = model
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "focus.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let tracks = vec![
        empty,
        audio,
        add(&mut model, "outside"),
        add(&mut model, "inside"),
        add(&mut model, "late"),
    ];
    let Created::Clip(outside_audio) = model.execute(Command::DuplicateClip(clip)).unwrap() else {
        panic!("duplicate audio")
    };
    model
        .execute(Command::MoveClip {
            clip: outside_audio,
            track: tracks[1],
            start: 192_000,
        })
        .unwrap();
    let mut clips = vec![clip];
    for (track, start, length) in [
        (tracks[2], 0, 2),
        (tracks[3], 2, 1),
        (tracks[4], 3, 1),
        (tracks[1], 6, 1),
    ] {
        let Created::Clip(id) = model
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::from_quarters(length).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        clips.push(id);
    }
    clips.push(outside_audio);
    let mut sections = Vec::new();
    for (start, end) in [(2, 3), (4, 5), (6, 7)] {
        let Created::ArrangementSection(id) = model
            .execute(Command::AddArrangementSection {
                name: format!("section {}", sections.len()),
                color: hex_project::ElementColor::Cyan,
                start: BeatTime::from_quarters(start).unwrap(),
                end: BeatTime::from_quarters(end).unwrap(),
            })
            .unwrap()
        else {
            panic!("section")
        };
        sections.push(id);
    }
    model
        .execute(Command::SetLoopRegion {
            start: 48_000,
            end: 72_000,
            enabled: false,
        })
        .unwrap();
    Fixture {
        model,
        tracks,
        clips,
        asset,
        sections,
    }
}
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
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
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

fn select_section(visual: &mut VisualTestContext, index: u64, shift: bool) {
    let bounds = visual
        .debug_bounds(selector("arrangement-section", index))
        .unwrap();
    visual.simulate_mouse_down(
        bounds.center(),
        MouseButton::Left,
        Modifiers {
            shift,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    visual.simulate_mouse_up(
        bounds.center(),
        MouseButton::Left,
        Modifiers {
            shift,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
}

#[test]
fn projection_uses_cached_section_union_and_half_open_boundaries() {
    let f = model();
    let mut state = State::default();
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    state.enabled = true;
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    assert_eq!(state.indices, [1, 3]);
    assert_eq!(
        state.clips,
        BTreeSet::from([f.clips[0], f.clips[2], f.clips[4], f.clips[5]])
    );
    assert!(
        !state.rebuild(1, 1, f.model.project(), &f.sections[..1], None),
        "unchanged paints must not scan the project again"
    );
    assert!(!state.rebuild(1, 1, f.model.project(), &f.sections[..1], None));
    assert!(state.rebuild(
        1,
        1,
        f.model.project(),
        &[f.sections[0], f.sections[2]],
        None
    ));
    assert_eq!(state.indices, [1, 3]);
    assert_eq!(
        state.clips,
        BTreeSet::from([f.clips[0], f.clips[2], f.clips[4], f.clips[5]])
    );
    assert!(!state.clips.contains(&f.clips[3]), "gap is excluded");
    assert!(state.rebuild(1, 1, f.model.project(), &f.sections[1..2], None));
    assert!(
        state.indices.is_empty(),
        "audio ending at the section start is excluded"
    );
    assert!(state.rebuild(1, 1, f.model.project(), &[], None));
    assert!(state.enabled, "no selection leaves the mode armed");
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
    assert!(state.rebuild(2, 1, f.model.project(), &f.sections[..1], None));
    assert!(
        !state.enabled,
        "replacement document must start in the normal view"
    );
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
}
#[test]
fn new_track_exception_expires_without_repinning_on_history_or_focus_changes() {
    let mut f = model();
    let mut state = State {
        enabled: true,
        ..State::default()
    };
    state.rebuild(1, 1, f.model.project(), &f.sections[..1], None);
    let Created::Track(first) = f
        .model
        .execute(Command::AddTrack {
            name: "first new".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    state.reveal_created_track(first);
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first));
    assert!(state.tracks.contains(&first));
    assert!(!state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first)));
    // Losing the active track invalidates the cached rows at the same revision.
    assert!(state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(f.tracks[1])));
    assert!(!state.tracks.contains(&first));
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], Some(first));
    assert!(
        !state.tracks.contains(&first),
        "selection cannot repin an empty track"
    );

    state.reveal_created_track(first);
    let Created::Track(second) = f
        .model
        .execute(Command::AddTrack {
            name: "second new".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    state.reveal_created_track(second);
    state.rebuild(1, 3, f.model.project(), &f.sections[..1], Some(second));
    assert!(state.tracks.contains(&second));
    assert!(!state.tracks.contains(&first));
    assert!(f.model.undo());
    state.rebuild(1, 4, f.model.project(), &f.sections[..1], Some(second));
    assert!(f.model.redo());
    state.rebuild(1, 5, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "redo does not revive stale visibility"
    );

    state.reveal_created_track(second);
    state.rebuild(1, 6, f.model.project(), &f.sections[1..2], Some(second));
    assert!(!state.tracks.contains(&second));
    state.rebuild(1, 6, f.model.project(), &f.sections[..1], Some(second));
    assert!(!state.tracks.contains(&second));
    state.reveal_created_track(second);
    f.model
        .execute(Command::SetArrangementSectionRange {
            section: f.sections[0],
            start: BeatTime::ZERO,
            end: BeatTime::ONE,
        })
        .unwrap();
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "editing the focused range retires the exception"
    );
    state.reveal_created_track(second);
    state.enabled = false;
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    state.enabled = true;
    state.rebuild(1, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(!state.tracks.contains(&second));
    state.reveal_created_track(second);
    state.rebuild(2, 7, f.model.project(), &f.sections[..1], Some(second));
    state.enabled = true;
    state.rebuild(2, 7, f.model.project(), &f.sections[..1], Some(second));
    assert!(
        !state.tracks.contains(&second),
        "document identities cannot retain an exception"
    );
}

#[test]
fn section_projection_follows_canonical_tempo_range_and_removal() {
    let mut f = model();
    let mut state = State {
        enabled: true,
        ..State::default()
    };
    state.rebuild(1, 1, f.model.project(), &f.sections[..1], None);
    f.model
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    state.rebuild(1, 2, f.model.project(), &f.sections[..1], None);
    assert_eq!(state.ranges.as_ref(), &[(96_000, 144_000)]);
    assert_eq!(state.indices, [3]);
    f.model
        .execute(Command::SetArrangementSectionRange {
            section: f.sections[0],
            start: BeatTime::ZERO,
            end: BeatTime::ONE,
        })
        .unwrap();
    state.rebuild(1, 3, f.model.project(), &f.sections[..1], None);
    assert_eq!(state.indices, [1, 2]);
    f.model
        .execute(Command::RemoveArrangementSection(f.sections[0]))
        .unwrap();
    state.rebuild(1, 4, f.model.project(), &f.sections[..1], None);
    assert!(state.enabled);
    assert_eq!(state.indices, [0, 1, 2, 3, 4]);
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
        assert_eq!(v.arrangement_focus.indices, [1, 3]);
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
            v.clip_handle_at_pointer(f.tracks[1], x, lane.top + 3.0, viewport),
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
        assert!(v.arrangement_focus.enabled);
        assert_eq!(v.arrangement_focus.indices, [0, 1, 2, 3, 4]);
    });
    select_section(&mut visual, 0, false);
    select_section(&mut visual, 2, true);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.arrangement_focus.indices, [1, 3]);
        assert!(v.arrangement_clip_visible(f.clips[4]));
        assert!(!v.arrangement_clip_visible(f.clips[3]));
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
        assert_eq!(v.arrangement_focus.indices, [1, 3])
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
        assert_eq!(v.arrangement_focus.indices, [1, 3]);
    });
    select_section(&mut visual, 1, false);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.enabled);
        assert!(v.arrangement_focus.indices.is_empty());
        assert!(v.focused_neighbor_track(true).is_none());
    });
    select_section(&mut visual, 1, true);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.enabled);
        assert_eq!(v.arrangement_focus.indices, [0, 1, 2, 3, 4]);
        assert!(v.last_enqueued_edit.is_none());
    });
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(!v.arrangement_focus.enabled));
    select_section(&mut visual, 0, false);
    let button = visual
        .debug_bounds("arrangement-focus-toggle")
        .unwrap()
        .center();
    visual.simulate_click(button, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.enabled);
        assert_eq!(v.sections.selected_ids(), &f.sections[..1]);
        assert_eq!(v.arrangement_focus.indices, [1, 3]);
    });
    f.model
        .execute(Command::RemoveArrangementSection(f.sections[0]))
        .unwrap();
    replace(&mut visual, &editor, f.model.project().clone());
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.enabled);
        assert!(v.sections.selected_ids().is_empty());
        assert_eq!(v.arrangement_focus.indices, [0, 1, 2, 3, 4]);
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
    editor.read_with(&visual, |v, _| assert!(!v.arrangement_focus.enabled));
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("q");
    visual.run_until_parked();
    editor.read_with(&visual, |v, cx| {
        assert_eq!(v.browser.search.read(cx).text(), "q");
        assert!(!v.arrangement_focus.enabled);
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
    editor.read_with(&visual, |v, _| assert!(v.arrangement_focus.enabled));
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
            v.arrangement_focus.enabled,
            "capture must keep its row projection"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insertion-line").is_none());
    editor.read_with(&visual, |v, _| assert!(v.last_enqueued_edit.is_none()));
}
