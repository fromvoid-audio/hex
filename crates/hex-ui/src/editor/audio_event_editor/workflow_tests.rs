use super::*;
use crate::editor::clip_drag_ui_test::{pump_move as pump, upper_rail_owned_fixture};
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
    assert!(
        error.contains("budget") || error.contains("limit") || error.contains("memory"),
        "expected bounded preparation refusal: {error}"
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
    assert_eq!(std::fs::read(original_path).unwrap(), original_bytes);
}

fn open(
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
fn edit(
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
    let down = point(b.left() + b.size.width * from, b.top() + px(10.0));
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
fn released_event_move_keeps_completed_paint_until_its_real_worker_terminal(
    cx: &mut TestAppContext,
) {
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
    let down = point(b.left() + b.size.width * 0.125, b.center().y + px(12.0));
    let up = point(b.left() + b.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-1").unwrap();
    let fill = visual.rendered_background("audio-event-1");
    assert!(fill.is_some());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
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
