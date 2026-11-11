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

const TIMELINE_FRAMES: u64 = 384_000;

/// Edge-trim grab point in the clip-name band. A hovered clip shows its
/// corner fade handles over the rest of the edge, and those take precedence.
pub(super) fn trim_grab_point(bounds: gpui::Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    gpui::point(bounds.center().x, bounds.top() + bounds.size.height / 6.0)
}

pub(super) fn audio_project() -> (
    Project,
    hex_project::TrackId,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let Created::Imported {
        track: source,
        clip,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio source".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .expect("import")
    else {
        panic!("import result")
    };
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .expect("target track")
    else {
        panic!("track result")
    };
    (editor.project().clone(), source, target, clip)
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

pub(super) fn editor_window(
    cx: &mut TestAppContext,
    project: Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, 8_000_000).expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                // Geometry fixtures use a fixed frame span, independent of the
                // application's default trailing navigation allowance.
                view.viewport = Some(super::viewport::Viewport::fit(TIMELINE_FRAMES));
                view.state = Some(Arc::new(ViewState {
                    canonical_audio_source: None,
                    comparison_audio: None,
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    source_overviews: Default::default(),
                    audio_clip_waveforms: std::collections::BTreeMap::new(),
                    midi_preview,
                    gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                    timeline_frames: TIMELINE_FRAMES,
                    can_undo: false,
                    can_redo: false,
                    revision: 1,
                    unsaved: false,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    (handle, editor)
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
            view.pointer_down.is_some(),
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
            .active_clip_drag
            .as_ref()
            .expect("threshold movement starts clip drag");
        assert!(
            drag.capture.get().is_some(),
            "drag start retains the original pointer: clip={:?}, generation={}, down={:?}",
            drag.clip,
            drag.project_generation,
            view.pointer_down
        );
        assert!(
            drag.grab_offset.get().is_some(),
            "drag start computes the source grab offset"
        );
    });
    let move_count_before_target = editor.read_with(cx, |view, _| view.drag_move_event_count);

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
        let drag = view.active_clip_drag.as_ref().expect("clip drag active");
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
        assert!(
            view.drag_move_event_count > move_count_before_target,
            "source on_drag_move callback should run for the target-row movement"
        );
        let drag = view.active_clip_drag.as_ref().expect("ui drag state");
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
    let expected_gain =
        editor.read_with(cx, |view, _| view.track_defaults.captured_gain().unwrap());
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
            let bounds =
                editor.read_with(cx, |view, _| view.arrangement_bounds.expect("painted body"));
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
        let drag = view.active_clip_drag.as_ref().expect("active clip drag");
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
        view.active_clip_drag
            .as_ref()
            .expect("active clip drag")
            .position
    }) {
        super::drag::ClipPosition::Audio { .. } => {
            let grab_offset = editor.read_with(cx, |view, _| {
                view.active_clip_drag
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
                view.active_clip_drag
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
            .active_clip_drag
            .as_ref()
            .expect("clip drag remains active");
        assert_eq!(
            drag.destination.get(),
            Some(super::drag::ClipDropDestination::NewTrack {
                before_track: anchor,
                position: expected,
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
                matches!(view.last_enqueued_edit.as_ref(), Some(Command::CopyClipToNewTrack {
                    clip: copied_clip, before_track, initial_gain, start: copied_start, ..
                }) if *copied_clip == clip && *before_track == anchor && *copied_start == start && *initial_gain == Some(expected_gain))
            } else {
                matches!(view.last_enqueued_edit.as_ref(), Some(Command::MoveClipToNewTrack {
                    clip: moved_clip, before_track, initial_gain, start: moved_start, ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start && *initial_gain == Some(expected_gain))
            },
            "audio new-track release request={:?}, destination={expected:?}, error={:?}",
            view.last_enqueued_edit,
            view.error
        ),
        super::drag::ClipPosition::Midi { start, .. } => assert!(
            if copy {
                matches!(view.last_enqueued_edit.as_ref(), Some(Command::CopyMidiClipToNewTrack {
                    clip: copied_clip, before_track, initial_gain, start: copied_start, ..
                }) if *copied_clip == clip && *before_track == anchor && *copied_start == start && *initial_gain == Some(expected_gain))
            } else {
                matches!(view.last_enqueued_edit.as_ref(), Some(Command::MoveMidiClipToNewTrack {
                    clip: moved_clip, before_track, initial_gain, start: moved_start, ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start && *initial_gain == Some(expected_gain))
            },
            "midi new-track release request={:?}, destination={expected:?}, error={:?}",
            view.last_enqueued_edit,
            view.error
        ),
    });
    for _ in 0..2 {
        editor.update(cx, |view, cx| {
            assert!(view.active_clip_drag.is_none());
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

#[gpui::test]
fn new_track_clip_release_keeps_preview_until_worker_accepts_or_refuses(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    for midi in [false, true] {
        for copy in [false, true] {
            for refuse in [false, true] {
                let (_files, handle, editor) = upper_rail_owned_fixture(cx);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
                visual.run_until_parked();
                editor.update(&mut visual, |view, _| {
                    view.track_defaults.set_test_state(0.5, false, true)
                });
                let (source, anchor, audio) = editor.read_with(&visual, |view, _| {
                    let project = &view.state.as_ref().unwrap().project;
                    (
                        project.tracks()[0].id(),
                        project.tracks()[1].id(),
                        project.tracks()[0].clips()[0].id(),
                    )
                });
                let clip = if midi {
                    editor.update(&mut visual, |view, _| {
                        assert!(view.enqueue(Request::Edit(Command::RemoveClip(audio))))
                    });
                    pump_move(&editor, &mut visual);
                    editor.update(&mut visual, |view, _| {
                        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                            track: source,
                            start: BeatTime::ZERO,
                            length: BeatTime::ONE,
                        })))
                    });
                    pump_move(&editor, &mut visual);
                    editor.read_with(&visual, |view, _| {
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .track(source)
                            .unwrap()
                            .midi_clips()[0]
                            .id()
                    })
                } else {
                    audio
                };
                let before = editor.read_with(&visual, |view, _| {
                    Arc::clone(&view.state.as_ref().unwrap().project)
                });
                if refuse {
                    // Advance only the real worker, leaving the displayed snapshot stale.
                    // The captured insertion anchor is then rejected by the worker.
                    editor.update(&mut visual, |view, _| {
                        view.service
                            .submit(Request::Edit(Command::RemoveTrack(anchor)))
                            .unwrap();
                        upper_rail_next(&view.service);
                    });
                }
                drag_source_to_inserted_track(
                    cx,
                    &mut visual,
                    &editor,
                    &before,
                    clip,
                    source,
                    Some(anchor),
                    copy,
                );
                editor.update(&mut visual, |view, _| {
                    view.track_defaults.set_test_state(0.25, false, true)
                });
                if refuse {
                    editor.update(&mut visual, |view, cx| {
                        let event = upper_rail_terminal(&view.service);
                        assert!(event.error.is_some());
                        view.receive(event);
                        assert!(view.pending_clip_move.is_none());
                        assert!(view.state.as_ref().unwrap().project.track(anchor).is_none());
                        cx.notify();
                    });
                } else {
                    pump_move(&editor, &mut visual);
                    editor.read_with(&visual, |view, _| {
                        assert!(view.pending_clip_move.is_none());
                        assert_eq!(
                            view.state.as_ref().unwrap().project.tracks().len(),
                            before.tracks().len() + 1
                        );
                        assert_eq!(view.state.as_ref().unwrap().project.tracks()[1].gain(), 0.5);
                    });
                }
                visual.run_until_parked();
                assert!(visual.debug_bounds("track-insert-preview").is_none());
            }
        }
    }
}

#[gpui::test]
fn new_track_clip_drop_refuses_unavailable_defaults_without_blocking_existing_tracks(
    cx: &mut TestAppContext,
) {
    for loading in [true, false] {
        let (project, _, anchor, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.5, loading, false)
        });
        let down = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap()
            .center();
        let lane = visual
            .debug_bounds(selector(format!("arrangement-lane-{}", anchor.value())))
            .unwrap();
        let target = point(
            lane.origin.x + lane.size.width * 0.7,
            lane.origin.y + px(1.0),
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(view.error.as_deref().unwrap().contains(if loading {
                "loading"
            } else {
                "open settings"
            }));
            assert!(view.last_enqueued_edit.is_none());
            assert!(view.pending_clip_move.is_none());
        });
        editor.update(&mut visual, |view, _| view.error = None);
        let (_, target) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                view.last_enqueued_edit,
                Some(Command::MoveClipWithOverwrite { .. })
            ));
        });
    }
}

#[gpui::test]
fn linked_clip_new_track_drop_captures_gain_and_retains_link(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.5, false, true)
        });
        let (source, anchor, audio) = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            (
                p.tracks()[0].id(),
                p.tracks()[1].id(),
                p.tracks()[0].clips()[0].id(),
            )
        });
        let clip = if midi {
            editor.update(&mut visual, |view, _| {
                assert!(
                    view.enqueue(hex_session::service::Request::Edit(Command::RemoveClip(
                        audio
                    )))
                )
            });
            pump_move(&editor, &mut visual);
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(hex_session::service::Request::Edit(
                    Command::PlaceMidiClip {
                        track: source,
                        start: BeatTime::ZERO,
                        length: BeatTime::ONE
                    }
                )))
            });
            pump_move(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(source)
                    .unwrap()
                    .midi_clips()[0]
                    .id()
            })
        } else {
            audio
        };
        let source_bounds = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let lane = visual
            .debug_bounds(selector(format!("arrangement-lane-{}", anchor.value())))
            .unwrap();
        let down = source_bounds.center();
        let target = point(
            lane.origin.x + lane.size.width * 0.7,
            lane.origin.y + px(1.0),
        );
        let modifiers = Modifiers {
            alt: true,
            platform: true,
            ..Default::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        editor.read_with(&visual, |view, _| {
            assert!(matches!(view.last_enqueued_edit.as_ref(), Some(Command::DuplicateAliasClipToNewTrack { initial_gain: Some(gain), .. }) if *gain == 0.5));
            assert!(view.pending_clip_move.is_some());
        });
        editor.update(&mut visual, |view, _| {
            view.track_defaults.set_test_state(0.25, false, true)
        });
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            let target = &p.tracks()[1];
            assert_eq!(target.gain(), 0.5);
            let linked = if midi {
                target.midi_clips()[0].id()
            } else {
                target.clips()[0].id()
            };
            assert_eq!(p.clip_alias(clip), p.clip_alias(linked));
            assert!(view.pending_clip_move.is_none());
        });
    }
}

#[gpui::test]
fn rendered_clip_drop_highlights_only_occupied_audio_and_midi_spans(cx: &mut TestAppContext) {
    let (project, _, target, clip) = audio_project();
    let mut model = Editor::new(project, 8).expect("model");
    model
        .execute(Command::CopyClipTo {
            clip,
            track: target,
            start: 0,
        })
        .expect("audio victim");
    model
        .execute(Command::PlaceMidiClip {
            track: target,
            start: BeatTime::from_quarters(5).unwrap(),
            length: BeatTime::ONE,
        })
        .expect("midi victim");
    let project = model.project().clone();
    let media = BTreeMap::from([(
        project.clip(clip).unwrap().asset(),
        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
    )]);
    let (_, midi) =
        hex_engine::compile::project_with_midi_preview(&project, &media).expect("compile previews");
    let (handle, editor) = editor_window(cx, project, Arc::new(midi));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .unwrap();
    let delta = f32::from(lane.size.width) * 48_000.0 / TIMELINE_FRAMES as f32;
    let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, delta);
    let (start, end) = editor.read_with(cx, |view, _| {
        let preview = view
            .active_clip_drag
            .as_ref()
            .unwrap()
            .preview
            .get()
            .unwrap();
        (preview.start_frame, preview.end_frame)
    });
    for (index, (victim_start, victim_end)) in [(0_u64, 96_000_u64), (120_000, 144_000)]
        .into_iter()
        .enumerate()
    {
        let overlap = visual
            .debug_bounds(Box::leak(
                format!("clip-drop-overlap-{index}").into_boxed_str(),
            ))
            .unwrap_or_else(|| {
                panic!("occupied span {index} highlighted for landing {start}..{end}")
            });
        let expected_left = lane.origin.x
            + lane.size.width * (start.max(victim_start) as f32 / TIMELINE_FRAMES as f32);
        let expected_width = lane.size.width
            * ((end.min(victim_end) - start.max(victim_start)) as f32 / TIMELINE_FRAMES as f32);
        assert!((overlap.origin.x - expected_left).abs() <= px(1.0));
        assert!((overlap.size.width - expected_width).abs() <= px(1.0));
    }
    assert!(visual.debug_bounds("clip-drop-overlap-2").is_none());
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("clip-drop-overlap-0").is_none(),
        "release clears highlights"
    );
}

#[gpui::test]
fn rendered_clip_drop_excludes_moved_source_but_highlights_copied_source(cx: &mut TestAppContext) {
    let (project, _, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 0, 0.0);
    assert!(visual.debug_bounds("clip-drop-overlap-0").is_none());
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(option);
    visual.run_until_parked();
    let overlap = visual
        .debug_bounds("clip-drop-overlap-0")
        .expect("copy intersects original");
    let preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert!((overlap.origin.x - preview.origin.x).abs() <= px(1.0));
    assert!((overlap.size.width - preview.size.width).abs() <= px(1.0));
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-drop-overlap-0").is_none());
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn rendered_audio_clip_drag_starts_previews_and_commits_a_move(cx: &mut TestAppContext) {
    let (project, source, target, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(
                view.last_enqueued_edit.as_ref(),
                Some(Command::MoveClipWithOverwrite {
                    clip: requested_clip,
                    track: requested_track,
                    ..
                }) if *requested_clip == clip && *requested_track == target
            ),
            "audio release should enqueue MoveClip for source {source:?}; error={:?}",
            view.error
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn option_drag_copies_audio_clip_and_modifier_changes_apply_without_pointer_motion(
    cx: &mut TestAppContext,
) {
    let (project, _source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("audio drag surface");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("destination lane");
    let down = point(
        surface.origin.x + surface.size.width / 2.0,
        surface.origin.y + surface.size.height / 2.0,
    );
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, option);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, option);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_clip_drag
                .as_ref()
                .is_some_and(|drag| drag.copy_modifier.get())
        );
    });
    let target = point(
        target_lane.origin.x + target_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, option);
    visual.run_until_parked();
    let first_preview = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active drag");
        assert!(drag.copy_modifier.get());
        drag.preview.get().expect("copy preview").position
    });

    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active drag");
        assert!(!drag.copy_modifier.get(), "option release updates in place");
    });
    visual.simulate_modifiers_change(option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active drag");
        assert!(drag.copy_modifier.get(), "option press updates in place");
        assert_eq!(
            drag.preview.get().expect("updated preview").position,
            first_preview
        );
    });

    visual.simulate_mouse_up(target, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::CopyClipTo { clip: copied, track, .. })
                if *copied == clip && *track == target_track
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn audio_clip_drop_uses_option_state_at_release_even_without_modifier_event(
    cx: &mut TestAppContext,
) {
    let (project, _source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);

    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_up(release, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::CopyClipTo { clip: copied, track, .. })
                if *copied == clip && *track == target_track
        ));
    });
    visual.run_until_parked();
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
            waveforms,
            source_overviews: old.source_overviews.clone(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
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
            view.pointer_down.as_ref().map(|down| down.part()),
            Some(hex_widgets::RectanglePart::Trailing),
            "right trim edge captures its edge mouse-down: {:?}",
            view.pointer_down
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
        let drag = view.active_trim_drag.as_ref().expect("active trim drag");
        assert_eq!(drag.source_track, source);
        assert!(!drag.snap_modifier.get());
        let state = drag.preview.borrow();
        let preview = state.as_ref().unwrap_or_else(|| panic!(
            "snapped preview missing: current={}, drag={drag:?}, lanes={:?}, pending={}, worker={}",
            view.trim_drag_current(drag), view.drag_lanes, view.pending_requests.len(), view.worker_requests_pending
        ));
        preview.clip.duration()
    });
    let snapped_bounds = visual
        .debug_bounds("trim-live-preview")
        .expect("snapped clip preview overlay");
    editor.update(cx, |view, cx| {
        if let Ok(mut cache) = view.waveform_geometry_cache.try_borrow_mut() {
            cache.clear();
        }
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.waveform_geometry_cache
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
        let drag = view.active_trim_drag.as_ref().expect("trim remains held");
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
            view.last_enqueued_edit.is_none(),
            "preview must not mutate canonical project"
        );
    });
    assert!(f32::from(lane.size.width) > 0.0);

    visual.simulate_mouse_up(target, MouseButton::Left, command);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.last_trim_release_snap, Some(true), "mouse-up platform modifier reaches drop resolver");
        let expected_end = free_duration;
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::TrimClipEdge { clip: requested, edge: hex_project::TrimEdge::Right, frame })
                if *requested == clip && *frame == expected_end
        ), "release must dispatch the same free candidate as its preview ({expected_end}): {:?}", view.last_enqueued_edit);
        assert!(view.active_trim_drag.is_none(), "release clears the transient trim preview");
    });
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
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
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
            view.recent_projects.test_path(),
            root.join("config/recent-projects.json")
        );
        assert_eq!(
            view.theme_settings.test_path(),
            root.join("config/theme.json")
        );
        assert_eq!(
            view.project_directory.test_path(),
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
        assert_eq!(view.arrangement_bounds_epoch, view.drag_layout_generation);
        let body = view.arrangement_bounds.expect("actual Arrangement body");
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
        assert!(view.last_enqueued_edit.is_none());
        (preview.command_frame_for_test(), preview.clip.clone())
    });
    let preview_bounds = visual.debug_bounds("trim-live-preview").unwrap();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.last_trim_release_snap, Some(false), "actual release modifier");
        assert!(matches!(view.last_enqueued_edit.as_ref(),
            Some(Command::TrimClipEdge { clip: requested, edge: hex_project::TrimEdge::Left, frame })
                if *requested == clip && *frame == expected_frame),
            "upper-rail release submits the captured displayed left edge: {:?}", view.last_enqueued_edit);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before, "canonical remains unchanged before actual worker ACK");
        assert!(view.active_trim_drag.is_none(), "release retires active capture");
        assert!(view.pending_trim.is_some(), "accepted preview remains while its own ACK is pending");
        assert_eq!(view.worker_requests_pending, 1);
    });
    assert!(!cx.update(|app| app.has_active_drag()));
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
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
            view.pending_trim.is_none(),
            "own canonical terminal retires pending preview"
        );
        assert!(view.active_trim_drag.is_none());
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
            .active_midi_trim_drag
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
        assert!(view.last_enqueued_edit.is_none());
    });

    visual.simulate_mouse_up(target, MouseButton::Left, command);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(
                view.last_enqueued_edit.as_ref(),
                Some(Command::TrimMidiClipEdge {
                    clip: requested,
                    edge: hex_project::TrimEdge::Right,
                    beat,
                }) if *requested == clip && beat.raw() == preview_end
            ),
            "release commits the same MIDI edge shown by the preview: {:?}",
            view.last_enqueued_edit
        );
        assert!(view.active_midi_trim_drag.is_none());
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
        let drag = view.active_midi_trim_drag.as_ref().expect("left MIDI trim");
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
            view.last_enqueued_edit.as_ref(),
            Some(Command::TrimMidiClipEdge {
                clip: requested,
                edge: hex_project::TrimEdge::Left,
                beat,
            }) if *requested == clip && *beat == preview.start()
        ));
    });
}

#[gpui::test]
fn command_modifier_bypasses_adaptive_snap_while_drag_is_held_and_on_release(
    cx: &mut TestAppContext,
) {
    let (project, source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("rendered audio move surface");
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("rendered source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("rendered target lane");
    let grid_before = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("adaptive grid visible before the modifier gesture");
    let down = point(
        source_bounds.origin.x + source_bounds.size.width / 2.0,
        source_bounds.origin.y + source_bounds.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        source_lane.origin.x + source_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let grid_held = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("command bypass changes snapping without hiding grid");
    assert_eq!(grid_held.origin, grid_before.origin);
    let free_start = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active audio drag");
        assert!(drag.snap_modifier.get());
        match drag.preview.get().expect("free preview").position {
            super::drag::ClipPosition::Audio { start, .. } => start,
            _ => panic!("audio drag stays frame anchored"),
        }
    });

    let free_preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert_clip_move_guides(&mut visual, free_preview.left());
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert_clip_move_guides(&mut visual, snapped_preview.left());
    let grid_released = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("grid remains visible after releasing command");
    assert_eq!(grid_released.origin, grid_before.origin);
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active audio drag");
        assert!(!drag.snap_modifier.get());
        match drag
            .preview
            .get()
            .expect("adaptive preview after key release")
            .position
        {
            super::drag::ClipPosition::Audio { start, .. } => start,
            _ => panic!("audio drag stays frame anchored"),
        }
    });
    assert_ne!(
        snapped_start, free_start,
        "stationary modifier release restores snapping"
    );

    // The release event's modifier wins even when no final mouse move arrives.
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::MoveClipWithOverwrite { clip: moved, track, start })
                if *moved == clip && *track == target_track && *start == free_start
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn command_modifier_temporarily_bypasses_beat_anchored_midi_drag_snap(cx: &mut TestAppContext) {
    let (project, source, target_track, clip) = midi_project();
    let (_, midi_preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(midi_preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("rendered midi move surface");
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("rendered source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("rendered target lane");
    let down = point(
        source_bounds.origin.x + source_bounds.size.width / 2.0,
        source_bounds.origin.y + source_bounds.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        source_lane.origin.x + source_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let free_start = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active midi drag");
        assert!(drag.snap_modifier.get());
        match drag.preview.get().expect("free preview").position {
            super::drag::ClipPosition::Midi { start, .. } => start,
            _ => panic!("midi drag stays beat anchored"),
        }
    });
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active midi drag");
        assert!(!drag.snap_modifier.get());
        match drag
            .preview
            .get()
            .expect("adaptive preview after key release")
            .position
        {
            super::drag::ClipPosition::Midi { start, .. } => start,
            _ => panic!("midi drag stays beat anchored"),
        }
    });
    assert_ne!(
        snapped_start, free_start,
        "stationary modifier release restores snapping"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::MoveMidiClipWithOverwrite { clip: moved, track, start })
                if *moved == clip && *track == target_track && *start == free_start
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn rendered_audio_clip_drop_between_tracks_inserts_and_preserves_pointer_grab(
    cx: &mut TestAppContext,
) {
    let (project, source, anchor, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        false,
    );
}

#[gpui::test]
fn rendered_audio_clip_drop_below_last_track_inserts_without_workspace_jump(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        None,
        false,
    );
}

#[gpui::test]
fn option_drag_copies_audio_clip_into_an_appended_track(cx: &mut TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(cx, &mut visual, &editor, &project, clip, source, None, true);
}

#[gpui::test]
fn rendered_midi_clip_drop_between_tracks_inserts_and_preserves_beat_anchor(
    cx: &mut TestAppContext,
) {
    let (project, source, anchor, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        false,
    );
}

#[gpui::test]
fn option_drag_copies_midi_clip_into_a_track_between_rows(cx: &mut TestAppContext) {
    let (project, source, anchor, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        true,
    );
}

#[gpui::test]
fn rendered_midi_clip_drop_below_last_track_inserts_without_workspace_jump(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        None,
        false,
    );
}

#[gpui::test]
fn dropping_clip_on_pinned_main_cancels_instead_of_appending_track(cx: &mut TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let track_count = project.tracks().len();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("source clip surface");
    let main = visual
        .debug_bounds("pinned-main-track")
        .expect("pinned main row");
    let down = point(
        source_surface.origin.x + source_surface.size.width / 2.0,
        source_surface.origin.y + source_surface.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));

    let target = point(main.origin.x + px(20.0), main.center().y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("drag stays captured");
        assert!(drag.destination.get().is_none());
        assert!(drag.preview.get().is_none());
        assert_eq!(view.arrangement_bounds_epoch, view.drag_layout_generation);
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks().len(),
            track_count
        );
        assert!(view.last_enqueued_edit.is_none());
        assert!(view.active_clip_drag.is_none());
        assert!(view.state.as_ref().unwrap().project.track(source).is_some());
    });
}

#[gpui::test]
fn rendered_midi_drag_moves_between_rows_and_remains_pickable(cx: &mut TestAppContext) {
    let (project, source, target, clip) = midi_project();
    let original_project = project.clone();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_drop_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 320.0);
    let destination_start = editor
        .read_with(cx, |view, _| {
            let preview = view.active_clip_drag.as_ref()?.preview.get()?;
            let super::drag::ClipPosition::Midi { start, .. } = preview.position else {
                return None;
            };
            Some(start)
        })
        .expect("beat-anchored destination");
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.last_enqueued_edit.as_ref(),
            Some(Command::MoveMidiClipWithOverwrite {
                clip: requested_clip,
                track: requested_track,
                start,
            }) if *requested_clip == clip
                && *requested_track == target
                && *start == destination_start
        ), "midi release should enqueue beat-anchored MoveMidiClip; request={:?}, destination={destination_start:?}, source={source:?}, error={:?}", view.last_enqueued_edit, view.error);
    });
    visual.run_until_parked();

    // Publish the validated model command as the worker would, then verify the
    // moved clip remains pickable at its new owner after a revision change.
    let mut moved_project = Editor::new(original_project, 8).expect("editor model");
    moved_project
        .execute(Command::MoveMidiClip {
            clip,
            track: target,
            start: destination_start,
        })
        .expect("validated midi move");
    assert_eq!(
        moved_project
            .project()
            .tracks()
            .iter()
            .find(|track| track.midi_clips().iter().any(|item| item.id() == clip))
            .map(|track| track.id()),
        Some(target),
    );
    let (_, moved_preview) = hex_engine::compile::project_with_midi_preview(
        moved_project.project(),
        &Default::default(),
    )
    .expect("moved midi preview");
    editor.update(cx, |view, cx| {
        let previous = view.state.as_ref().expect("project state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(moved_project.project().clone()),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(moved_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: TIMELINE_FRAMES,
            can_undo: true,
            can_redo: false,
            revision: previous.revision + 1,
            unsaved: true,
            document_path: None,
        }));
        view.pending_requests.clear();
        view.worker_requests_pending = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let moved = visual
        .debug_bounds(selector)
        .expect("moved midi clip has a new drag surface");
    let down = point(
        moved.origin.x + moved.size.width / 2.0,
        moved.origin.y + moved.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.active_clip_drag.is_some(),
            "moved midi clip can be picked up again; pointer_down={:?}, pending={}, worker_pending={}, lanes={:?}, revision={:?}, owners={:?}, moved_bounds={moved:?}, down={down:?}",
            view.pointer_down,
            view.pending_requests.len(),
            view.worker_requests_pending,
            view.drag_lanes
                .iter()
                .map(|(id, lane)| (*id, lane.left, lane.width, lane.top, lane.height))
                .collect::<Vec<_>>(),
            view.state.as_ref().map(|state| state.revision),
            view.state.as_ref().map(|state| state.project.tracks().iter().filter(|track| !track.midi_clips().is_empty()).map(|track| (track.id(), track.midi_clips().iter().map(|clip| clip.id()).collect::<Vec<_>>())).collect::<Vec<_>>()),
        )
    });
}

#[gpui::test]
fn arrangement_scroll_rebuilds_drag_lane_eligibility_from_painted_rows(cx: &mut TestAppContext) {
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 32).expect("editor model");
    for index in 0..24 {
        project_editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track");
    }
    let first_track = project_editor.project().tracks()[0].id();
    let last_track = project_editor.project().tracks()[23].id();
    let (handle, editor) = editor_window(cx, project_editor.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let arrangement = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let pan_start = point(
        arrangement.origin.x + px(500.0),
        arrangement.origin.y + px(250.0),
    );
    visual.simulate_mouse_down(pan_start, gpui::MouseButton::Middle, Default::default());
    visual.simulate_mouse_move(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        gpui::MouseButton::Middle,
        Default::default(),
    );
    visual.simulate_mouse_up(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        gpui::MouseButton::Middle,
        Default::default(),
    );
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        let generation = view.drag_layout_generation;
        let painted = view
            .drag_lane_epochs
            .iter()
            .filter_map(|(track, epoch)| (*epoch == generation).then_some(*track))
            .collect::<Vec<_>>();
        assert!(
            painted.contains(&last_track),
            "last row should be painted after scroll"
        );
        assert!(
            !painted.contains(&first_track),
            "offscreen first row must not be eligible in the new paint generation"
        );
        let target_lane = view.drag_lanes[&last_track];
        let pointer_x = target_lane.left + target_lane.width / 2.0;
        let pointer_y = target_lane.top + target_lane.height / 2.0;
        assert!(view.drag_lane_epochs[&first_track] < generation);
        // Recreate the dangerous stale-cache collision: the old offscreen track
        // still has bounds overlapping a currently painted row.
        view.drag_lanes.insert(first_track, target_lane);
        assert_eq!(
            view.hovered_drag_target(pointer_x, pointer_y)
                .map(|(track, _)| track),
            Some(last_track),
            "stale offscreen lane geometry cannot win over the currently painted row"
        );
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
        let lane = view.drag_lanes[&source];
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
            .active_clip_drag
            .as_ref()
            .unwrap()
            .preview
            .get()
            .unwrap();
        view.tick_clip_gesture_autoscroll(now, cx);
        assert!(view.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx));
        let drag = view.active_clip_drag.as_ref().unwrap();
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
            audio_clip_waveforms: std::collections::BTreeMap::new(),
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
        view.active_clip_drag = None;
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
        let body = view.arrangement_bounds.expect("painted Arrangement body");
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
        let generation = view.drag_layout_generation;
        let now = Instant::now();
        let _ = view.tick_clip_gesture_autoscroll(now, cx);
        if view.track_scroll.offset().y == before_offset {
            assert!(view.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx));
        }
        assert!(
            view.track_scroll.offset().y < before_offset,
            "bottom edge scrolls the actual UniformList"
        );
        assert!(view.drag_layout_generation > generation);
        assert_ne!(view.arrangement_bounds_epoch, view.drag_layout_generation);
        let drag = view.active_clip_drag.as_ref().expect("active clip drag");
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
        assert!(view.last_enqueued_edit.is_none());
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
        let body = view.arrangement_bounds.expect("painted Arrangement body");
        point(
            px(body.left + body.width * 0.6),
            px(body.top + body.height - 5.0),
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let (old_track, scroll_tick) = editor.read_with(cx, |view, _| {
        let drag = view.active_clip_drag.as_ref().expect("active clip drag");
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
        let drag = view.active_clip_drag.as_ref().expect("active clip drag");
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
        let drag = view.active_clip_drag.as_ref().expect("active clip drag");
        assert!(matches!(
            drag.destination.get(),
            Some(super::drag::ClipDropDestination::ExistingTrack(track)) if track == new_track
        ));
        assert!(drag.preview.get().is_some());
    });

    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(cx, |view, _| {
        let command = view.last_enqueued_edit.clone().expect("one release edit");
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
fn check_shift_edge(cx: &mut TestAppContext, edge: hex_project::TrimEdge) {
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
        let drag = view.active_trim_drag.as_ref().expect("active edge stretch");
        assert!(drag.stretch_modifier.get());
        let preview = drag.preview.borrow();
        let preview = &preview.as_ref().unwrap().clip;
        assert!(preview.audio_edit().unwrap().domain.warp_enabled);
        assert_eq!(preview.source_length(), 96_000);
        assert!(preview.duration() < 96_000);
        assert!(view.last_enqueued_edit.is_none());
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
        let preview = view.active_trim_drag.as_ref().unwrap().preview.borrow();
        assert!(
            preview.as_ref().unwrap().clip.audio_edit().is_none(),
            "releasing Shift restores ordinary trim at same pointer"
        );
    });
    visual.simulate_modifiers_change(shift);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.active_trim_drag
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
        assert!(matches!(view.last_enqueued_edit, Some(Command::StretchAudioClipEdge { clip: id, edge: requested_edge, frame }) if id == clip && requested_edge == edge && frame == if edge == hex_project::TrimEdge::Right { stretched.start() + stretched.duration() } else { stretched.start() }));
        assert!(view.active_trim_drag.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.clip(clip).unwrap().duration(), 96_000, "UI preview has not changed canonical state");
    });
}

#[gpui::test]
fn shift_edge_stretch_escape_and_focus_loss_cancel_without_release_edit(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for cancel in [0, 1, 2] {
        let (project, _, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        let target = point(down.x - px(42.0), down.y);
        visual.simulate_mouse_move(point(down.x - px(12.0), down.y), MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| assert!(view.active_trim_drag.is_some()));
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                assert!(
                    view.focus.is_focused(window),
                    "root focus before cancellation {cancel}"
                )
            })
        });
        if cancel == 1 {
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
            });
        } else if cancel == 2 {
            visual.deactivate_window();
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.active_trim_drag.is_none(),
                "Escape/focus loss clears metadata preview: lose_focus={cancel}"
            )
        });
        // Inactive windows retain their last painted frame. Reactivate and
        // return focus before inspecting the repainted overlay and re-entry.
        visual.update(|window, cx| {
            window.activate_window();
            editor.read_with(cx, |view, _| view.focus.focus(window));
        });
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("trim-live-preview").is_none(),
            "repaint after cancel {cancel}"
        );
        visual.simulate_mouse_move(
            point(target.x - px(10.0), target.y),
            MouseButton::Left,
            shift,
        );
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.active_trim_drag.is_none(),
                "canceled payload cannot re-enter"
            )
        });
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    }
}

#[gpui::test]
fn upper_rail_fixture_same_root_survives_actual_catalog_job_and_view_drop(cx: &mut TestAppContext) {
    use std::{sync::atomic::Ordering, time::Instant};
    let (files, handle, editor) = upper_rail_owned_fixture(cx);
    let root = files.root().to_owned();
    let weak = Arc::downgrade(&files);
    let gate = files.gate_next_catalog_save();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "actual catalog worker reached save gate"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    let catalog = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&files, &view.test_environment));
        view.browser
            .test_preferences_path()
            .with_extension("samples-v1.db")
    });
    assert_eq!(catalog, root.join("config/browser.samples-v1.db"));
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(
        editor_weak.upgrade().is_none(),
        "real view retired before save continuation"
    );
    drop(files);
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual paused filesystem job retains fixture root"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none(), "no leaked fixture/view/job Arc");
    let until = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            Instant::now() < until,
            "owned background cleanup timed out: {root:?}"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists(), "completed job cannot recreate retired root");
    eprintln!(
        "upper_rail_fixture_custody actual_save=true same_root=true all_four_preferences=true view_retired=true cleaned=true"
    );
}

#[test]
fn upper_rail_fixture_pre_ack_real_import_refusal_preserves_disk_without_arc_leak() {
    use hex_session::service::Request;
    use std::time::Duration;
    let files = super::test_environment::Environment::new().unwrap();
    let root = files.root().to_owned();
    let weak = Arc::downgrade(&files);
    let observed = std::cell::Cell::new(false);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut setup = UpperRailPendingSetup {
            environment: files,
            acknowledged: false,
        };
        let missing = root.join("missing.wav");
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        upper_rail_next(&service);
        service.submit(Request::Import(missing.clone())).unwrap();
        let until = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(
                std::time::Instant::now() < until,
                "upper_rail_fixture_pre_ack_real_import_refusal_preserves_disk_without_arc_leak: hang guard expired"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(event.request_complete);
        assert!(event.error.is_some(), "actual missing-file import refused");
        observed.set(true);
        setup.acknowledge_import(&event, &missing); // Must unwind before disarming.
    }));
    assert!(caught.is_err());
    assert!(
        observed.get(),
        "actual response observed, not a timeout panic"
    );
    assert!(
        weak.upgrade().is_none(),
        "disk evidence retention does not leak Arc"
    );
    assert!(root.is_dir());
    eprintln!(
        "upper_rail_fixture_pre_ack actual_refusal=true disk_retained=true weak_expired=true root={root:?}"
    );
    // Only this artificial case proved its sole request finished, with no UI jobs.
    std::fs::remove_dir_all(root).unwrap();
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

#[gpui::test]
fn rendered_move_retains_destination_until_real_worker_ack(cx: &mut TestAppContext) {
    assert_move_handoff(cx, &[false], true);
}
#[gpui::test]
fn rendered_move_audio_horizontal_handoff(cx: &mut TestAppContext) {
    assert_move_handoff(cx, &[false], false);
}
#[gpui::test]
fn rendered_move_midi_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[true], vertical);
    }
}
#[gpui::test]
fn rendered_move_audio_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[false, false], vertical);
    }
}
#[gpui::test]
fn rendered_move_midi_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[true, true], vertical);
    }
}
#[gpui::test]
fn rendered_move_mixed_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[false, true], vertical);
    }
}

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn assert_clip_move_guides(visual: &mut VisualTestContext, expected_x: gpui::Pixels) {
    for edge in ["top", "bottom"] {
        let selector = Box::leak(format!("clip-move-guide-{edge}").into_boxed_str());
        let marker = visual.debug_bounds(selector).expect("painted drag guide");
        assert!(
            (marker.center().x - expected_x).abs() <= px(1.0),
            "{edge} guide follows the dragged anchor"
        );
    }
}

fn assert_move_handoff(cx: &mut TestAppContext, midi: &[bool], vertical: bool) {
    assert_drag_handoff(cx, midi, vertical, false);
}

#[gpui::test]
fn rendered_copy_retains_destination_until_real_worker_ack(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff(cx, midi, true, true);
    }
}

#[gpui::test]
fn rendered_move_survives_same_revision_worker_work(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff_with_pending_request(cx, midi, false, false, true);
    }
}

#[gpui::test]
fn rendered_copy_survives_same_revision_worker_work(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff_with_pending_request(cx, midi, false, true, true);
    }
}

fn assert_drag_handoff(cx: &mut TestAppContext, midi: &[bool], vertical: bool, copy: bool) {
    assert_drag_handoff_with_pending_request(cx, midi, vertical, copy, false);
}

#[gpui::test]
fn rendered_move_released_after_first_drag_motion_commits(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    for midi in [false, true] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let (track, audio) = editor.read_with(&visual, |v, _| {
            let track = &v.state.as_ref().unwrap().project.tracks()[0];
            (track.id(), track.clips()[0].id())
        });
        if midi {
            for command in [
                Command::RemoveClip(audio),
                Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(4).unwrap(),
                },
            ] {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(command)))
                });
                pump_move(&editor, &mut visual);
            }
        }
        let (clip, before) = editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            (
                if midi {
                    project.track(track).unwrap().midi_clips()[0].id()
                } else {
                    audio
                },
                project.clone(),
            )
        });
        let source = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let down = source.center();
        let end = point(down.x + px(36.0), down.y);
        assert!(
            source.contains(&end),
            "release still hits the painted source header"
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        // One coalesced motion crosses the native drag threshold. There need
        // not be a second mouse-move before release over the original header.
        let destination = visual.update(|window, app| {
            window.dispatch_input(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                app,
            );
            assert!(app.has_active_drag());
            let destination = {
                let drag = editor.read(app).active_clip_drag.as_ref().unwrap();
                let destination = drag
                    .preview
                    .get()
                    .expect("valid destination preview")
                    .position;
                assert_ne!(destination, drag.position);
                destination
            };
            // Dispatch against the same completed frame, as native events can
            // arrive between display refreshes.
            window.dispatch_input(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position: end,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                app,
            );
            destination
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.worker_requests_pending, 1,
                "first-motion release must submit the move: {:?}",
                v.error
            );
        });
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            assert_ne!(project.snapshot(), before.snapshot());
            match destination {
                super::drag::ClipPosition::Midi { start, .. } => {
                    assert_eq!(project.midi_clip(clip).unwrap().start(), start)
                }
                super::drag::ClipPosition::Audio { start, .. } => {
                    assert_eq!(project.clip(clip).unwrap().start(), start)
                }
            }
        });
        let moved = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-z");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before.snapshot()
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), moved)
        });
    }
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

#[gpui::test]
fn initial_left_third_keeps_the_move_start_on_grid(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(cx, &[midi], false, false, false, Some(1.0 / 3.0), None);
    }
}

#[gpui::test]
fn initial_two_thirds_boundary_snaps_the_move_end(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(cx, &[midi], false, false, false, Some(2.0 / 3.0), None);
    }
}

#[gpui::test]
fn initial_right_third_retains_one_snap_delta_for_mixed_group_copy(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(cx, &[false, true], true, true, false, Some(5.0 / 6.0), None);
}

#[gpui::test]
fn cropped_clip_move_chooses_from_the_full_audio_and_midi_span(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for midi in [false, true] {
        let (project, source, target, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let (start, end) = if midi {
            let clip = project.midi_clip(clip).unwrap();
            (
                project.tempo_map().beat_to_frame(clip.start()).unwrap(),
                project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
                    .unwrap(),
            )
        } else {
            let clip = project.clip(clip).unwrap();
            (clip.start(), clip.start() + clip.duration())
        };
        let threshold = start + (end - start) * 2 / 3;
        let before = project.snapshot();
        let preview = if midi {
            Arc::new(
                hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                    .unwrap()
                    .1,
            )
        } else {
            Arc::default()
        };
        let (handle, editor) = editor_window(cx, project, preview);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(super::viewport::Viewport {
                start: threshold,
                length: TIMELINE_FRAMES - threshold,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let surface = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let down = point(
            surface.left() + surface.size.width / 4.0,
            surface.center().y,
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let lane = editor.read_with(&visual, |view, _| view.drag_lanes[&target]);
        visual.simulate_mouse_move(
            point(down.x + px(80.0), px(lane.top + lane.height / 2.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let drag = view.active_clip_drag.as_ref().unwrap();
            assert_eq!(drag.source_track, source);
            assert_eq!(
                drag.snap_edge.get(),
                Some(super::drag::ClipSnapEdge::Trailing),
                "the left quarter of this visible fragment is already in the real trailing third"
            );
        });
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |view, _| {
            assert!(view.active_clip_drag.is_none());
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
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
#[gpui::test]
fn audio_origin_capture_uses_window_distance_at_close_zoom(cx: &mut TestAppContext) {
    assert_audio_origin_window(cx, 192_000);
}
#[gpui::test]
fn audio_origin_capture_uses_window_distance_at_wide_zoom(cx: &mut TestAppContext) {
    assert_audio_origin_window(cx, 384_000);
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
fn origin_capture_still_applies_with_the_grid_off(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(
            cx,
            &[midi],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid: super::musical_time::SnapGrid::Off,
                ..Default::default()
            }),
        );
    }
}
#[gpui::test]
fn command_bypasses_audio_origin_and_both_grid_settings(cx: &mut TestAppContext) {
    for grid in [
        super::musical_time::SnapGrid::Quarter,
        super::musical_time::SnapGrid::Off,
    ] {
        assert_drag_handoff_at_grab(
            cx,
            &[false],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid,
                bypass: true,
                ..Default::default()
            }),
        );
    }
}
#[gpui::test]
fn command_bypasses_midi_origin_and_both_grid_settings(cx: &mut TestAppContext) {
    for grid in [
        super::musical_time::SnapGrid::Quarter,
        super::musical_time::SnapGrid::Off,
    ] {
        assert_drag_handoff_at_grab(
            cx,
            &[true],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid,
                bypass: true,
                ..Default::default()
            }),
        );
    }
}
#[gpui::test]
fn outside_origin_capture_returns_to_the_edge_grid(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(
            cx,
            &[midi],
            true,
            false,
            false,
            Some(5.0 / 6.0),
            Some(OriginProbe {
                dx: 20.0,
                ..Default::default()
            }),
        );
    }
}
#[gpui::test]
fn origin_capture_keeps_one_delta_for_mixed_group_move(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(
        cx,
        &[false, true],
        true,
        false,
        false,
        Some(5.0 / 6.0),
        Some(OriginProbe::default()),
    );
}
#[gpui::test]
fn origin_capture_keeps_mixed_copy_paint_through_a_queued_ack(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(
        cx,
        &[false, true],
        true,
        true,
        true,
        Some(5.0 / 6.0),
        Some(OriginProbe::default()),
    );
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
                view.pointer_down.unwrap().snap_edge,
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
    let lane = editor.read_with(&visual, |v, _| v.drag_lanes[&tracks[usize::from(vertical)]]);
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
            let drag = view.active_clip_drag.as_ref().unwrap();
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
        assert!(v.active_clip_drag.is_none());
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
            let cache = v.waveform_geometry_cache.borrow();
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
    let command = editor.read_with(&visual, |v, _| v.last_enqueued_edit.clone().unwrap());
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
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            expected.project().snapshot(),
            "exact canonical content, IDs, offsets and extent"
        );
    });
    assert!(visual.debug_bounds("clip-move-guide-top").is_none());
    assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
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
fn rendered_move_cancel_and_stale_controls_restore_source(cx: &mut TestAppContext) {
    for control in 0..4 {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let clip = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
        });
        let source = selector(format!("clip-visual-{}", clip.value()));
        let old = visual.debug_bounds(source).unwrap();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let (_, end) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 120.0);
        assert!(visual.debug_bounds("clip-move-guide-top").is_some());
        match control {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| {
                assert!(cx.stop_active_drag(window));
            }),
            _ => {
                editor.update(&mut visual, |v, _| {
                    assert!(
                        v.enqueue(hex_session::service::Request::Edit(Command::RenameClip {
                            clip,
                            name: "independent edit".into()
                        }))
                    )
                });
                if control == 3 {
                    pump_move(&editor, &mut visual);
                }
            }
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        if control == 2 {
            editor.update(&mut visual, |v, cx| {
                assert_eq!(v.worker_requests_pending, 2);
                assert!(v.pending_clip_move.is_some());
                let renamed = upper_rail_next(&v.service);
                assert!(renamed.error.is_none());
                v.receive(renamed);
                assert!(v.pending_clip_move.is_some());
                let refused = upper_rail_terminal(&v.service);
                assert!(refused.error.as_ref().unwrap().contains("project changed"));
                v.receive(refused);
                assert!(v.pending_clip_move.is_none());
                cx.notify();
            });
        }
        editor.update(&mut visual, |v, _| {
            v.error = None;
        });
        pump_move(&editor, &mut visual);
        assert!(visual.debug_bounds("clip-move-guide-top").is_none());
        assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_clip_move.is_none() && v.active_clip_drag.is_none());
            if control < 2 {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            } else {
                let p = &v.state.as_ref().unwrap().project;
                assert_eq!(p.clip(clip).unwrap().start(), 0);
                assert_eq!(p.clip_name(clip), Some("independent edit"));
                if control == 3 {
                    assert!(matches!(
                        v.last_enqueued_edit,
                        Some(Command::RenameClip { .. })
                    ));
                } else {
                    assert!(matches!(
                        v.last_enqueued_edit,
                        Some(Command::MoveClipWithOverwrite { .. })
                    ));
                }
            }
        });
        assert!(visual.debug_bounds("clip-drop-preview").is_none());
        assert_eq!(visual.debug_bounds(source), Some(old));
        assert!(visual.rendered_background(source).is_some());
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
    let retained = editor.read_with(&visual, |v, _| v.active_clip_drag.as_ref().unwrap().clone());
    visual.update(|window, cx| {
        assert!(cx.stop_active_drag(window));
    });
    editor.update(&mut visual, |v, cx| {
        // Negative control: retain an actual pointer projection, then submit an
        // explicitly invalid extent to the REAL worker through the same owner.
        // Production release never fabricates this invalid command.
        let drag = v.active_clip_drag.take().unwrap();
        let invalid = Command::MoveClipWithOverwrite {
            clip,
            track,
            start: u64::MAX,
        };
        v.pending_clip_move = Some(super::pending_clip_move::Pending::new(
            drag,
            invalid.clone(),
            Arc::clone(&v.state.as_ref().unwrap().project),
        ));
        assert!(v.enqueue(Request::DeviceGesture(
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

#[gpui::test]
fn alias_drag_preview_and_release_follow_live_modifiers_for_audio_and_midi(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        let (project, _, target, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let midi_preview = if midi {
            let (_, preview) =
                hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                    .unwrap();
            Arc::new(preview)
        } else {
            Arc::default()
        };
        let (handle, editor) = editor_window(cx, project, midi_preview);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_none());
        let alias = Modifiers {
            alt: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_modifiers_change(alias);
        visual.run_until_parked();
        let badge = visual.debug_bounds("clip-alias-drag-indicator").unwrap();
        let preview = visual.debug_bounds("clip-drop-preview").unwrap();
        assert!(
            badge.right() <= preview.right() - px(5.0),
            "alias badge clears the clip corner cut"
        );
        visual.simulate_modifiers_change(Modifiers {
            alt: true,
            ..Modifiers::default()
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_none());
        visual.simulate_modifiers_change(alias);
        visual.run_until_parked();
        visual.simulate_mouse_up(release, MouseButton::Left, alias);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.last_enqueued_edit.as_ref(), Some(Command::DuplicateAliasClipsAt { clips, anchor, track, .. }) if clips == &[clip] && *anchor == clip && *track == target));
            assert!(view.pending_clip_move.is_some(), "preview remains in custody until the worker reply");
        });
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_some());
    }
}

#[gpui::test]
fn drag_guides_preserve_locked_play_start_through_cancel_and_ack(cx: &mut TestAppContext) {
    for cancel in [true, false] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let clip = editor.update(&mut visual, |view, cx| {
            let track = &view.state.as_ref().unwrap().project.tracks()[0];
            let clip = track.clips()[0].id();
            let cursor = super::edit_cursor::EditCursor {
                track: Some(track.id()),
                frame: 24_000,
                generation: view.project_generation,
                pulse_color: crate::theme::time_position_marker_color(),
                pulse_started: std::time::Instant::now(),
            };
            view.edit_cursor = Some(cursor);
            view.locked_play_start = Some(cursor);
            cx.notify();
            clip
        });
        visual.run_until_parked();
        let (_, end) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 120.0);
        assert!(visual.debug_bounds("clip-move-guide-top").is_some());
        assert!(visual.debug_bounds("locked-play-start-top").is_some());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.playback_start_frame(), 24_000)
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        if !cancel {
            visual.run_until_parked();
            assert!(visual.debug_bounds("clip-move-guide-top").is_some());
            pump_move(&editor, &mut visual);
        }
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-move-guide-top").is_none());
        assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
        assert!(visual.debug_bounds("locked-play-start-top").is_some());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.valid_locked_play_start().unwrap().frame, 24_000);
            assert_eq!(view.playback_start_frame(), 24_000);
            assert!(view.last_transport_command.is_none());
        });
    }
}

#[gpui::test]
fn parameter_observations_preserve_audio_and_midi_resize_custody(cx: &mut TestAppContext) {
    use hex_session::service::{PluginParameterQuery, Request};
    for midi in [false, true] {
        let (project, _, _, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let mut model = Editor::new(project, 8).unwrap();
        let Created::Device(device) = model
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let preview = if midi {
            hex_engine::compile::project_with_midi_preview(model.project(), &Default::default())
                .unwrap()
                .1
        } else {
            Default::default()
        };
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let prefix = if midi {
            "midi-clip-trim-right"
        } else {
            "clip-trim-right"
        };
        let selector = Box::leak(format!("{prefix}-{}", clip.value()).into_boxed_str());
        let down = trim_grab_point(visual.debug_bounds(selector).unwrap());
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            let query = PluginParameterQuery {
                context: hex_engine::AudioSourceContext {
                    project_generation: view.project_generation,
                    editor_revision: view.state.as_ref().unwrap().revision,
                },
                device,
                output_identity: 1,
                request_id: 1,
                page_first: Some(0),
            };
            assert!(view.enqueue(Request::QueryPluginParameters(query)));
        });
        visual.run_until_parked();
        let target = point(down.x - px(28.0), down.y);
        visual.simulate_mouse_move(
            target,
            MouseButton::Left,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                if midi {
                    view.active_midi_trim_drag
                        .as_ref()
                        .is_some_and(|drag| drag.preview.borrow().is_some())
                } else {
                    view.active_trim_drag
                        .as_ref()
                        .is_some_and(|drag| drag.preview.borrow().is_some())
                },
                "a completed repaint retains the held resize preview"
            );
            assert_eq!(
                view.worker_requests_pending, 0,
                "a parameter read cannot add a pending-edit badge"
            );
        });
        visual.simulate_mouse_up(
            target,
            MouseButton::Left,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            assert!(
                view.last_enqueued_edit.is_some(),
                "resize release commits the preview"
            );
            assert!(view.pending_trim.is_some());
            let pending = view.worker_requests_pending;
            let state = Arc::clone(view.state.as_ref().unwrap());
            let mut observation = super::arrangement_main_track_ui_test::completed_event(&state);
            observation.state = state;
            observation.plugin_parameter_observation = true;
            view.receive(observation);
            assert_eq!(
                view.worker_requests_pending, pending,
                "read completion cannot acknowledge the resize edit"
            );
            assert!(
                view.pending_trim.is_some(),
                "read completion retains the pending resize display"
            );
            let state = Arc::clone(view.state.as_ref().unwrap());
            let mut refused_edit = super::arrangement_main_track_ui_test::completed_event(&state);
            refused_edit.state = state;
            refused_edit.error = Some("fixture edit refused".into());
            view.receive(refused_edit);
            assert_eq!(view.worker_requests_pending, 0);
            assert!(
                view.pending_trim.is_none(),
                "a real edit terminal still retires resize custody"
            );
        });
        visual.update(|window, _| window.remove_window());
        visual.run_until_parked();
    }
}
