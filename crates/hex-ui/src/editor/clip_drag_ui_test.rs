use super::{EditorView, install_close_guard};
use gpui::{AppContext, Empty, TestAppContext, VisualTestContext, point, px};
use gpui::{Modifiers, MouseButton};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_session::service::{Service, ViewState};
use std::sync::Arc;

const TIMELINE_FRAMES: u64 = 384_000;

fn audio_project() -> (
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

fn midi_project() -> (
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

fn editor_window(
    cx: &mut TestAppContext,
    project: Project,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, super::MAX_RESIDENT_FRAMES)
                .expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            install_close_guard(window, cx, editor.downgrade());
            editor.update(cx, |view, cx| {
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    midi_preview,
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
            drag.origin_pointer_x.get().is_some(),
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

fn drag_source_to_inserted_track(
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    project: &Project,
    clip: hex_project::ClipId,
    source_track: hex_project::TrackId,
    anchor: Option<hex_project::TrackId>,
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
        body.size.width - px(super::tracks::TRACK_HEADER_WIDTH)
    );

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
                start: pointer_frame.saturating_sub(grab_offset),
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
                start: BeatTime::from_raw(pointer_beat.raw().saturating_sub(grab_offset)),
                length,
            }
        }
    };
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
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
        assert_eq!(drag.lane_hovered.get(), anchor.is_some());
        assert!(drag.preview.get().is_none());
    });
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
            .debug_bounds("clip-drag-floating-preview")
            .expect("bottom insertion keeps the cursor ghost");
        assert!(ghost.size.width > px(100.0));
        let last_track = project.tracks().last().expect("last project track").id();
        let last_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", last_track.value()).into_boxed_str(),
            ))
            .expect("painted final track lane");
        let highlight = visual
            .debug_bounds("append-track-drop-highlight")
            .expect("only the blank append region is highlighted");
        assert_eq!(highlight.origin.x, body.origin.x);
        assert_eq!(highlight.size.width, body.size.width);
        assert!(
            (highlight.origin.y - (last_lane.origin.y + last_lane.size.height)).abs() <= px(1.0)
        );
        assert!(
            (highlight.origin.y + highlight.size.height - (body.origin.y + body.size.height)).abs()
                <= px(1.0)
        );
        let body_after = visual
            .debug_bounds("arrangement-content")
            .expect("Arrangement body remains mounted");
        assert_eq!(body_after.origin, body.origin);
        assert_eq!(body_after.size, body.size);
    }
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| match expected {
        super::drag::ClipPosition::Audio { start, .. } => assert!(
            matches!(
                view.last_enqueued_edit.as_ref(),
                Some(Command::MoveClipToNewTrack {
                    clip: moved_clip,
                    before_track,
                    start: moved_start,
                    ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start
            ),
            "audio new-track release request={:?}, destination={expected:?}, error={:?}",
            view.last_enqueued_edit,
            view.error
        ),
        super::drag::ClipPosition::Midi { start, .. } => assert!(
            matches!(
                view.last_enqueued_edit.as_ref(),
                Some(Command::MoveMidiClipToNewTrack {
                    clip: moved_clip,
                    before_track,
                    start: moved_start,
                    ..
                }) if *moved_clip == clip && *before_track == anchor && *moved_start == start
            ),
            "midi new-track release request={:?}, destination={expected:?}, error={:?}",
            view.last_enqueued_edit,
            view.error
        ),
    });
    target
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
    );
}

#[gpui::test]
fn rendered_audio_clip_drop_below_last_track_inserts_without_workspace_jump(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(cx, &mut visual, &editor, &project, clip, source, None);
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
    drag_source_to_inserted_track(cx, &mut visual, &editor, &project, clip, source, None);
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
            project: Arc::new(moved_project.project().clone()),
            waveforms: Default::default(),
            midi_preview: Arc::new(moved_preview),
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

    visual.simulate_event(gpui::ScrollWheelEvent {
        position: point(px(500.0), px(250.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-1100.0))),
        ..Default::default()
    });
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
