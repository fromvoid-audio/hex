//! Delivered takes remain owned after the real canonical worker refuses a stale commit.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_session::service::{Event, Service};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::yield_now()
            }
            Err(error) => panic!("worker event: {error}"),
        }
    }
}

#[gpui::test]
fn delivered_gain_take_survives_finished_and_real_worker_source_refusal(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "recorded gain".into(),
        }))
        .unwrap();
    let canonical = next(&service).state;
    let track = canonical.project.tracks()[0].id();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare_for_gain_recording(
            &canonical.project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            track,
        )
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("headless output: {error}"),
        }
    };
    let control = output.publication.control().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 32_000_000,
                max_construction_peak_bytes: 32_000_000,
            },
        )
        .unwrap();
    let audio_revision = control
        .publish_with_admission_and_source_on_control(
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap(),
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: canonical.revision,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command(hex_engine::PlaybackCommand::Play).unwrap();
    let mut frames = [[0.0; 2]; 16];
    let mut host = [0.0; 32];
    backend.render_direct(&mut frames, &mut host).unwrap();
    let target = Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: canonical.revision,
        audio_revision,
        recording_epoch: 1,
        transport_epoch: client.status().unwrap().transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48_000,
    };
    client.gain_begin(target, 0.5).unwrap();
    backend.render_direct(&mut frames, &mut host).unwrap();
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.service = service;
        view.state = Some(Arc::clone(&canonical));
        view.project_generation = 0;
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless gain take".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.pending_playback = None;
        view.playback_candidates.clear();
        view.gain_recorder.mode = Mode::Touching;
        view.gain_recorder.track = Some(track);
        view.gain_recorder.generation = 0;
        view.gain_recorder.revision = canonical.revision;
        view.gain_recorder.audio_revision = Some(audio_revision);
        view.gain_recorder.target = Some(target);
        assert!(view.tick_gain_recording());
        assert_eq!(view.gain_recorder.events.len(), 1);
        view.end_gain_touch();
    });
    backend.render_direct(&mut frames, &mut host).unwrap();
    editor.update(&mut visual, |view, _| {
        // Another producer changed the canonical worker before this delivered
        // take was submitted. Keep its event pending so the UI still owns the
        // actual source view from which recording began.
        view.service
            .submit(Request::Edit(hex_project::Command::Gain {
                track,
                gain: 0.75,
            }))
            .unwrap();
        let changed = next(&view.service);
        assert!(changed.error.is_none());
        assert!(view.tick_gain_recording());
        assert_eq!(
            view.gain_recorder.events.len(),
            1,
            "Finished must transfer custody, never erase delivered events before commit acceptance"
        );
        let refused = next(&view.service);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("project changed"))
        );
        view.receive(refused);
        assert_eq!(
            view.gain_recorder.events.len(),
            1,
            "worker refusal must retain the sealed take for recovery"
        );
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
    });
    backend.close();
    drop(output);
}
