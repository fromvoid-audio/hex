use super::*;
pub(super) use crate::editor::clip_drag_ui_test::pump_move as pump;
use crate::editor::clip_drag_ui_test::upper_rail_owned_fixture;
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
use hex_project::Command;
use hex_session::service::Request;
use std::sync::Arc;

#[gpui::test]
fn refused_cut_keeps_previous_clipboard_and_restores_range_after_real_terminal(
    cx: &mut TestAppContext,
) {
    let (files, mut visual, editor, clip) = open(cx);
    // A legacy looping placement plays from its short original without a large
    // derived buffer. An inner edit must refuse the over-budget materialization
    // before allocating it; this exercises an actual prepare-before-commit error.
    let length = (hex_engine::audio_transform::MAX_RETAINED_DERIVED_BYTES
        / std::mem::size_of::<hex_engine::Frame>()) as u64
        + 1;
    let path = files.root().join("refused-cut.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    let (mut snapshot, original_path) = editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        (
            project.snapshot(),
            project
                .asset(project.clip(clip).unwrap().asset())
                .unwrap()
                .path()
                .to_owned(),
        )
    });
    let original_bytes = std::fs::read(&original_path).unwrap();
    let mut changed = false;
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            hex_project::TrackContentSnapshot::Audio(clips) => clips,
            hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            hex_project::TrackContentSnapshot::Instrument(_) => continue,
        };
        if let Some(target) = clips.iter_mut().find(|target| target.id == clip.value()) {
            target.start = 0;
            target.duration = length;
            target.looping = true;
            target.source_phase = 0;
            target.edit = None;
            target.fade = hex_project::ClipFade::neutral(length).unwrap();
            changed = true;
        }
    }
    assert!(changed);
    let seeded = hex_project::Project::from_snapshot(snapshot).unwrap();
    // Start from the real current writer, keeping every unrelated inventory.
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(document["tracks"][0]["audio_clips"].is_array());
    let stored = &mut document["tracks"][0]["audio_clips"][0];
    assert_eq!(stored["id"].as_u64(), Some(clip.value()));
    stored["start"] = 0.into();
    stored["duration"] = length.into();
    stored["looping"] = true.into();
    stored["source_phase"] = 0.into();
    stored["fade"] = serde_json::json!({
        "span": length, "offset": 0, "fade_in": 0, "fade_out": 0
    });
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            seeded.snapshot()
        );
        v.selected_track = Some(v.state.as_ref().unwrap().project.tracks()[0].id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        assert_eq!(v.event_context().unwrap().1.domain.clock.length(), length);
        v.audio_event_editor.viewport = Some(Viewport {
            start: 0,
            length: 96_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    select(&mut visual, 0.0, 0.125);
    visual.simulate_keystrokes("cmd-c");
    select(&mut visual, 0.125, 0.375);
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.audio_event_editor.clipboard.as_ref().unwrap().length,
            12_000
        );
        assert!(!v.state.as_ref().unwrap().can_undo);
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("cmd-x");
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(std::time::Instant::now() < deadline, "real cut terminal");
        std::thread::yield_now();
    };
    assert!(
        terminal.error.is_some(),
        "derived root must exceed its budget"
    );
    let error = format!("{:?}", terminal.error);
    // #4145 names the bounded transform domain and its recovery path.
    assert!(
        error.contains("support clips up to") && error.contains("trim or split the clip first"),
        "expected bounded transform-domain refusal: {error}"
    );
    for _ in 0..3 {
        repaint(&mut visual, &editor);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.audio_event_editor.clipboard.as_ref().unwrap().length,
                12_000
            );
            assert!(v.audio_event_editor.range.is_none());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    repaint(&mut visual, &editor);
    assert!(visual.debug_bounds("audio-event-time-selection").is_some());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.audio_event_editor.clipboard.as_ref().unwrap().length,
            12_000
        );
        assert_eq!(v.audio_event_editor.range, Some((12_000, 36_000)));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(
            !v.state.as_ref().unwrap().can_undo,
            "refusal adds no history"
        );
        assert!(v.error.is_some());
    });
    // The same oversized source refuses event gain during preparation. Its
    // released value remains displayed until the real terminal arrives.
    editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.range = None;
        v.error = None;
        cx.notify();
    });
    repaint(&mut visual, &editor);
    let grip = visual.debug_bounds("audio-event-gain-1").unwrap().center();
    let moved = point(grip.x, grip.y + px(20.0));
    visual.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real event gain terminal"
        );
        std::thread::yield_now();
    };
    assert!(terminal.error.is_some());
    for _ in 0..3 {
        repaint(&mut visual, &editor);
        assert!(visual.debug_bounds("audio-event-gain-value-1").is_some());
        editor.read_with(&visual, |v, _| {
            assert!(v.audio_event_editor.pending.is_some());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!v.state.as_ref().unwrap().can_undo);
        assert!(v.error.is_some());
    });
    assert_eq!(std::fs::read(original_path).unwrap(), original_bytes);
}

pub(super) fn open(
    cx: &mut TestAppContext,
) -> (
    Arc<crate::editor::test_environment::Environment>,
    VisualTestContext,
    Entity<EditorView>,
    ClipId,
) {
    let (files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1280.0), px(800.0)));
    let clip = editor.update(&mut visual, |v, cx| {
        let track = &v.state.as_ref().unwrap().project.tracks()[0];
        let clip = track.clips()[0].id();
        v.selected_track = Some(track.id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-events-waveform").is_some());
    (files, visual, editor, clip)
}
pub(super) fn edit(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    clip: ClipId,
    operation: AudioEventEdit,
) {
    editor.update(&mut *visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::EditAudioEvents { clip, operation })))
    });
    pump(editor, visual);
}
fn select(visual: &mut VisualTestContext, from: f32, to: f32) {
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    // Paint blank ruler time below the distinct playback-start grab target.
    let down = point(b.left() + b.size.width * from, b.top() + px(16.0));
    let up = point(b.left() + b.size.width * to, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
fn repaint(visual: &mut VisualTestContext, editor: &Entity<EditorView>) {
    editor.update(&mut *visual, |_, cx| cx.notify());
    visual.run_until_parked();
}

fn render_state(v: &EditorView) -> Vec<hex_engine::Frame> {
    let state = v.state.as_ref().unwrap();
    let media = state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .required_sources()
        .map(|(id, binding)| {
            let hex_session::AudioSourceBinding::Resident(audio) = binding else {
                panic!("resident fixture")
            };
            (id, audio.clone())
        })
        .collect();
    let prepared = hex_engine::PreparedRevision::prepare_with_audio_transforms(
        &state.project,
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
    )
    .unwrap();
    let mut renderer = hex_engine::Renderer::new(prepared.arrangement());
    renderer.play();
    let mut pcm = vec![[0.0; 2]; 96_000];
    for chunk in pcm.chunks_mut(257) {
        renderer.render(chunk).unwrap();
    }
    pcm
}

#[gpui::test]
fn event_overlap_accent_covers_only_replaced_audio_and_survives_release(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, clip) = open(cx);
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: 48_000 },
    );
    editor.update(&mut visual, |v, _| {
        v.audio_event_editor.grid = SnapGrid::Off
    });
    let body = visual.debug_bounds("audio-events-waveform").unwrap();
    // The top five pixels now own event gain; move from the remaining rail.
    let down = point(body.left() + body.size.width * 0.125, body.top() + px(32.0));
    let up = point(body.left() + body.size.width * 0.375, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let overlap = visual
        .debug_bounds("audio-event-overlap-0")
        .expect("occupied replacement accent");
    assert!((overlap.left() - (body.left() + body.size.width * 0.5)).abs() <= px(1.0));
    assert!((overlap.size.width - body.size.width * 0.25).abs() <= px(1.0));
    let fill = visual.rendered_background("audio-event-overlap-0").unwrap();
    let before = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    repaint(&mut visual, &editor);
    assert!(visual.debug_bounds("audio-event-overlap-0").is_none());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("audio-event-overlap-0"), Some(overlap));
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    for _ in 0..2 {
        repaint(&mut visual, &editor);
        assert_eq!(visual.debug_bounds("audio-event-overlap-0"), Some(overlap));
        assert_eq!(
            visual.rendered_background("audio-event-overlap-0"),
            Some(fill)
        );
    }
    pump(&editor, &mut visual);
    assert!(visual.debug_bounds("audio-event-overlap-0").is_none());
    assert!(visual.debug_bounds("audio-event-1").is_some());
}

#[gpui::test]
fn resampled_source_range_paste_matches_normalized_pcm_offsets(cx: &mut TestAppContext) {
    let (files, mut visual, editor, _) = open(cx);
    let (old_track, path) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (
            p.tracks()[0].id(),
            p.assets().next().unwrap().path().to_owned(),
        )
    });
    let mut wav = std::fs::read(path).unwrap();
    wav.truncate(44 + 48_000 * 2);
    wav[4..8].copy_from_slice(&(36 + 48_000_u32 * 2).to_le_bytes());
    wav[24..28].copy_from_slice(&24_000_u32.to_le_bytes());
    wav[28..32].copy_from_slice(&48_000_u32.to_le_bytes());
    wav[40..44].copy_from_slice(&(48_000_u32 * 2).to_le_bytes());
    for (i, sample) in wav[44..].chunks_exact_mut(2).enumerate() {
        sample.copy_from_slice(&(((i % 101) as i16 - 50) * 128).to_le_bytes());
    }
    let path = files.root().join("source-24k.wav");
    std::fs::write(&path, &wav).unwrap();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Import(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::Mute {
            track: old_track,
            muted: true
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        let p = &v.state.as_ref().unwrap().project;
        let track = p.tracks().last().unwrap();
        let clip = &track.clips()[0];
        let asset = p.asset(clip.asset()).unwrap();
        assert_eq!(asset.source_sample_rate(), 24_000);
        assert_eq!(asset.playback_frames(), 96_000);
        v.selected_track = Some(track.id());
        v.selected_clip = Some(clip.id());
        v.device_panel_active = false;
        cx.notify();
    });
    visual.run_until_parked();
    let baseline = editor.read_with(&visual, |v, _| render_state(v));
    select(&mut visual, 0.125, 0.375);
    visual.simulate_keystrokes("cmd-c");
    select(&mut visual, 0.375, 0.375);
    visual.simulate_keystrokes("cmd-v");
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.audio_event_editor
                .pending
                .as_ref()
                .unwrap()
                .waveform
                .is_some()
        )
    });
    pump(&editor, &mut visual);
    let mut expected = baseline.clone();
    expected[36_000..60_000].copy_from_slice(&baseline[12_000..36_000]);
    editor.read_with(&visual, |v, _| assert_eq!(render_state(v), expected));
    assert_eq!(std::fs::read(path).unwrap(), wav);
}

#[gpui::test]
fn duplicate_partial_range_with_gaps_retains_destination_through_real_ack_and_repeats(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, clip) = open(cx);
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Delete {
            start: 24_000,
            end: 48_000,
        },
    );
    select(&mut visual, 0.125, 0.375);
    let before = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    let source = visual.debug_bounds("audio-event-time-selection").unwrap();
    visual.simulate_keystrokes("cmd-d");
    repaint(&mut visual, &editor);
    let destination = visual.debug_bounds("audio-event-time-selection").unwrap();
    assert!(
        destination.left() > source.right() - px(1.0),
        "duplicate selects the destination"
    );
    let fill = visual.rendered_background("audio-event-time-selection");
    assert!(fill.is_some());
    for _ in 0..2 {
        repaint(&mut visual, &editor);
        assert_eq!(
            visual.debug_bounds("audio-event-time-selection"),
            Some(destination)
        );
        assert_eq!(
            visual.rendered_background("audio-event-time-selection"),
            fill
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before)
        });
    }
    pump(&editor, &mut visual);
    assert_eq!(
        visual.debug_bounds("audio-event-time-selection"),
        Some(destination)
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.range, Some((36_000, 60_000)));
        let events = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events();
        assert_eq!(
            events
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            [
                (0, 24_000, 0),
                (36_000, 48_000, 12_000),
                (60_000, 96_000, 60_000)
            ]
        );
    });
    let once = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.range, Some((60_000, 84_000)))
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, once)
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, once)
    });
}

#[gpui::test]
fn released_event_leading_trim_keeps_details_until_ack(cx: &mut TestAppContext) {
    check_trim_release(cx, true, false);
}

#[gpui::test]
fn released_event_trailing_trim_keeps_details_until_ack(cx: &mut TestAppContext) {
    check_trim_release(cx, false, false);
}

#[gpui::test]
fn released_event_leading_trim_outside_keeps_details_until_ack(cx: &mut TestAppContext) {
    check_trim_release(cx, true, true);
}

#[gpui::test]
fn released_event_trailing_trim_outside_keeps_details_until_ack(cx: &mut TestAppContext) {
    check_trim_release(cx, false, true);
}

fn check_trim_release(cx: &mut TestAppContext, leading: bool, outside: bool) {
    let (_files, mut visual, editor, clip) = open(cx);
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: 48_000 },
    );
    let before = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    editor.update(&mut visual, |v, _| {
        v.audio_event_editor.grid = SnapGrid::Off
    });
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    // Trim the shared seam inward, retaining a visible gap and both events.
    let down = point(
        b.left() + b.size.width * 0.5 + px(if leading { 1.0 } else { -1.0 }),
        b.top() + px(26.0),
    );
    let up = point(
        b.left() + b.size.width * if leading { 0.625 } else { 0.375 },
        down.y,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let selector = if leading {
        "audio-event-2"
    } else {
        "audio-event-1"
    };
    let waveform = if leading {
        "audio-event-waveform-2"
    } else {
        "audio-event-waveform-1"
    };
    let held = visual.debug_bounds(selector).unwrap();
    let wave = visual.debug_bounds(waveform).unwrap();
    let fill = visual.rendered_background(selector).unwrap();
    let panel = visual.debug_bounds("arrangement-audio-controls").unwrap();
    let (operation, detail, selected, viewport) = editor.read_with(&visual, |v, _| {
        let gesture = v.audio_event_editor.gesture.as_ref().unwrap();
        assert!(matches!(gesture.kind, Kind::Leading(_) | Kind::Trailing(_)));
        (
            gesture.operation.clone().unwrap(),
            v.detail_audio_clip_data(),
            v.audio_event_editor.selected,
            v.audio_event_editor.viewport,
        )
    });
    assert!(matches!(operation, AudioEventEdit::Trim { .. }));
    let released = if outside {
        point(up.x, b.bottom() + px(8.0))
    } else {
        up
    };
    visual.simulate_mouse_up(released, MouseButton::Left, Modifiers::default());
    for _ in 0..2 {
        repaint(&mut visual, &editor);
        assert_eq!(visual.debug_bounds(selector), Some(held));
        assert_eq!(visual.debug_bounds(waveform), Some(wave));
        assert_eq!(visual.rendered_background(selector), Some(fill));
        assert_eq!(
            visual.debug_bounds("arrangement-audio-controls"),
            Some(panel)
        );
        editor.read_with(&visual, |v, _| {
            assert!(v.audio_event_editor.gesture.is_none());
            assert!(v.audio_event_editor.pending.is_some());
            assert_eq!(v.audio_event_editor.selected, selected);
            assert_eq!(v.audio_event_editor.viewport, viewport);
            assert_eq!(v.detail_audio_clip_data(), detail);
            assert_eq!(v.state.as_ref().unwrap().project, before);
        });
    }
    let mut expected = hex_project::Editor::new((*before).clone(), 8).unwrap();
    expected
        .execute(Command::EditAudioEvents { clip, operation })
        .unwrap();
    pump(&editor, &mut visual);
    assert_eq!(visual.debug_bounds(selector), Some(held));
    assert_eq!(visual.debug_bounds(waveform), Some(wave));
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_none());
        assert_eq!(
            v.state.as_ref().unwrap().project.as_ref(),
            expected.project()
        );
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.as_ref(),
            expected.project()
        );
    });
}

#[gpui::test]
fn released_event_move_keeps_completed_paint_until_its_real_worker_terminal(
    cx: &mut TestAppContext,
) {
    check_event_move_release(cx, false);
}

#[gpui::test]
fn released_event_move_outside_keeps_completed_paint_until_its_real_worker_terminal(
    cx: &mut TestAppContext,
) {
    check_event_move_release(cx, true);
}

fn check_event_move_release(cx: &mut TestAppContext, outside: bool) {
    let (_files, mut visual, editor, clip) = open(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::DuplicateClip(clip))))
    });
    pump(&editor, &mut visual);
    let other = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0]
            .clips()
            .iter()
            .find(|c| c.id() != clip)
            .unwrap()
            .id()
    });
    let name = |id: ClipId| {
        Box::leak(format!("clip-name-{}", id.value()).into_boxed_str()) as &'static str
    };
    let original = visual.debug_bounds(name(clip)).unwrap().center();
    visual.simulate_click(original, Modifiers::default());
    visual.run_until_parked();
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: 24_000 },
    );
    let before = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    let b = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = point(b.left() + b.size.width * 0.125, b.top() + px(32.0));
    let up = point(b.left() + b.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-1").unwrap();
    let fill = visual.rendered_background("audio-event-1");
    assert!(fill.is_some());
    let released = if outside {
        point(up.x, b.bottom() + px(8.0))
    } else {
        up
    };
    visual.simulate_mouse_up(released, MouseButton::Left, Modifiers::default());
    for _ in 0..2 {
        repaint(&mut visual, &editor);
        assert_eq!(
            visual.debug_bounds("audio-event-1"),
            Some(held),
            "release keeps frozen geometry"
        );
        assert_eq!(visual.rendered_background("audio-event-1"), fill);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before)
        });
    }
    let lane = editor.read_with(&visual, |v, _| v.drag_lanes[&v.selected_track.unwrap()]);
    let empty = point(
        px(lane.left + lane.width * 0.95),
        px(lane.top + lane.height * 0.75),
    );
    visual.simulate_click(empty, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-events-waveform").is_none());
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_some())
    });
    let original_header = visual.debug_bounds(name(clip)).unwrap().center();
    visual.simulate_click(original_header, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("audio-event-1"), Some(held));
    let other_header = visual.debug_bounds(name(other)).unwrap().center();
    visual.simulate_click(other_header, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("audio-event-1").unwrap().size.width > held.size.width * 2.0,
        "foreign clip does not paint pending geometry"
    );
    let original_header = visual.debug_bounds(name(clip)).unwrap().center();
    visual.simulate_click(original_header, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("audio-event-1"),
        Some(held),
        "returning inspector recovers pending paint"
    );
    editor.update(&mut visual, |v, cx| {
        v.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("audio-event-1"), Some(held));
    visual.simulate_keystrokes("escape");
    repaint(&mut visual, &editor);
    assert_eq!(visual.debug_bounds("audio-event-1"), Some(held));
    pump(&editor, &mut visual);
    assert_eq!(visual.debug_bounds("audio-event-1"), Some(held));
    editor.read_with(&visual, |v, _| {
        assert_ne!(v.state.as_ref().unwrap().project, before)
    });
}

#[gpui::test]
fn cut_publishes_clipboard_on_ack_and_paste_survives_save_reopen_with_original_media(
    cx: &mut TestAppContext,
) {
    let (files, mut visual, editor, clip) = open(cx);
    let original_path = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .assets()
            .next()
            .unwrap()
            .path()
            .to_owned()
    });
    let original_bytes = std::fs::read(&original_path).unwrap();
    select(&mut visual, 0.0, 0.125);
    visual.simulate_keystrokes("cmd-c");
    select(&mut visual, 0.125, 0.375);
    visual.simulate_keystrokes("cmd-x");
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.audio_event_editor.clipboard.as_ref().unwrap().length,
            12_000,
            "cut does not publish before its terminal"
        )
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.audio_event_editor.clipboard.as_ref().unwrap().length,
            24_000
        )
    });
    let cut = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    select(&mut visual, 0.625, 0.625);
    visual.simulate_keystrokes("cmd-v");
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.audio_event_editor.range, Some((60_000, 84_000)));
        assert_eq!(v.state.as_ref().unwrap().project, cut);
    });
    editor.read_with(&visual, |v, _| {
        let waveform = &v
            .audio_event_editor
            .pending
            .as_ref()
            .unwrap()
            .waveform
            .as_ref()
            .unwrap()
            .waveform;
        assert_eq!(waveform.source_frames, 24_000);
        assert!(waveform.bins.len() <= 1_024);
        assert_eq!(
            waveform.peaks_for_frames(0, 24_000),
            Some(
                [hex_session::CachedPeak {
                    min: 0.25,
                    max: 0.25
                }; 2]
            )
        );
    });
    let pasted_event = editor.read_with(&visual, |v, _| {
        v.audio_event_editor
            .pending
            .as_ref()
            .unwrap()
            .preview
            .events
            .iter()
            .find(|e| e.start == 60_000 && e.source_start == 12_000)
            .unwrap()
            .id
    });
    let waveform = Box::leak(format!("audio-event-waveform-{pasted_event}").into_boxed_str());
    assert!(
        visual.debug_bounds(waveform).is_some(),
        "pasted cut material has a waveform before ACK"
    );
    pump(&editor, &mut visual);
    let pasted = editor.read_with(&visual, |v, _| {
        Arc::clone(&v.state.as_ref().unwrap().project)
    });
    editor.read_with(&visual, |v, _| {
        let mut expected = vec![[0.25; 2]; 96_000];
        expected[12_000..36_000].fill([0.0; 2]);
        assert_eq!(
            render_state(v),
            expected,
            "literal PCM keeps the cut gap and pasted source"
        );
    });
    let saved = files.root().join("audio-event-workflow.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(saved.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(saved))));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project,
            pasted,
            "complete project and media references reopen"
        )
    });
    assert_eq!(
        std::fs::read(original_path).unwrap(),
        original_bytes,
        "source PCM file is preserved"
    );
    assert!(pasted.clip(clip).is_some());
}

#[gpui::test]
fn held_f_shows_event_fade_controls_without_outer_clip_edits(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, _) = open(cx);
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let click = point(bounds.center().x, bounds.bottom() - px(12.0));
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_none());
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("f").unwrap(),
        is_held: false,
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("audio-event-fade-in-top-1").is_some(),
        "held F must expose the real event fade-in control"
    );
    assert!(
        visual.debug_bounds("audio-event-fade-out-top-1").is_some(),
        "held F must expose the real event fade-out control"
    );
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("f").unwrap(),
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_none());
    assert!(visual.debug_bounds("audio-event-fade-out-top-1").is_none());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
    });
}

fn fade_key(visual: &mut VisualTestContext, down: bool) {
    let key = gpui::Keystroke::parse("f").unwrap();
    if down {
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: key,
            is_held: false,
        });
    } else {
        visual.simulate_event(gpui::KeyUpEvent { keystroke: key });
    }
    visual.run_until_parked();
}
fn focus_events(visual: &mut VisualTestContext) {
    let bounds = visual.debug_bounds("audio-events-waveform").unwrap();
    let at = point(bounds.center().x, bounds.bottom() - px(12.0));
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
#[gpui::test]
fn event_fade_drag_keeps_pending_paint_then_one_undo_and_reset(cx: &mut TestAppContext) {
    let (files, mut visual, editor, clip) = open(cx);
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .unwrap()
        .center();
    let target = point(down.x + px(64.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-fade-in-top-1").unwrap();
    assert!(held.center().x > down.x + px(20.0));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(matches!(
            v.audio_event_editor.gesture.as_ref().unwrap().operation,
            Some(AudioEventEdit::SetFade {
                event: 1,
                fade: Some(_)
            })
        ));
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    fade_key(&mut visual, false);
    repaint(&mut visual, &editor);
    assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.gesture.is_none());
        assert!(v.audio_event_editor.pending.is_some());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_keystrokes("escape");
    repaint(&mut visual, &editor);
    assert_eq!(
        visual.debug_bounds("audio-event-fade-in-top-1"),
        Some(held),
        "admitted fade survives input cancellation"
    );
    pump(&editor, &mut visual);
    repaint(&mut visual, &editor);
    let accepted = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(v.audio_event_editor.pending.is_none());
        let event = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        assert!(event.fade.unwrap().shape().fade_in() > 0);
        v.state.as_ref().unwrap().project.snapshot()
    });
    editor.read_with(&visual, |v, _| {
        let pcm = render_accepted_event(v, clip);
        let count = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0]
            .fade
            .unwrap()
            .shape()
            .fade_in() as usize;
        assert_eq!(pcm[0], [0.0; 2]);
        assert!((pcm[1][0] - 0.25 / (count - 1) as f32).abs() < 1e-7);
        assert_eq!(pcm[count - 1], [0.25; 2]);
    });
    assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted)
    });
    let at = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .unwrap()
        .center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    repaint(&mut visual, &editor);
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_none());
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_events()
                .events[0]
                .fade
                .is_none()
        )
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted)
    });
    let original_path = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .assets()
            .next()
            .unwrap()
            .path()
            .to_owned()
    });
    let original_bytes = std::fs::read(&original_path).unwrap();
    let saved = files.root().join("event-fades.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(saved.clone())))
    });
    pump(&editor, &mut visual);
    let previous_generation = editor.read_with(&visual, |v, _| v.project_generation);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(saved))));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_ne!(
            v.project_generation, previous_generation,
            "open installs the saved document"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted);
        assert_eq!(render_accepted_event(v, clip)[0], [0.0; 2]);
        v.selected_track = Some(v.state.as_ref().unwrap().project.tracks()[0].id());
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        cx.notify();
    });
    repaint(&mut visual, &editor);
    assert!(
        visual.debug_bounds("audio-event-fade-in-top-1").is_some(),
        "reopened non-default fades stay visible with F released"
    );
    assert_eq!(std::fs::read(&original_path).unwrap(), original_bytes);
}
#[gpui::test]
fn event_fade_escape_blur_stale_and_nonfinite_capture_never_commit(cx: &mut TestAppContext) {
    for cancel in 0..4 {
        let (_files, mut visual, editor, _) = open(cx);
        focus_events(&mut visual);
        fade_key(&mut visual, true);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = visual
            .debug_bounds("audio-event-fade-out-top-1")
            .unwrap()
            .center();
        let target = point(down.x - px(64.0), down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(
                v.audio_event_editor
                    .gesture
                    .as_ref()
                    .unwrap()
                    .operation
                    .is_some()
            )
        });
        match cancel {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window)),
            2 => editor.update(&mut visual, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            }),
            _ => visual.update(|window, cx| {
                editor.update(cx, |v, cx| {
                    assert!(v.update_audio_event_fade_pointer(
                        point(px(f32::NAN), target.y),
                        false,
                        window,
                        cx
                    ));
                })
            }),
        }
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        fade_key(&mut visual, false);
        repaint(&mut visual, &editor);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "cancel {cancel}"
            );
            assert!(
                v.audio_event_editor.gesture.is_none() && v.audio_event_editor.pending.is_none(),
                "cancel {cancel}: held={} pending={} last={:?}",
                v.audio_event_editor.gesture.is_some(),
                v.audio_event_editor.pending.is_some(),
                v.last_enqueued_edit
            );
            assert!(v.last_enqueued_edit.is_none());
        });
        assert!(visual.debug_bounds("audio-event-fade-out-top-1").is_none());
    }
}

#[gpui::test]
fn event_fade_warped_moved_source_uses_local_clock_after_zoom_and_pan(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, clip) = open(cx);
    editor.update(&mut visual, |v, _| {
        let settings = hex_project::default_audio_warp(
            v.state.as_ref().unwrap().project.clip(clip).unwrap(),
            v.state.as_ref().unwrap().project.tempo_map(),
            hex_project::WarpMode::RePitch,
        )
        .unwrap();
        assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings)
        })));
    });
    pump(&editor, &mut visual);
    let length = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_event_root()
            .domain
            .clock
            .length()
    });
    assert!(length > 96_000, "fixture must use Q32.32 beats");
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: length / 2 },
    );
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Move {
            event: 1,
            start: length / 2,
        },
    );
    editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.viewport = Some(crate::editor::viewport::Viewport {
            start: 24_000,
            length: 72_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    let before = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .clone()
    });
    let down = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .unwrap()
        .center();
    let target = point(down.x + px(48.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-fade-in-top-1").unwrap();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    fade_key(&mut visual, false);
    assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
    pump(&editor, &mut visual);
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let current = v.state.as_ref().unwrap().project.clip(clip).unwrap();
        assert_eq!(
            (current.start(), current.duration()),
            (before.start(), before.duration())
        );
        assert_eq!(
            current.audio_event_root().domain.warp,
            before.audio_event_root().domain.warp
        );
        let event = current.audio_events().events[0];
        assert_eq!((event.start, event.source_start), (length / 2, 0));
        let fade = event.fade.unwrap();
        assert_eq!(fade.source_start(), 0);
        assert_eq!(fade.shape().span(), length / 2);
        assert!(fade.shape().fade_in() > 0);
    });
    assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
}
#[gpui::test]
fn event_fade_f_respects_computer_keyboard_and_search_focus(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, _) = open(cx);
    focus_events(&mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    editor.update(&mut visual, |v, _| v.computer_keyboard.enabled = true);
    fade_key(&mut visual, true);
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_none());
    fade_key(&mut visual, false);
    editor.update(&mut visual, |v, _| v.computer_keyboard.enabled = false);
    visual.simulate_keystrokes("cmd-f");
    fade_key(&mut visual, true);
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_none());
    fade_key(&mut visual, false);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn event_fade_single_sample_can_author_both_ends_after_zoom(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, clip) = open(cx);
    edit(&mut visual, &editor, clip, AudioEventEdit::Split { at: 1 });
    editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.viewport = Some(crate::editor::viewport::Viewport {
            start: 0,
            length: 16,
        });
        cx.notify();
    });
    visual.run_until_parked();
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    let leading = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .unwrap()
        .center();
    let trailing = visual
        .debug_bounds("audio-event-fade-out-top-1")
        .unwrap()
        .center();
    let target = point(trailing.x - px(64.0), trailing.y);
    visual.simulate_mouse_down(trailing, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(
            matches!(
                v.last_enqueued_edit,
                Some(Command::EditAudioEvents {
                    operation: AudioEventEdit::SetFade {
                        event: 1,
                        fade: Some(_)
                    },
                    ..
                })
            ),
            "single-sample fade-out must dispatch a real edit"
        )
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let e = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        assert_eq!(e.fade.unwrap().shape().fade_out(), 1);
    });
    assert_ne!(
        leading, trailing,
        "coincident sample endpoints need distinct hit targets"
    );
    let down = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .unwrap()
        .center();
    let target = point(down.x + px(64.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let fade = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0]
            .fade
            .unwrap();
        // Top handles push each other: the fade in takes the single sample.
        assert_eq!((fade.shape().fade_in(), fade.shape().fade_out()), (1, 0));
    });
    fade_key(&mut visual, false);
    assert!(visual.debug_bounds("audio-event-fade-in-top-1").is_some());
    assert!(visual.debug_bounds("audio-event-fade-out-top-1").is_some());
}

fn render_accepted_event(v: &EditorView, clip: ClipId) -> Vec<hex_engine::Frame> {
    let state = v.state.as_ref().unwrap();
    let pending = v
        .pending_playback
        .as_ref()
        .expect("actual worker accepted playback");
    assert_eq!(
        (pending.project_generation, pending.project_revision),
        (v.project_generation, state.revision)
    );
    let prepared = pending
        .prepared_revision
        .as_ref()
        .or_else(|| pending.playback.prepared_revision())
        .unwrap();
    let clip = state.project.clip(clip).unwrap();
    let mut renderer = hex_engine::Renderer::new(prepared.arrangement());
    renderer.seek(clip.start());
    renderer.play();
    let mut pcm = vec![[0.0; 2]; clip.duration() as usize];
    for block in pcm.chunks_mut(257) {
        renderer.render(block).unwrap();
    }
    pcm
}
#[gpui::test]
fn event_fade_curves_and_lower_knots_keep_source_and_resolve_final_snap_modifier(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, clip) = open(cx);
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    for (selector, delta) in [
        ("audio-event-fade-in-top-1", 64.0),
        ("audio-event-fade-out-top-1", -64.0),
    ] {
        let down = visual.debug_bounds(selector).unwrap().center();
        let target = point(down.x + px(delta), down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
    }
    let before_curve =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let original = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0]
    });
    let down = visual
        .debug_bounds("audio-event-fade-in-curve-1")
        .unwrap()
        .center();
    let target = point(down.x + px(16.0), down.y + px(24.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let before_lower = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let current = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        let shape = current.fade.unwrap().shape();
        let old = original.fade.unwrap().shape();
        assert_eq!(
            (current.start, current.end, current.source_start),
            (original.start, original.end, original.source_start)
        );
        assert_eq!(
            (shape.fade_in(), shape.fade_out()),
            (old.fade_in(), old.fade_out())
        );
        assert_ne!(shape.in_curve(), old.in_curve());
        assert_ne!(shape.in_position(), old.in_position());
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_curve)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    let down = visual
        .debug_bounds("audio-event-fade-in-bottom-1")
        .unwrap()
        .center();
    let target = point(down.x + px(24.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let (free, snapped) = editor.read_with(&visual, |v, _| {
        let current = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        assert!(current.start > 0);
        assert_eq!(current.source_start, current.start);
        assert_eq!(current.fade.unwrap().source_start(), current.source_start);
        let snapped = v
            .event_snap_position(current.start, v.audio_event_editor.grid == SnapGrid::Off)
            .unwrap();
        assert_ne!(
            current.start, snapped,
            "free lower-knot edit must exercise an off-grid point"
        );
        (current.start, snapped)
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_lower)
    });
    let down = visual
        .debug_bounds("audio-event-fade-in-bottom-1")
        .unwrap()
        .center();
    let target = point(down.x + px(24.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(
        target,
        MouseButton::Left,
        Modifiers {
            platform: true,
            ..Modifiers::default()
        },
    );
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let current = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        assert_eq!(current.start, snapped);
        assert_ne!(current.start, free);
    });
    fade_key(&mut visual, false);
}

#[gpui::test]
fn event_fade_lower_pointer_extremes_clamp_and_recover_without_canonical_edits(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, _) = open(cx);
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds("audio-event-fade-out-bottom-1")
        .unwrap()
        .center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    for x in [f32::MAX, -f32::MAX] {
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                assert!(v.update_audio_event_fade_pointer(point(px(x), down.y), false, window, cx));
                assert!(v.audio_event_editor.gesture.is_some());
                assert!(v.audio_event_editor.pending.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            })
        });
    }
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    repaint(&mut visual, &editor);
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.gesture.is_none());
        assert!(v.audio_event_editor.pending.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn grouped_fade_handles_use_retained_source_extent_and_keep_release_geometry(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, first) = open(cx);
    let (track, asset, start, length) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        let c = p.clip(first).unwrap();
        (p.tracks()[0].id(), c.asset(), c.start(), c.duration())
    });
    let command = |visual: &mut VisualTestContext, command| {
        editor.update(visual, |v, _| assert!(v.enqueue(Request::Edit(command))));
        pump(&editor, visual);
    };
    command(
        &mut visual,
        Command::PlaceClip {
            track,
            asset,
            start: start + length + 1_000,
            duration: 1_000,
            source_offset: 0,
            source_length: 1_000,
            looping: false,
        },
    );
    let second = editor.read_with(&visual, |v, _| v.selected_clip.unwrap());
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::GroupClips {
            project_generation: v.project_generation,
            revision: v.state.as_ref().unwrap().revision,
            clips: vec![first, second],
        }))
    });
    pump(&editor, &mut visual);
    let group = editor.read_with(&visual, |v, _| v.selected_clip.unwrap());
    command(
        &mut visual,
        Command::GroupAudioEventRange {
            clip: group,
            start: length - 1_000,
            end: length,
        },
    );
    let range = editor.read_with(&visual, |v, _| v.selected_clip.unwrap());
    let tail = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .find(|c| c.start() == start + length)
            .unwrap()
            .id()
    });
    command(
        &mut visual,
        Command::GroupAudioEventRange {
            clip: tail,
            start: length + 1_000,
            end: length + 2_000,
        },
    );
    let other = editor.read_with(&visual, |v, _| v.selected_clip.unwrap());
    let removed: Vec<_> = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.id())
            .filter(|id| *id != range && *id != other)
            .collect()
    });
    for clip in removed {
        command(&mut visual, Command::RemoveClip(clip));
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::GroupClips {
            project_generation: v.project_generation,
            revision: v.state.as_ref().unwrap().revision,
            clips: vec![range, other],
        }))
    });
    pump(&editor, &mut visual);
    let group = editor.read_with(&visual, |v, _| v.selected_clip.unwrap());
    editor.update(&mut visual, |v, cx| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let clip = v.state.as_ref().unwrap().project.clip(group).unwrap();
        let root = clip.audio_event_root();
        let event = clip.audio_events().events[0];
        assert!(event.source_start > root.domain.clock.length());
        v.audio_event_editor.viewport = None;
        v.audio_event_editor.grid = SnapGrid::Off;
        cx.notify();
    });
    repaint(&mut visual, &editor);
    focus_events(&mut visual);
    fade_key(&mut visual, true);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = visual
        .debug_bounds("audio-event-fade-in-top-1")
        .expect("handles for a retained source beyond container extent")
        .center();
    let target = point(down.x + px(36.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    repaint(&mut visual, &editor);
    let held = visual.debug_bounds("audio-event-fade-in-top-1").unwrap();
    assert!(held.center().x > down.x + px(8.0));
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    fade_key(&mut visual, false);
    for _ in 0..3 {
        repaint(&mut visual, &editor);
        assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
    }
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_some());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    pump(&editor, &mut visual);
    repaint(&mut visual, &editor);
    assert_eq!(visual.debug_bounds("audio-event-fade-in-top-1"), Some(held));
    let accepted = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(v.audio_event_editor.pending.is_none());
        let event = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(group)
            .unwrap()
            .audio_events()
            .events[0];
        let fade = event.fade.unwrap();
        assert_eq!(fade.source_start(), event.source_start);
        assert!(fade.shape().fade_in() > 0);
        v.state.as_ref().unwrap().project.snapshot()
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted)
    });
}

#[gpui::test]
fn held_f_keeps_authored_fade_knots_aligned_in_an_offset_loop(cx: &mut TestAppContext) {
    for (looping, warped) in [(true, false), (false, false), (true, true), (false, true)] {
        let (files, mut visual, editor, clip) = open(cx);
        if warped {
            editor.update(&mut visual, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                let settings = hex_project::default_audio_warp(
                    p.clip(clip).unwrap(),
                    p.tempo_map(),
                    hex_project::WarpMode::RePitch,
                )
                .unwrap();
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings)
                })));
            });
            pump(&editor, &mut visual);
        }
        let length = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root()
                .domain
                .clock
                .length()
        });
        let shape = hex_project::ClipFade::new(
            length,
            0,
            length * 5 / 8,
            length / 3 + if warped { 17 } else { 0 },
        )
        .unwrap()
        .with_shape(0, 0, 200, -200)
        .unwrap()
        .with_curve_positions(300, 700)
        .unwrap();
        edit(
            &mut visual,
            &editor,
            clip,
            AudioEventEdit::SetFade {
                event: 1,
                fade: Some(hex_project::AudioEventFade::new(0, shape).unwrap()),
            },
        );
        let track = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].id()
        });
        for command in [
            Command::MoveClip {
                clip,
                track,
                start: 96_000,
            },
            Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: length / 4,
                    end: length * 3 / 4,
                    looping,
                },
            },
        ] {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(command)))
            });
            pump(&editor, &mut visual);
        }
        focus_events(&mut visual);
        // Exercise the same retained source coordinates in a scrolled/zoomed
        // editor; placement position and loop repetition do not own this view.
        editor.update(&mut visual, |v, cx| {
            v.audio_event_editor.viewport = Some(Viewport {
                start: 12_000,
                length: 72_000,
            });
            cx.notify();
        });
        repaint(&mut visual, &editor);
        let baseline_pcm = editor.read_with(&visual, |v, _| render_accepted_event(v, clip));
        let (before, middle) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let c = p.clip(clip).unwrap();
            assert_eq!(c.start(), 96_000);
            let projection = hex_project::AudioViewProjection::new(c, p.tempo_map()).unwrap();
            assert_eq!(projection.play_start_position().unwrap(), Some(length / 4));
            assert_eq!(projection.occurrence_count() >= 2, looping);
            let events = c.editable_audio_events().unwrap();
            let middle = events
                .events
                .iter()
                .find(|e| e.start == length / 4 && e.end == length * 3 / 4)
                .unwrap();
            assert_eq!(middle.source_start, length / 4);
            assert_eq!(middle.fade.unwrap().source_start(), 0);
            assert_eq!(
                middle.fade.unwrap().shape(),
                shape,
                "crop retains the authored fade domain"
            );
            (p.snapshot(), middle.id)
        });
        let selector = |side| {
            Box::leak(format!("audio-event-fade-{side}-top-{middle}").into_boxed_str())
                as &'static str
        };
        let incoming = selector("in");
        let outgoing = selector("out");
        let old = [
            visual.debug_bounds(incoming).unwrap(),
            visual.debug_bounds(outgoing).unwrap(),
        ];
        fade_key(&mut visual, true);
        assert_eq!(
            visual.debug_bounds(incoming),
            Some(old[0]),
            "F is view-only; the retained incoming knot must not reanchor into the loop window"
        );
        assert_eq!(
            visual.debug_bounds(outgoing),
            Some(old[1]),
            "the outgoing knot must still align with its authored source curve"
        );
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        fade_key(&mut visual, false);
        assert_eq!(visual.debug_bounds(incoming), Some(old[0]));
        assert_eq!(visual.debug_bounds(outgoing), Some(old[1]));

        // Hit the actual retained knot and edit just its incoming endpoint. The
        // outgoing law/domain must survive, including fractional beat coordinates.
        fade_key(&mut visual, true);
        let down = old[0].center();
        let target = point(down.x - px(18.0), down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        repaint(&mut visual, &editor);
        let preview = visual.debug_bounds(incoming).unwrap();
        assert!(
            preview.center().x < down.x - px(8.0),
            "looping={looping} warped={warped}: {preview:?} from {down:?}"
        );
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        fade_key(&mut visual, false);
        for _ in 0..3 {
            repaint(&mut visual, &editor);
            assert_eq!(visual.debug_bounds(incoming), Some(preview));
        }
        editor.read_with(&visual, |v, _| {
            assert!(v.audio_event_editor.pending.is_some());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        pump(&editor, &mut visual);
        repaint(&mut visual, &editor);
        assert_eq!(visual.debug_bounds(incoming), Some(preview));
        let accepted = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let event = p
                .clip(clip)
                .unwrap()
                .audio_events()
                .events
                .iter()
                .find(|e| e.id == middle)
                .copied()
                .unwrap();
            let fade = event.fade.unwrap();
            assert_eq!(fade.source_start(), 0);
            assert_eq!(fade.shape().span(), length);
            assert!(fade.shape().fade_in() < shape.fade_in());
            assert_eq!(
                fade.shape().fade_out(),
                shape.fade_out(),
                "editing the incoming knot must preserve the exact outgoing beat coordinates"
            );
            assert_eq!(fade.shape().in_curve(), shape.in_curve());
            assert_eq!(fade.shape().out_curve(), shape.out_curve());
            assert_eq!(fade.shape().in_position(), shape.in_position());
            assert_eq!(fade.shape().out_position(), shape.out_position());
            p.snapshot()
        });
        editor.read_with(&visual, |v, _| {
            let pcm = render_accepted_event(v, clip);
            assert_ne!(
                &pcm[..24_000],
                &baseline_pcm[..24_000],
                "incoming edit must change audible source samples"
            );
            assert_eq!(
                &pcm[40_000..48_000],
                &baseline_pcm[40_000..48_000],
                "outgoing source envelope must remain sample exact"
            );
            if looping {
                assert_eq!(
                    &pcm[..48_000],
                    &pcm[48_000..96_000],
                    "repeated occurrences read the same edited source envelope"
                );
            }
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted)
        });
        let path = files.root().join("retained-fade.hex");
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::SaveNew(path.clone())))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), accepted);
        });
    }
}

#[gpui::test]
fn cmd_f_fits_the_focused_event_selection_without_opening_browser(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, clip) = open(cx);
    editor.update(&mut visual, |v, _| {
        v.audio_event_editor.grid = SnapGrid::Off
    });
    focus_events(&mut visual);
    select(&mut visual, 0.125, 0.0);
    let (start, end, before) = editor.read_with(&visual, |v, _| {
        let (start, end) = v.audio_loop_time_selection().unwrap();
        assert_eq!(start, 0);
        assert!(end > 0 && end < 96_000);
        (start, end, v.state.as_ref().unwrap().project.snapshot())
    });
    visual.simulate_keystrokes("cmd-f");
    editor.read_with(&visual, |v, _| assert!(matches!(v.last_enqueued_edit,
        Some(Command::CreateAudioEventFades { clip: id, start: a, end: b }) if id == clip && (a,b) == (start,end))));
    for _ in 0..3 {
        repaint(&mut visual, &editor);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let event = v
            .state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .audio_events()
            .events[0];
        assert_eq!(event.fade.unwrap().shape().fade_in(), end - start);
        assert_eq!(v.audio_loop_time_selection(), Some((start, end)));
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}
#[path = "pointer_tests.rs"]
mod pointer_tests;

#[gpui::test]
fn held_two_selects_audio_event_edges_without_editing_source(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, _clip) = open(cx);
    focus_events(&mut visual);
    let event = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(event.left() + px(2.), event.center().y);
    let end = point(down.x + px(70.), down.y);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.run_until_parked();
    let key = gpui::Keystroke::parse("2").unwrap();
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: key.clone(),
        is_held: false,
    });
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::IBeam)
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::IBeam)
    );
    visual.simulate_event(gpui::KeyUpEvent { keystroke: key });
    visual.run_until_parked();
    assert_ne!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::IBeam)
    );
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let (start, end) = v.audio_event_editor.range.unwrap();
        assert!(end > start);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[path = "play_start_tests.rs"]
mod saved_start_tests;
