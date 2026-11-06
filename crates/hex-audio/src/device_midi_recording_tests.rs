//! Actual native conversion boundary, driven headlessly. No physical-device claim.
use super::*;
use crate::midi_recording::{AbortReason, Receipt, Target};
use hex_engine::{
    PreparedRevision, SineTable, computer_keyboard::ComputerKeyboardInput,
    midi_capture::MidiCaptureEvent, playback::Playback,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::atomic::Ordering::Acquire};

fn counted<T>(action: impl FnOnce() -> T) -> T {
    #[cfg(feature = "allocation-probe")]
    {
        let mut result = None;
        let counts = allocation_counter::measure(|| result = Some(action()));
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        result.unwrap()
    }
    #[cfg(not(feature = "allocation-probe"))]
    action()
}
fn deliver(callback: &mut prepared::Callback, frames: &mut [Frame], valid_channels: usize) -> bool {
    assert!(frames.len() <= 64);
    let mut host = [0.0_f32; 128];
    counted(|| {
        callback.begin_block();
        let start = 0;
        let rendered = callback.render(frames);
        finish_host_chunk(
            callback,
            &mut host[..frames.len() * 2],
            frames,
            valid_channels,
            &rendered,
            start,
            Instant::now(),
        )
    })
}
fn fixture(
    end: u64,
) -> (
    prepared::Control,
    prepared::Callback,
    Arc<ComputerKeyboardInput>,
    Target,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "record".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
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
    let input = Arc::new(
        control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_recording_support()
            .unwrap(),
    );
    let prepared = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut graph = Playback::try_from_prepared_with_session(prepared).unwrap();
    graph
        .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
        .unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 11,
    };
    let revision = control
        .publish_with_admission_and_source_on_control(
            graph,
            prepared::PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
    let actual = control.status.recording_source().unwrap();
    assert_eq!(actual.context, context);
    assert_eq!(
        control.status.applied_audio_source(),
        None,
        "ordinary recording metadata does not fabricate eligible constructor proof"
    );
    let keys = input.begin_ordered_keys_on_control().unwrap();
    let target = Target {
        capture: hex_engine::midi_capture::MidiCaptureBinding {
            take_id: 1,
            track,
            device: editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            sample_rate: 48_000,
            output_epoch: input.output_epoch_on_control().unwrap(),
            keys,
            hardware: None,
            first_frame: 0,
            end_frame: end,
        },
        project_generation: 7,
        canonical_revision: 11,
        audio_revision: revision,
        output_identity: actual.output_identity,
        transport_epoch: control.status.transport_epoch.load(Acquire),
    };
    (control, callback, input, target)
}
fn receipts(control: &mut prepared::Control) -> Vec<Receipt> {
    std::iter::from_fn(|| control.midi_recording.try_receipt()).collect()
}
#[test]
fn sample_recording_captures_the_live_instrument_during_midi_recording() {
    use crate::sample_recording::{End, OutputCaptureId, Reader};
    let (mut control, mut callback, input, target) = fixture(48_000);
    let mut take = control
        .sample_recording
        .start(
            target.project_generation,
            &[OutputCaptureId::Track(target.capture.track)],
            false,
        )
        .unwrap();
    assert!(take.ready());
    control.midi_recording.begin(target).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut actual = [[0.0; 2]; 64];
    assert!(deliver(&mut callback, &mut actual, 2));
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    control.sample_recording.stop();
    assert!(deliver(&mut callback, &mut [[0.0; 2]; 64], 2));
    let mut delivered = false;
    assert_eq!(
        take.drain(|frames, _, packet| {
            assert!(!delivered);
            assert_eq!(Reader::source_frames(packet, frames, 0), Some(&actual[..]));
            delivered = true;
        }),
        Some(End::Stopped)
    );
    assert!(delivered);
}

#[test]
fn midi_recording_confirms_short_notes_only_after_real_direct_conversion_and_stop_uses_delivered_clock()
 {
    let (mut control, mut callback, input, target) = fixture(48_000);
    control.midi_recording.begin(target).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    input.set_key(0, None).unwrap();
    let mut source = [[0.0; 2]; 64];
    let mut host = [0.0_f32; 128];
    counted(|| {
        callback.begin_block();
        callback.render(&mut source).unwrap();
    });
    assert!(control.midi_recording.try_receipt().is_none());
    counted(|| {
        assert!(finish_host_chunk(
            &mut callback,
            &mut host,
            &mut source,
            2,
            &Ok(()),
            8,
            Instant::now()
        ));
    });
    assert!(host.iter().any(|sample| *sample != 0.0));
    let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
    let note = instrument.prepare_note(60, 100).unwrap();
    for (offset, actual) in source.iter().enumerate() {
        assert_eq!(
            *actual,
            note.sample(offset as u64, 1, &hex_lattice::EMPTY_DEVICE_RESOURCES)
        );
    }
    control.stop();
    counted(|| callback.begin_block());
    let observed = receipts(&mut control);
    assert!(matches!(observed[0], Receipt::Armed { .. }));
    assert!(matches!(
        observed[1],
        Receipt::Note {
            note: hex_engine::midi_capture::AcceptedMidiNote {
                event: MidiCaptureEvent::On {
                    pitch: 60,
                    velocity: 100
                },
                performance_frame: 8,
                ..
            },
            content_frame: 8,
            ..
        }
    ));
    assert!(matches!(
        observed[2],
        Receipt::Note {
            note: hex_engine::midi_capture::AcceptedMidiNote {
                event: MidiCaptureEvent::Off,
                performance_frame: 9,
                ..
            },
            content_frame: 9,
            ..
        }
    ));
    assert!(matches!(
        observed[3],
        Receipt::Finished {
            clock: Some(hex_engine::midi_capture::MidiCaptureClock {
                content_frame: 72,
                performance_frame: 72
            }),
            ..
        }
    ));
    assert_eq!(control.status.position.load(Acquire), 0);
}
#[test]
fn device_error_aborts_a_queued_midi_take_before_the_next_callback() {
    let (mut control, mut callback, _input, target) = fixture(48_000);
    control.midi_recording.begin(target).unwrap();
    control
        .status
        .device_fault
        .store(true, std::sync::atomic::Ordering::Release);
    counted(|| callback.begin_block());
    assert!(matches!(
        receipts(&mut control).as_slice(),
        [Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            last_delivered: None,
            ..
        }]
    ));
    assert!(control.midi_recording.begin(target).is_err());
}

#[test]
fn midi_recording_conversion_failure_aborts_without_publishing_armed_or_notes() {
    let (mut control, mut callback, input, target) = fixture(48_000);
    control.midi_recording.begin(target).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    assert!(!deliver(&mut callback, &mut [[0.0; 2]; 8], 0));
    assert!(matches!(
        receipts(&mut control).as_slice(),
        [Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            last_delivered: None,
            ..
        }]
    ));
}
#[test]
fn midi_recording_stop_racing_render_retains_only_previously_delivered_prefix_on_input_change() {
    let (mut control, mut callback, input, target) = fixture(48_000);
    control.midi_recording.begin(target).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
    let mut source = [[0.0; 2]; 8];
    let mut host = [0.0_f32; 16];
    counted(|| {
        callback.begin_block();
        control.stop();
        callback.render(&mut source).unwrap();
        assert!(finish_host_chunk(
            &mut callback,
            &mut host,
            &mut source,
            2,
            &Ok(()),
            16,
            Instant::now()
        ));
    });
    assert!(matches!(
        receipts(&mut control).last(),
        Some(Receipt::Finished {
            clock: Some(hex_engine::midi_capture::MidiCaptureClock {
                content_frame: 16,
                ..
            }),
            ..
        })
    ));
}
#[test]
fn midi_receipt_pressure_preserves_delivered_prefix_and_terminal_through_stop() {
    let (mut control, mut callback, input, target) = fixture(96_000);
    control.midi_recording.begin(target).unwrap();
    for event in 0..=1024_u64 {
        if event % 2 == 0 {
            input.set_key(0, Some((60, 100, event / 2 + 1))).unwrap();
        } else {
            input.set_key(0, None).unwrap();
        }
        assert!(deliver(&mut callback, &mut [[0.0; 2]; 1], 2));
        if event % 2 == 1 {
            // Allow released instrument voices to retire without draining the
            // independent recording receipt queue.
            for _ in 0..2 {
                assert!(deliver(&mut callback, &mut [[0.0; 2]; 64], 2));
            }
        }
    }
    let next = Target {
        capture: hex_engine::midi_capture::MidiCaptureBinding {
            take_id: target.capture.take_id + 1,
            ..target.capture
        },
        ..target
    };
    assert!(control.midi_recording.begin(next).is_err());
    control.stop();
    counted(|| callback.begin_block());
    let all = receipts(&mut control);
    assert!(matches!(all.first(), Some(Receipt::Armed { .. })));
    assert_eq!(all.len(), 1026, "terminal: {:?}", all.last());
    for (index, receipt) in all[1..1025].iter().enumerate() {
        let Receipt::Note {
            target: receipt_target,
            note,
            content_frame,
        } = receipt
        else {
            panic!("unexpected receipt at {index}: {receipt:?}");
        };
        let expected = if index % 2 == 0 {
            MidiCaptureEvent::On {
                pitch: 60,
                velocity: 100,
            }
        } else {
            MidiCaptureEvent::Off
        };
        assert_eq!(*receipt_target, target);
        assert_eq!(note.event, expected);
        assert_eq!(*content_frame, 8 + index as u64 + 128 * (index as u64 / 2));
        assert_eq!(note.performance_frame, *content_frame);
        assert_eq!(note.occurrence_id, index as u64 / 2 + 1);
    }
    assert!(
        matches!(
            all.last(),
            Some(Receipt::Aborted { reason: AbortReason::ReceiptPressure, last_delivered: Some(clock), .. })
                if clock.content_frame == 8 + 1025 + 128 * 512
                    && clock.performance_frame == clock.content_frame
        ),
        "terminal: {:?}",
        all.last()
    );
    assert!(control.midi_recording.begin(next).is_ok());
}

#[test]
fn midi_recording_overflow_racing_stop_aborts_and_explicit_cancel_cannot_become_finish() {
    for cancel in [false, true] {
        let (mut control, mut callback, input, target) = fixture(48_000);
        control.midi_recording.begin(target).unwrap();
        assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
        if cancel {
            control.midi_recording.cancel();
        } else {
            for token in 1..=129 {
                input.set_key(0, Some((60, 100, token))).unwrap();
                input.set_key(0, None).unwrap();
            }
        }
        control.stop();
        counted(|| callback.begin_block());
        assert!(
            matches!(receipts(&mut control).last(), Some(Receipt::Aborted { reason, .. }) if *reason == if cancel { AbortReason::Cancelled } else { AbortReason::Capacity })
        );
    }
}
#[test]
fn midi_recording_finite_end_and_pending_stop_return_exact_reserved_terminals() {
    for pending in [false, true] {
        let (mut control, mut callback, input, target) = fixture(12);
        control.midi_recording.begin(target).unwrap();
        if pending {
            control.stop();
            counted(|| callback.begin_block());
        } else {
            input.set_key(0, Some((60, 100, 1))).unwrap();
            assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
        }
        let observed = receipts(&mut control);
        assert!(
            matches!(observed.last(), Some(Receipt::Finished { clock, .. }) if clock.map(|clock| clock.content_frame) == if pending { None } else { Some(12) })
        );
        assert!(control.midi_recording.target().is_none());
    }
}
#[test]
fn midi_recording_unknown_source_and_mutual_publication_gain_admission_refuse() {
    let (mut control, mut callback, _input, target) = fixture(48_000);
    control
        .midi_recording
        .begin(Target {
            canonical_revision: 12,
            ..target
        })
        .unwrap();
    assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
    assert!(matches!(
        receipts(&mut control).as_slice(),
        [Receipt::Aborted {
            reason: AbortReason::SourceChanged,
            ..
        }]
    ));
    let next = Target {
        capture: hex_engine::midi_capture::MidiCaptureBinding {
            take_id: 2,
            ..target.capture
        },
        ..target
    };
    control.midi_recording.begin(next).unwrap();
    let gain = crate::gain_recording::Target {
        track_id: target.capture.track.value(),
        project_generation: 7,
        canonical_revision: 11,
        audio_revision: target.audio_revision,
        recording_epoch: 1,
        transport_epoch: target.transport_epoch,
        gesture_serial: 1,
        sample_rate: 48_000,
    };
    assert!(control.gain_recording.begin(gain, 1.0).is_err());
    let graph = Playback::new(hex_engine::Project::new(48_000, vec![]).unwrap());
    assert!(control.publish(graph).is_err());
    control.midi_recording.cancel();
    assert!(deliver(&mut callback, &mut [[0.0; 2]; 8], 2));
    receipts(&mut control);
    assert!(control.gain_recording.begin(gain, 1.0).is_ok());
}

#[test]
fn midi_recording_rejects_delayed_output_transport_before_engine_arm() {
    let (_, _, _, target) = fixture(48_000);
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (_transport, _worker) = callback
        .prepare_output_transport_on_control(1, 1, 1, 64)
        .unwrap();
    control.midi_recording.begin(target).unwrap();
    counted(|| {
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 8]).unwrap();
    });
    assert!(matches!(
        receipts(&mut control).as_slice(),
        [Receipt::Aborted {
            reason: AbortReason::UnsupportedOutput,
            ..
        }]
    ));
}
