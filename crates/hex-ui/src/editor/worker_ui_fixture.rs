//! Bounded worker-backed GPUI fixture. Canonical Service state, ordinary resource
//! admission, protected observations and callback ACKs remain real. No native
//! device/window is opened and no scalar ACK or owner is fabricated.
use super::EditorView;
use crate::editor_backend::EditorAudio;
use gpui::{App, AppContext, AsyncApp, Entity, TestAppContext};
use hex_session::service::{Event, Request, Service};
use std::{
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, MutexGuard, atomic::AtomicBool, mpsc::sync_channel},
    time::{Duration, Instant},
};

pub(super) struct WorkerUiFixture {
    pub handle: gpui::AnyWindowHandle,
    pub editor: Entity<EditorView>,
    pub backend: hex_audio::worker_fixture::HeadlessBackend,
    consume_next_inherited_candidate: Arc<AtomicBool>,
    cleanup_app: Option<AsyncApp>,
    _serial: MutexGuard<'static, ()>,
}

fn receive(service: &Service, backend: &mut hex_audio::worker_fixture::HeadlessBackend) -> Event {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "fixture canonical event unavailable"
        );
        std::thread::yield_now();
    }
}

/// Open with authoritative canonical state and an acknowledged ordinary graph.
/// Commands are fixture setup, before the view exists. Keep callbacks paused by
/// withholding `backend.render`; UI ticks/repaints alone never imply an ACK.
pub(super) fn open(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
) -> WorkerUiFixture {
    open_with_audition_readiness(cx, commands, false)
}

pub(super) fn open_with_audition_readiness(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
    audition_ready: bool,
) -> WorkerUiFixture {
    open_with_section_support(cx, commands, audition_ready, false)
}

pub(super) fn open_with_section_support(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
    audition_ready: bool,
    section_fixture: bool,
) -> WorkerUiFixture {
    open_with_section_and_recovery(cx, commands, audition_ready, section_fixture, None)
}

pub(super) fn open_with_section_and_recovery(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
    audition_ready: bool,
    section_fixture: bool,
    recovery: Option<std::path::PathBuf>,
) -> WorkerUiFixture {
    open_worker(
        cx,
        commands,
        audition_ready,
        section_fixture,
        recovery,
        false,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn open_for_paused_monitor(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
) -> WorkerUiFixture {
    open_worker(cx, commands, false, true, None, true)
}

fn open_worker(
    cx: &mut TestAppContext,
    commands: Vec<hex_project::Command>,
    audition_ready: bool,
    section_fixture: bool,
    recovery: Option<std::path::PathBuf>,
    paused_monitor_enabled: bool,
) -> WorkerUiFixture {
    let serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let runtime = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
    let reservation = hex_session::ResidentNativeReservation {
        storage: hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: runtime * 2,
            max_construction_peak_bytes: runtime * 3,
        },
        max_scan_frames: 100_000,
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(reservation.storage)
        .unwrap();
    let mut client = output.take_client().unwrap();
    if section_fixture {
        client.bind_resident_transport(0);
    }
    let (events, receiver) = sync_channel(64);
    let consume_next_inherited_candidate = Arc::new(AtomicBool::new(false));
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        recovery,
        super::recovery::tests::worker_fixture_sink(
            output,
            events,
            audition_ready,
            Arc::clone(&consume_next_inherited_candidate),
            section_fixture,
            paused_monitor_enabled,
        ),
        reservation,
    )
    .unwrap();
    let mut current = receive(&service, &mut backend).state;
    for command in commands {
        let request = if matches!(command, hex_project::Command::AddArrangementSection { .. }) {
            Request::EditArrangementSection {
                project_generation: 0,
                revision: current.revision,
                command,
            }
        } else {
            Request::Edit(command)
        };
        service.submit(request).unwrap();
        current = receive(&service, &mut backend).state;
    }
    let (handle, editor) = super::clip_drag_ui_test::editor_window(
        cx,
        current.project.as_ref().clone(),
        current.midi_preview.clone(),
    );
    let mut fixture = WorkerUiFixture {
        handle,
        editor,
        backend,
        consume_next_inherited_candidate,
        cleanup_app: Some(cx.to_async()),
        _serial: serial,
    };
    fixture.editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(current);
        view.project_generation = 0;
        view.audio_events = receiver;
        view.audio = Some(EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "worker ui fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.pending_playback = None;
        view.error = None;
        view.tick(cx);
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = fixture.editor.update(cx, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| view.applied_project_revision == Some(state.revision))
                && view.audible_playback.as_ref().is_some_and(|source| {
                    source.audio_revision
                        == view
                            .audio_status()
                            .unwrap()
                            .applied_revision
                            .load(std::sync::atomic::Ordering::Acquire)
                })
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture ordinary source was not acknowledged"
        );
        std::thread::yield_now();
    }
    fixture
}

impl WorkerUiFixture {
    /// Quiesce callback before joining its canonical control owner. Removing the
    /// disposable test window never invokes the user's real close/save workflow.
    pub fn finish(mut self, cx: &mut TestAppContext) {
        self.cleanup_app = None;
        cx.update(|cx| self.retire(cx)).unwrap();
    }

    fn retire(&mut self, cx: &mut App) -> gpui::Result<()> {
        self.editor.update(cx, |view, _| {
            if let Some(audio) = view.audio.as_mut() {
                audio.control.close();
            }
            view.audio = None;
        });
        self.backend.close();
        let shutdown = self
            .editor
            .update(cx, |view, _| view.service.shutdown(Duration::from_secs(2)));
        let removed = cx.update_window(self.handle, |_, window, _| window.remove_window());
        shutdown?;
        removed
    }
}

impl Drop for WorkerUiFixture {
    fn drop(&mut self) {
        let Some(app) = self.cleanup_app.take() else {
            return;
        };
        // A failed test can leave the editor alive in GPUI. Retire it before
        // releasing the fixture lock. A second GPUI panic must leave the
        // original failure observable.
        let cleanup = catch_unwind(AssertUnwindSafe(|| app.update(|cx| self.retire(cx))?));
        if let Ok(Err(error)) = cleanup {
            let _ = writeln!(std::io::stderr(), "worker ui fixture cleanup: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::arrangement_seek::SeekStage;
    use super::*;
    use gpui::{Modifiers, MouseButton, VisualTestContext};
    use hex_project::{BeatTime, Command, ElementColor, LoopRegion};
    use std::sync::atomic::Ordering::{Acquire, Release};

    #[gpui::test]
    fn worker_fixture_unwind_retires_output_before_next_open(cx: &mut TestAppContext) {
        let cx = &mut cx.new_app();
        let fixture = open(cx, Vec::new());
        let failure = catch_unwind(AssertUnwindSafe(move || {
            let _fixture = fixture;
            panic!("fixture test assertion");
        }))
        .unwrap_err();
        assert_eq!(
            failure.downcast_ref::<&str>().copied(),
            Some("fixture test assertion")
        );
        assert!(
            cx.windows().is_empty(),
            "fixture left its editor window alive after unwinding"
        );
        open(cx, Vec::new()).finish(cx);
    }

    #[gpui::test]
    fn section_button_plays_through_and_l_click_uses_the_visible_loop(cx: &mut TestAppContext) {
        section_button_case(cx, false);
    }

    #[gpui::test]
    fn section_button_launches_after_live_loop_handoff_refusal(cx: &mut TestAppContext) {
        section_button_case(cx, true);
    }

    fn section_button_case(cx: &mut TestAppContext, refuse_loop_handoff: bool) {
        cx.update(super::super::bind_editor_keys);
        let start = BeatTime::from_quarters(2).unwrap();
        let end = BeatTime::from_quarters(3).unwrap();
        let region = LoopRegion::new(48_000, 72_000, true).unwrap();
        let mut fixture = open(
            cx,
            vec![
                Command::AddTrack {
                    name: "arrangement".into(),
                },
                Command::AddArrangementSection {
                    name: "section".into(),
                    color: ElementColor::Blue,
                    start,
                    end,
                },
                Command::AddArrangementSection {
                    name: "next section".into(),
                    color: ElementColor::Blue,
                    start: end,
                    end: BeatTime::from_quarters(4).unwrap(),
                },
            ],
        );
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        let selector: &'static str = fixture.editor.read_with(&visual, |view, _| {
            let id = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
            Box::leak(format!("section-launch-{}", id.value()).into_boxed_str())
        });
        fixture.editor.update(&mut visual, |view, cx| {
            view.session_quantization = hex_engine::LaunchQuantization::Immediate;
            cx.notify();
        });
        visual.run_until_parked();
        let button = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(button, Modifiers::default());
        fixture.editor.update(&mut visual, |view, _| {
            assert!(view.section_launch_waiting());
            assert!(view.pending_loop_region().is_none());
            assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
            view.cancel_section_launch_wait();
        });
        crate::editor::section_launch::test_section_loop_key(&mut visual, true);
        visual.simulate_mouse_down(button, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(button, MouseButton::Left, Modifiers::default());
        crate::editor::section_launch::test_section_loop_key(&mut visual, false);
        visual.run_until_parked();
        let pending_bar = visual.debug_bounds("loop-rail-range").unwrap();
        fixture.editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert!(state.project.loop_region().is_none());
            assert_eq!(
                super::super::loop_rail::displayed_region(view, state),
                Some(region)
            );
            assert!(view.prepared_seek.is_none());
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let committed = fixture.editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.state.as_ref().unwrap().project.loop_region() == Some(region)
                    && view.applied_project_revision()
                        == Some(view.state.as_ref().unwrap().revision)
            });
            if committed {
                break;
            }
            assert!(Instant::now() < deadline, "loop revision was not applied");
            std::thread::yield_now();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let published = fixture.editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                matches!(
                    view.prepared_seek.as_ref().map(|pending| pending.stage),
                    Some(SeekStage::Published { .. })
                )
            });
            if published {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not submit: {:?}",
                fixture.editor.read_with(&visual, |view, _| (
                    view.status.to_string(),
                    view.error.as_ref().map(|e| e.to_string())
                ))
            );
            std::thread::yield_now();
        }
        for _ in 0..2 {
            fixture.editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            visual.run_until_parked();
            fixture.editor.read_with(&visual, |view, _| {
                assert!(view.prepared_seek.is_some());
                assert_eq!(view.audio_status().unwrap().position.load(Acquire), 0);
            });
        }
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        fixture.editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("loop-rail-range"), Some(pending_bar));
        fixture.editor.read_with(&visual, |view, _| {
            assert!(view.prepared_seek.is_none());
            let status = view.audio_status().unwrap();
            assert_eq!(status.position.load(Acquire), region.start() + 64);
            assert!(status.playing.load(Acquire));
        });
        visual.simulate_keystrokes("space");
        // The callback finishes the transport declick before reporting Pause.
        for _ in 0..3 {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        fixture.editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert_eq!(
                view.last_transport_command,
                Some(hex_engine::PlaybackCommand::Pause)
            );
            assert!(!view.audio_status().unwrap().playing.load(Acquire));
            assert!(view.prepared_seek.is_none());
        });
        visual.simulate_keystrokes("space");
        for _ in 0..400 {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        fixture.editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            let status = view.audio_status().unwrap();
            assert!(status.playing.load(Acquire));
            assert!((region.start()..region.end()).contains(&status.position.load(Acquire)));
        });
        let button = visual.debug_bounds(selector).unwrap().center();
        visual.update(|window, cx| {
            fixture.editor.update(cx, |view, cx| {
                view.computer_keyboard.enabled = true;
                view.focus.focus(window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        visual.simulate_mouse_move(
            gpui::point(gpui::px(1.0), gpui::px(1.0)),
            None,
            Modifiers::default(),
        );
        crate::editor::section_launch::test_section_loop_key(&mut visual, true);
        fixture.editor.read_with(&visual, |view, _| {
            assert_eq!(view.computer_keyboard.physical_notes().count(), 1);
        });
        visual.simulate_click(button, Modifiers::default());
        crate::editor::section_launch::test_section_loop_key(&mut visual, false);
        fixture.editor.update(&mut visual, |view, _| {
            assert_eq!(view.computer_keyboard.physical_notes().count(), 0);
            assert!(view.section_launch_waiting());
            assert!(view.pending_loop_region().is_none());
            view.cancel_section_launch_wait();
            view.computer_keyboard.enabled = false;
        });

        fixture
            .consume_next_inherited_candidate
            .store(refuse_loop_handoff, Release);
        visual.simulate_click(button, Modifiers::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let installed = fixture.editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                let state = view.state.as_ref().unwrap();
                state.project.loop_region()
                    == Some(LoopRegion::new(region.start(), region.end(), false).unwrap())
                    && view.applied_project_revision() == Some(state.revision)
                    && view.audio_admission.current().is_some_and(|admission| {
                        admission.state == super::super::audio_admission::AdmissionState::Applied
                    })
                    && !view.section_launch_waiting()
                    && view.prepared_seek.is_none()
                    && view.latched_seek_intent.is_none()
            });
            if installed {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "plain section launch was not applied: {:?}",
                fixture.editor.read_with(&visual, |view, _| (
                    view.status.to_string(),
                    view.error.as_ref().map(|e| e.to_string())
                ))
            );
            std::thread::yield_now();
        }
        for _ in 0..400 {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        fixture.editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            if refuse_loop_handoff {
                assert!(matches!(
                    status.rejected_revision(),
                    Some((revision, hex_engine::PlaybackHandoffRejection::SessionRuntime))
                        if revision < status.applied_revision.load(Acquire)
                ));
            }
            assert!(
                status.position.load(Acquire) > region.end(),
                "plain launch must advance into the next section"
            );
            assert!(status.playing.load(Acquire));
        });
        fixture.finish(cx);
    }
}
