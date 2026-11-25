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
    while editor.read_with(cx, |view, _| view.settings.playback.loading) {
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
    let (mut output, backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    struct CloseBackendOnDrop {
        backend: Option<hex_audio::worker_fixture::HeadlessBackend>,
        output: Option<hex_audio::WorkerPreparedOutput>,
    }
    impl Drop for CloseBackendOnDrop {
        fn drop(&mut self) {
            if let Some(mut backend) = self.backend.take() {
                backend.close();
            }
            drop(self.output.take());
        }
    }
    let client = output.take_client().unwrap();
    let applied = client.status().unwrap().applied_revision.load(Acquire);
    let _audio_guard = CloseBackendOnDrop {
        backend: Some(backend),
        output: Some(output),
    };

    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    visual.run_until_parked();

    let start = Instant::now();
    editor.update(&mut visual, |view, cx| {
        // Select the MIDI clip so Piano Roll is open alongside Arrangement.
        view.audio_detail.clip_view_open = true;
        view.device_panel.active = false;
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
    drop(_audio_guard);

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
        view.audio_detail.clip_view_open = false;
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
        view.audio_detail.clip_view_open = false;
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

#[gpui::test]
fn opening_foreground_overlay_after_cached_playback_tick_rebuilds_track_headers_without_panic(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, tracks, _first_clip) = three_track_audio_project();
    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    editor.update(&mut visual, |view, cx| {
        view.audio_detail.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();

    let baseline_editor_renders = editor.read_with(&visual, |view, _| view.render_count);

    // Trigger an overlay-only redraw so EditorWindowRoot hits EditorView's
    // outer cache while inner TrackHeaderView states are carried across a frame
    // whose text layout count changes.
    visual.update(|window, cx| {
        let overlay = editor.read(cx).playback_subviews.overlay.clone();
        let cpu = editor.read(cx).playback_subviews.cpu_meter.clone();
        cpu.update(cx, |view, cx| {
            view.sync(
                "0%".into(),
                "cpu".into(),
                0.0,
                false,
                (0x111111, 0x222222, 0x333333),
            );
            cx.notify();
        });
        overlay.update(cx, |_, cx| cx.notify());
        window.draw(cx).clear();
    });
    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline_editor_renders,
        "overlay-only tick must hit EditorView's outer cache"
    );

    // Open a foreground overlay so EditorWindowRoot switches from cached
    // AnyView(EditorView) to uncached Entity<EditorView>; nested track headers
    // must cleanly reject their stale outer-cached frame indices.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.theme.open = true;
            cx.notify();
        });
        window.draw(cx).clear();
    });
    let header_selector =
        Box::leak(format!("arrangement-track-header-{}", tracks[0].value()).into_boxed_str());
    assert!(
        visual.debug_bounds(header_selector).is_some(),
        "track header must remain painted after switching EditorView out of outer cache"
    );
}

#[gpui::test]
fn pulsing_edit_cursor_and_locked_play_start_do_not_rerender_editor_view_or_pin_poll_cadence(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, tracks, _first_clip) = three_track_audio_project();
    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    visual.run_until_parked();

    let pulse_start = Instant::now();
    editor.update(&mut visual, |view, cx| {
        view.audio_detail.clip_view_open = false;
        view.mode = EditorMode::Arrangement;
        view.play_start_at_beginning = false;
        view.selected_clip = None;
        view.edit_cursor = Some(crate::editor::edit_cursor::EditCursor {
            track: Some(tracks[0]),
            frame: 48_000,
            generation: view.project_generation,
            pulse_color: crate::theme::color(crate::theme::PaletteRole::Text),
            pulse_started: pulse_start,
        });
        view.time_selection = Some(crate::editor::time_selection::TimeSelection {
            anchor_track: Some(tracks[0]),
            end_track: Some(tracks[0]),
            anchor_frame: 48_000,
            end_frame: 48_000,
        });
        view.locked_play_start = Some(crate::editor::edit_cursor::EditCursor {
            track: None,
            frame: 96_000,
            generation: view.project_generation,
            pulse_color: crate::theme::locked_time_position_marker_color(0.0),
            pulse_started: pulse_start,
        });
        cx.notify();
    });
    visual.run_until_parked();

    assert!(
        visual.debug_bounds("locked-play-start-top").is_some(),
        "locked top marker must be painted in sibling overlay"
    );
    assert!(
        visual.debug_bounds("locked-play-start-bottom").is_some(),
        "locked bottom marker must be painted in sibling overlay"
    );
    let rest: gpui::Background =
        gpui::rgb(crate::theme::locked_time_position_marker_color(0.0)).into();
    let peak: gpui::Background =
        gpui::rgb(crate::theme::locked_time_position_marker_color(1.0)).into();
    assert_eq!(
        visual.rendered_path_triangles("locked-play-start-top")[0].color,
        rest
    );

    // Let any initial post-setup tick settle once before recording baselines.
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let _ = view.poll_tick(window, cx);
        });
    });
    visual.run_until_parked();

    let baseline_editor_renders = editor.read_with(&visual, |view, _| view.render_count);
    let baseline_cursor_renders = editor.read_with(&visual, |view, cx| {
        view.playback_subviews
            .subview_render_counts(cx)
            .arrangement_edit_cursor
    });

    // Advance across multiple quantized pulse steps (including the locked peak at
    // 1,200 ms) and confirm root EditorView::render() never re-executes and
    // PollCadence never receives tick activity from cursor pulsing.
    let mut saw_pulse = false;
    for step in 1_u64..=12 {
        let now = pulse_start + Duration::from_millis(step * 100);
        let root_changed = editor.update(&mut visual, |view, cx| {
            let cursor_tick = view.tick_edit_cursor_detailed(now);
            assert!(
                !cursor_tick.structural_changed,
                "active cursor pulse must not report a structural change"
            );
            if cursor_tick.pulse_changed {
                saw_pulse = true;
                view.sync_edit_cursor_subview(cx, true);
            }
            let root = view.observe_audio_frame_for_tick(None, false, false, now, cx);
            assert!(
                !view.playback_subviews.take_tick_activity(),
                "cursor pulse must not mark playback tick activity or reset PollCadence"
            );
            root
        });
        assert!(!root_changed, "cursor pulse must not request root redraw");
        visual.update(|window, cx| {
            window.draw(cx).clear();
        });
    }
    assert!(saw_pulse, "pulse steps should advance across 1,200 ms");
    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline_editor_renders,
        "EditorView render count must remain constant across edit-cursor and locked-marker pulses"
    );
    let after_cursor_renders = editor.read_with(&visual, |view, cx| {
        view.playback_subviews
            .subview_render_counts(cx)
            .arrangement_edit_cursor
    });
    assert!(
        after_cursor_renders > baseline_cursor_renders,
        "isolated ArrangementEditCursorView must repaint on pulse steps"
    );
    assert_eq!(
        visual.rendered_path_triangles("locked-play-start-top")[0].color,
        peak,
        "locked marker triangle must reach peak pulse color inside sibling overlay"
    );

    // Run enough stopped/idle poll_tick iterations while shifting pulse phase
    // each tick to verify PollCadence backs off into idle sleep and root
    // EditorView never re-renders even with live edit_cursor + locked_play_start pulsing.
    let quiet_needed = crate::editor::poll_cadence::QUIET_TICKS_BEFORE_IDLE + 4;
    for step in 1_u64..=u64::from(quiet_needed) {
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                if let Some(started) = Instant::now().checked_sub(Duration::from_millis(step * 100))
                {
                    if let Some(cursor) = view.edit_cursor.as_mut() {
                        cursor.pulse_started = started;
                    }
                    if let Some(locked) = view.locked_play_start.as_mut() {
                        locked.pulse_started = started;
                    }
                }
                let _ = view.poll_tick(window, cx);
            });
        });
    }
    assert!(
        editor.read_with(&visual, |view, _| view.poll_cadence.is_idle()),
        "PollCadence must sleep cleanly when stopped/idle with active edit_cursor and locked_play_start"
    );
    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline_editor_renders,
        "idle poll_tick loop with active edit_cursor must not re-render root EditorView"
    );
}

#[gpui::test]
fn audio_detail_playhead_isolates_from_editor_view_during_playback(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, tracks, audio_clip) = three_track_audio_project();
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    struct CloseBackendOnDrop {
        backend: Option<hex_audio::worker_fixture::HeadlessBackend>,
        output: Option<hex_audio::WorkerPreparedOutput>,
    }
    impl Drop for CloseBackendOnDrop {
        fn drop(&mut self) {
            if let Some(mut backend) = self.backend.take() {
                backend.close();
            }
            drop(self.output.take());
        }
    }
    let client = output.take_client().unwrap();
    let applied = client.status().unwrap().applied_revision.load(Acquire);
    let _audio_guard = CloseBackendOnDrop {
        backend: Some(backend),
        output: Some(output),
    };

    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    visual.run_until_parked();

    let start = Instant::now();
    editor.update(&mut visual, |view, cx| {
        view.audio_detail.clip_view_open = true;
        view.audio_detail.event.source_view = false;
        view.device_panel.active = false;
        view.selected_track = Some(tracks[0]);
        view.selected_clip = Some(audio_clip);
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "audio detail isolation fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audible_playback = Some(crate::editor::PlaybackCandidate {
            clip_preview_identity: None,
            worker_requires_preroll: None,
            observation: None,
            audio_revision: applied,
            project_generation: view.project_generation,
            state: Arc::clone(view.state.as_ref().unwrap()),
            prepared_revision: None,
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        let status = view.audio_status().unwrap();
        status.playing.store(true, Relaxed);
        status.position.store(4_800, Relaxed);
        let source = view
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            .unwrap_or(1);
        let mut frame = crate::editor::refresh::AudioFrame::capture(status);
        frame.source = source;
        let _ = view.observe_audio_frame_for_tick(Some(frame), false, false, start, cx);
        cx.notify();
    });
    visual.run_until_parked();
    // Flush any one-shot bounds-measurement refresh emitted by audio_event_editor on first mount.
    visual.update(|window, cx| {
        window.draw(cx).clear();
    });
    visual.run_until_parked();

    let output_source = editor.read_with(&visual, |view, _| {
        assert!(
            !view.has_inline_playhead_panel(),
            "hosted audio detail panel must use sibling AudioDetailPlayheadView"
        );
        view.audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
            .unwrap_or(1)
    });

    let baseline_editor_renders = editor.read_with(&visual, |view, _| view.render_count);
    let baseline_detail_renders = editor.read_with(&visual, |view, cx| {
        view.playback_subviews
            .subview_render_counts(cx)
            .audio_detail_playhead
    });

    for tick in 1_u64..=10 {
        let at = start + Duration::from_millis(16 * tick);
        let root_redraw = editor.update(&mut visual, |view, cx| {
            let status = view.audio_status().unwrap();
            status.position.store(4_800 + tick * 1_200, Relaxed);
            let mut frame = crate::editor::refresh::AudioFrame::capture(status);
            frame.source = output_source;
            view.observe_audio_frame_for_tick(Some(frame), false, false, at, cx)
        });
        assert!(
            !root_redraw,
            "audio detail playhead tick must not trigger root EditorView redraw"
        );
        visual.run_until_parked();
    }

    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline_editor_renders,
        "EditorView render count must stay constant while audio detail playhead advances"
    );
    let after_detail_renders = editor.read_with(&visual, |view, cx| {
        view.playback_subviews
            .subview_render_counts(cx)
            .audio_detail_playhead
    });
    assert!(
        after_detail_renders >= baseline_detail_renders,
        "AudioDetailPlayheadView should stay mounted in sibling overlay"
    );

    drop(editor);
    drop(visual);
    drop(_audio_guard);
}

#[gpui::test]
fn hovered_device_meter_and_eq8_spectrum_do_not_rerender_editor_view(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = model
        .execute(Command::AddTrack {
            name: "eq track".into(),
        })
        .unwrap()
    else {
        panic!("track created");
    };
    let eq8_state = hex_engine::default_native_device_state(hex_devices::eq8::EQ8_TYPE_ID).unwrap();
    let Created::Device(device_id) = model
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: eq8_state,
        })
        .unwrap()
    else {
        panic!("eq8 device created");
    };
    let project = model.project().clone();

    let (handle, editor) = open_hosted_editor_window(cx, project);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(720.0)));
    visual.run_until_parked();

    let _callback = editor.update(&mut visual, |view, cx| {
        let callback = crate::editor::device_panel::install_spectrum_test_channel(view);
        view.selected_track = Some(track);
        view.audio_detail.clip_view_open = true;
        view.device_panel.active = true;
        crate::editor::device_panel::set_hovered_meter_for_test(view, Some(device_id));
        cx.notify();
        callback
    });
    visual.run_until_parked();
    // Flush any one-shot layout measurement refresh (e.g. chain_window bounds defer)
    // before capturing the steady-state render baseline.
    for _ in 0..2 {
        visual.update(|window, cx| {
            window.draw(cx).clear();
        });
        visual.run_until_parked();
    }

    assert!(
        editor.read_with(&visual, |view, _| {
            crate::editor::device_panel::has_mounted_eq8_and_meter_for_test(view, device_id)
        }),
        "EQ8 graph and isolated DeviceMeterView must be mounted after initial render"
    );

    let target = editor
        .read_with(&visual, |view, _| {
            let identity = hex_engine::PostNodeIdentity::new(
                hex_engine::ProcessorChainOwner::Track(track.value()),
                device_id.value(),
                hex_devices::eq8::EQ8_TYPE_ID,
            );
            crate::editor::device_panel::desired_spectrum_target_for_test(view, identity)
        })
        .expect("mounted EQ8 should publish a spectrum target");

    // Generate 5 distinct real FFT output frames through the spectrum worker so
    // every iteration below triggers a genuine `ResponseGraph::set_analyzers`
    // update and `cx.notify()`.
    let mut frames_published = Vec::with_capacity(5);
    for step in 0..5_u32 {
        let captured_at = Instant::now();
        let stamp = hex_audio::spectrum_worker::SpectrumStamp {
            target: target.token,
            generation: target.project_generation,
            sample_rate_hz: target.sample_rate_hz,
            applied_revision: target.activation_floor,
        };
        let (mut capture, mut reader, reaper) =
            hex_audio::spectrum_worker::spawn_spectrum_worker(stamp).unwrap();
        let freq = 400.0 + step as f32 * 300.0;
        for chunk in 0..16 {
            let frames: [[f32; 2]; 256] = std::array::from_fn(|i| {
                let phase = std::f32::consts::TAU * freq * (chunk * 256 + i) as f32
                    / target.sample_rate_hz as f32;
                [phase.sin() * 0.5; 2]
            });
            let _ = capture.submit_pair_at(stamp, &frames, &frames, captured_at);
        }
        let deadline = captured_at + Duration::from_secs(2);
        let published = loop {
            if let Some(pair) = reader.try_latest_pair_at(captured_at) {
                break pair;
            }
            assert!(Instant::now() < deadline, "real FFT result missing");
            std::thread::yield_now();
        };
        drop(capture);
        drop(reader);
        drop(reaper);
        frames_published.push(published);
    }

    let baseline_editor_renders = editor.read_with(&visual, |view, _| view.render_count);
    let sibling_on = editor.read_with(&visual, |view, _| view.uses_sibling_playback_overlay());
    assert!(sibling_on, "sibling overlay must be active");

    // Push 5 distinct live EQ8 spectrum frames and tick the hovered device meter.
    // Neither should require root EditorView::render().
    for (idx, published_step) in frames_published.into_iter().enumerate() {
        let before = editor.read_with(&visual, |view, _| view.render_count);
        editor.update(&mut visual, |view, cx| {
            view.device_panel
                .spectrum_results
                .insert(target.token, published_step);

            let (updated, needs_root) = crate::editor::device_panel::push_live_spectra(view, cx);
            assert!(updated, "EQ8 live spectrum should push into mounted graph");
            assert!(
                !needs_root,
                "mounted EQ8 graph must not require root EditorView re-render"
            );

            let _ = crate::editor::device_panel::tick_hovered_meter_isolated(view, cx);
        });
        let after_update = editor.read_with(&visual, |view, _| view.render_count);
        visual.run_until_parked();
        let after_parked = editor.read_with(&visual, |view, _| view.render_count);
        let a11y_nodes = visual.update(|window, _| window.accessibility_nodes().count());
        assert!(
            a11y_nodes > 0,
            "EQ8 panel must maintain a valid accessibility tree so AnyView::cached stays active"
        );
        visual.update(|window, cx| {
            window.draw(cx).clear();
        });
        let after_draw = editor.read_with(&visual, |view, _| view.render_count);
        assert_eq!(
            (after_update, after_parked, after_draw),
            (before, before, before),
            "step {idx}: a11y_nodes={a11y_nodes}, before={before}, after_update={after_update}, after_parked={after_parked}, after_draw={after_draw}"
        );
    }

    assert_eq!(
        editor.read_with(&visual, |view, _| view.render_count),
        baseline_editor_renders,
        "EQ8 live spectrum and hovered device meter updates must not re-render root EditorView"
    );
}
