use super::*;
use crate::{EditorAudioEvent, EditorPublicationState, editor_backend::EditorAudio};
use hex_audio::prepared::{AdmittedPublishError, PublicationPolicy};
use hex_engine::{
    PreparedPlaybackObservation, native_device_resources::DeviceResourceAdmission,
    playback::Playback,
};
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{
    CanonicalAudioSourceSnapshot, Request, ResidentPublicationAcceptance,
    ResidentPublicationMetadata, ResidentPublicationPolicy, ResidentPublicationRefusal, Service,
    WorkerAudioSink,
};
use std::sync::{
    Arc, Mutex,
    atomic::{
        AtomicBool,
        Ordering::{AcqRel, Release},
    },
    mpsc::{SyncSender, TryRecvError, sync_channel},
};
use std::time::Instant;

#[gpui::test]
fn stop_during_fresh_vst3_recovery_cancels_play_without_revoking_sources(
    cx: &mut gpui::TestAppContext,
) {
    use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};

    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    client.set_audio_diagnostics(AudioDiagnosticsFlags::TRANSPORT);
    let mut project =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    project
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::from_static("hex.vst3"),
                1,
                vec![0],
            )
            .unwrap(),
        })
        .unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        project.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "fresh recovery fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        let generation = view.recovery.begin(120).unwrap();
        assert!(view.recovery.opened(generation));
        view.pending_recovery_generation = Some(generation);
        view.defer_play_until_audio_recovers();
        view.stop();
        assert!(view.deferred_play.is_none());
        assert_eq!(view.recovery.position(), Some(0));
        assert_eq!(view.recovery.generation(), Some(generation));
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let control = &mut view.audio.as_mut().unwrap().control;
        assert!(
            !std::iter::from_fn(|| control.take_audio_diagnostic())
                .any(|event| matches!(event.kind, AudioDiagnosticKind::PriorityStop { .. })),
            "stop must leave the fresh recovery output intact"
        );
    });
    drop(backend);
    output.publication.close();
}

#[gpui::test]
fn stop_renews_current_resident_vst3_playback_before_an_edit(cx: &mut gpui::TestAppContext) {
    use super::super::audio_admission::StoppedPlaybackRestore;
    use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};

    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    client.set_audio_diagnostics(AudioDiagnosticsFlags::TRANSPORT);
    let stop_count = |view: &mut EditorView| {
        let control = &mut view.audio.as_mut().unwrap().control;
        std::iter::from_fn(|| control.take_audio_diagnostic())
            .filter(|event| matches!(event.kind, AudioDiagnosticKind::PriorityStop { .. }))
            .count()
    };
    let mut project =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = project
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::from_static("hex.vst3"),
                1,
                vec![0],
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("vst3 effect");
    };
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        project.project().clone(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        let identity = client.output_identity().unwrap();
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "stopped vst3 fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.applied_project_revision = Some(revision);
        view.audible_playback = Some(super::super::PlaybackCandidate {
            worker_requires_preroll: None,
            observation: None,
            audio_revision: 0,
            project_generation: view.project_generation,
            state: view.state.as_ref().unwrap().clone(),
            prepared_revision: None,
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        view.stop_from_escape();
        let restore = Some(StoppedPlaybackRestore::Preparing);
        assert_eq!(view.stopped_playback_restore, restore);
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert_eq!(stop_count(view), 1, "first stop reaches the callback");

        view.stop_from_escape();
        assert!(view.play_start_at_beginning);
        assert_eq!(view.stopped_playback_restore, restore);
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert_eq!(
            stop_count(view),
            0,
            "repeat stop leaves the pending restore alone"
        );

        view.degraded_plugins = crate::editor_backend::DegradedDevices::new(vec![device.value()]);
        view.queue_audio_command(PlaybackCommand::PlayFrom(0));
        assert!(view.deferred_play.is_some());
        view.stop();
        assert!(view.deferred_play.is_none());
        assert_eq!(view.stopped_playback_restore, restore);
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert_eq!(
            stop_count(view),
            0,
            "cancel deferred play without revoking the restore"
        );

        view.queue_audio_command(PlaybackCommand::PlayFrom(0));
        assert!(view.deferred_play.is_some());
        assert_eq!(
            view.audio.as_ref().unwrap().control.output_identity(),
            Some(identity)
        );
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))));
        assert!(matches!(
            view.pending_requests.back(),
            Some(Request::Edit(Command::RemoveDevice(id))) if *id == device
        ));
    });
    drop(backend);
    output.publication.close();
}

#[gpui::test]
fn degraded_native_effect_waits_for_recovery_and_uses_the_original_track(
    cx: &mut gpui::TestAppContext,
) {
    graph_edit_recovery(cx, EditFixture::Effect, EditRecovery::Ready);
}

#[gpui::test]
fn degraded_native_effect_refuses_changed_project_and_failed_recovery(
    cx: &mut gpui::TestAppContext,
) {
    graph_edit_recovery(
        &mut cx.new_app(),
        EditFixture::Effect,
        EditRecovery::ProjectChanged,
    );
    graph_edit_recovery(&mut cx.new_app(), EditFixture::Effect, EditRecovery::Failed);
}

#[gpui::test]
fn degraded_sample_drop_waits_for_recovery_and_keeps_the_captured_destination(
    cx: &mut gpui::TestAppContext,
) {
    graph_edit_recovery(
        &mut cx.new_app(),
        EditFixture::ExistingTrackSample,
        EditRecovery::Ready,
    );
    graph_edit_recovery(cx, EditFixture::NewTrackSample, EditRecovery::Ready);
}

#[gpui::test]
fn degraded_sample_drop_releases_the_preview_on_failed_or_stale_recovery(
    cx: &mut gpui::TestAppContext,
) {
    graph_edit_recovery(
        &mut cx.new_app(),
        EditFixture::NewTrackSample,
        EditRecovery::ProjectChanged,
    );
    graph_edit_recovery(cx, EditFixture::NewTrackSample, EditRecovery::Failed);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditFixture {
    Effect,
    ExistingTrackSample,
    NewTrackSample,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditRecovery {
    Ready,
    ProjectChanged,
    Failed,
}

fn graph_edit_recovery(cx: &mut gpui::TestAppContext, edit: EditFixture, outcome: EditRecovery) {
    use crate::editor::drag::BrowserDropDestination;
    use hex_engine::{DeviceTypeId, NativeDeviceClass};
    use hex_session::service::ArrangementDeviceDestination;

    // Substitute only output creation. Restart and the eventual edit go
    // through the real session FIFO, and a headless callback owns the cursor.
    struct RestartObserver(SyncSender<(u64, u64)>);
    impl WorkerAudioSink for RestartObserver {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn restart_audio(&mut self, generation: u64, request: u64) -> Result<(), String> {
            self.0
                .try_send((generation, request))
                .map_err(|e| e.to_string())
        }
    }

    let (restarts, restarted) = sync_channel(1);
    let service =
        Service::with_audio_sink(16, 8_000_000, None, Box::new(RestartObserver(restarts))).unwrap();
    let next = |service: &Service| {
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match service.try_event() {
                Ok(event) => {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    break event;
                }
                Err(TryRecvError::Empty) => {
                    assert!(Instant::now() < deadline, "session event");
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    next(&service);
    let add_track = |name: &str| Request::Edit(Command::AddTrack { name: name.into() });
    service.submit(add_track("target")).unwrap();
    let first = next(&service);
    let target = first.state.project.tracks()[0].id();
    service.submit(add_track("other")).unwrap();
    let ready = next(&service).state;
    let other = ready.project.tracks()[1].id();
    let revision = ready.revision;
    let sample = (edit != EditFixture::Effect).then(|| {
        let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
        let path = files.wav("recovery.wav", 48_000, 512);
        (files, path)
    });
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        ready.project.as_ref().clone(),
        Arc::clone(&ready.midi_preview),
    );
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let open = || {
        let (mut output, mut backend) =
            hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
        let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
        output
            .publication
            .control()
            .unwrap()
            .publish(playback)
            .unwrap();
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let audio = EditorAudio {
            control: output.take_client().unwrap(),
            sample_rate: 48_000,
            label: "effect recovery fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        };
        (output, backend, audio)
    };
    let (mut old_output, mut old_backend, mut old_audio) = open();
    old_audio.control.command(PlaybackCommand::Play).unwrap();
    old_backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let request = || Request::DropBrowserDeviceOnArrangement {
        project_generation: 0,
        revision,
        class: NativeDeviceClass::Effect,
        type_id: DeviceTypeId::from_static("hex.eq"),
        destination: ArrangementDeviceDestination::ExistingTrack {
            track: target,
            instrument: None,
        },
        new_track_gain: None,
    };
    let recovery = editor.update(cx, |v, _| {
        v.service = service;
        v.state = Some(ready);
        v.project_generation = 0;
        v.audio = Some(old_audio);
        v.degraded_plugins = crate::editor_backend::DegradedDevices::new(vec![2]);
        if let Some((_, path)) = &sample {
            let start = 24_000;
            let (destination, import) = if edit == EditFixture::NewTrackSample {
                let before_track = Some(other);
                (
                    BrowserDropDestination::NewTrack {
                        before_track,
                        start,
                    },
                    Request::ImportToNewTrack {
                        path: path.clone(),
                        before_track,
                        start,
                    },
                )
            } else {
                (
                    BrowserDropDestination::ExistingTrack {
                        track: target,
                        start,
                    },
                    Request::ImportToTrackWithOverwrite {
                        path: path.clone(),
                        track: target,
                        start,
                    },
                )
            };
            v.track_defaults.set_test_state(0.25, false, true);
            v.set_auto_warp_imports_for_test(edit == EditFixture::NewTrackSample);
            let drag = v
                .audio_file_drag_payload(
                    path.clone(),
                    "recovery".into(),
                    None,
                    Default::default(),
                    Default::default(),
                )
                .unwrap();
            assert!(
                v.enqueue_sample_drop(import, &drag, destination),
                "{:?}",
                v.error
            );
            assert_eq!(v.sample_drop_target(), Some(destination));
            v.set_auto_warp_imports_for_test(false);
            v.track_defaults.set_test_state(0.5, false, true);
        } else {
            assert!(v.enqueue_device_barrier(request()));
        }
        let generation = v
            .recovery
            .generation()
            .expect("graph edit starts audio recovery");
        assert_eq!(v.recovery.position(), Some(64));
        assert!(v.deferred_play.is_some());
        assert!(!v.enqueue_device_barrier(request()), "one deferred effect");
        assert_eq!(v.worker_requests_pending, 1, "only restart is admitted");
        v.selected_track = Some(other);
        generation
    });
    assert_eq!(
        restarted
            .recv_timeout(std::time::Duration::from_millis(500))
            .unwrap(),
        (0, 1)
    );
    drop(old_backend);
    old_output.publication.close();
    drop(old_output);

    editor.update(cx, |v, _| {
        let restart = next(&v.service);
        assert_eq!(restart.state.revision, revision);
        v.receive(restart);
        assert!(v.service.try_event().is_err());
        if outcome == EditRecovery::ProjectChanged {
            assert!(v.enqueue(add_track("intervening edit")));
            let changed = next(&v.service);
            assert_eq!(changed.state.revision, revision + 1);
            v.receive(changed);
        }
        if outcome == EditRecovery::Failed {
            v.fail_audio_recovery("fixture audio recovery failed");
            assert!(v.recovery.deferred_edit.is_none());
            assert!(v.pending_sample_drop.is_none());
            assert!(v.service.try_event().is_err());
            assert_eq!(v.state.as_ref().unwrap().revision, revision);
        }
    });
    if outcome == EditRecovery::Failed {
        return;
    }

    let (mut fresh_output, mut fresh_backend, fresh_audio) = open();
    editor.update(cx, |v, _| {
        v.audio = Some(fresh_audio);
        // Opening a new output clears observations belonging to the old one.
        v.degraded_plugins = Default::default();
        v.applied_project_revision = v.state.as_ref().map(|s| s.revision);
        assert!(v.recovery.opened(recovery));
        assert!(
            !v.finish_audio_recovery_tick(),
            "an open output is not a restored graph"
        );
        assert!(v.service.try_event().is_err());
        assert!(v.recovery.restored(recovery));
        assert!(
            !v.finish_audio_recovery_tick(),
            "cursor seek must reach the callback"
        );
    });
    fresh_backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(cx, |v, _| {
        assert!(v.finish_audio_recovery_tick());
        assert!(v.recovery.deferred_edit.is_none());
        assert!(!v.finish_audio_recovery_tick());
        if outcome == EditRecovery::ProjectChanged {
            assert!(v.service.try_event().is_err());
            assert!(v.pending_sample_drop.is_none());
            assert!(
                v.error
                    .as_deref()
                    .unwrap()
                    .contains("project changed during audio recovery")
            );
            return;
        }
        assert_eq!(v.worker_requests_pending, 1, "{:?}", v.error);
        let added = next(&v.service);
        assert_eq!(added.state.revision, revision + 1);
        let project = &added.state.project;
        match edit {
            EditFixture::Effect => {
                let devices = project.track(target).unwrap().devices();
                assert_eq!(devices.len(), 1);
                assert_eq!(
                    devices[0].state().type_id(),
                    DeviceTypeId::from_static("hex.eq")
                );
                assert!(project.track(other).unwrap().devices().is_empty());
            }
            EditFixture::ExistingTrackSample => {
                assert_eq!(project.tracks().len(), 2);
                let [clip] = project.track(target).unwrap().clips() else {
                    panic!("one sample on the captured track");
                };
                assert_eq!(clip.start(), 24_000);
                assert!(project.track(other).unwrap().clips().is_empty());
            }
            EditFixture::NewTrackSample => {
                let [first, inserted, last] = project.tracks() else {
                    panic!("one inserted track");
                };
                assert_eq!((first.id(), last.id()), (target, other));
                assert_eq!(inserted.gain(), 0.25);
                let [clip] = inserted.clips() else {
                    panic!("one sample on the inserted track");
                };
                assert_eq!(clip.start(), 24_000);
                assert!(
                    clip.audio_edit().is_some(),
                    "capture auto-warp before recovery"
                );
            }
        }
        if sample.is_some() {
            assert!(
                v.pending_sample_drop.is_some(),
                "preview waits for its terminal"
            );
        }
        v.receive(added);
        assert!(v.pending_sample_drop.is_none());
        assert!(v.service.try_event().is_err(), "no repeated insertion");
    });
    editor.update(cx, |v, _| assert!(v.retire_audio_preserving_gain_take()));
    drop(fresh_backend);
    fresh_output.publication.close();
}

// An already-open actual worker endpoint, without a second lifecycle implementation. The native
// desktop tests own reopen/teardown; this fixture exercises UI recovery through the real Service
// seek worker, ordinary resource book, callback and protected scalar observation channel.
struct Sink {
    output: hex_audio::WorkerPreparedOutput,
    events: SyncSender<EditorAudioEvent>,
    hold: Arc<AtomicBool>,
    held: Arc<Mutex<Option<EditorAudioEvent>>>,
    reject_next_fresh: Arc<AtomicBool>,
    cancel_next_fresh: Arc<AtomicBool>,
    cancellation_done: Arc<AtomicBool>,
}

/// Existing production-path test sink with neutral gates. Callers retain the
/// real resource reservation, output carriers and protected observation channel.
pub(in crate::editor) fn worker_fixture_sink(
    output: hex_audio::WorkerPreparedOutput,
    events: SyncSender<EditorAudioEvent>,
) -> Box<dyn WorkerAudioSink> {
    Box::new(Sink {
        output,
        events,
        hold: Arc::new(AtomicBool::new(false)),
        held: Arc::new(Mutex::new(None)),
        reject_next_fresh: Arc::new(AtomicBool::new(false)),
        cancel_next_fresh: Arc::new(AtomicBool::new(false)),
        cancellation_done: Arc::new(AtomicBool::new(false)),
    })
}

#[gpui::test]
fn restart_uses_fault_cursor_instead_of_rewound_transport(cx: &mut gpui::TestAppContext) {
    restart_from_fault(cx, false);
}

#[gpui::test]
fn restart_respects_unconsumed_seek_and_stop_after_fault(cx: &mut gpui::TestAppContext) {
    restart_from_fault(cx, true);
}

fn restart_from_fault(cx: &mut gpui::TestAppContext, stop: bool) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
    output
        .publication
        .control()
        .unwrap()
        .publish(playback)
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command(PlaybackCommand::Play).unwrap();
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    backend.report_render_error().unwrap();
    let status = client.status().unwrap();
    assert_eq!(status.position.load(Acquire), 0);
    assert_eq!(status.recovery_position(), 64);

    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "recovery fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        if stop {
            let ticket = view
                .audio
                .as_mut()
                .unwrap()
                .control
                .command_with_ticket(PlaybackCommand::Seek(37))
                .unwrap();
            view.pending_seek_ticket = Some(ticket);
            view.recover_audio();
            assert!(view.audio.is_some());
            assert_eq!(
                view.error.as_deref(),
                Some("seek pending · wait for audio or press stop before restarting")
            );
            view.stop();
        }
        view.recover_audio();
        assert_eq!(view.offline_cursor, if stop { 0 } else { 64 });
    });
    drop(backend);
    output.publication.close();
}
#[gpui::test]
fn stop_restarts_audio_from_zero_when_a_vst3_worker_is_degraded(cx: &mut gpui::TestAppContext) {
    degraded_worker_recovers(cx, true, None);
}

#[gpui::test]
fn play_restarts_audio_if_stop_has_already_degraded_the_worker(cx: &mut gpui::TestAppContext) {
    degraded_worker_recovers(cx, false, None);
}

#[gpui::test]
fn explicit_play_from_recovers_degraded_vst3_at_the_requested_frame(cx: &mut gpui::TestAppContext) {
    degraded_worker_recovers(cx, false, Some(18_000));
}

fn degraded_worker_recovers(
    cx: &mut gpui::TestAppContext,
    stop_first: bool,
    explicit_frame: Option<u64>,
) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
    output
        .publication
        .control()
        .unwrap()
        .publish(playback)
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command(PlaybackCommand::Play).unwrap();
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(client.status().unwrap().position.load(Acquire), 64);
    if !stop_first {
        client.stop();
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
    }

    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "degraded vst3 fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.degraded_plugins = crate::editor_backend::DegradedDevices::new(vec![2]);
        if stop_first {
            view.stop();
            assert!(view.deferred_play.is_none());
        }
        if let Some(frame) = explicit_frame {
            view.queue_audio_command(PlaybackCommand::PlayFrom(frame));
        } else {
            view.toggle_play();
        }
        assert_eq!(
            view.deferred_play,
            Some((
                hex_engine::playback::PlaybackAction::Transport(PlaybackCommand::Play),
                None,
                view.project_generation
            ))
        );
        assert!(!view.tick_deferred_play());
        assert!(view.audio.is_none());
        assert!(view.pending_restart_request.is_some());
        assert!(view.stopped_playback_restore.is_none());
        assert_eq!(view.recovery.position(), Some(explicit_frame.unwrap_or(0)));
        view.stop();
        assert!(view.deferred_play.is_none());
    });
    drop(backend);
    output.publication.close();
}

#[gpui::test]
fn a_changed_output_route_restarts_audio_once_its_guards_allow(cx: &mut gpui::TestAppContext) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let identity = client.output_identity().unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "route fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        // A notice for an output that is no longer current is dropped.
        view.route_follow = Some(identity + 1);
        assert!(!view.follow_output_route());
        assert_eq!(view.route_follow, None);
        // An open dialog defers the move without reporting an error.
        let error = view.error.clone();
        view.route_follow = Some(identity);
        view.save_dialog_open = true;
        assert!(!view.follow_output_route());
        assert_eq!(view.route_follow, Some(identity));
        assert_eq!(view.error, error);
        assert!(view.pending_recovery_generation.is_none());
        // Once allowed, the move uses the restart-audio recovery exactly once.
        view.save_dialog_open = false;
        assert!(view.follow_output_route());
        assert_eq!(view.route_follow, None);
        assert!(view.pending_recovery_generation.is_some());
        assert!(view.pending_restart_request.is_some());
        assert_eq!(view.status.to_string(), "audio output changed · reopening");
        assert!(!view.follow_output_route());
    });
    drop(backend);
    output.publication.close();
}
impl WorkerAudioSink for Sink {
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.output
                .publication
                .device_resource_admission_on_control()
                .unwrap(),
            DeviceResourceAdmission::default(),
            DeviceResourceAdmission::default(),
        ]
    }
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {
        self.output.publication.control().unwrap().reclaim();
    }
    fn close(&mut self) {
        self.output.publication.close();
    }
    fn publish_resident(
        &mut self,
        playback: Playback,
        _: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        _: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        if matches!(policy, ResidentPublicationPolicy::InstallFreshRuntime)
            && self.reject_next_fresh.swap(false, AcqRel)
        {
            return Err(ResidentPublicationRefusal {
                playback,
                error: "fixture refused stopped restoration".into(),
            });
        }
        let cancel_fresh = matches!(policy, ResidentPublicationPolicy::InstallFreshRuntime)
            && self.cancel_next_fresh.swap(false, AcqRel);
        let control = self.output.publication.control().unwrap();
        let output_identity = control.output_identity().unwrap();
        let policy = match policy {
            ResidentPublicationPolicy::InheritRuntime => PublicationPolicy::InheritRuntime,
            ResidentPublicationPolicy::InheritRuntimeAt(_) => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not capture resident loop witnesses".into(),
                });
            }
            ResidentPublicationPolicy::InstallFreshRuntime => {
                if cancel_fresh {
                    PublicationPolicy::InstallFreshRuntimeIfUnchanged(
                        control.capture_fresh_runtime_fence().unwrap(),
                    )
                } else {
                    PublicationPolicy::InstallFreshRuntime
                }
            }
            ResidentPublicationPolicy::RestorePluginState => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture has no fresh native plugin restoration".into(),
                });
            }
            ResidentPublicationPolicy::FreshIfUnchanged(expected)
            | ResidentPublicationPolicy::SectionWhenReady {
                fence: expected, ..
            } => {
                let Some(actual) = control.capture_fresh_runtime_fence().filter(|actual| {
                    output_identity == expected.output_identity
                        && actual.expected_active_revision() == expected.active_audio_revision
                        && actual.route_state_generation() == expected.route_generation
                        && actual.admitted_command_ticket() == expected.admitted_command_ticket
                        && actual.output_fault_epoch() == expected.output_fault_epoch
                }) else {
                    return Err(ResidentPublicationRefusal {
                        playback,
                        error: "fixture fence changed".into(),
                    });
                };
                match policy {
                    ResidentPublicationPolicy::SectionWhenReady { quantization, .. } => {
                        PublicationPolicy::InstallSectionWhenReady {
                            fence: actual,
                            quantization,
                        }
                    }
                    _ => PublicationPolicy::InstallFreshRuntimeIfUnchanged(actual),
                }
            }
            ResidentPublicationPolicy::ArrangementEdit => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not publish stopped arrangement replacements".into(),
                });
            }
            ResidentPublicationPolicy::ReplaceDocument { .. } => {
                return Err(ResidentPublicationRefusal {
                    playback,
                    error: "fixture does not replace documents".into(),
                });
            }
        };
        let revision = control
            .publish_with_admission_on_control(
                playback,
                policy,
                metadata.context.project_generation,
                |view| {
                    admit(&[
                        view,
                        DeviceResourceAdmission::default(),
                        DeviceResourceAdmission::default(),
                    ])
                },
            )
            .map_err(|error| match error {
                AdmittedPublishError::Admission { reason, playback } => {
                    ResidentPublicationRefusal {
                        playback,
                        error: reason,
                    }
                }
                AdmittedPublishError::Publication(error) => ResidentPublicationRefusal {
                    playback: error.playback,
                    error: error.reason.into(),
                },
            })?;
        if cancel_fresh {
            assert_eq!(
                control.cancel_fresh_runtime_publication(revision),
                hex_audio::prepared::FreshRuntimeCancellationResult::Cancelled
            );
            self.cancellation_done.store(true, Release);
        }
        let event = EditorAudioEvent::Publication {
            context: metadata.context,
            output_identity,
            state: EditorPublicationState::ResidentSubmitted {
                audio_revision: revision,
                metadata,
                observation,
            },
        };
        if self.hold.load(Acquire) {
            assert!(self.held.lock().unwrap().replace(event).is_none());
        } else {
            self.events.try_send(event).unwrap();
        }
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity,
            audio_revision: revision,
        })
    }
}

#[gpui::test]
fn seek_during_latency_rebuild_cancels_the_automatic_resume(cx: &mut gpui::TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    editor.update(cx, |view, _| {
        view.latency_rebuild = Some(super::super::audio_admission::LatencyRebuild::Restoring {
            project: super::super::audio_admission::ProjectRevision {
                generation: 0,
                revision: 0,
            },
            output: 1,
            frame: 123,
            resume: true,
            runtime_audio: None,
            installed: false,
        });
        view.stopped_playback_restore =
            Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
        view.request_seek(456);
        assert!(view.latency_rebuild.is_none());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("wait before seeking")
        );
    });
}

#[gpui::test]
fn replaced_project_cannot_resume_an_older_latency_rebuild(cx: &mut gpui::TestAppContext) {
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        view.latency_rebuild = Some(super::super::audio_admission::LatencyRebuild::Restoring {
            project: super::super::audio_admission::ProjectRevision {
                generation: 0,
                revision: 0,
            },
            output: 1,
            frame: 123,
            resume: true,
            runtime_audio: None,
            installed: false,
        });
        view.project_generation = 1;
        assert!(!view.tick_latency_rebuild());
        assert!(view.latency_rebuild.is_none());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn failed_native_latency_restore_cannot_resume_the_old_audio(cx: &mut gpui::TestAppContext) {
    use super::super::audio_admission::{LatencyRebuild, ProjectRevision, StoppedPlaybackRestore};
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let client = output.take_client().unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        let output = client.output_identity().unwrap();
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "latency restore fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        let revision = view.state.as_ref().unwrap().revision;
        view.applied_project_revision = Some(revision);
        view.latency_rebuild = Some(LatencyRebuild::Restoring {
            project: ProjectRevision {
                generation: view.project_generation,
                revision,
            },
            output,
            frame: 123,
            resume: true,
            runtime_audio: Some(1),
            installed: false,
        });
        view.stopped_playback_restore = Some(StoppedPlaybackRestore::Published(2));
        assert!(!view.tick_latency_rebuild());
        assert!(view.latency_rebuild.is_some());
        // A failed publication clears its pending state. The old graph still
        // represents the canonical project revision, but cannot justify Play.
        view.stopped_playback_restore = None;
        assert!(!view.tick_latency_rebuild());
        assert!(view.latency_rebuild.is_none());
        let mut pcm = [[0.0; 2]; 64];
        backend.render(&mut pcm).unwrap();
        let status = view.audio.as_ref().unwrap().status().unwrap();
        assert!(!status.playing.load(Acquire));
        assert_eq!(status.position.load(Acquire), 0);
        view.audio.take().unwrap().control.close();
    });
    backend.close();
}

#[gpui::test]
fn second_limiter_while_playing_applies_without_stopping(cx: &mut gpui::TestAppContext) {
    let files = super::super::test_environment::Environment::new().unwrap();
    let wav_path = files.root().join("latency-recovery.wav");
    let frames = 96_000_u32;
    let mut wav = Vec::with_capacity(44 + frames as usize * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(frames * 2).to_le_bytes());
    for _ in 0..frames {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&wav_path, wav).unwrap();
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: bytes.checked_mul(2).unwrap(),
            max_construction_peak_bytes: bytes.checked_mul(3).unwrap(),
        },
        max_scan_frames: 100_000,
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(reservation.storage)
        .unwrap();
    let client = output.take_client().unwrap();
    let (events, receiver) = sync_channel(64);
    let hold = Arc::new(AtomicBool::new(false));
    let held = Arc::new(Mutex::new(None));
    let reject_next_fresh = Arc::new(AtomicBool::new(false));
    let cancel_next_fresh = Arc::new(AtomicBool::new(false));
    let cancellation_done = Arc::new(AtomicBool::new(false));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        Box::new(Sink {
            output,
            events,
            hold: Arc::clone(&hold),
            held: Arc::clone(&held),
            reject_next_fresh: Arc::clone(&reject_next_fresh),
            cancel_next_fresh: Arc::clone(&cancel_next_fresh),
            cancellation_done: Arc::clone(&cancellation_done),
        }),
        reservation,
    )
    .unwrap();
    let mut receive = || {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            match service.try_event() {
                Ok(event) => {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    break event;
                }
                Err(TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "second_limiter_while_playing_applies_without_stopping: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    receive();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "audio".into(),
        }))
        .unwrap();
    let track = match receive().created.unwrap() {
        Created::Track(track) => track,
        other => panic!("{other:?}"),
    };
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "second".into(),
        }))
        .unwrap();
    let second_track = match receive().created.unwrap() {
        Created::Track(track) => track,
        other => panic!("{other:?}"),
    };
    service
        .submit(Request::ImportToTrack {
            path: wav_path,
            track,
            start: 0,
        })
        .unwrap();
    let imported = receive();
    assert_eq!(
        imported.state.project.track(track).unwrap().clips().len(),
        1
    );
    let add_limiter = |track| Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_engine::DeviceTypeId::from_static("hex.limiter")),
        before: None,
    };
    service.submit(add_limiter(track)).unwrap();
    let current = receive().state;
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        current.project.as_ref().clone(),
        Arc::clone(&current.midi_preview),
    );
    editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(current);
        view.project_generation = 0;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "latency fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.pending_playback = None;
        view.tick(cx);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|s| view.applied_project_revision == Some(s.revision))
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "initial graph did not apply");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        view.queue_audio_command(PlaybackCommand::PlayFrom(123))
    });
    let mut audible_before = false;
    for _ in 0..32 {
        let mut output = [[0.0; 2]; 64];
        backend.render(&mut output).unwrap();
        audible_before |= output.iter().flatten().any(|sample| sample.abs() > 0.05);
    }
    assert!(
        audible_before,
        "the original graph must produce the fixture audio"
    );
    // A latency-layout change while playing applies at a block boundary:
    // the transport keeps running, with no stop/rebuild and no error.
    editor.update(cx, |view, _| {
        assert!(view.enqueue(add_limiter(second_track)));
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            assert!(view.latency_rebuild.is_none());
            assert!(view.audio_status().unwrap().playing.load(Acquire));
            view.state.as_ref().is_some_and(|s| {
                view.applied_project_revision == Some(s.revision)
                    && s.project
                        .track(second_track)
                        .is_some_and(|track| !track.devices().is_empty())
            })
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "latency change did not apply while playing: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(second_track)
                .unwrap()
                .devices()
                .len(),
            1
        );
    });
    let mut audible_after = false;
    for _ in 0..32 {
        let mut output = [[0.0; 2]; 64];
        backend.render(&mut output).unwrap();
        audible_after |= output.iter().flatten().any(|sample| sample.abs() > 0.05);
    }
    assert!(audible_after, "playback continues with the new graph");
}

#[gpui::test]
fn recovered_mutable_cursor_uses_stopped_preroll_and_exact_callback_ack(
    cx: &mut gpui::TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    // Recovery constructs an interactive runtime with full release/handoff banks.
    let runtime_storage = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: runtime_storage.checked_mul(2).unwrap(),
            max_construction_peak_bytes: runtime_storage.checked_mul(3).unwrap(),
        },
        max_scan_frames: 100_000,
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(reservation.storage)
        .unwrap();
    let client = output.take_client().unwrap();
    let (events, receiver) = sync_channel(64);
    let hold = Arc::new(AtomicBool::new(false));
    let held = Arc::new(Mutex::new(None));
    let release = events.clone();
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        Box::new(Sink {
            output,
            events,
            hold: hold.clone(),
            held: held.clone(),
            reject_next_fresh: Arc::new(AtomicBool::new(false)),
            cancel_next_fresh: Arc::new(AtomicBool::new(false)),
            cancellation_done: Arc::new(AtomicBool::new(false)),
        }),
        reservation,
    )
    .unwrap();
    let mut receive = || {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            match service.try_event() {
                Ok(event) => {
                    backend.render(&mut [[0.0; 2]; 64]).unwrap();
                    assert!(event.error.is_none(), "{:?}", event.error);
                    break event;
                }
                Err(TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "recovered_mutable_cursor_uses_stopped_preroll_and_exact_callback_ack: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    receive();
    service
        .submit(hex_session::service::Request::Edit(Command::AddMidiTrack {
            name: "recovery".into(),
        }))
        .unwrap();
    let event = receive();
    let track = event.state.project.tracks()[0].id();
    let path = std::env::temp_dir().join(format!(
        "hex-recovery-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&68_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&192_000_u32.to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&32_u32.to_le_bytes());
    for _ in 0..16 {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    service
        .submit(hex_session::service::Request::LoadFirstSampler {
            request_id: 1,
            project_generation: 0,
            revision: event.state.revision,
            track,
            slot: hex_session::service::SamplerLoadSlot::New(
                hex_project::InstrumentLoadPlacement::First,
            ),
            path: path.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    let loaded = receive();
    let Created::Asset(asset) = loaded.created.unwrap() else {
        panic!("asset")
    };
    let state = hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
        8, asset, 0.0,
    )
    .unwrap();
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    service
        .submit(hex_session::service::Request::Edit(
            Command::ReplaceDeviceState { device, state },
        ))
        .unwrap();
    receive();
    service
        .submit(hex_session::service::Request::Edit(
            Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            },
        ))
        .unwrap();
    let clip = receive();
    let Created::Clip(clip) = clip.created.unwrap() else {
        panic!("clip")
    };
    service
        .submit(hex_session::service::Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            velocity: 127,
            channel: 0,
        }))
        .unwrap();
    let current = receive().state;
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        current.project.as_ref().clone(),
        Arc::clone(&current.midi_preview),
    );
    editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(current);
        view.project_generation = 0;
        view.mode = super::super::EditorMode::Session;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "recovery fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.pending_playback = None;
        view.tick(cx);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| view.applied_project_revision == Some(state.revision))
                && view.current_seek_requires_preroll() == Some(true)
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "initial protected publication did not apply"
        );
        std::thread::yield_now();
    }
    editor.update(cx, |view, cx| {
        assert_eq!(view.current_seek_requires_preroll(), Some(true));
        let generation = view.recovery.begin(129).unwrap();
        view.recovery.opened(generation);
        view.recovery.restored(generation);
        view.pending_recovery_generation = Some(generation);
        view.tick(cx);
        let pending = view
            .prepared_seek
            .as_ref()
            .expect("recovery must submit a prepared seek, not raw Seek");
        assert_eq!(pending.recovery_generation, Some(generation));
        assert!(!pending.start_playing);
        assert!(pending.prepared_revision.is_none());
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.recovery.generation().is_none()
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "recovery did not finish");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let audible = view.audible_playback.as_ref().unwrap();
        assert!(audible.seek_request_id.is_some());
        let audio = view.audio.as_mut().unwrap();
        let status = audio.control.status().unwrap();
        assert_eq!(status.position.load(Relaxed), 129);
        assert!(!status.playing.load(Relaxed));
        assert_eq!(
            status.applied_revision.load(Acquire),
            audible.audio_revision
        );
        audio
            .control
            .command_with_ticket(PlaybackCommand::Play)
            .unwrap();
    });
    // Resume from a nonzero cursor ramps in (128 frames at 48 kHz, rate
    // scaled; 257 frames cover up to 96 kHz). The prerolled voice must sound
    // from the first frame and settle at its exact level after the ramp.
    let mut ramp = [[0.0; 2]; 257];
    backend.render(&mut ramp).unwrap();
    assert!(
        ramp[0][0] > 0.0 && ramp.windows(2).all(|pair| pair[0][0] <= pair[1][0]),
        "prerolled sampler history must remain playable through the ramp-in: {:?}",
        &ramp[..4]
    );
    assert_eq!(
        ramp[256], [0.25; 2],
        "prerolled sampler history must settle at its exact level"
    );
    hold.store(true, std::sync::atomic::Ordering::Release);
    editor.update(cx, |view, cx| {
        let generation = view.recovery.begin(257).unwrap();
        view.recovery.opened(generation);
        view.recovery.restored(generation);
        view.pending_recovery_generation = Some(generation);
        view.tick(cx);
        let request = view.recovery.prepared_request().unwrap();
        view.request_seek(64);
        assert_eq!(
            view.recovery.prepared_request(),
            Some(request),
            "user seek cannot replace recovery's exact request"
        );
    });
    // Apply the real graph while its protected metadata is withheld from the UI.
    let deadline = crate::test_wait::hang_deadline();
    let delayed_revision = loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let revision = held.lock().unwrap().as_ref().and_then(|event| match event {
            EditorAudioEvent::Publication {
                state: EditorPublicationState::ResidentSubmitted { audio_revision, .. },
                ..
            } => Some(*audio_revision),
            _ => None,
        });
        let applied = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire)
        });
        if let Some(revision) = revision.filter(|revision| applied == *revision) {
            break revision;
        }
        assert!(Instant::now() < deadline, "withheld recovery did not apply");
        std::thread::yield_now();
    };
    editor.update(cx, |view, _| {
        view.stop();
        assert_eq!(view.recovery.position(), Some(0));
        assert_eq!(view.recovery.prepared_request(), None);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let done = editor.update(cx, |view, cx| {
            view.tick(cx);
            view.recovery.generation().is_none()
        });
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Stop did not supersede prepared recovery"
        );
        std::thread::yield_now();
    }
    hold.store(false, std::sync::atomic::Ordering::Release);
    release
        .try_send(held.lock().unwrap().take().unwrap())
        .unwrap();
    for _ in 0..4 {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        editor.update(cx, |view, cx| {
            view.tick(cx);
        });
    }
    editor.update(cx, |view, _| {
        let status = view.audio.as_ref().unwrap().control.status().unwrap();
        assert_eq!(status.position.load(Relaxed), 0);
        assert!(!status.playing.load(Relaxed));
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            delayed_revision,
            "late metadata must preserve the actual applied source after Stop"
        );
        assert!(
            view.current_preroll_source().is_some(),
            "normal seek must remain available"
        );
        view.audio.take().unwrap().control.close();
        let old = std::mem::replace(&mut view.service, Service::new(48_000, 1, 0).unwrap());
        drop(old);
    });
    backend.close();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn automatic_latency_rebuild_renders_the_stop_fade_before_requesting_restore(
    cx: &mut gpui::TestAppContext,
) {
    use super::super::audio_admission::{LatencyRebuild, ProjectRevision, StoppedPlaybackRestore};
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let dc = hex_engine::Audio::new(48_000, vec![[1.0, -1.0]; 4096]).unwrap();
    let clip = hex_engine::Clip::new(dc, 0, 4096, false).unwrap();
    let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
    output
        .publication
        .control()
        .unwrap()
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![track]).unwrap(),
        ))
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command(PlaybackCommand::Play).unwrap();
    let mut pcm = [[0.0; 2]; 64];
    backend.render(&mut pcm).unwrap();
    assert_eq!(pcm[63], [1.0, -1.0]);
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, _| {
        let output = client.output_identity().unwrap();
        let frame = client.status().unwrap().position.load(Acquire);
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "latency fade fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.latency_rebuild = Some(LatencyRebuild::Waiting {
            project: ProjectRevision { generation: view.project_generation, revision: view.state.as_ref().unwrap().revision },
            output,
            runtime_audio: None,
        });
        assert!(view.tick_latency_rebuild());
        backend.render(&mut pcm).unwrap();
        assert!(pcm[0][0] > 0.9 && pcm[63][0] > 0.0 && pcm[63][0] < pcm[0][0], "the automatic transition must render the falling ramp");
        assert!(!view.tick_latency_rebuild(), "consuming Stop only starts the fade");
        assert!(view.stopped_playback_restore.is_none());
        backend.render(&mut pcm).unwrap();
        assert_eq!(pcm[63], [0.0; 2]);
        assert!(view.tick_latency_rebuild());
        assert_eq!(view.stopped_playback_restore, Some(StoppedPlaybackRestore::Preparing));
        assert!(matches!(view.latency_rebuild, Some(LatencyRebuild::Restoring { frame: saved, .. }) if saved == frame));
        view.audio.take().unwrap().control.close();
    });
    backend.close();
}

#[gpui::test]
fn refused_latency_edit_in_midi_project_shows_waiting_until_applied_or_stopped(
    cx: &mut gpui::TestAppContext,
) {
    use super::super::audio_admission::{LatencyRebuild, ProjectRevision, StoppedPlaybackRestore};
    use super::super::controller::LATENCY_EDIT_WAITING;
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let (project, ..) = super::super::clip_drag_ui_test::midi_project();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, _backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let client = output.take_client().unwrap();
    editor.update(cx, |view, _| {
        let output = client.output_identity().unwrap();
        // The callback reports playing; the refused revision is current.
        client.status().unwrap().playing.store(true, Release);
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "midi latency fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.project_generation = 0;
        let revision = view.state.as_ref().unwrap().revision;
        let project = ProjectRevision {
            generation: 0,
            revision,
        };
        view.latency_rebuild = Some(LatencyRebuild::Waiting {
            project,
            output,
            runtime_audio: None,
        });

        // Playing: MIDI voices are kept, and the wait is visible, once.
        assert!(view.tick_latency_rebuild());
        assert_eq!(view.error.as_deref(), Some(LATENCY_EDIT_WAITING));
        assert!(view.latency_rebuild.is_some());
        assert!(!view.tick_latency_rebuild());
        assert!(view.pending_audio_commands.is_empty());

        // Once the revision is audible, the notice retires with the wait.
        view.applied_project_revision = Some(revision);
        assert!(view.tick_latency_rebuild());
        assert!(view.error.is_none());
        assert!(view.latency_rebuild.is_none());

        // Stop installs the waiting revision through the stopped restore.
        view.applied_project_revision = None;
        view.latency_rebuild = Some(LatencyRebuild::Waiting {
            project,
            output,
            runtime_audio: None,
        });
        assert!(view.tick_latency_rebuild());
        view.stop();
        assert!(view.latency_rebuild.is_none());
        assert_ne!(view.error.as_deref(), Some(LATENCY_EDIT_WAITING));
        assert_eq!(
            view.stopped_playback_restore,
            Some(StoppedPlaybackRestore::Preparing)
        );
    });
}

/// Pulse #3784: a streamed document replaces its sampler output only while
/// stopped. A playing edit stops first and resumes at the same frame once it
/// settles, even when the worker refuses it; it never asks the user to stop.
#[gpui::test]
fn streamed_sampler_edit_while_playing_stops_and_resumes_at_its_frame(
    cx: &mut gpui::TestAppContext,
) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    // A long silent clip keeps the fixture transport playing.
    let audio = hex_engine::Audio::new(48_000, vec![[0.0; 2]; 48_000]).unwrap();
    let graph = Playback::from_shared(Arc::new(
        hex_engine::Project::new(
            48_000,
            vec![
                hex_engine::Track::new(
                    vec![hex_engine::Clip::new(audio, 0, 48_000, false).unwrap()],
                    1.0,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap(),
    ));
    output
        .publication
        .control()
        .unwrap()
        .publish(graph)
        .unwrap();
    let client = output.take_client().unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let transport = |view: &EditorView| {
        let status = view.audio.as_ref().unwrap().control.status().unwrap();
        (
            status.playing.load(std::sync::atomic::Ordering::Acquire),
            status.position.load(std::sync::atomic::Ordering::Acquire),
        )
    };
    let frame = editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "streamed fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::VerifiedStreaming,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::VerifiedStreaming;
        view.applied_project_revision = view.state.as_ref().map(|state| state.revision);
        view.queue_audio_command(hex_engine::PlaybackCommand::Play);
        for _ in 0..4 {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        let (playing, frame) = transport(view);
        assert!(playing && frame > 0);
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue(Request::SamplerEdit(
            hex_session::service::SamplerEdit::History(hex_session::service::SamplerHistoryEdit {
                request_id: 1,
                project_generation: view.project_generation,
                revision,
                redo: false,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
        )));
        assert_eq!(view.sampler_edit_resume, Some(frame));
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(!transport(view).0, "the edit stops playback first");
        frame
    });
    // The fixture has no worker sink: settle the worker's refusal, then
    // stand in for the native stopped restore and its applied acknowledgement.
    let deadline = crate::test_wait::hang_deadline();
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            !view.edit_pending()
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "sampler edit settle hang guard");
        std::thread::yield_now();
    }
    editor.update(cx, |view, _| {
        assert_eq!(view.sampler_edit_resume, Some(frame));
        assert!(
            !view.tick_sampler_edit_resume(),
            "waits for the stopped restore"
        );
        view.stopped_playback_restore = None;
        view.applied_project_revision = view.state.as_ref().map(|state| state.revision);
        assert!(view.tick_sampler_edit_resume());
        assert!(view.sampler_edit_resume.is_none());
    });
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(cx, |view, _| {
        let (playing, position) = transport(view);
        assert!(playing, "playback resumes after the edit settles");
        assert!(
            position >= frame && position <= frame + 128,
            "{position} vs {frame}"
        );
    });
    // Release the shared headless output for other fixture tests.
    backend.close();
    drop(output);
    editor.update(cx, |view, _| view.audio = None);
}

/// Pulse #3784: Play pressed while the stopped restore renews effects (for
/// example vst3) is held rather than refused, then starts once the restore
/// settles. A later Stop cancels the held Play.
#[gpui::test]
fn play_during_stopped_restore_starts_once_the_restore_settles(cx: &mut gpui::TestAppContext) {
    let _output_serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.0; 2]; 48_000]).unwrap();
    let graph = Playback::from_shared(Arc::new(
        hex_engine::Project::new(
            48_000,
            vec![
                hex_engine::Track::new(
                    vec![hex_engine::Clip::new(audio, 0, 48_000, false).unwrap()],
                    1.0,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap(),
    ));
    output
        .publication
        .control()
        .unwrap()
        .publish(graph)
        .unwrap();
    let client = output.take_client().unwrap();
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let playing = |view: &EditorView| {
        view.audio
            .as_ref()
            .unwrap()
            .control
            .status()
            .unwrap()
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
    };
    editor.update(cx, |view, _| {
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "restore fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        let restoring = Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
        // A Stop while held cancels the deferred Play.
        view.stopped_playback_restore = restoring;
        view.queue_audio_command(hex_engine::PlaybackCommand::Play);
        assert!(view.deferred_play.is_some());
        view.queue_audio_command(hex_engine::PlaybackCommand::Stop);
        assert!(view.deferred_play.is_none());
        view.queue_audio_command(hex_engine::PlaybackCommand::Play);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.status.contains("playback starts when ready"));
        assert!(!view.tick_deferred_play(), "held while restoring");
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(!playing(view));
        view.stopped_playback_restore = None;
        assert!(view.tick_deferred_play());
        assert!(view.deferred_play.is_none());
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        assert!(playing(view), "held play starts after the restore");
    });
    // Release the shared headless output for other fixture tests.
    backend.close();
    drop(output);
    editor.update(cx, |view, _| view.audio = None);
}
