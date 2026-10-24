//! Native input adapter: ordered wire messages enter the already-admitted host
//! root. No preparation, scheduling, project mutation or audio work lives here.
use super::*;

pub(super) fn sink(
    input: Arc<LiveExpressionInput>,
    notes: Arc<hex_midi::HeldNoteInput>,
    note_input: Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
    activation: hex_engine::computer_keyboard::LiveNoteActivation,
) -> hex_midi::Sink {
    let notes_cancel = Arc::clone(&notes);
    let notes_cancelled = Arc::clone(&notes);
    let writer = Arc::clone(&input);
    let cancel = Arc::clone(&input);
    let cancelled = Arc::clone(&input);
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
            writer.publish(hex_engine::live_expression::Snapshot {
                pressure: value.pressure,
                bend: value.bend,
                wheel: value.wheel,
            })
        }),
        cancel: Box::new(move || {
            if let Some(input) = note_cancel.live_notes() {
                let _ = input.cancel_activation(activation);
            }
            cancel.cancel();
            notes_cancel.cancel();
        }),
        is_cancelled: Box::new(move || {
            cancelled.is_cancelled()
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
            Arc::new(LiveExpressionInput::new(1, track, device)),
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
