use super::*;
use crate::editor::{EditorMode, ViewState};
use gpui::{
    AnyWindowHandle, AppContext, Empty, Modifiers, ModifiersChangedEvent, MouseButton,
    TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackId};
use hex_session::service::Service;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::Ordering::{Acquire, Relaxed},
};
use std::time::{Duration, Instant};

const TIMELINE_FRAMES: u64 = 384_000;

fn open_hosted_editor_window(
    cx: &mut TestAppContext,
    project: Project,
) -> (AnyWindowHandle, Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle: AnyWindowHandle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let root = window.replace_root(cx, |window, cx| {
                let editor = cx.new(|cx| {
                    let mut view = EditorView::new(
                        window,
                        cx,
                        service,
                        Err("hosted playback subview test".into()),
                    );
                    while view.service.try_event().is_ok() {}
                    view.viewport = Some(Viewport::fit(TIMELINE_FRAMES));
                    view.state = Some(Arc::new(ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::new(project),
                        waveforms: Default::default(),
                        source_overviews: Default::default(),
                        audio_clip_waveforms: BTreeMap::new(),
                        midi_preview: Default::default(),
                        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                        timeline_frames: TIMELINE_FRAMES,
                        can_undo: false,
                        can_redo: false,
                        revision: 1,
                        unsaved: false,
                        document_path: None,
                    }));
                    view
                });
                EditorWindowRoot::new(editor, cx)
            });
            root.read(cx).editor()
        })
        .expect("hosted root window");
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(cx, |view, _| view.playback_settings.loading) {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "playback settings did not load");
    }
    (handle, editor)
}

fn three_track_audio_project() -> (Project, [TrackId; 3], hex_project::ClipId) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    let mut first_clip = None;
    for index in 0..3 {
        let Created::Imported { track, clip, .. } = model
            .execute(Command::ImportAudioTrack {
                name: format!("track {index}"),
                path: format!("missing-{index}.wav").into(),
                sample_rate: 48_000,
                frames: 48_000,
            })
            .unwrap()
        else {
            panic!("imported audio track");
        };
        if index == 0 {
            first_clip = Some(clip);
        }
        tracks.push(track);
    }
    (
        model.project().clone(),
        [tracks[0], tracks[1], tracks[2]],
        first_clip.unwrap(),
    )
}

fn three_track_midi_project() -> (Project, [TrackId; 3], hex_project::ClipId) {
    let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    let mut first_clip = None;
    for index in 0..3 {
        let Created::Track(track) = model
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .unwrap()
        else {
            panic!("track created");
        };
        if index == 0 {
            let Created::Clip(clip) = model
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(8).unwrap(),
                })
                .unwrap()
            else {
                panic!("midi clip created");
            };
            first_clip = Some(clip);
        }
        tracks.push(track);
    }
    (
        model.project().clone(),
        [tracks[0], tracks[1], tracks[2]],
        first_clip.unwrap(),
    )
}

#[gpui::test]
fn steady_playback_ticks_do_not_rerender_root_editor_view(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, tracks, midi_clip) = three_track_midi_project();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let observation =
        Arc::new(hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap());
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let applied = client.status().unwrap().applied_revision.load(Acquire);

    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    visual.run_until_parked();

    let start = Instant::now();
    editor.update(&mut visual, |view, cx| {
        // Select the MIDI clip so Piano Roll is open alongside Arrangement.
        view.clip_view_open = true;
        view.device_panel_active = false;
        view.selected_track = Some(tracks[0]);
        view.selected_clip = Some(midi_clip);
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "playback isolation fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audible_playback = Some(crate::editor::PlaybackCandidate {
            clip_preview_identity: None,
            worker_requires_preroll: None,
            observation: Some(Arc::clone(&observation)),
            audio_revision: applied,
            project_generation: view.project_generation,
            state: Arc::clone(view.state.as_ref().unwrap()),
            prepared_revision: Some(Arc::clone(&prepared)),
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        let status = view.audio_status().unwrap();
        status.playing.store(true, Relaxed);
        let source = view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            .unwrap_or(1);
        let mut frame = crate::editor::refresh::AudioFrame::capture(status);
        frame.source = source;
        let track_changed = view.tick_track_meters(start);
        let _ = view.observe_audio_frame_for_tick(Some(frame), false, track_changed, start, cx);
        cx.notify();
    });
    visual.run_until_parked();
    let output_source = editor.read_with(&visual, |view, _| {
        view.audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            .unwrap_or(1)
    });

    let baseline_editor_renders = editor.read_with(&visual, |view, _| view.render_count);
    let baseline_subviews = editor.read_with(&visual, |view, cx| {
        view.playback_subviews.subview_render_counts(cx)
    });

    let track_fill_selector =
        Box::leak(format!("track-output-meter-{}-left-fill", tracks[0].value()).into_boxed_str());
    let mut saw_track_fill = px(0.0);
    let mut saw_output_fill = px(0.0);
    let mut arrangement_root_redraw = false;

    for tick in 1_u64..=40 {
        let peak = if tick <= 20 {
            0.15 + 0.03 * tick as f32
        } else {
            0.05
        };
        observation
            .track_output_meter(tracks[0])
            .unwrap()
            .observe_track_output(&[[peak, peak * 0.5]]);

        let at = start + Duration::from_millis(16 * tick);
        let root_redraw = editor.update(&mut visual, |view, cx| {
            let status = view.audio_status().unwrap();
            status.position.store(tick * 768, Relaxed);
            status.peak_left.store(peak.to_bits(), Relaxed);
            status.peak_right.store((peak * 0.7).to_bits(), Relaxed);
            status.record_output_peaks([peak, peak * 0.7]);
            let mut frame = crate::editor::refresh::AudioFrame::capture(status);
            frame.source = output_source;
            frame.cpu = hex_audio::cpu_meter::CpuSample {
                elapsed: 1_000_000 + tick * 50_000,
                budget: 5_333_333,
                peak: 2_000_000,
                frames: 256,
                rate: 48_000,
            };
            let track_changed = view.tick_track_meters(at);
            view.observe_audio_frame_for_tick(Some(frame), false, track_changed, at, cx)
        });
        arrangement_root_redraw |= root_redraw;
        visual.run_until_parked();

        if let Some(fill) = visual.debug_bounds(track_fill_selector) {
            saw_track_fill = saw_track_fill.max(fill.size.height);
        }
        if let Some(fill) = visual.debug_bounds("output-meter-left-fill") {
            saw_output_fill = saw_output_fill.max(fill.size.height);
        }
    }

    let after_arrangement_renders = editor.read_with(&visual, |view, _| view.render_count);
    let after_arrangement_subviews = editor.read_with(&visual, |view, cx| {
        view.playback_subviews.subview_render_counts(cx)
    });

    // Switch to Console view and settle its initial mount/layout pass, then
    // verify Console meters also update without root EditorView redraws.
    editor.update(&mut visual, |view, cx| {
        view.mode = EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        let _ = view.tick_track_meters(start + Duration::from_millis(16 * 40));
    });
    visual.run_until_parked();

    let console_baseline_renders = editor.read_with(&visual, |view, _| view.render_count);
    let console_baseline_meters = editor.read_with(&visual, |view, cx| {
        view.playback_subviews.subview_render_counts(cx).live_meters
    });
    let mut saw_console_meter = false;
    let mut console_root_redraw = false;
    for tick in 41_u64..=65 {
        let peak = 0.25 + 0.02 * (tick - 40) as f32;
        observation
            .track_output_meter(tracks[0])
            .unwrap()
            .observe_track_output(&[[peak, peak * 0.5]]);
        let at = start + Duration::from_millis(16 * tick);
        let root_redraw = editor.update(&mut visual, |view, cx| {
            let status = view.audio_status().unwrap();
            status.position.store(tick * 768, Relaxed);
            status.peak_left.store(peak.to_bits(), Relaxed);
            status.peak_right.store((peak * 0.6).to_bits(), Relaxed);
            status.record_output_peaks([peak, peak * 0.6]);
            let mut frame = crate::editor::refresh::AudioFrame::capture(status);
            frame.source = output_source;
            let track_changed = view.tick_track_meters(at);
            view.observe_audio_frame_for_tick(Some(frame), false, track_changed, at, cx)
        });
        console_root_redraw |= root_redraw;
        visual.run_until_parked();
        saw_console_meter |= visual
            .debug_bounds(Box::leak(
                format!("console-track-meter-{}", tracks[0].value()).into_boxed_str(),
            ))
            .is_some();
    }
    let console_after_renders = editor.read_with(&visual, |view, _| view.render_count);
    let console_after_meters = editor.read_with(&visual, |view, cx| {
        view.playback_subviews.subview_render_counts(cx).live_meters
    });

    // Always close audio output before assertions so `_output_serial` never
    // leaves an open output for subsequent tests on assertion failure.
    editor.update(&mut visual, |view, _| {
        if let Some(mut audio) = view.audio.take() {
            audio.control.close();
        }
    });
    backend.close();
    drop(output);

    assert!(
        baseline_editor_renders >= 1,
        "editor must have rendered initially"
    );
    assert!(
        !arrangement_root_redraw,
        "steady Arrangement + Piano Roll playback ticks must not request a root EditorView redraw"
    );
    assert_eq!(
        after_arrangement_renders, baseline_editor_renders,
        "root EditorView::render must not re-execute on steady Arrangement + Piano Roll playback ticks"
    );
    assert!(
        after_arrangement_subviews.position_readout > baseline_subviews.position_readout,
        "isolated transport position readout must repaint during playback"
    );
    assert!(
        after_arrangement_subviews.output_meter > baseline_subviews.output_meter,
        "isolated output meter must repaint during playback"
    );
    assert!(
        after_arrangement_subviews.arrangement_playhead > baseline_subviews.arrangement_playhead,
        "isolated Arrangement playhead must repaint during playback"
    );
    assert!(
        after_arrangement_subviews.piano_roll_playhead > baseline_subviews.piano_roll_playhead,
        "isolated Piano Roll playhead must repaint during playback"
    );
    assert!(
        after_arrangement_subviews.live_meters > baseline_subviews.live_meters,
        "isolated track/main meters must repaint during playback"
    );
    assert!(
        after_arrangement_subviews.cpu_meter > baseline_subviews.cpu_meter,
        "isolated CPU meter must repaint during playback"
    );
    assert!(
        saw_track_fill > px(5.0) && saw_output_fill > px(3.0),
        "isolated meters must visually paint live levels: track={saw_track_fill:?}, output={saw_output_fill:?}"
    );
    assert!(
        !console_root_redraw,
        "steady Console playback ticks must not request a root EditorView redraw"
    );
    assert_eq!(
        console_after_renders, console_baseline_renders,
        "root EditorView::render must not re-execute on steady Console meter ticks"
    );
    assert!(
        console_after_meters > console_baseline_meters,
        "isolated Console meters must repaint on live peaks"
    );
    assert!(saw_console_meter, "Console track meter must remain visible");
}

#[gpui::test]
fn idle_modifier_presses_do_not_rerender_root_editor_view(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, _tracks, _clip) = three_track_audio_project();
    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();

    // Place pointer in the idle top-left toolbar away from track/section lanes.
    visual.simulate_mouse_move(point(px(10.0), px(10.0)), None, Modifiers::default());
    visual.run_until_parked();

    let baseline = editor.read_with(&visual, |view, _| view.render_count);
    for modifiers in [
        Modifiers::shift(),
        Modifiers::command(),
        Modifiers::control(),
        Modifiers::alt(),
        Modifiers {
            alt: true,
            platform: true,
            ..Modifiers::default()
        },
        Modifiers::default(),
    ] {
        visual.simulate_event(ModifiersChangedEvent {
            modifiers,
            capslock: Default::default(),
        });
        visual.run_until_parked();
    }
    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline,
        "idle modifier presses must not trigger root EditorView::render"
    );
}

#[gpui::test]
fn single_track_clip_drag_retains_cached_scene_replay_on_untouched_tracks(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, tracks, first_clip) = three_track_audio_project();
    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();

    let surface = Box::leak(format!("clip-drag-surface-{}", first_clip.value()).into_boxed_str());
    let start = visual
        .debug_bounds(surface)
        .expect("drag surface on track 0")
        .center();

    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(start.x + px(16.0), start.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();

    let (builds_at_drag_start, epochs_at_drag_start): (Vec<usize>, Vec<u64>) =
        editor.read_with(&visual, |view, cx| {
            assert!(view.active_clip_drag.is_some(), "clip drag must be active");
            let builds = tracks
                .iter()
                .map(|&track| {
                    view.clip_content_cache
                        .build_count(track, cx)
                        .expect("cached clip contents entity")
                })
                .collect();
            let epochs = tracks
                .iter()
                .map(|&track| {
                    view.clip_content_cache
                        .epoch(track, cx)
                        .expect("cached clip contents epoch")
                })
                .collect();
            (builds, epochs)
        });

    // Redraw while the single-track drag is active on track 0: only track 0
    // bypasses `ClipContents` caching; untouched tracks keep their cache epoch
    // and replay their cached scene without rebuilding.
    for _ in 0..4 {
        visual.update(|window, cx| {
            editor.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
    }

    let (builds_after_draws, epochs_after_draws): (Vec<usize>, Vec<u64>) =
        editor.read_with(&visual, |view, cx| {
            let builds = tracks
                .iter()
                .map(|&track| {
                    view.clip_content_cache
                        .build_count(track, cx)
                        .expect("cached clip contents entity")
                })
                .collect();
            let epochs = tracks
                .iter()
                .map(|&track| {
                    view.clip_content_cache
                        .epoch(track, cx)
                        .expect("cached clip contents epoch")
                })
                .collect();
            (builds, epochs)
        });

    assert!(
        epochs_after_draws[0] >= epochs_at_drag_start[0] + 4,
        "dragged track 0 must stay fresh across every redraw while dragging: before={}, after={}",
        epochs_at_drag_start[0],
        epochs_after_draws[0]
    );
    assert!(
        builds_after_draws[0] >= builds_at_drag_start[0] + 4,
        "dragged track 0 must rebuild across drag redraws: before={}, after={}",
        builds_at_drag_start[0],
        builds_after_draws[0]
    );
    assert_eq!(
        epochs_after_draws[1], epochs_at_drag_start[1],
        "untouched track 1 must not invalidate its ClipContents cache epoch"
    );
    assert_eq!(
        builds_after_draws[1], builds_at_drag_start[1],
        "untouched track 1 must retain cached scene replay during track 0 clip drag"
    );
    assert_eq!(
        epochs_after_draws[2], epochs_at_drag_start[2],
        "untouched track 2 must not invalidate its ClipContents cache epoch"
    );
    assert_eq!(
        builds_after_draws[2], builds_at_drag_start[2],
        "untouched track 2 must retain cached scene replay during track 0 clip drag"
    );

    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}
