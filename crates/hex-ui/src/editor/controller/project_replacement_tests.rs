//! GPUI production-event regressions for project replacement and publication policy.
use super::*;
use gpui::TestAppContext;
use hex_engine::{PreparedRevision, SineTable};
use hex_project::{Command, Editor, Project};
use hex_session::service::{Request, Service, ViewState};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn event(project: Arc<Project>, revision: u64, replaced: bool) -> Event {
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("test revision"),
    );
    Event {
        state: Arc::new(ViewState {
            project,
            waveforms: Default::default(),
            midi_preview: Arc::new(Default::default()),
            timeline_frames: 384_000,
            can_undo: false,
            can_redo: false,
            revision,
            unsaved: !replaced,
            document_path: None,
        }),
        playback: Some(Playback::try_from_prepared_with_session(prepared).expect("playback")),
        status: None,
        error: None,
        request_complete: true,
        save_complete: false,
        open_complete: replaced,
        project_replaced: replaced,
        created: None,
        duplicate_complete: false,
        restore_complete: None,
        device_preview: None,
        gesture_ack: None,
        gesture_protocol_ack: false,
        gesture_request: false,
        active_device_gesture: None,
    }
}

fn open_response(mut event: Event, path: Option<std::path::PathBuf>, failed: bool) -> Event {
    let previous = &event.state;
    event.state = Arc::new(ViewState {
        project: Arc::clone(&previous.project),
        waveforms: Default::default(),
        midi_preview: Arc::new(Default::default()),
        timeline_frames: previous.timeline_frames,
        can_undo: previous.can_undo,
        can_redo: previous.can_redo,
        revision: previous.revision,
        unsaved: previous.unsaved,
        document_path: path,
    });
    event.open_complete = true;
    event.project_replaced = !failed;
    event.error = failed.then(|| "could not read project".to_owned());
    event
}

fn eq8_project() -> (
    Arc<Project>,
    hex_project::TrackId,
    hex_project::DeviceId,
    hex_project::TrackId,
) {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "eq8 source".into(),
        })
        .expect("track")
    {
        hex_project::Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
    )
    .expect("eq8 state");
    let device = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .expect("insert eq8")
    {
        hex_project::Created::Device(device) => device,
        other => panic!("unexpected device result: {other:?}"),
    };
    let other_track = match editor
        .execute(Command::AddTrack {
            name: "other".into(),
        })
        .expect("other track")
    {
        hex_project::Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    (
        Arc::new(editor.project().clone()),
        track,
        device,
        other_track,
    )
}

#[gpui::test]
fn spectrum_target_is_retired_when_detail_context_or_project_changes(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let (project, track, device, other_track) = eq8_project();
    let target = hex_audio::spectrum_target::SpectrumTarget {
        token: hex_audio::spectrum_worker::SpectrumTargetId(1),
        identity: hex_engine::PostNodeIdentity::new(
            hex_engine::ProcessorChainOwner::Track(track.value()),
            device.value(),
            hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
        ),
        project_generation: 1,
        activation_floor: 1,
        sample_rate_hz: 48_000,
    };

    window
        .update(cx, |view, _, cx| {
            view.receive(event(Arc::clone(&project), 1, true));
            view.project_generation = 1;
            view.selected_track = Some(track);
            view.clip_view_open = true;
            view.device_panel_active = true;
            assert!(view.spectrum_target_is_visible_and_current(target));

            view.clip_view_open = false;
            assert!(!view.spectrum_target_is_visible_and_current(target));
            view.clip_view_open = true;
            view.device_panel_active = false;
            assert!(!view.spectrum_target_is_visible_and_current(target));
            view.device_panel_active = true;

            view.selected_track = Some(other_track);
            assert!(!view.spectrum_target_is_visible_and_current(target));
            view.selected_track = Some(track);

            let mut without_eq8 = Editor::new((*project).clone(), 8).expect("editor");
            without_eq8
                .execute(Command::RemoveDevice(device))
                .expect("remove eq8");
            view.state = Some(event(Arc::new(without_eq8.project().clone()), 2, false).state);
            assert!(!view.spectrum_target_is_visible_and_current(target));
            view.state = Some(event(Arc::clone(&project), 3, false).state);

            let mut replaced_eq8 = Editor::new((*project).clone(), 8).expect("editor");
            replaced_eq8
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: hex_engine::default_native_device_state_for_class(
                        hex_engine::NativeDeviceClass::Effect,
                        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                    )
                    .expect("utility state"),
                })
                .expect("replace eq8 type");
            view.state = Some(event(Arc::new(replaced_eq8.project().clone()), 4, false).state);
            assert!(!view.spectrum_target_is_visible_and_current(target));
            view.state = Some(event(Arc::clone(&project), 5, false).state);

            // Reopening the same project preserves numeric IDs but advances the
            // project generation, so an old callback target cannot reattach.
            view.receive(event(Arc::clone(&project), 2, true));
            assert_ne!(view.project_generation, target.project_generation);
            assert!(!view.spectrum_target_is_visible_and_current(target));

            view.device_panel.spectrum_result =
                Some(hex_audio::spectrum_worker::PublishedSpectrum {
                    stamp: hex_audio::spectrum_worker::SpectrumStamp {
                        target: target.token,
                        generation: target.project_generation,
                        sample_rate_hz: target.sample_rate_hz,
                        applied_revision: target.activation_floor,
                    },
                    frame: hex_audio::spectrum::SpectrumFrame {
                        sample_rate_hz: target.sample_rate_hz,
                        bins_dbfs: [-120.0; hex_audio::spectrum::SPECTRUM_BANDS],
                    },
                    captured_at: std::time::Instant::now(),
                    processed_at: std::time::Instant::now(),
                });
            // A backend replacement starts without a selected target. Tick's
            // retirement path clears cached data even before a new frame arrives.
            assert!(view.retire_stale_spectrum_target());
            assert!(view.device_panel.spectrum_result.is_none());
            cx.notify();
        })
        .expect("spectrum target context");
}

#[gpui::test]
fn only_successful_open_response_promotes_the_worker_document_path(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let project = Arc::new(Project::new(48_000).expect("project"));
    let actual_path = std::env::temp_dir().join("recent-open-document.hex");
    window
        .update(cx, |view, _, cx| {
            let success = open_response(
                event(Arc::clone(&project), 1, true),
                Some(actual_path.clone()),
                false,
            );
            if let Some(path) = view.receive(success) {
                view.record_opened_project(path, cx);
            }
            assert_eq!(
                view.recent_projects.projects.paths(),
                std::slice::from_ref(&actual_path)
            );

            let failure_path = std::env::temp_dir().join("failed-open-request.hex");
            let failure = open_response(event(project, 2, false), Some(failure_path), true);
            assert!(view.receive(failure).is_none());
            assert_eq!(view.recent_projects.projects.paths(), &[actual_path]);
        })
        .expect("recent open response");
}

#[gpui::test]
fn worker_snapshots_and_undo_keep_the_last_user_detail_focus(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let (project, track, _, clip) = super::super::clip_drag_ui_test::midi_project();
    let project = Arc::new(project);
    let mut without_clip = Editor::new((*project).clone(), 8).expect("project editor");
    without_clip
        .execute(Command::RemoveMidiClip(clip))
        .expect("remove clip");
    let without_clip = Arc::new(without_clip.project().clone());

    window
        .update(cx, |view, _, _| {
            view.receive(event(Arc::clone(&project), 1, true));
            view.selected_track = Some(track);
            view.selected_clip = Some(clip);
            view.clip_view_open = true;
            view.device_panel_active = false;

            view.receive(event(Arc::clone(&without_clip), 2, false));
            assert_eq!(view.selected_clip, None, "snapshot removes stale selection");
            assert!(
                !view.device_panel_active,
                "snapshot does not replace clip focus"
            );
            assert!(view.clip_view_open, "selection removal leaves pane open");

            view.device_panel_active = true;
            view.receive(event(project, 3, false));
            assert!(
                view.device_panel_active,
                "undo snapshot does not replace track focus"
            );
            assert!(view.clip_view_open, "undo leaves pane open");
        })
        .expect("receive snapshot and undo events");
}

fn receive_worker_event(view: &mut EditorView) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match view.service.try_event() {
            Ok(event) => {
                view.receive(event);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("worker event unavailable: {error}"),
        }
    }
}

#[gpui::test]
fn receive_preserves_fresh_installation_through_edits_and_failed_open(cx: &mut TestAppContext) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 8, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    let mut project = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let hex_project::Created::Track(track) = project
        .execute(Command::AddTrack {
            name: "new file".into(),
        })
        .expect("track")
    else {
        panic!("track expected")
    };
    let original = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("first edit");
    let first_edit = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.25 })
        .expect("second edit");
    let second_edit = Arc::new(project.project().clone());
    project
        .execute(Command::Gain { track, gain: 0.75 })
        .expect("later edit");
    let later_edit = Arc::new(project.project().clone());
    window
        .update(cx, |view, _, _| {
            view.receive(event(Arc::clone(&original), 0, true));
            let generation = view.project_generation;
            assert!(
                view.pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );
            // Multiple committed edits can arrive before audio publication has
            // space. They replace the candidate, not its project identity.
            view.receive(event(first_edit, 1, false));
            view.receive(event(Arc::clone(&second_edit), 2, false));
            let pending = view.pending_playback.as_ref().unwrap();
            assert!(pending.install_fresh_runtime);
            assert_eq!(pending.project_revision, 2);
            assert_eq!(pending.project_generation, generation);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .gain(),
                0.25
            );

            let mut refused_open = event(second_edit, 2, false);
            refused_open.playback = None;
            refused_open.open_complete = true;
            refused_open.error = Some("missing project".into());
            view.receive(refused_open);
            assert_eq!(view.project_generation, generation);
            assert!(
                view.pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );

            // The audio queue owns an admitted candidate. Later same-project
            // edits inherit, while another successful Open installs fresh again.
            let admitted = view.pending_playback.take().unwrap();
            let (mut control, _callback) = hex_audio::prepared::channel(48_000).unwrap();
            assert!(
                control
                    .publish_with_policy(admitted.playback, PublicationPolicy::InstallFreshRuntime)
                    .is_ok()
            );
            view.receive(event(later_edit, 3, false));
            assert!(
                !view
                    .pending_playback
                    .as_ref()
                    .unwrap()
                    .install_fresh_runtime
            );
            view.receive(event(original, 0, true));
            assert_ne!(view.project_generation, generation);
            let pending = view.pending_playback.as_ref().unwrap();
            assert!(pending.install_fresh_runtime);
            assert_eq!(pending.project_revision, 0);
            assert_eq!(pending.project_generation, view.project_generation);
        })
        .expect("test window");
}

#[gpui::test]
fn arrangement_growth_keeps_auto_fit_span_and_explicit_fit_tracks_current_horizon(
    cx: &mut TestAppContext,
) {
    let window = cx.add_window(|window, cx| {
        let service =
            Service::new(48_000, 16, super::super::MAX_RESIDENT_FRAMES).expect("test service");
        EditorView::new(window, cx, service, Err("offline test".into()))
    });
    window
        .update(cx, |view, _, _| {
            receive_worker_event(view);
            assert_eq!(view.viewport, None, "the initial project remains auto-fit");
            let initial_horizon = view.state.as_ref().expect("initial state").timeline_frames;

            view.service
                .submit(Request::Edit(Command::AddTrack {
                    name: "source".into(),
                }))
                .expect("add track");
            receive_worker_event(view);
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: hex_project::BeatTime::ZERO,
                    length: hex_project::BeatTime::from_quarters(4).expect("length"),
                }))
                .expect("place source clip");
            receive_worker_event(view);
            assert_eq!(
                view.state.as_ref().unwrap().timeline_frames,
                initial_horizon
            );
            assert_eq!(view.viewport, None, "same-horizon edits preserve auto-fit");
            let clip = view.state.as_ref().unwrap().project.tracks()[0].midi_clips()[0].id();

            view.service
                .submit(Request::Edit(Command::MoveMidiClip {
                    clip,
                    track,
                    start: hex_project::BeatTime::from_quarters(32).expect("destination"),
                }))
                .expect("move beyond current extent");
            receive_worker_event(view);
            let grown_horizon = view.state.as_ref().unwrap().timeline_frames;
            assert!(grown_horizon > initial_horizon);
            assert_eq!(view.viewport, Some(Viewport::fit(initial_horizon)));
            assert_eq!(
                view.visible_viewport(grown_horizon),
                Viewport::fit(initial_horizon)
            );

            view.service
                .submit(Request::Edit(Command::Gain { track, gain: 0.5 }))
                .expect("same-horizon edit");
            receive_worker_event(view);
            assert_eq!(view.viewport, Some(Viewport::fit(initial_horizon)));

            view.fit_timeline();
            assert_eq!(view.viewport, None);
            assert_eq!(
                view.visible_viewport(grown_horizon),
                Viewport::fit(grown_horizon)
            );
            view.zoom_timeline(true);
            let explicit_zoom = view.viewport.expect("explicit zoom");
            view.service
                .submit(Request::Edit(Command::MoveMidiClip {
                    clip,
                    track,
                    start: hex_project::BeatTime::from_quarters(64).expect("far destination"),
                }))
                .expect("grow after explicit zoom");
            receive_worker_event(view);
            assert!(view.state.as_ref().unwrap().timeline_frames > grown_horizon);
            assert_eq!(view.viewport, Some(explicit_zoom));

            view.pan_timeline(true);
            let explicit_pan = view.viewport.expect("explicit pan");
            view.service
                .submit(Request::Edit(Command::MoveMidiClip {
                    clip,
                    track,
                    start: hex_project::BeatTime::from_quarters(96).expect("farther destination"),
                }))
                .expect("grow after explicit pan");
            receive_worker_event(view);
            assert_eq!(view.viewport, Some(explicit_pan));

            let replacement = event(
                Arc::new(Project::new(48_000).expect("replacement project")),
                view.state.as_ref().unwrap().revision + 1,
                true,
            );
            view.receive(replacement);
            assert_eq!(
                view.viewport, None,
                "project replacement returns to auto-fit"
            );
            let replacement_horizon = view.state.as_ref().unwrap().timeline_frames;
            assert_eq!(
                view.visible_viewport(replacement_horizon),
                Viewport::fit(replacement_horizon)
            );
        })
        .expect("run viewport growth assertions");
}
