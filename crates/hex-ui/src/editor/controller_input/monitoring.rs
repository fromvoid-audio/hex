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
        note: Box::new(move |timed| {
            use hex_engine::computer_keyboard::LiveNoteEvent;
            let Some(input) = note_writer.live_notes() else {
                return;
            };
            let event = match timed.event {
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
                hex_midi::NoteEvent::NotePitchBend {
                    channel,
                    pitch,
                    cents,
                } => LiveNoteEvent::NotePitchBend {
                    channel,
                    pitch,
                    cents,
                },
                hex_midi::NoteEvent::NoteTimbre {
                    channel,
                    pitch,
                    value,
                } => LiveNoteEvent::NoteTimbre {
                    channel,
                    pitch,
                    value,
                },
                hex_midi::NoteEvent::ControlChange { channel, cc, value } => {
                    LiveNoteEvent::ControlChange { channel, cc, value }
                }
                hex_midi::NoteEvent::AllNotesOff => LiveNoteEvent::AllNotesOff,
                hex_midi::NoteEvent::InputLost(_) => {
                    let _ = input.cancel_activation(activation);
                    held_cancel.cancel();
                    return;
                }
            };
            if input
                .publish_at(activation, event, timed.timestamp_ns)
                .is_err()
            {
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

#[cfg(all(test, feature = "ui-test-support"))]
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
            Some(Arc::new(LiveExpressionInput::new(1, track, device))),
            Arc::clone(&held),
            Arc::clone(&input),
            activation,
        );
        let mut clock = hex_midi::MonotonicTimestampClock::default();
        hex_midi::deliver_timed_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            hex_midi::MachTimebase::APPLE_SILICON_24MHZ,
            &mut clock,
            [hex_midi::TimedPacket {
                timestamp_ticks: 24_000,
                data: &[0x92, 60, 37, 60, 0][..],
            }],
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
        (sink.note)(hex_midi::TimestampedNoteEvent {
            timestamp_ns: 0,
            event: hex_midi::NoteEvent::Off {
                channel: 2,
                pitch: 60,
                velocity: 0,
            },
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

    #[test]
    fn mpe_wire_ingress_routes_per_note_expression_to_host_pcm_and_retires_held_notes_on_cancel() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let fm4_state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.fm4"),
        )
        .unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "mpe".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: fm4_state,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let project = editor.project();
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
        let mut playback_mpe =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        playback_mpe
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
        let config =
            hex_midi::MpeConfig::new(hex_midi::MpeZone::Lower { member_channels: 7 }, 48).unwrap();
        let agg = hex_midi::MultiPortAggregator::new_mpe(
            1,
            config,
            hex_midi::MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            sink.publish,
            sink.note,
        )
        .unwrap();
        // Lower zone members ch 1 and ch 2 press the same pitch (60) independently;
        // only ch 2 bends +1200 cents and slides timbre to 120. Out-of-zone ch 9 is ignored.
        agg.deliver_timed_packets(
            0,
            [hex_midi::TimedPacket {
                timestamp_ticks: 1_000,
                data: &[
                    0x91, 60, 100, // member ch 1 NoteOn
                    0x92, 60, 100, // member ch 2 NoteOn
                    0xe2, 0x00, 0x50, // member ch 2 pitch bend up (+1200c)
                    0xb2, 74, 120, // member ch 2 CC74 timbre
                    0xd2, 110, // member ch 2 channel pressure
                    0x99, 72, 120, // out-of-zone ch 9 -> ignored
                ],
            }],
        );
        let mut mpe_frames = [[0.0; 2]; 128];
        playback_mpe.render(&mut mpe_frames).unwrap();
        assert!(mpe_frames.iter().flatten().any(|s| *s != 0.0));

        // Compare against unbent two-voice baseline to prove per-note MPE expression modulated PCM.
        let baseline_input = Arc::new(
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
                .with_live_notes()
                .unwrap(),
        );
        let mut playback_plain = Playback::try_from_prepared_with_session(revision).unwrap();
        playback_plain
            .prepare_computer_keyboard(project, track, Arc::clone(&baseline_input))
            .unwrap();
        let plain_act = baseline_input
            .begin_live_notes_activation_on_control()
            .unwrap();
        for ch in [1_u8, 2] {
            baseline_input
                .live_notes()
                .unwrap()
                .publish(
                    plain_act,
                    hex_engine::computer_keyboard::LiveNoteEvent::NoteOn {
                        channel: ch,
                        pitch: 60,
                        velocity: 100,
                    },
                )
                .unwrap();
        }
        let mut plain_frames = [[0.0; 2]; 128];
        playback_plain.render(&mut plain_frames).unwrap();
        assert_ne!(
            mpe_frames, plain_frames,
            "MPE member bend/timbre/pressure must alter synthesized host PCM"
        );

        // Cancelling the sink retires held notes safely and ends the activation.
        (sink.cancel)();
        assert!(held.is_cancelled());
        assert!(
            !input
                .live_notes()
                .unwrap()
                .same_current_activation(activation)
        );
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod unit_tests {
    use super::sink;
    use std::sync::Arc;

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
        let mut clock = hex_midi::MonotonicTimestampClock::default();
        hex_midi::deliver_timed_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            hex_midi::MachTimebase::NANOSECONDS,
            &mut clock,
            [hex_midi::TimedPacket {
                timestamp_ticks: 1_000,
                data: &[0x92, 60, 37][..],
            }],
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
        let mut clock = hex_midi::MonotonicTimestampClock::default();
        hex_midi::deliver_timed_packets_with_notes(
            &mut hex_midi::Decoder::default(),
            2,
            Default::default(),
            hex_midi::MachTimebase::NANOSECONDS,
            &mut clock,
            [hex_midi::TimedPacket {
                timestamp_ticks: 1_000,
                data: packet.as_slice(),
            }],
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
}
