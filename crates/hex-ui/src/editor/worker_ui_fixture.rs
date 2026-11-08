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
        service.submit(Request::Edit(command)).unwrap();
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
    use gpui::VisualTestContext;

    #[gpui::test]
    fn worker_section_submission_waits_for_callback_ack_through_repaints(cx: &mut TestAppContext) {
        let mut fixture = open(
            cx,
            vec![hex_project::Command::AddTrack {
                name: "arrangement".into(),
            }],
        );
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        fixture.editor.update(&mut visual, |view, cx| {
            view.request_section_launch(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(256, 512, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Immediate,
            });
            cx.notify();
        });
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
                assert_eq!(
                    view.audio_status()
                        .unwrap()
                        .position
                        .load(std::sync::atomic::Ordering::Acquire),
                    0
                );
            });
        }
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        fixture.editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        fixture.editor.read_with(&visual, |view, _| {
            assert!(view.prepared_seek.is_none());
            assert_eq!(
                view.audio_status()
                    .unwrap()
                    .position
                    .load(std::sync::atomic::Ordering::Acquire),
                320
            );
        });
        fixture.finish(cx);
    }
}
