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
        .debug_bounds("clip-drag-ghost")
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
    (preview, point(target_x, lane_y))
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
                Some(Command::MoveClip {
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
            Some(Command::MoveMidiClip {
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
