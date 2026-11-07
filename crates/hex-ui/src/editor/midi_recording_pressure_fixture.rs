//! Feature-gated real editor/callback fixture for receipt-pressure coverage.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
use hex_session::service::{Event, Request, Service};
use std::{path::PathBuf, sync::mpsc, time::Instant};

fn next(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < deadline, "session did not respond");
        std::thread::yield_now();
    }
}

pub(crate) fn exercise(
    cx: &mut TestAppContext,
    root: PathBuf,
    reservation: hex_session::SourcePlanReservation,
    mut output: hex_audio::WorkerPreparedOutput,
    mut render: impl FnMut(&mut [[f32; 2]], &mut [f32]) -> Result<(), hex_engine::Invalid>,
) {
    assert!(root.is_absolute() && root.starts_with(std::env::temp_dir()));
    let directory = root.join("samples");
    std::fs::create_dir(&directory).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "receipt pressure".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    let clip = event.state.project.track(track).unwrap().midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    let (sender, receiver) = mpsc::channel();
    let backend = crate::EditorBackend {
        service,
        audio_events: receiver,
        source_reservation: reservation,
    };
    let window = super::super::sampler_load_fixture(cx, backend, root, directory).unwrap();
    let editor = cx
        .update_window(window, |_, window, _| {
            window.root::<super::super::EditorView>().flatten().unwrap()
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(window, cx);
    editor.update(&mut visual, |view, cx| {
        view.receive(event);
        view.selected_track = Some(track);
        view.selected_clip = Some(clip);
        view.clip_view_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    let (project, revision, generation) = editor.read_with(&visual, |view, _| {
        (
            Arc::clone(&view.state.as_ref().unwrap().project),
            view.state.as_ref().unwrap().revision,
            view.project_generation,
        )
    });
    let context = hex_engine::AudioSourceContext {
        project_generation: generation,
        editor_revision: revision,
    };
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &Default::default(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let client = output.take_client().unwrap();
    let old_input = Arc::new(client.new_computer_keyboard_input_on_control().unwrap());
    let mut old =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
            .unwrap();
    old.prepare_computer_keyboard(&project, track, old_input)
        .unwrap();
    let old_revision = output
        .publication
        .control()
        .unwrap()
        .publish_with_admission_and_source_on_control(
            old,
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let mut frames = [[0.0; 2]; 64];
    let mut host = [0.0; 128];
    render(&mut frames, &mut host).unwrap();
    let input = Arc::new(
        client
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_recording_support()
            .unwrap(),
    );
    let mut next_playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
    next_playback
        .prepare_computer_keyboard(&project, track, Arc::clone(&input))
        .unwrap();
    let new_revision = output
        .publication
        .control()
        .unwrap()
        .publish_with_admission_and_source_on_control(
            next_playback,
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    editor.update(&mut visual, |view, _| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.computer_keyboard.enabled = true;
        view.computer_keyboard.prepared = true;
        view.computer_keyboard.request_id = 42;
        view.computer_keyboard.input = Some(Arc::clone(&input));
        view.midi_recorder.mode = Mode::Preparing;
        view.midi_recorder.binding = Some(Arc::new(
            MidiTakeBinding::capture(&project, track, clip).unwrap(),
        ));
        view.midi_recorder.generation = generation;
        view.midi_recorder.revision = revision;
        view.midi_recorder.output_identity = view.audio.as_ref().unwrap().control.output_identity();
        view.midi_recording_published(context, Some(42), new_revision);
        assert_eq!(
            view.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .recording_source()
                .unwrap()
                .audio_revision,
            old_revision
        );
        assert!(!view.arm_prepared_midi_take().unwrap());
        assert!(view.audio.as_ref().unwrap().control.midi_target().is_none());
    });
    render(&mut frames, &mut host).unwrap();
    editor.update(&mut visual, |view, _| {
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap()
    });
    render(&mut frames, &mut host).unwrap();
    editor.update(&mut visual, |view, _| {
        assert!(view.arm_prepared_midi_take().unwrap())
    });
    for event in 0..=1024_u64 {
        if event % 2 == 0 {
            input.set_key(0, Some((60, 100, event / 2 + 1))).unwrap();
        } else {
            input.set_key(0, None).unwrap();
        }
        render(&mut frames[..1], &mut host[..2]).unwrap();
        if event % 2 == 1 {
            for _ in 0..2 {
                render(&mut frames, &mut host).unwrap();
            }
        }
    }
    let next_target = editor.update(&mut visual, |view, _| {
        let mut next = view.midi_recorder.target.unwrap();
        next.capture.take_id += 1;
        assert!(
            view.audio
                .as_mut()
                .unwrap()
                .control
                .midi_begin(next)
                .is_err()
        );
        view.stop();
        next
    });
    render(&mut frames[..1], &mut host[..2]).unwrap();
    editor.update(&mut visual, |view, _| {
        let before = view.state.as_ref().unwrap().project.snapshot();
        assert!(view.tick_midi_recording());
        assert!(view.midi_recorder.mode == Mode::Interrupted);
        assert!(view.midi_recorder.blocks_edits());
        assert!(view.midi_recorder.pending_commit.is_none());
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("recording events arrived faster than they could be saved")
        );
        let notes = view.midi_recorder.take.as_ref().unwrap().notes();
        assert_eq!(notes.len(), 512);
        let first = notes[0].start_frame;
        for (index, note) in notes.iter().enumerate() {
            assert_eq!(note.start_frame, first + 130 * index as u64);
            assert_eq!(note.end_frame, note.start_frame + 1);
            assert_eq!((note.pitch, note.velocity), (60, 100));
        }
        assert!(!view.enqueue(Request::Edit(Command::AddMidiTrack {
            name: "blocked while take is retained".into(),
        })));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(view.audio.as_ref().unwrap().control.midi_target().is_none());
        view.discard_midi_take();
        assert!(!view.midi_recorder.blocks_edits());
        view.audio
            .as_mut()
            .unwrap()
            .control
            .midi_begin(next_target)
            .unwrap();
        view.audio.as_ref().unwrap().control.midi_cancel();
    });
    render(&mut frames[..1], &mut host[..2]).unwrap();
    editor.update(&mut visual, |view, _| {
        assert!(view.tick_midi_recording());
        assert!(!view.midi_recorder.blocks_edits());
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            project.as_ref()
        );
        view.audio.as_mut().unwrap().control.close();
        view.audio = None;
    });
    visual.update(|window, _| window.remove_window());
    drop(editor);
    drop(output);
    drop(sender);
}
