//! Native input adapter: ordered wire messages enter the already-admitted host
//! root. No preparation, scheduling, project mutation or audio work lives here.
use super::*;

pub(super) fn sink(
    input: Option<Arc<LiveExpressionInput>>,
    notes: Arc<hex_midi::HeldNoteInput>,
    note_input: Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    activation: hex_engine::computer_keyboard::LiveNoteActivation,
) -> hex_midi::Sink {
    let notes_cancel = Arc::clone(&notes);
    let notes_cancelled = Arc::clone(&notes);
    let writer = input.clone();
    let cancel = input.clone();
    let cancelled = input;
    let note_writer = Arc::clone(&note_input);
    let note_cancel = Arc::clone(&note_input);
    let note_cancelled = Arc::clone(&note_input);
    let held_cancel = Arc::clone(&notes);
    hex_midi::Sink {
        note: Box::new(move |event| {
            use hex_engine::computer_keyboard::LiveNoteEvent;
            let Some(input) = note_writer.live_notes() else {
                return;
            };
            let event = match event {
                hex_midi::NoteEvent::On {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOn {
                    channel,
                    pitch,
                    velocity,
                },
                hex_midi::NoteEvent::Off {
                    channel,
                    pitch,
                    velocity,
                } => LiveNoteEvent::NoteOff {
                    channel,
                    pitch,
                    velocity,
                },
                hex_midi::NoteEvent::PolyPressure {
                    channel,
                    pitch,
                    value,
                } => LiveNoteEvent::PolyPressure {
                    channel,
                    pitch,
                    value,
                },
                hex_midi::NoteEvent::AllNotesOff => LiveNoteEvent::AllNotesOff,
                hex_midi::NoteEvent::InputLost(_) => {
                    let _ = input.cancel_activation(activation);
                    held_cancel.cancel();
                    return;
                }
            };
            if input.publish(activation, event).is_err() {
                held_cancel.cancel();
            }
        }),
        notes: Arc::clone(&notes),
        publish: Box::new(move |value| {
            if let Some(writer) = &writer {
                writer.publish(hex_engine::live_expression::Snapshot {
                    pressure: value.pressure,
                    bend: value.bend,
                    wheel: value.wheel,
                });
            }
        }),
        cancel: Box::new(move || {
            if let Some(input) = note_cancel.live_notes() {
                let _ = input.cancel_activation(activation);
            }
            if let Some(cancel) = &cancel {
                cancel.cancel();
            }
            notes_cancel.cancel();
        }),
        is_cancelled: Box::new(move || {
            cancelled.as_ref().is_some_and(|input| input.is_cancelled())
                || notes_cancelled.is_cancelled()
                || note_cancelled.is_cancelled()
                || note_cancelled
                    .live_notes()
                    .is_none_or(|input| !input.same_current_activation(activation))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{PreparedRevision, SineTable, playback::Playback};
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn notes_only_sink_tracks_the_selected_port_and_cancels_its_activation() {
        let input = Arc::new(
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                .with_live_notes()
                .unwrap(),
        );
        let token = input.begin_live_notes_activation_on_control().unwrap();
        let held = Arc::new(hex_midi::HeldNoteInput::default());
        let sink = sink(None, Arc::clone(&held), Arc::clone(&input), token);
        hex_midi::deliver_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            [&[0x92, 60, 37][..]],
            &held,
            sink.publish.as_ref(),
            sink.note.as_ref(),
        );
        let hex_midi::HeldNotesRead::Snapshot(notes) = held.read() else {
            panic!("expected a published hardware note");
        };
        let note = notes.after(0).next().unwrap();
        assert_eq!((note.pitch, note.velocity), (60, 37));
        assert!(!(sink.is_cancelled)());
        (sink.cancel)();
        assert!((sink.is_cancelled)());
        assert!(!input.live_notes().unwrap().same_current_activation(token));
        assert!(
            !input.is_cancelled(),
            "physical computer keys keep their root"
        );
    }

    #[test]
    fn poly_pressure_reaches_the_bounded_note_queue_and_cancels_on_overflow() {
        let input = Arc::new(
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                .with_live_notes()
                .unwrap(),
        );
        let activation = input.begin_live_notes_activation_on_control().unwrap();
        let held = Arc::new(hex_midi::HeldNoteInput::default());
        let sink = sink(None, Arc::clone(&held), Arc::clone(&input), activation);
        // One pressure status plus 512 running-status messages remains inside
        // the decoder's packet budget. The smaller host queue must reject it.
        let mut packet = vec![0x92, 60, 91, 0xa2];
        packet.extend(std::iter::repeat_n([60, 85], 512).flatten());
        hex_midi::deliver_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            [packet.as_slice()],
            &held,
            sink.publish.as_ref(),
            sink.note.as_ref(),
        );
        assert!((sink.is_cancelled)());
        assert!(held.is_cancelled());
        assert!(
            !input
                .live_notes()
                .unwrap()
                .same_current_activation(activation)
        );
        assert!(!input.is_cancelled(), "computer keys keep their own root");
    }

    #[test]
    fn actual_wire_ingress_reaches_host_pcm_and_stale_sink_cannot_cancel_reconnect() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "input".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let project = editor.project();
        let before = project.snapshot();
        let device = project.track(track).unwrap().instrument().unwrap().id();
        let revision = Arc::new(
            PreparedRevision::prepare(
                project,
                &Default::default(),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let input = Arc::new(
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                .with_live_notes()
                .unwrap(),
        );
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        playback
            .prepare_computer_keyboard(project, track, Arc::clone(&input))
            .unwrap();
        let activation = input.begin_live_notes_activation_on_control().unwrap();
        let held = Arc::new(hex_midi::HeldNoteInput::default());
        let sink = sink(
            Some(Arc::new(LiveExpressionInput::new(1, track, device))),
            Arc::clone(&held),
            Arc::clone(&input),
            activation,
        );
        hex_midi::deliver_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            [&[0x92, 60, 37, 60, 0][..]],
            &held,
            sink.publish.as_ref(),
            sink.note.as_ref(),
        );
        let mut actual = [[0.0; 2]; 64];
        playback.render(&mut actual).unwrap();
        let oracle = Arc::new(
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                .with_note_audition(&[(60, 37)], 1)
                .unwrap(),
        );
        let mut expected = Playback::try_from_prepared_with_session(revision).unwrap();
        expected
            .prepare_computer_keyboard(project, track, Arc::clone(&oracle))
            .unwrap();
        oracle.trigger_note_audition().unwrap();
        let mut frames = [[0.0; 2]; 64];
        for frame in &mut frames {
            expected.render(std::slice::from_mut(frame)).unwrap();
        }
        assert_eq!(actual, frames);
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        assert_eq!(project.snapshot(), before);
        let fresh = input.begin_live_notes_activation_on_control().unwrap();
        (sink.cancel)();
        (sink.note)(hex_midi::NoteEvent::Off {
            channel: 2,
            pitch: 60,
            velocity: 0,
        });
        assert!(input.live_notes().unwrap().same_current_activation(fresh));
        assert!(!input.is_cancelled());
        input
            .live_notes()
            .unwrap()
            .publish(
                fresh,
                hex_engine::computer_keyboard::LiveNoteEvent::NoteOn {
                    channel: 2,
                    pitch: 64,
                    velocity: 93,
                },
            )
            .unwrap();
    }
}
