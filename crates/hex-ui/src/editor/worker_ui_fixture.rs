//! Bounded worker-backed GPUI fixture. Canonical Service state, ordinary resource
//! admission, protected observations and callback ACKs remain real. No native
//! device/window is opened and no scalar ACK or owner is fabricated.
use super::EditorView;
use crate::editor_backend::EditorAudio;
use gpui::{AppContext, Entity, TestAppContext};
use hex_session::service::{Event, Request, Service};
use std::{
    sync::{MutexGuard, mpsc::sync_channel},
    time::{Duration, Instant},
};

pub(super) struct WorkerUiFixture {
    pub handle: gpui::AnyWindowHandle,
    pub editor: Entity<EditorView>,
    pub backend: hex_audio::worker_fixture::HeadlessBackend,
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
    let client = output.take_client().unwrap();
    let (events, receiver) = sync_channel(64);
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        super::recovery::tests::worker_fixture_sink(output, events),
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
    editor.update(cx, |view, cx| {
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
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(cx, |view, cx| {
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
    WorkerUiFixture {
        handle,
        editor,
        backend,
        _serial: serial,
    }
}

impl WorkerUiFixture {
    /// Quiesce callback before joining its canonical control owner. Removing the
    /// disposable test window never invokes the user's real close/save workflow.
    pub fn finish(mut self, cx: &mut TestAppContext) {
        self.editor.update(cx, |view, _| {
            if let Some(audio) = view.audio.as_mut() {
                audio.control.close();
            }
            view.audio = None;
        });
        self.backend.close();
        self.editor.update(cx, |view, _| {
            view.service.shutdown(Duration::from_secs(2)).unwrap()
        });
        cx.update_window(self.handle, |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::super::arrangement_seek::SeekStage;
    use super::*;
    use gpui::{Modifiers, MouseButton, VisualTestContext};
    use hex_project::{BeatTime, Command, ElementColor, LoopRegion};
    use std::sync::atomic::Ordering::Acquire;

    #[gpui::test]
    fn section_button_plays_once_and_l_click_uses_the_visible_loop(cx: &mut TestAppContext) {
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
        visual.simulate_click(button, Modifiers::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let installed = fixture.editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                let state = view.state.as_ref().unwrap();
                state.project.loop_region()
                    == Some(LoopRegion::new(region.start(), region.end(), false).unwrap())
                    && !view.section_launch_waiting()
                    && view.prepared_seek.is_none()
                    && view.latched_seek_intent.is_none()
            });
            if installed {
                break;
            }
            assert!(Instant::now() < deadline, "one-shot launch was not applied");
            std::thread::yield_now();
        }
        for _ in 0..400 {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        fixture.editor.read_with(&visual, |view, _| {
            let status = view.audio_status().unwrap();
            assert_eq!(status.position.load(Acquire), region.end());
            assert!(!status.playing.load(Acquire));
        });
        fixture.finish(cx);
    }
}
