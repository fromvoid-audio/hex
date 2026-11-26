use super::{EditorView, install_close_guard};

use gpui::{AppContext, Empty, TestAppContext, VisualTestContext, point, px};

use gpui::{CursorStyle, Modifiers, MouseButton};

use hex_project::{BeatTime, Command, Created, Editor, Project};

use hex_session::{
    CachedPeak, CachedWaveform,
    service::{Service, ViewState},
};

use std::collections::BTreeMap;

use std::sync::Arc;

use super::rendered_test_fixture::{TIMELINE_FRAMES, editor_window_with_timeline};

pub(super) use super::rendered_test_fixture::{audio_project, editor_window};

/// Edge-trim grab point in the clip-name band. A hovered clip shows its
/// corner fade handles over the rest of the edge, and those take precedence.
pub(super) fn trim_grab_point(bounds: gpui::Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    gpui::point(bounds.center().x, bounds.top() + bounds.size.height / 6.0)
}

pub(super) fn midi_project() -> (
    Project,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Track(source) = editor
        .execute(Command::AddTrack {
            name: "midi source".into(),
        })
        .expect("source track")
    else {
        panic!("track result")
    };
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target track")
    else {
        panic!("track result")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).expect("length"),
        })
        .expect("place midi clip")
    else {
        panic!("clip result")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("add note");
    (editor.project().clone(), source, target, clip)
}

fn drag_source_to_target(
    cx: &TestAppContext,
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    clip: hex_project::ClipId,
    target_row: usize,
    target_delta_x: f32,
) -> (gpui::Bounds<gpui::Pixels>, gpui::Point<gpui::Pixels>) {
    let selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    visual.run_until_parked();
    let source = visual
        .debug_bounds(selector)
        .expect("rendered move surface has a hit target");
    assert!(f32::from(source.size.width) > 10.0);
    assert!(f32::from(source.size.height) > 10.0);
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            view.interaction.pointer_down.is_some(),
            "clip overlay remembers original mouse-down"
        )
    });
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        cx.update(|app| app.has_active_drag()),
        "gpui reports a native drag after threshold crossing"
    );
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("threshold movement starts clip drag");
        assert!(
            drag.capture.get().is_some(),
            "drag start retains the original pointer: clip={:?}, generation={}, down={:?}",
            drag.clip,
            drag.project_generation,
            view.interaction.pointer_down
        );
        assert!(
            drag.grab_offset.get().is_some(),
            "drag start computes the source grab offset"
        );
    });
    let move_count_before_target =
        editor.read_with(cx, |view, _| view.test_probes.drag_move_event_count);

    // Crossing out of the painted lane restores a cursor-following ghost for
    // either clip kind; the on-lane overlay remains the only preview in rows.
    let outside_lane = point(px(200.0), down.y);
    visual.simulate_mouse_move(outside_lane, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let ghost = visual
        .debug_bounds("clip-drag-floating-preview")
        .expect("off-lane clip drag shows a floating ghost");
    assert!(f32::from(ghost.size.width) > 100.0);
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("clip drag active");
        assert!(!drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });

    // Track rows are 44 px high and adjacent in Arrangement; the initial row is
    // identified by the rendered source bounds rather than a hard-coded window y.
    let lane_y = source.origin.y + px(22.0) + px(44.0 * target_row as f32);
    let target_x = down.x + px(target_delta_x);
    visual.simulate_mouse_move(
        point(target_x, lane_y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        cx.update(|app| app.has_active_drag()),
        "gpui drag remains active while crossing the target row"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.drag_move_event_count - move_count_before_target,
            2,
            "the Arrangement should resolve each move once, across and outside lanes"
        );
        let drag = view.interaction.active_clip_drag.as_ref().expect("ui drag state");
        let direct = view
            .hovered_drag_target(f32::from(target_x), f32::from(lane_y))
            .and_then(|(target, lane)| view.drag_candidate(drag, target, f32::from(target_x), lane));
        assert!(
            drag.preview.get().is_some(),
            "cross-row movement resolves an in-track preview; source={source:?}, pointer={:?}, target={target_row}, direct={direct:?}",
            point(target_x, lane_y)
        );
    });
    let preview = visual
        .debug_bounds("clip-drop-preview")
        .expect("valid target row shows in-track drop preview");
    let hidden = visual
        .debug_bounds("clip-drag-floating-preview-hidden")
        .expect("in-lane drop suppresses the cursor ghost");
    assert_eq!(hidden.size.width, px(0.0));
    assert_eq!(hidden.size.height, px(0.0));
    (preview, point(target_x, lane_y))
}

#[allow(clippy::too_many_arguments)]
fn drag_source_to_inserted_track(
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    project: &Project,
    clip: hex_project::ClipId,
    source_track: hex_project::TrackId,
    anchor: Option<hex_project::TrackId>,
    copy: bool,
) -> gpui::Point<gpui::Pixels> {
    visual.run_until_parked();
    let source_surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("rendered source clip surface");
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source_track.value()).into_boxed_str(),
        ))
        .expect("rendered source lane");
    let body = visual
        .debug_bounds("arrangement-content")
        .expect("rendered Arrangement body");
    let target_lane = anchor.map(|track| {
        visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("rendered insertion anchor lane")
    });
    assert_eq!(source_lane.origin.x, body.origin.x);
    assert_eq!(
        source_lane.size.width,
        body.size.width - editor.read_with(cx, |view, _| px(view.track_header_width()))
    );

    let down = point(
        source_surface.origin.x + source_surface.size.width / 2.0,
        source_surface.origin.y + source_surface.size.height / 2.0,
    );
    let expected_gain = editor.read_with(cx, |view, _| {
        view.settings.track_defaults.captured_gain().unwrap()
    });
    let modifiers = Modifiers {
        alt: copy,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));

    let target_x = source_lane.origin.x + source_lane.size.width * 0.73;
    let target_y = match target_lane {
        Some(lane) => (source_lane.origin.y + source_lane.size.height + lane.origin.y) / 2.0,
        None => {
            let bounds = editor.read_with(cx, |view, _| {
                view.timeline_caches
                    .arrangement_bounds
                    .expect("painted body")
            });
            px(bounds.top + bounds.height - 8.0)
        }
    };
    let target = point(target_x, target_y);
    if let Some(lane) = target_lane {
        assert!((f32::from(target_y) - f32::from(lane.origin.y)).abs() <= 1.0);
    }
    let target_fraction =
        (f64::from(target_x) - f64::from(source_lane.origin.x)) / f64::from(source_lane.size.width);
    let pointer_frame = (target_fraction * TIMELINE_FRAMES as f64).floor() as u64;
    let (snap_grid, snap_viewport, snap_width) = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag");
        (
            drag.snap_grid,
            super::viewport::Viewport {
                start: drag.viewport_start,
                length: drag.viewport_length,
            },
            f32::from(source_lane.size.width),
        )
    });
    let expected = match editor.read_with(cx, |view, _| {
        view.interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag")
            .position
    }) {
        super::drag::ClipPosition::Audio { .. } => {
            let grab_offset = editor.read_with(cx, |view, _| {
                view.interaction
                    .active_clip_drag
                    .as_ref()
                    .and_then(|drag| drag.grab_offset.get())
                    .expect("audio grab offset")
            });
            super::drag::ClipPosition::Audio {
                start: snap_grid
                    .snap_frame(
                        project,
                        pointer_frame.saturating_sub(grab_offset),
                        snap_viewport,
                        snap_width,
                    )
                    .expect("snap audio start"),
                duration: project.clip(clip).expect("source audio").duration(),
            }
        }
        super::drag::ClipPosition::Midi { length, .. } => {
            let pointer_beat = project
                .tempo_map()
                .frame_to_beat(pointer_frame)
                .expect("pointer beat");
            let grab_offset = editor.read_with(cx, |view, _| {
                view.interaction
                    .active_clip_drag
                    .as_ref()
                    .and_then(|drag| drag.grab_offset.get())
                    .expect("midi grab offset")
            });
            super::drag::ClipPosition::Midi {
                start: snap_grid
                    .snap_beat(
                        project,
                        BeatTime::from_raw(pointer_beat.raw().saturating_sub(grab_offset)),
                        snap_viewport,
                        snap_width,
                    )
                    .expect("snap midi start"),
                length,
            }
        }
    };
    visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("clip drag remains active");
        assert_eq!(
            drag.destination.get(),
            Some(super::drag::ClipDropDestination::NewTrack {
                before_track: anchor,
                position: expected,
                start_frame: super::drag::preview_for(
                    &view.state.as_ref().unwrap().project,
                    source_track,
                    expected,
                )
                .unwrap()
                .start_frame,
            })
        );
        assert!(drag.lane_hovered.get());
        assert!(drag.preview.get().is_none());
    });
    let guide_x = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        let preview = super::drag::preview_for(&state.project, source_track, expected).unwrap();
        let viewport = view.visible_viewport(state.timeline_frames);
        source_lane.left()
            + source_lane.size.width * viewport.fraction(preview.start_frame).unwrap()
    });
    assert_clip_move_guides(visual, guide_x);
    let line = visual
        .debug_bounds("track-insert-preview")
        .expect("new-track target shows insertion line");
    assert_eq!(line.size.width, body.size.width);
    if anchor.is_some() {
        assert!(
            visual.debug_bounds("append-track-drop-highlight").is_none(),
            "between-track insertion must not highlight the rows below"
        );
        let hidden = visual
            .debug_bounds("clip-drag-floating-preview-hidden")
            .expect("between-row insertion hides the cursor ghost");
        assert_eq!(hidden.size.width, px(0.0));
        assert_eq!(hidden.size.height, px(0.0));
    } else {
        let ghost = visual
            .debug_bounds("clip-drag-append-preview")
            .expect("bottom insertion locks the clip ghost to the new row");
        assert!(ghost.size.width > px(3.0));
        let last_track = project.tracks().last().expect("last project track").id();
        let last_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", last_track.value()).into_boxed_str(),
            ))
            .expect("painted final track lane");
        let (expected_left, expected_width) = editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().expect("project");
            let preview = super::drag::preview_for(&state.project, source_track, expected)
                .expect("frame bounds for appended clip");
            let viewport = view.visible_viewport(state.timeline_frames);
            let left = body.origin.x
                + px((preview.start_frame.saturating_sub(viewport.start) as f64
                    / viewport.length as f64
                    * f64::from(source_lane.size.width)) as f32);
            let width = px(
                (preview.end_frame.saturating_sub(preview.start_frame) as f64
                    / viewport.length as f64
                    * f64::from(source_lane.size.width)) as f32,
            );
            (left, width)
        });
        assert!((ghost.origin.x - expected_left).abs() <= px(1.0));
        assert!((ghost.size.width - expected_width).abs() <= px(1.0));
        assert!((ghost.origin.y - (last_lane.origin.y + last_lane.size.height)).abs() <= px(1.0));
        assert!(visual.debug_bounds("append-track-drop-highlight").is_none());
        let line = visual
            .debug_bounds("track-insert-preview")
            .expect("bottom insertion line");
        assert!(
            (line.origin.y - (last_lane.origin.y + last_lane.size.height - px(1.0))).abs()
                <= px(1.0)
        );
        let body_after = visual
            .debug_bounds("arrangement-content")
            .expect("Arrangement body remains mounted");
        assert_eq!(body_after.origin, body.origin);
        assert_eq!(body_after.size, body.size);
    }
    visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
    editor.read_with(cx, |view, _| match expected {
        super::drag::ClipPosition::Audio { start, .. } => assert!(
            if copy {
                matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::CopyClipToNewTrack {
                    clip: copied_clip, before_track, initial_gain, start: copied_start, ..
                }) if *copied_clip == clip && *before_track == anchor && *copied_start == start && *initial_gain == Some(expected_gain))
            } else {
                matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::MoveClipToNewTrack {
                    clip: moved_clip, before_track, initial_gain, start: moved_start, ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start && *initial_gain == Some(expected_gain))
            },
            "audio new-track release request={:?}, destination={expected:?}, error={:?}",
            view.test_probes.last_enqueued_edit,
            view.error
        ),
        super::drag::ClipPosition::Midi { start, .. } => assert!(
            if copy {
                matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::CopyMidiClipToNewTrack {
                    clip: copied_clip, before_track, initial_gain, start: copied_start, ..
                }) if *copied_clip == clip && *before_track == anchor && *copied_start == start && *initial_gain == Some(expected_gain))
            } else {
                matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::MoveMidiClipToNewTrack {
                    clip: moved_clip, before_track, initial_gain, start: moved_start, ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start && *initial_gain == Some(expected_gain))
            },
            "midi new-track release request={:?}, destination={expected:?}, error={:?}",
            view.test_probes.last_enqueued_edit,
            view.error
        ),
    });
    for _ in 0..2 {
        editor.update(cx, |view, cx| {
            assert!(view.interaction.active_clip_drag.is_none());
            assert!(view.pending_clip_move.is_some());
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("track-insert-preview"), Some(line));
        assert_clip_move_guides(visual, guide_x);
        if anchor.is_none() {
            assert!(visual.debug_bounds("clip-drag-append-preview").is_some());
        }
    }
    target
}

// Same private pre-ACK custody pattern as the existing group-drag fixture.
// Service shutdown is not a worker join; uncertain setup retains disk evidence.
struct UpperRailPendingSetup {
    environment: Arc<super::test_environment::Environment>,
    acknowledged: bool,
}

impl UpperRailPendingSetup {
    fn acknowledge_import(&mut self, event: &hex_session::service::Event, path: &std::path::Path) {
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(event.request_complete);
        assert_eq!(event.status.as_deref(), Some("audio imported"));
        assert!(event.state.project.assets().any(|asset| {
            asset.path() == path
                && asset.source_sample_rate() == 48_000
                && asset.source_frames() == 96_000
        }));
        self.acknowledged = true;
    }
}

impl Drop for UpperRailPendingSetup {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}

fn upper_rail_next(service: &Service) -> hex_session::service::Event {
    let event = upper_rail_terminal(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

fn upper_rail_terminal(service: &Service) -> hex_session::service::Event {
    use std::time::Duration;
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "upper_rail_terminal: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn upper_rail_owned_fixture(
    cx: &mut TestAppContext,
) -> (
    Arc<super::test_environment::Environment>,
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
) {
    upper_rail_fixture_with_service(cx, Service::new(48_000, 32, 8_000_000).unwrap())
}

pub(super) fn upper_rail_fixture_with_service(
    cx: &mut TestAppContext,
    service: Service,
) -> (
    Arc<super::test_environment::Environment>,
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
) {
    use hex_session::service::Request;
    cx.update(super::bind_editor_keys);
    let files = super::test_environment::Environment::new().unwrap();
    let mut setup = UpperRailPendingSetup {
        environment: Arc::clone(&files),
        acknowledged: false,
    };
    let wav = files.root().join("upper-rail-resize.wav");
    // Identical mono PCM16, 48kHz/96k frames, now under the actual leased root.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + 96_000_u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(96_000_u32 * 2).to_le_bytes());
    for _ in 0..96_000 {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&wav, bytes).unwrap();
    std::fs::create_dir(files.root().join("samples")).unwrap();
    upper_rail_next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let imported = upper_rail_next(&service);
    setup.acknowledge_import(&imported, &wav);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "untouched target".into(),
        }))
        .unwrap();
    let event = upper_rail_next(&service);
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(window, cx, service, Arc::clone(&files))
            });
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                view.receive(event);
                // Rail/detail geometry uses an explicit fixture time span.
                view.viewport = Some(super::viewport::Viewport::fit(
                    view.state.as_ref().unwrap().timeline_frames,
                ));
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    editor.read_with(cx, |view, _| {
        let root = files.root();
        assert!(Arc::ptr_eq(&files, &view.test_environment));
        assert_eq!(view.browser.directory, root.join("samples"));
        assert_eq!(view.browser.places, vec![root.join("samples")]);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .assets()
                .all(|asset| asset.path().starts_with(root))
        );
        assert_eq!(
            view.browser.test_preferences_path(),
            root.join("config/browser.json")
        );
        assert_eq!(
            view.settings.recent_projects.test_path(),
            root.join("config/recent-projects.json")
        );
        assert_eq!(
            view.settings.theme.test_path(),
            root.join("config/theme.json")
        );
        assert_eq!(
            view.settings.project_directory.test_path(),
            root.join("config/project-directory.json")
        );
        assert_eq!(
            view.browser
                .test_preferences_path()
                .with_extension("samples-v1.db"),
            root.join("config/browser.samples-v1.db")
        );
    });
    (files, handle, editor)
}

fn check_shift_edge(cx: &mut TestAppContext, edge: hex_project::TrimEdge) {
    use super::device_panel::tests::eq_ui_tests::graph_layout_tests::{
        headless_status_output, status_fixture_guard,
    };
    use std::sync::atomic::Ordering::Release;
    let _output_guard = status_fixture_guard();
    let direction = if edge == hex_project::TrimEdge::Right {
        -1.0
    } else {
        1.0
    };
    let name = if edge == hex_project::TrimEdge::Right {
        "right"
    } else {
        "left"
    };
    let (project, _, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (mut output, _callback) = headless_status_output();
    let control = output.take_client().unwrap();
    control.status().unwrap().playing.store(true, Release);
    editor.update(cx, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control,
            sample_rate: 48_000,
            label: "held stretch fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
    });
    let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, shift);
    visual.simulate_mouse_move(
        point(down.x + px(12.0 * direction), down.y),
        MouseButton::Left,
        shift,
    );
    visual.run_until_parked();
    let target = point(down.x + px(42.0 * direction), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, shift);
    visual.run_until_parked();
    let stretched = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_trim_drag
            .as_ref()
            .expect("active edge stretch");
        assert!(drag.stretch_modifier.get());
        let preview = drag.preview.borrow();
        let preview = &preview.as_ref().unwrap().clip;
        assert!(preview.audio_edit().unwrap().domain.warp_enabled);
        assert_eq!(preview.source_length(), 96_000);
        assert!(preview.duration() < 96_000);
        assert!(view.test_probes.last_enqueued_edit.is_none());
        let target = view
            .service
            .held_arrangement_stretch_audition_for_test()
            .unwrap();
        assert_eq!((target.clip, target.edge), (clip, edge));
        assert_eq!(
            target.frame,
            if edge == hex_project::TrimEdge::Right {
                preview.start() + preview.duration()
            } else {
                preview.start()
            }
        );
        preview.clone()
    });
    let first_bounds = visual.debug_bounds("trim-live-preview").unwrap();
    visual.simulate_mouse_move(
        point(target.x + px(20.0 * direction), target.y),
        MouseButton::Left,
        shift,
    );
    visual.run_until_parked();
    assert_ne!(
        visual.debug_bounds("trim-live-preview").unwrap().size.width,
        first_bounds.size.width
    );
    visual.simulate_mouse_move(target, MouseButton::Left, shift);
    visual.run_until_parked();
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let preview = view
            .interaction
            .active_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow();
        assert!(
            preview.as_ref().unwrap().clip.audio_edit().is_none(),
            "releasing Shift restores ordinary trim at same pointer"
        );
        assert!(
            view.service
                .held_arrangement_stretch_audition_for_test()
                .is_none()
        );
    });
    visual.simulate_modifiers_change(shift);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.interaction
                .active_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .as_ref()
                .unwrap()
                .clip,
            stretched
        )
    });
    visual.simulate_mouse_up(target, MouseButton::Left, shift);
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.test_probes.last_enqueued_edit, Some(Command::StretchAudioClipEdge { clip: id, edge: requested_edge, frame }) if id == clip && requested_edge == edge && frame == if edge == hex_project::TrimEdge::Right { stretched.start() + stretched.duration() } else { stretched.start() }));
        assert!(view.interaction.active_trim_drag.is_none());
        assert!(view.service.held_arrangement_stretch_audition_for_test().is_none());
        assert_eq!(view.state.as_ref().unwrap().project.clip(clip).unwrap().duration(), 96_000, "UI preview has not changed canonical state");
    });
    editor.update(cx, |view, _| view.audio = None);
}

fn check_long_stretch_to_painted_endpoint(
    cx: &mut TestAppContext,
    authored_mode: Option<hex_project::WarpMode>,
) {
    const SOURCE: u64 = 20_000_000;
    let mut project = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = project
        .execute(Command::ImportAudioTrack {
            name: "long".into(),
            path: "long.wav".into(),
            sample_rate: 48_000,
            frames: SOURCE,
        })
        .unwrap()
    else {
        panic!("fixture import");
    };
    if let Some(mode) = authored_mode {
        let settings = hex_project::default_audio_warp(
            project.project().clip(clip).unwrap(),
            project.project().tempo_map(),
            mode,
        )
        .unwrap();
        project
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
    }
    let before = project.project().snapshot();
    let (handle, editor) =
        editor_window_with_timeline(cx, project.project().clone(), Arc::default(), 32_000_000);
    editor.update(cx, |view, cx| {
        view.viewport = Some(super::viewport::Viewport {
            start: 18_000_000,
            length: 6_000_000,
        });
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let end = point(down.x + px(80.0), down.y);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, shift);
    visual.simulate_mouse_move(end, MouseButton::Left, shift);
    visual.run_until_parked();
    assert!(visual.debug_bounds("trim-live-preview").is_some());
    let painted = editor.read_with(&visual, |view, _| {
        let drag = view.interaction.active_trim_drag.as_ref().unwrap();
        let preview = drag.preview.borrow();
        let preview = &preview.as_ref().unwrap().clip;
        assert!(preview.duration() > SOURCE);
        assert_eq!(
            preview
                .audio_edit()
                .unwrap()
                .domain
                .warp
                .as_ref()
                .unwrap()
                .mode,
            authored_mode.unwrap_or(hex_project::WarpMode::Tones { grain_ms: 40 })
        );
        preview.clone()
    });
    visual.simulate_mouse_up(end, MouseButton::Left, shift);
    let command = editor.read_with(&visual, |view, _| {
        view.test_probes.last_enqueued_edit.clone().unwrap()
    });
    assert!(matches!(
        command,
        Command::StretchAudioClipEdge { clip: id, .. } if id == clip
    ));
    project.execute(command).unwrap();
    assert_eq!(project.project().clip(clip), Some(&painted));
    assert!(project.undo());
    assert_eq!(project.project().snapshot(), before);
}

/// The test executor leaves the UI poll timer parked: the real worker may finish,
/// but its ACK is not installed until pump_move explicitly ticks the controller.
pub(super) fn pump_move(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            assert!(view.error.is_none(), "{:?}", view.error);
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pump_move: hang guard expired"
        );
        std::thread::yield_now();
    }
}

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

pub(super) fn assert_clip_move_guides(visual: &mut VisualTestContext, expected_x: gpui::Pixels) {
    for edge in ["top", "bottom"] {
        let selector = Box::leak(format!("clip-move-guide-{edge}").into_boxed_str());
        let marker = visual.debug_bounds(selector).expect("painted drag guide");
        assert!(
            (marker.center().x - expected_x).abs() <= px(1.0),
            "{edge} guide follows the dragged anchor"
        );
    }
    let line = visual
        .debug_bounds("clip-move-guide-line")
        .expect("full-height drag cursor");
    let surface = visual.debug_bounds("arrangement-surface").unwrap();
    assert!((line.left() - expected_x).abs() <= px(1.0));
    assert_eq!(
        (line.top(), line.bottom()),
        (surface.top(), surface.bottom())
    );
    assert!(visual.rendered_background("clip-move-guide-line").is_some());
}

fn assert_move_handoff(cx: &mut TestAppContext, midi: &[bool], vertical: bool) {
    assert_drag_handoff(cx, midi, vertical, false);
}

fn assert_drag_handoff(cx: &mut TestAppContext, midi: &[bool], vertical: bool, copy: bool) {
    assert_drag_handoff_with_pending_request(cx, midi, vertical, copy, false);
}

fn assert_drag_handoff_with_pending_request(
    cx: &mut TestAppContext,
    midi: &[bool],
    vertical: bool,
    copy: bool,
    pending: bool,
) {
    assert_drag_handoff_at_grab(cx, midi, vertical, copy, pending, None, None);
}

#[derive(Clone, Copy)]
struct OriginProbe {
    dx: f32,
    grid: super::musical_time::SnapGrid,
    bypass: bool,
    viewport_length: u64,
}

impl Default for OriginProbe {
    fn default() -> Self {
        Self {
            dx: 2.0,
            grid: super::musical_time::SnapGrid::Quarter,
            bypass: false,
            viewport_length: 384_000,
        }
    }
}

impl OriginProbe {
    fn preserves_origin(self) -> bool {
        !self.bypass && self.dx.abs() <= 8.0
    }
}

fn assert_audio_origin_window(cx: &mut TestAppContext, viewport_length: u64) {
    // A fixed frame radius cannot satisfy both zooms' near/far pixel pairs.
    for dx in [7.0, 9.0] {
        assert_drag_handoff_at_grab(
            cx,
            &[false],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                dx,
                viewport_length,
                ..Default::default()
            }),
        );
    }
}

fn assert_drag_handoff_at_grab(
    cx: &mut TestAppContext,
    midi: &[bool],
    vertical: bool,
    copy: bool,
    pending: bool,
    grab_fraction: Option<f32>,
    origin: Option<OriginProbe>,
) {
    use hex_session::service::Request;
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (audio, tracks) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (
            p.tracks()[0].clips()[0].id(),
            p.tracks().iter().map(|t| t.id()).collect::<Vec<_>>(),
        )
    });
    let edit = |command, visual: &mut VisualTestContext| {
        editor.update(&mut *visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump_move(&editor, visual);
    };
    edit(
        Command::AddTrack {
            name: "third".into(),
        },
        &mut visual,
    );
    if midi[0] {
        edit(Command::RemoveClip(audio), &mut visual);
    } else if grab_fraction.is_some() {
        edit(
            Command::TrimClip {
                clip: audio,
                start: 0,
                duration: 84_000,
                source_offset: 0,
                source_length: 84_000,
            },
            &mut visual,
        );
    }
    let mut clips = Vec::new();
    for (index, is_midi) in midi.iter().copied().enumerate() {
        let clip = if is_midi {
            edit(
                Command::PlaceMidiClip {
                    track: tracks[index],
                    start: BeatTime::from_quarters(2 * index as u64).unwrap(),
                    length: if grab_fraction.is_some() {
                        BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2)
                    } else {
                        BeatTime::ONE
                    },
                },
                &mut visual,
            );
            let clip = editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(tracks[index])
                    .unwrap()
                    .midi_clips()[0]
                    .id()
            });
            edit(
                Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                },
                &mut visual,
            );
            clip
        } else if index == 0 {
            audio
        } else {
            edit(
                Command::CopyClipTo {
                    clip: audio,
                    track: tracks[index],
                    start: 48_000,
                },
                &mut visual,
            );
            editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(tracks[index])
                    .unwrap()
                    .clips()[0]
                    .id()
            })
        };
        clips.push(clip);
    }
    if origin.is_some() {
        for (clip, midi) in clips.iter().zip(midi) {
            let command = editor.read_with(&visual, |v, _| {
                let project = &v.state.as_ref().unwrap().project;
                if *midi {
                    let item = project.midi_clip(*clip).unwrap();
                    Command::MoveMidiClip {
                        clip: *clip,
                        track: project
                            .tracks()
                            .iter()
                            .find(|t| t.midi_clips().iter().any(|c| c.id() == *clip))
                            .unwrap()
                            .id(),
                        start: BeatTime::from_raw(
                            item.start().raw() + BeatTime::ONE.raw() / 3 + 137,
                        ),
                    }
                } else {
                    let item = project.clip(*clip).unwrap();
                    Command::MoveClip {
                        clip: *clip,
                        track: project
                            .tracks()
                            .iter()
                            .find(|t| t.clips().iter().any(|c| c.id() == *clip))
                            .unwrap()
                            .id(),
                        start: item.start() + 15_137,
                    }
                }
            });
            edit(command, &mut visual);
        }
    }
    for (i, clip) in clips.iter().enumerate() {
        let header = visual
            .debug_bounds(selector(format!("clip-name-{}", clip.value())))
            .unwrap();
        visual.simulate_click(
            header.center(),
            Modifiers {
                platform: i != 0,
                ..Default::default()
            },
        );
    }
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    if grab_fraction.is_some() {
        editor.update(&mut visual, |view, cx| {
            view.snap_grid = super::musical_time::SnapGrid::Quarter;
            cx.notify();
        });
    }
    if let Some(probe) = origin {
        editor.update(&mut visual, |view, cx| {
            view.snap_grid = probe.grid;
            view.viewport = Some(super::viewport::Viewport {
                start: 0,
                length: probe.viewport_length,
            });
            cx.notify();
        });
        visual.run_until_parked();
    }
    let anchor = clips[0]; // Deliberately drag the non-primary member of a group.
    let move_bounds = visual
        .debug_bounds(selector(format!("clip-drag-surface-{}", anchor.value())))
        .unwrap();
    let source_bounds = visual
        .debug_bounds(selector(if grab_fraction.is_some() {
            format!(
                "{}clip-visual-{}",
                if midi[0] { "midi-" } else { "" },
                anchor.value()
            )
        } else {
            format!("clip-drag-surface-{}", anchor.value())
        }))
        .unwrap();
    let down = point(
        source_bounds.left() + source_bounds.size.width * grab_fraction.unwrap_or(0.5),
        move_bounds.center().y,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    if let Some(fraction) = grab_fraction {
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.interaction.pointer_down.unwrap().snap_edge,
                Some(if fraction >= 2.0 / 3.0 {
                    super::drag::ClipSnapEdge::Trailing
                } else {
                    super::drag::ClipSnapEdge::Leading
                }),
                "edge is fixed at the initial press"
            );
        });
    }
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let lane = editor.read_with(&visual, |v, _| {
        v.timeline_caches.drag_lanes[&tracks[usize::from(vertical)]]
    });
    let end = point(
        down.x + px(origin.map_or(120.0, |probe| probe.dx)),
        px(lane.top + lane.height * 0.5),
    );
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    let release_modifiers = Modifiers {
        alt: copy,
        platform: origin.is_some_and(|probe| probe.bypass),
        ..Default::default()
    };
    visual.simulate_modifiers_change(release_modifiers);
    visual.run_until_parked();
    if let Some(fraction) = grab_fraction {
        editor.read_with(&visual, |view, _| {
            use super::drag::{ClipPosition, ClipSnapEdge};
            let drag = view.interaction.active_clip_drag.as_ref().unwrap();
            let trailing = fraction >= 2.0 / 3.0;
            assert_eq!(drag.source_span.get(), Some((f32::from(source_bounds.left()), f32::from(source_bounds.size.width))),
                "snap capture uses the completed full clip bounds");
            assert_eq!(
                drag.snap_edge.get(),
                Some(if trailing {
                    ClipSnapEdge::Trailing
                } else {
                    ClipSnapEdge::Leading
                }),
                "fraction={fraction}, source={source_bounds:?}, original={:?}, press={:?}, offset={:?}",
                drag.position, drag.capture.get(), drag.grab_offset.get()
            );
            let placed = drag.preview.get().unwrap().position;
            if let Some(probe) = origin {
                if probe.preserves_origin() { assert_eq!(placed, drag.position, "captured typed origin"); }
                else { assert_ne!(placed, drag.position, "free or out-of-capture movement"); }
                if probe.bypass {
                    let (original, actual) = match (drag.position, placed) {
                        (ClipPosition::Audio { start: original, .. }, ClipPosition::Audio { start: actual, .. }) => (original, actual),
                        (ClipPosition::Midi { start: original, .. }, ClipPosition::Midi { start: actual, .. }) => (view.state.as_ref().unwrap().project.tempo_map().beat_to_frame(original).unwrap(), view.state.as_ref().unwrap().project.tempo_map().beat_to_frame(actual).unwrap()),
                        _ => panic!("clip kind changed"),
                    };
                    let expected = original as f64 + f64::from(probe.dx) * drag.viewport().length as f64 / f64::from(lane.width);
                    assert!((actual as f64 - expected).abs() <= 2.0, "command is free: actual={actual}, expected={expected}");
                }
                if probe.bypass || matches!(probe.grid, super::musical_time::SnapGrid::Off) { return; }
                if probe.preserves_origin() { return; }
            }
            match (drag.position, placed) {
                (
                    ClipPosition::Audio { duration, .. },
                    ClipPosition::Audio {
                        start,
                        duration: moved,
                    },
                ) => {
                    assert_eq!(duration, moved);
                    assert_eq!((start + if trailing { duration } else { 0 }) % 24_000, 0);
                    if trailing {
                        assert_ne!(start % 24_000, 0);
                    }
                }
                (
                    ClipPosition::Midi { length, .. },
                    ClipPosition::Midi {
                        start,
                        length: moved,
                    },
                ) => {
                    assert_eq!(length, moved);
                    assert_eq!(
                        (start.raw() + if trailing { length.raw() } else { 0 })
                            % BeatTime::ONE.raw(),
                        0
                    );
                    if trailing {
                        assert_ne!(start.raw() % BeatTime::ONE.raw(), 0);
                    }
                }
                _ => panic!("clip kind changed"),
            }
        });
    }
    let previews: Vec<_> = clips
        .iter()
        .map(|clip| {
            let preview = if clips.len() == 1 {
                "clip-drop-preview"
            } else {
                selector(format!("group-clip-preview-{}", clip.value()))
            };
            let bounds = visual.debug_bounds(preview).expect("held destination");
            let fill = visual
                .rendered_background(preview)
                .expect("completed held scene fill");
            let title = selector(format!("clip-drag-name-{}", clip.value()));
            let title_fill = visual.rendered_background(title).expect("held title color");
            (preview, bounds, fill, title, title_fill)
        })
        .collect();
    let anchor_x = previews[0].1.left();
    assert_clip_move_guides(&mut visual, anchor_x);
    let destination = editor.read_with(&visual, |v, _| {
        v.interaction
            .active_clip_drag
            .as_ref()
            .unwrap()
            .cursor_position()
            .unwrap()
    });
    let held_notes: Vec<_> = clips
        .iter()
        .zip(midi)
        .filter(|(_, midi)| **midi)
        .map(|(clip, _)| {
            (
                *clip,
                super::midi_thumbnail::rendered_notes(&mut visual, *clip, true),
            )
        })
        .collect();
    if pending {
        // Real control-worker work that preserves the canonical project/revision.
        // The fixture poll timer remains parked until pump_move below.
        editor.update(&mut visual, |v, _| {
            let revision = v.state.as_ref().unwrap().revision;
            assert!(v.enqueue(Request::PrepareTuningAudition {
                revision,
                input: Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0)),
            }));
            assert_eq!(v.worker_requests_pending, 1);
        });
    }
    visual.simulate_mouse_up(end, MouseButton::Left, release_modifiers);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before);
        assert_eq!(
            v.worker_requests_pending,
            if pending { 2 } else { 1 },
            "a valid release must enqueue its own move behind unrelated work: {:?}",
            v.error
        );
        assert!(v.interaction.active_clip_drag.is_none());
    });
    // Repaint explicitly with ACK intake held. These inspect completed scenes,
    // not only layout bounds or a wall-clock promise that a frame occurred.
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_clip_move_guides(&mut visual, anchor_x);
        assert!(
            visual.debug_bounds("clip-drop-overlap-0").is_none(),
            "release retires hover feedback independently of destination paint"
        );
        for (preview, bounds, fill, title, title_fill) in &previews {
            assert_eq!(
                visual.debug_bounds(preview),
                Some(*bounds),
                "pending destination"
            );
            assert_eq!(visual.rendered_background(preview), Some(*fill));
            assert_eq!(visual.rendered_background(title), Some(*title_fill));
        }
        for (clip, notes) in &held_notes {
            assert!(!notes.is_empty(), "held production note rectangles");
            assert_eq!(
                super::midi_thumbnail::rendered_notes(&mut visual, *clip, true),
                *notes
            );
        }
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let cache = v.timeline_caches.waveform_geometry_cache.borrow();
            for ((clip, is_midi), (_, bounds, _, _, _)) in clips.iter().zip(midi).zip(&previews) {
                if *is_midi {
                    continue;
                }
                let root = state
                    .waveforms
                    .get(&state.project.clip(*clip).unwrap().asset())
                    .unwrap();
                assert!(
                    cache
                        .rendered_waveforms()
                        .iter()
                        .any(|p| Arc::ptr_eq(&p.source, root)),
                    "pending audio retains original waveform source"
                );
                assert!(
                    cache
                        .painted_waveforms()
                        .iter()
                        .any(|p| p.bounds.left() >= bounds.left()
                            && p.bounds.left() < bounds.right()
                            && p.bounds.top() >= bounds.top()
                            && p.bounds.top() < bounds.bottom()),
                    "actual destination waveform paint"
                );
            }
        });
        for (clip, is_midi) in clips.iter().zip(midi) {
            let source = selector(format!(
                "{}clip-visual-{}",
                if *is_midi { "midi-" } else { "" },
                clip.value()
            ));
            assert!(
                visual.debug_bounds(source).is_some() == copy,
                "copies retain originals; moves suppress originals"
            );
        }
    }
    let command = editor.read_with(&visual, |v, _| {
        v.test_probes.last_enqueued_edit.clone().unwrap()
    });
    let mut expected = Editor::new((*before).clone(), 32).unwrap();
    let created = expected.execute(command).unwrap();
    let canonical_ids: Vec<_> = if copy {
        let (first, last) = match created {
            hex_project::Created::Clip(id) => (id, id),
            hex_project::Created::Clips { first, last } => (first, last),
            other => panic!("expected canonical copies: {other:?}"),
        };
        let mut ids: Vec<_> = expected
            .project()
            .tracks()
            .iter()
            .flat_map(|track| {
                track
                    .clips()
                    .iter()
                    .map(|clip| clip.id())
                    .chain(track.midi_clips().iter().map(|clip| clip.id()))
            })
            .filter(|id| *id >= first && *id <= last)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids.len(), clips.len());
        ids
    } else {
        clips.clone()
    };
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_clip_move.is_none());
        let cursor = v
            .interaction
            .edit_cursor
            .expect("cursor retained at the committed drop");
        assert_eq!((cursor.track, cursor.frame), destination);
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            expected.project().snapshot(),
            "exact canonical content, IDs, offsets and extent"
        );
    });
    assert!(visual.debug_bounds("clip-move-guide-top").is_none());
    assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
    assert!(visual.debug_bounds("clip-move-guide-line").is_none());
    let marker = visual
        .debug_bounds("time-marker-top-start")
        .expect("committed edit cursor");
    assert!((marker.center().x - anchor_x).abs() <= px(1.0));
    for ((clip, is_midi), (preview, held, _, _, _)) in canonical_ids.iter().zip(midi).zip(&previews)
    {
        assert!(visual.debug_bounds(preview).is_none());
        let canonical = selector(format!(
            "{}clip-visual-{}",
            if *is_midi { "midi-" } else { "" },
            clip.value()
        ));
        let actual = visual
            .debug_bounds(canonical)
            .expect("painted canonical destination");
        assert!((f32::from(actual.left() - held.left())).abs() <= 1.0);
        assert!((f32::from(actual.top() - held.top())).abs() <= 1.0);
        assert!((f32::from(actual.size.width - held.size.width)).abs() <= 1.0);
        assert!(visual.rendered_background(canonical).is_some());
    }
    if origin.is_some_and(OriginProbe::preserves_origin) {
        editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            for ((original, current), midi) in clips.iter().zip(&canonical_ids).zip(midi) {
                if *midi {
                    assert_eq!(
                        project.midi_clip(*current).unwrap().start(),
                        before.midi_clip(*original).unwrap().start(),
                        "group MIDI origin stays Q32.32 exact"
                    );
                } else {
                    assert_eq!(
                        project.clip(*current).unwrap().start(),
                        before.clip(*original).unwrap().start(),
                        "group audio origin stays frame exact"
                    );
                }
            }
        });
    }
    let committed = expected.project().snapshot();
    visual.simulate_keystrokes("cmd-z");
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let actual = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = before.snapshot();
        // Undo retires copied identities; it never rewinds their allocator.
        assert_eq!(actual.next_id, committed.next_id);
        expected.next_id = committed.next_id;
        assert_eq!(actual, expected, "one group Undo")
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_move(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            committed,
            "one group Redo"
        )
    });
}

#[gpui::test]
fn rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = audio_project();
    let asset = project.clip(clip).expect("audio clip").asset();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    editor.update(cx, |view, cx| {
        let old = view.state.as_ref().expect("project state");
        let mut waveforms = BTreeMap::new();
        waveforms.insert(
            asset,
            Arc::new(
                CachedWaveform::from_peaks(
                    96_000,
                    vec![
                        [
                            CachedPeak {
                                min: -0.7,
                                max: 0.5,
                            },
                            CachedPeak {
                                min: -0.4,
                                max: 0.8,
                            },
                        ];
                        64
                    ],
                )
                .expect("waveform fixture"),
            ),
        );
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::clone(&old.project),
            waveforms: Arc::new(waveforms),
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::clone(&old.midi_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: old.timeline_frames,
            can_undo: old.can_undo,
            can_redo: old.can_redo,
            revision: old.revision,
            unsaved: old.unsaved,
            document_path: old.document_path.clone(),
        }));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let trim_selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
    let clip_selector = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let handle_bounds = visual.debug_bounds(trim_selector).expect("right trim edge");
    let original_bounds = visual
        .debug_bounds(clip_selector)
        .expect("painted audio clip");
    // Edge trimming covers only the header rail at the top of the clip.
    assert_eq!(handle_bounds.origin.y, original_bounds.origin.y);
    let header = super::trim::edge_trim_height(f32::from(original_bounds.size.height));
    assert!((f32::from(handle_bounds.size.height) - header).abs() <= 1.0);
    let move_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("central move target");
    assert!(
        (f32::from(move_bounds.size.height) - f32::from(original_bounds.size.height) / 3.0).abs()
            < 1.0
    );
    assert!(move_bounds.origin.x + move_bounds.size.width <= handle_bounds.origin.x);

    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    assert_eq!(original_bounds.origin.y, lane.origin.y);
    assert_eq!(original_bounds.size.height, lane.size.height);
    assert!(visual.debug_bounds("clip-musical-grid").is_some());
    let grid_frame = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("project state");
        let viewport = view.visible_viewport(state.timeline_frames);
        let width = view
            .timeline_caches
            .arrangement_bounds
            .map(|bounds| bounds.width - view.track_header_width())
            .unwrap_or(400.0);
        super::musical_time::grid_lines(&state.project, viewport, width, view.snap_grid)
            .into_iter()
            .find(|line| line.frame > 0 && line.frame < 96_000)
            .map(|line| line.frame)
    });
    let grid_frame = grid_frame.expect("adaptive grid has an interior line in the audio clip");
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("clip-grid-line-{grid_frame}").into_boxed_str()
            ))
            .is_some()
    );
    let down = trim_grab_point(handle_bounds);
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeRightBracket),
        "right edge selects its own bracket cursor"
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.interaction
                .pointer_down
                .as_ref()
                .map(|down| down.part()),
            Some(hex_widgets::RectanglePart::Trailing),
            "right trim edge captures its edge mouse-down: {:?}",
            view.interaction.pointer_down
        );
    });
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        cx.update(|app| app.has_active_drag()),
        "trim handle starts a native drag"
    );
    let target = point(down.x - px(42.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("selected-track-lane-{}", source.value()).into_boxed_str()
            ))
            .is_none(),
        "resizing a clip must not promote its track into explicit selection paint"
    );
    let snapped_duration = editor.read_with(cx, |view, _| {
        let drag = view.interaction.active_trim_drag.as_ref().expect("active trim drag");
        assert_eq!(drag.source_track, source);
        assert!(!drag.snap_modifier.get());
        let state = drag.preview.borrow();
        let preview = state.as_ref().unwrap_or_else(|| panic!(
            "snapped preview missing: current={}, drag={drag:?}, lanes={:?}, pending={}, worker={}",
            view.trim_drag_current(drag), view.timeline_caches.drag_lanes, view.pending_requests.len(), view.worker_requests_pending
        ));
        preview.clip.duration()
    });
    let snapped_bounds = visual
        .debug_bounds("trim-live-preview")
        .expect("snapped clip preview overlay");
    editor.update(cx, |view, cx| {
        if let Ok(mut cache) = view
            .timeline_caches
            .waveform_geometry_cache
            .try_borrow_mut()
        {
            cache.clear();
        }
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.timeline_caches
                .waveform_geometry_cache
                .try_borrow()
                .expect("waveform cache available")
                .entry_count(),
            1,
            "trim preview paints only its visible waveform geometry"
        );
    });
    assert!(f32::from(snapped_bounds.size.width) < f32::from(original_bounds.size.width));
    assert_eq!(
        visual
            .debug_bounds(clip_selector)
            .expect("stable source hit geometry"),
        original_bounds,
        "the source hit target stays at its original geometry while the preview moves"
    );

    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(command);
    visual.run_until_parked();
    let free_duration = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_trim_drag
            .as_ref()
            .expect("trim remains held");
        assert!(drag.snap_modifier.get());
        drag.preview
            .borrow()
            .as_ref()
            .expect("free trim preview after stationary modifier change")
            .clip
            .duration()
    });
    assert_ne!(
        snapped_duration, free_duration,
        "command recomputes a stationary trim candidate"
    );
    let free_bounds = visual
        .debug_bounds("trim-live-preview")
        .expect("free clip preview overlay");
    assert_ne!(free_bounds.size.width, snapped_bounds.size.width);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .expect("view state")
                .project
                .clip(clip)
                .expect("original clip")
                .duration(),
            96_000
        );
        assert!(
            view.test_probes.last_enqueued_edit.is_none(),
            "preview must not mutate canonical project"
        );
    });
    assert!(f32::from(lane.size.width) > 0.0);

    visual.simulate_mouse_up(target, MouseButton::Left, command);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.test_probes.last_trim_release_snap, Some(true), "mouse-up platform modifier reaches drop resolver");
        let expected_end = free_duration;
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::TrimClipEdge { clip: requested, edge: hex_project::TrimEdge::Right, frame })
                if *requested == clip && *frame == expected_end
        ), "release must dispatch the same free candidate as its preview ({expected_end}): {:?}", view.test_probes.last_enqueued_edit);
        assert!(view.interaction.active_trim_drag.is_none(), "release clears the transient trim preview");
    });
}

#[gpui::test]
fn rendered_audio_trim_release_upper_rail_commits_captured_clip_and_actual_worker_ack(
    cx: &mut TestAppContext,
) {
    use std::time::Duration;
    fn pump(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
        let until = crate::test_wait::hang_deadline();
        loop {
            let done = editor.update(&mut *visual, |view, cx| {
                if view.tick(cx) {
                    cx.notify();
                }
                assert!(view.error.is_none(), "{:?}", view.error);
                view.worker_requests_pending == 0 && view.pending_requests.is_empty()
            });
            if done {
                visual.run_until_parked();
                return;
            }
            assert!(
                std::time::Instant::now() < until,
                "pump: hang guard expired"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let (source, clip, before, target_before) = editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (
            project.tracks()[0].id(),
            project.tracks()[0].clips()[0].id(),
            project.snapshot(),
            project.tracks()[1].clone(),
        )
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-trim-left-{}", clip.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("left trim edge");
    let down = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    // The historical x=lane.left-20 was inside the browser. Browser release
    // intentionally commits, as does captured release on the actual upper rail.
    let outside = point(lane.center().x, lane.origin.y - px(20.0));
    let browser = visual
        .debug_bounds("resize-browser-hit-region")
        .expect("actual browser region");
    assert!(
        !browser.contains(&outside),
        "release fixture stays outside browser"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.timeline_caches.arrangement_bounds_epoch,
            view.timeline_caches.drag_layout_generation
        );
        let body = view
            .timeline_caches
            .arrangement_bounds
            .expect("actual Arrangement body");
        let bounds = gpui::Bounds::new(
            point(px(body.left), px(body.top)),
            gpui::size(px(body.width), px(body.height)),
        );
        assert!(
            !bounds.contains(&outside),
            "release fixture stays outside Arrangement body"
        );
        assert!(
            view.hovered_drag_target(f32::from(outside.x), f32::from(outside.y))
                .is_none()
        );
    });
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let (expected_frame, expected_clip) = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_trim_drag
            .as_ref()
            .expect("captured source survives upper rail");
        assert_eq!(drag.source_track, source);
        assert_eq!(drag.clip.id(), clip);
        assert!(!drag.stretch_modifier.get());
        let preview = drag.preview.borrow();
        let preview = preview.as_ref().expect("actual admitted displayed preview");
        // The canonical model may clamp a raw command point (here 192000)
        // to a one-frame source extent. Keep the exact command separately.
        let project = &view.state.as_ref().unwrap().project;
        let asset_frames = project.asset(drag.clip.asset()).unwrap().playback_frames();
        assert_eq!(
            drag.clip
                .preview_edge_trim_with_tempo(
                    hex_project::TrimEdge::Left,
                    preview.command_frame_for_test(),
                    asset_frames,
                    project.tempo_map()
                )
                .unwrap(),
            preview.clip,
            "displayed geometry is the canonical projection of the exact command"
        );
        assert_ne!(
            preview.clip.start(),
            0,
            "fixture makes a real left-edge edit"
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.test_probes.last_enqueued_edit.is_none());
        (preview.command_frame_for_test(), preview.clip.clone())
    });
    let preview_bounds = visual.debug_bounds("trim-live-preview").unwrap();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.test_probes.last_trim_release_snap, Some(false), "actual release modifier");
        assert!(matches!(view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::TrimClipEdge { clip: requested, edge: hex_project::TrimEdge::Left, frame })
                if *requested == clip && *frame == expected_frame),
            "upper-rail release submits the captured displayed left edge: {:?}", view.test_probes.last_enqueued_edit);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before, "canonical remains unchanged before actual worker ACK");
        assert!(view.interaction.active_trim_drag.is_none(), "release retires active capture");
        assert!(view.interaction.pending_trim.is_some(), "accepted preview remains while its own ACK is pending");
        assert_eq!(view.worker_requests_pending, 1);
    });
    assert!(!cx.update(|app| app.has_active_drag()));
    for _ in 0..3 {
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("trim-live-preview"),
            Some(preview_bounds)
        );
    }
    pump(&editor, &mut visual);
    assert_eq!(
        visual.debug_bounds(self::selector(format!("clip-visual-{}", clip.value()))),
        Some(preview_bounds),
        "one-frame trim keeps identical endpoints through its actual ACK"
    );
    let committed = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.clip(clip), Some(&expected_clip));
        assert_eq!(
            state.project.tracks()[1],
            target_before,
            "no hovered/other track retarget"
        );
        assert!(
            view.interaction.pending_trim.is_none(),
            "own canonical terminal retires pending preview"
        );
        assert!(view.interaction.active_trim_drag.is_none());
        assert_eq!(view.worker_requests_pending, 0);
        state.project.snapshot()
    });
    eprintln!(
        "upper-rail release source={source:?} clip={clip:?} frame={expected_frame} actual worker accepted extent={}..{}",
        expected_clip.start(),
        expected_clip.start() + expected_clip.duration()
    );
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            before,
            "one Undo restores the entire canonical snapshot"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            committed,
            "one Redo restores exactly the accepted edit"
        )
    });
}

#[gpui::test]
fn rendered_midi_trim_previews_note_geometry_and_commits_release_snap(cx: &mut TestAppContext) {
    let (project, source, _, clip) = midi_project();
    let (_, midi_preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(midi_preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let trim_selector =
        Box::leak(format!("midi-clip-trim-right-{}", clip.value()).into_boxed_str());
    let clip_selector = Box::leak(format!("midi-clip-visual-{}", clip.value()).into_boxed_str());
    let handle_bounds = visual
        .debug_bounds(trim_selector)
        .expect("right MIDI trim edge");
    let original_bounds = visual
        .debug_bounds(clip_selector)
        .expect("painted MIDI clip");
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("MIDI source lane");
    assert_eq!(original_bounds.origin.y, lane.origin.y);
    assert_eq!(original_bounds.size.height, lane.size.height);
    assert!(visual.debug_bounds("clip-musical-grid").is_some());
    let down = trim_grab_point(handle_bounds);
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeRightBracket),
        "right MIDI edge selects its own bracket cursor"
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        cx.update(|app| app.has_active_drag()),
        "MIDI trim starts a drag"
    );

    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let target = point(down.x - px(28.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let preview_end = editor.read_with(cx, |view, _| {
        let drag = view
            .midi_editor
            .active_trim_drag
            .as_ref()
            .expect("active MIDI trim");
        assert!(
            drag.snap_modifier.get(),
            "command bypasses the adaptive grid"
        );
        let preview = drag.preview.borrow().expect("live MIDI preview");
        assert_eq!(preview.start(), BeatTime::ONE);
        preview.start().raw() + preview.length().raw()
    });
    let preview_bounds = visual
        .debug_bounds("midi-trim-live-preview")
        .expect("resized MIDI preview overlay");
    assert!(
        f32::from(preview_bounds.size.width) < f32::from(original_bounds.size.width),
        "resizing right edge shortens the visible MIDI clip"
    );
    assert_eq!(
        visual
            .debug_bounds(clip_selector)
            .expect("stable source geometry"),
        original_bounds,
        "original MIDI hit target remains at its initial bounds during preview"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_clip(clip)
                .unwrap()
                .length(),
            BeatTime::from_quarters(4).unwrap(),
            "preview does not mutate canonical project state"
        );
        assert!(view.test_probes.last_enqueued_edit.is_none());
    });

    visual.simulate_mouse_up(target, MouseButton::Left, command);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(
                view.test_probes.last_enqueued_edit.as_ref(),
                Some(Command::TrimMidiClipEdge {
                    clip: requested,
                    edge: hex_project::TrimEdge::Right,
                    beat,
                }) if *requested == clip && beat.raw() == preview_end
            ),
            "release commits the same MIDI edge shown by the preview: {:?}",
            view.test_probes.last_enqueued_edit
        );
        assert!(view.midi_editor.active_trim_drag.is_none());
    });
}

#[gpui::test]
fn rendered_midi_left_trim_keeps_source_note_timeline_position(cx: &mut TestAppContext) {
    let (project, source, _, clip) = midi_project();
    let mut editor_model = Editor::new(project, 8).expect("editable project");
    editor_model
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(2).expect("note start"),
            duration: BeatTime::ONE,
            pitch: 72,
            channel: 0,
            velocity: 90,
        })
        .expect("second source note");
    let project = editor_model.project().clone();
    let (_, midi_preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(midi_preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let trim_selector = Box::leak(format!("midi-clip-trim-left-{}", clip.value()).into_boxed_str());
    let clip_selector = Box::leak(format!("midi-clip-visual-{}", clip.value()).into_boxed_str());
    let handle_bounds = visual
        .debug_bounds(trim_selector)
        .expect("left MIDI trim edge");
    let original_bounds = visual
        .debug_bounds(clip_selector)
        .expect("painted MIDI clip");
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("source lane");
    let down = trim_grab_point(handle_bounds);
    visual.simulate_mouse_move(down, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(CursorStyle::ResizeLeftBracket),
        "left MIDI edge selects its own bracket cursor"
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    let target = point(down.x + px(26.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let preview = editor.read_with(cx, |view, _| {
        let drag = view
            .midi_editor
            .active_trim_drag
            .as_ref()
            .expect("left MIDI trim");
        assert!(drag.snap_modifier.get());
        let preview = drag.preview.borrow().expect("left trim preview");
        assert!(preview.start() > BeatTime::ONE);
        let canonical = view
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_clip(clip)
            .unwrap();
        assert_eq!(canonical.start(), BeatTime::ONE);
        assert_eq!(canonical.source_start(), BeatTime::ZERO);
        assert_eq!(preview.source_start(), canonical.source_start());
        assert_eq!(
            preview.playback_offset().raw(),
            preview.start().raw() - canonical.start().raw(),
            "inward trim advances the retained source window"
        );
        assert_eq!(
            canonical.notes().len(),
            2,
            "notes stay canonical before release"
        );
        preview
    });
    assert_eq!(
        visual
            .debug_bounds(clip_selector)
            .expect("fixed source clip bounds"),
        original_bounds
    );
    let notes = super::midi_thumbnail::rendered_note_layer(&mut visual, "midi-trim-preview-notes");
    let note_frame = 72_000.0_f64; // beat 3 at the project's 120 bpm default
    let expected_note_x = f32::from(lane.origin.x)
        + (f64::from(lane.size.width) * note_frame / TIMELINE_FRAMES as f64) as f32;
    assert!(
        notes
            .iter()
            .any(|(bounds, _)| (f32::from(bounds.origin.x) - expected_note_x).abs() < 2.0),
        "advancing the source marker keeps the later note at its original timeline x: got {notes:?}, expected {expected_note_x}, preview={preview:?}"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::TrimMidiClipEdge {
                clip: requested,
                edge: hex_project::TrimEdge::Left,
                beat,
            }) if *requested == clip && *beat == preview.start()
        ));
    });
}

#[gpui::test]
fn held_clip_drag_scrolls_preview_without_editing_and_rejects_stale_project(
    cx: &mut TestAppContext,
) {
    use super::{musical_time::SnapGrid, viewport::Viewport};
    use std::time::{Duration, Instant};
    let (project, source, _, clip) = audio_project();
    let original = project.clip(clip).unwrap().clone();
    let (window, editor) = editor_window(cx, project, Arc::new(Default::default()));
    editor.update(cx, |view, cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES / 2,
        });
        view.snap_grid = SnapGrid::Off;
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let down = point(
        surface.origin.x + surface.size.width / 2.0,
        surface.origin.y + surface.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let edge = editor.read_with(cx, |view, _| {
        let lane = view.timeline_caches.drag_lanes[&source];
        point(
            px(lane.left + lane.width - 4.0),
            px(lane.top + lane.height / 2.0),
        )
    });
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let now = editor.update(cx, |view, cx| {
        let now = Instant::now();
        let before = view
            .interaction
            .active_clip_drag
            .as_ref()
            .unwrap()
            .preview
            .get()
            .unwrap();
        view.tick_clip_gesture_autoscroll(now, cx);
        assert!(view.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx));
        let drag = view.interaction.active_clip_drag.as_ref().unwrap();
        let after = drag.preview.get().unwrap();
        assert!(after.start_frame > before.start_frame);
        assert_eq!(
            view.state.as_ref().unwrap().project.clip(clip),
            Some(&original)
        );
        assert!(view.drag_start_is_current(drag));
        cx.notify();
        now
    });
    visual.run_until_parked();
    let preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert_clip_move_guides(visual, preview.left());
    editor.update(cx, |view, cx| {
        let viewport = view.viewport;
        let previous = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: previous.project.clone(),
            waveforms: previous.waveforms.clone(),
            source_overviews: previous.source_overviews.clone(),
            audio_clip_waveforms: Default::default(),
            midi_preview: previous.midi_preview.clone(),
            gain_preview: previous.gain_preview.clone(),
            timeline_frames: previous.timeline_frames,
            can_undo: previous.can_undo,
            can_redo: previous.can_redo,
            revision: previous.revision + 1,
            unsaved: previous.unsaved,
            document_path: previous.document_path.clone(),
        }));
        assert!(!view.tick_clip_gesture_autoscroll(now + Duration::from_millis(100), cx));
        assert_eq!(view.viewport, viewport);
        view.interaction.active_clip_drag = None;
        assert!(!view.tick_clip_gesture_autoscroll(now + Duration::from_millis(150), cx));
    });
}

#[gpui::test]
fn vertical_drag_scroll_invalidates_old_rows_before_fast_release(cx: &mut TestAppContext) {
    use super::viewport::Viewport;
    use std::time::{Duration, Instant};

    let (project, source, _, clip) = audio_project();
    let mut project_editor = Editor::new(project, 8).expect("project editor");
    for index in 0..24 {
        let result = project_editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track");
        assert!(matches!(result, Created::Track(_)));
    }
    let project = project_editor.project().clone();
    let (window, editor) = editor_window(cx, project, Arc::new(Default::default()));
    editor.update(cx, |view, cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES / 2,
        });
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("source clip surface");
    let down = point(
        surface.origin.x + surface.size.width / 2.0,
        surface.origin.y + surface.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = editor.read_with(cx, |view, _| {
        let body = view
            .timeline_caches
            .arrangement_bounds
            .expect("painted Arrangement body");
        point(
            px(body.left + body.width * 0.6),
            px(body.top + body.height - 5.0),
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let initial_revision = editor.read_with(cx, |view, _| {
        view.state.as_ref().expect("project state").revision
    });
    editor.update(cx, |view, cx| {
        let before_offset = view.track_scroll.offset().y;
        let generation = view.timeline_caches.drag_layout_generation;
        let now = Instant::now();
        let _ = view.tick_clip_gesture_autoscroll(now, cx);
        if view.track_scroll.offset().y == before_offset {
            assert!(view.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx));
        }
        assert!(
            view.track_scroll.offset().y < before_offset,
            "bottom edge scrolls the actual UniformList"
        );
        assert!(view.timeline_caches.drag_layout_generation > generation);
        assert_ne!(
            view.timeline_caches.arrangement_bounds_epoch,
            view.timeline_caches.drag_layout_generation
        );
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag");
        assert!(drag.preview.get().is_none());
        assert!(drag.destination.get().is_none());
        assert_eq!(
            view.state.as_ref().expect("project state").project.tracks()[0].id(),
            source
        );
    });

    // Release before the invalidated list has painted. The old row still
    // occupies the pointer's screen position, but its epoch cannot authorize an edit.
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.test_probes.last_enqueued_edit.is_none());
        assert_eq!(
            view.state.as_ref().expect("project state").revision,
            initial_revision
        );
    });
}

#[gpui::test]
fn vertical_drag_scroll_retargets_stationary_clip_before_release(cx: &mut TestAppContext) {
    use super::viewport::Viewport;
    use std::time::{Duration, Instant};

    let (project, _source, _, clip) = audio_project();
    let mut project_editor = Editor::new(project, 8).expect("project editor");
    for index in 0..24 {
        let result = project_editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track");
        assert!(matches!(result, Created::Track(_)));
    }
    let project = project_editor.project().clone();
    let original = project.clone();
    let (window, editor) = editor_window(cx, project, Arc::new(Default::default()));
    editor.update(cx, |view, cx| {
        view.viewport = Some(Viewport {
            start: 0,
            length: TIMELINE_FRAMES / 2,
        });
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("source clip surface");
    let down = point(
        surface.origin.x + surface.size.width / 2.0,
        surface.origin.y + surface.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = editor.read_with(cx, |view, _| {
        let body = view
            .timeline_caches
            .arrangement_bounds
            .expect("painted Arrangement body");
        point(
            px(body.left + body.width * 0.6),
            px(body.top + body.height - 5.0),
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let (old_track, scroll_tick) = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag");
        let (x, y) = drag.last_pointer.get().expect("held pointer");
        let old_track = view
            .hovered_drag_target(x, y)
            .expect("pointer begins over a painted track")
            .0;
        (old_track, Instant::now())
    });
    editor.update(cx, |view, cx| {
        let offset = view.track_scroll.offset().y;
        let mut moved = false;
        for tick in 0..4 {
            let now = scroll_tick + Duration::from_millis(tick * 50);
            let _ = view.tick_clip_gesture_autoscroll(now, cx);
            if view.track_scroll.offset().y != offset {
                moved = true;
                break;
            }
        }
        assert!(moved, "held pointer scrolls the UniformList by a row");
        cx.notify();
    });
    visual.run_until_parked();

    let new_track = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag");
        let (x, y) = drag.last_pointer.get().expect("stationary pointer");
        view.hovered_drag_target(x, y)
            .expect("repaint exposes a new track under the stationary pointer")
            .0
    });
    assert_ne!(
        new_track, old_track,
        "the painted destination follows scrolling"
    );
    editor.update(cx, |view, cx| {
        let _ = view.tick_clip_gesture_autoscroll(scroll_tick + Duration::from_millis(1), cx);
    });
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active clip drag");
        assert!(matches!(
            drag.destination.get(),
            Some(super::drag::ClipDropDestination::ExistingTrack(track)) if track == new_track
        ));
        assert!(drag.preview.get().is_some());
    });

    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(cx, |view, _| {
        let command = view
            .test_probes
            .last_enqueued_edit
            .clone()
            .expect("one release edit");
        assert!(matches!(
            &command,
            Command::MoveClipWithOverwrite { clip: moved, track, .. }
                if *moved == clip && *track == new_track
        ));
        command
    });
    let mut history = Editor::new(original.clone(), 8).expect("undo fixture");
    history.execute(command).expect("apply released edit");
    assert!(history.undo(), "one undo reverses the single released edit");
    assert_eq!(history.project(), &original);
}

#[gpui::test]
fn shift_edge_stretch_previews_live_and_stationary_modifier_changes_dispatch_one_edit(
    cx: &mut TestAppContext,
) {
    check_shift_edge(cx, hex_project::TrimEdge::Right);
    check_shift_edge(cx, hex_project::TrimEdge::Left);
}

#[gpui::test]
fn rendered_move_retains_destination_until_real_worker_ack(cx: &mut TestAppContext) {
    assert_move_handoff(cx, &[false], true);
}

#[gpui::test]
fn rendered_move_survives_same_revision_worker_work(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff_with_pending_request(cx, midi, false, false, true);
    }
}

#[gpui::test]
fn midi_origin_capture_is_beat_exact_at_both_edges(cx: &mut TestAppContext) {
    for grab in [1.0 / 3.0, 5.0 / 6.0] {
        assert_drag_handoff_at_grab(
            cx,
            &[true],
            true,
            false,
            false,
            Some(grab),
            Some(OriginProbe::default()),
        );
    }
}

#[gpui::test]
fn rendered_move_real_refusal_retires_only_its_matching_fifo_terminal(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    let (_files, handle, editor) = upper_rail_owned_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (clip, track, before) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (
            p.tracks()[0].clips()[0].id(),
            p.tracks()[1].id(),
            p.snapshot(),
        )
    });
    let (_, end) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 120.0);
    let held = visual.debug_bounds("clip-drop-preview").unwrap();
    let retained = editor.read_with(&visual, |v, _| {
        v.interaction.active_clip_drag.as_ref().unwrap().clone()
    });
    visual.update(|window, cx| {
        assert!(cx.stop_active_drag(window));
    });
    editor.update(&mut visual, |v, cx| {
        // Negative control: retain an actual pointer projection, then submit an
        // explicitly invalid extent to the REAL worker through the same owner.
        // Production release never fabricates this invalid command.
        let drag = v.interaction.active_clip_drag.take().unwrap();
        let invalid = Command::MoveClipWithOverwrite {
            clip,
            track,
            start: u64::MAX,
        };
        v.pending_clip_move = Some(super::pending_clip_move::Pending::new(
            drag,
            invalid.clone(),
            Arc::clone(&v.state.as_ref().unwrap().project),
            v.interaction.selection_intent_epoch,
        ));
        assert!(v.enqueue(Request::device_gesture(
            hex_session::device_gesture::GestureRequest::Cancel {
                key: hex_session::device_gesture::GestureKey {
                    generation: v.project_generation,
                    token: 987
                }
            }
        )));
        assert!(v.enqueue(Request::Edit(invalid)));
        let unrelated = upper_rail_next(&v.service);
        assert!(unrelated.request_complete && unrelated.gesture_request);
        v.receive(unrelated);
        assert!(v.pending_clip_move.is_some());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("clip-drop-preview"),
        Some(held),
        "unrelated real terminal retains display"
    );
    editor.update(&mut visual, |v, cx| {
        let deadline = crate::test_wait::hang_deadline();
        let refused = loop {
            if let Ok(event) = v.service.try_event() {
                break event;
            }
            assert!(std::time::Instant::now() < deadline, "rendered_move_real_refusal_retires_only_its_matching_fifo_terminal: hang guard expired");
            std::thread::yield_now();
        };
        assert!(refused.request_complete && refused.error.is_some());
        assert_eq!(refused.state.project.snapshot(), before);
        v.receive(refused);
        assert!(v.pending_clip_move.is_none());
        cx.notify();
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-drop-preview").is_none());
    assert!(
        retained.preview.get().is_none(),
        "own refusal retires shared payload too"
    );
    let source = selector(format!("clip-visual-{}", clip.value()));
    assert!(visual.debug_bounds(source).is_some() && visual.rendered_background(source).is_some());
}

#[path = "clip_drag_ui_test/move_and_drop_tests.rs"]
mod move_and_drop_tests;
#[path = "clip_drag_ui_test/scroll_and_autoscroll_tests.rs"]
mod scroll_and_autoscroll_tests;
#[path = "clip_drag_ui_test/trim_and_stretch_tests.rs"]
mod trim_and_stretch_tests;
