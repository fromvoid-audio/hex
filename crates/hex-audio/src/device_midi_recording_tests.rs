//! Actual native conversion boundary, driven headlessly. No physical-device claim.
use super::*;
use crate::midi_recording::{AbortReason, Receipt, Target};
use hex_engine::{
    PreparedRevision, SineTable, computer_keyboard::ComputerKeyboardInput,
    midi_capture::MidiCaptureEvent, playback::Playback,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::atomic::Ordering::Acquire};

#[path = "device_midi_seven_frame_fixture.rs"]
mod seven_frame;

#[derive(Clone, Copy)]
enum TestLatency {
    None,
    Limiter,
    SevenFrames,
}

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
    let (control, callback, input, target, _) = fixture_with_latency(end, false);
    (control, callback, input, target)
}

fn fixture_with_latency(
    end: u64,
    latency: bool,
) -> (
    prepared::Control,
    prepared::Callback,
    Arc<ComputerKeyboardInput>,
    Target,
    u64,
) {
    fixture_with_mode(
        end,
        if latency {
            TestLatency::Limiter
        } else {
            TestLatency::None
        },
    )
}

fn fixture_with_mode(
    end: u64,
    latency: TestLatency,
) -> (
    prepared::Control,
    prepared::Callback,
    Arc<ComputerKeyboardInput>,
    Target,
    u64,
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
    let mut media = BTreeMap::new();
    if let TestLatency::SevenFrames = latency {
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "reference".into(),
                path: "reference.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 1024,
                playback_frames: 1024,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("reference track")
        };
        let mut reference = vec![[0.0; 2]; 1024];
        reference[100] = [0.5, 0.5];
        media.insert(asset, hex_engine::Audio::new(48_000, reference).unwrap());
    }
    if !matches!(latency, TestLatency::None) {
        let effect = match latency {
            TestLatency::Limiter => hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
            TestLatency::SevenFrames => seven_frame::state(7).unwrap(),
            TestLatency::None => unreachable!(),
        };
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: effect,
            })
            .unwrap();
    }
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
    let prepared = Arc::new(if matches!(latency, TestLatency::SevenFrames) {
        let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&seven_frame::Definition];
        let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            &effects,
        )
        .unwrap()
    } else {
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap()
    });
    let mut graph = Playback::try_from_prepared_with_session(prepared).unwrap();
    let output_latency = u64::from(graph.main_output_latency_frames());
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
    (control, callback, input, target, output_latency)
}

#[test]
fn seven_frame_reference_and_recorded_note_align_across_blocks() {
    for partition in [1_usize, 7, 64] {
        let (mut control, mut callback, input, target, latency) =
            fixture_with_mode(48_000, TestLatency::SevenFrames);
        assert_eq!(latency, 7);
        while control.status.position.load(Acquire) < 107 {
            let remaining = 107 - control.status.position.load(Acquire);
            let size = partition.min(remaining as usize);
            assert!(deliver(&mut callback, &mut vec![[0.0; 2]; size], 2));
        }
        control.midi_recording.begin(target).unwrap();
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let mut audible = [[0.0; 2]; 1];
        assert!(deliver(&mut callback, &mut audible, 2));
        assert!(
            audible[0].iter().any(|sample| sample.abs() > 0.1),
            "reference at frame 100 was not heard at output frame 107: {audible:?}"
        );
        assert!(
            receipts(&mut control)
                .iter()
                .any(|receipt| matches!(receipt,
                    Receipt::Note { content_frame: 100, note, .. }
                        if matches!(note.event, MidiCaptureEvent::On { pitch: 60, .. })
                )),
            "partition={partition}"
        );
    }
}

#[test]
fn midi_recording_aligns_to_audible_position_with_device_latency() {
    let (mut control, mut callback, input, target, latency) = fixture_with_latency(48_000, true);
    let (mut direct_control, mut direct_callback, direct_input, direct_target, direct_latency) =
        fixture_with_latency(48_000, false);
    assert!(latency > 0);
    assert_eq!(direct_latency, 0);
    for _ in 0..4 {
        assert!(deliver(&mut callback, &mut [[0.0; 2]; 64], 2));
        assert!(deliver(&mut direct_callback, &mut [[0.0; 2]; 64], 2));
    }
    let cursor = control.status.position.load(Acquire);
    assert_eq!(cursor, direct_control.status.position.load(Acquire));
    control.midi_recording.begin(target).unwrap();
    direct_control.midi_recording.begin(direct_target).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    direct_input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut delayed = Vec::new();
    let mut direct = Vec::new();
    for _ in 0..4 {
        let mut delayed_block = [[0.0; 2]; 64];
        let mut direct_block = [[0.0; 2]; 64];
        assert!(deliver(&mut callback, &mut delayed_block, 2));
        assert!(deliver(&mut direct_callback, &mut direct_block, 2));
        delayed.extend_from_slice(&delayed_block);
        direct.extend_from_slice(&direct_block);
    }
    let first_sound = |frames: &[[f32; 2]]| {
        frames
            .iter()
            .position(|frame| frame.iter().any(|sample| sample.abs() > 1e-7))
            .unwrap()
    };
    let measured_latency = (first_sound(&delayed) - first_sound(&direct)) as u64;
    assert_eq!(latency, measured_latency);
    let receipts = receipts(&mut control);
    assert!(
        receipts.iter().any(|receipt| matches!(
            receipt,
            Receipt::Note { content_frame, note, .. }
                if matches!(note.event, MidiCaptureEvent::On { pitch: 60, .. })
                    && *content_frame == cursor - measured_latency
        )),
        "expected audible-position note at {}, got {receipts:?}",
        cursor - measured_latency
    );
}

#[test]
fn latency_corrected_capture_reaches_the_clip_end_across_partitions() {
    for block in [1_usize, 7, 64] {
        let (mut control, mut callback, input, target, latency) = fixture_with_latency(100, true);
        assert!(latency > 0 && latency < 100);
        control.midi_recording.begin(target).unwrap();
        // Wait until the scheduled cursor is past the clip's uncorrected end,
        // while its final latency-delayed samples are still being delivered.
        while control.status.position.load(Acquire) < 110 {
            assert!(deliver(&mut callback, &mut vec![[0.0; 2]; block], 2));
        }
        let cursor = control.status.position.load(Acquire);
        assert!(cursor < target.capture.end_frame + latency);
        input.set_key(0, Some((60, 100, 1))).unwrap();
        assert!(deliver(&mut callback, &mut [[0.0; 2]; 1], 2));
        let captured = receipts(&mut control);
        let origin = captured
            .iter()
            .find_map(|receipt| match receipt {
                Receipt::Armed { clock, .. } => Some(*clock),
                _ => None,
            })
            .expect("arm receipt");
        assert!(
            captured.iter().any(|receipt| matches!(receipt,
                Receipt::Note { content_frame, note, .. }
                    if matches!(note.event, MidiCaptureEvent::On { pitch: 60, .. })
                        && *content_frame == cursor - latency
            )),
            "block={block}, cursor={cursor}, receipts={captured:?}"
        );
        while control.status.position.load(Acquire) < target.capture.end_frame + latency {
            assert!(deliver(&mut callback, &mut vec![[0.0; 2]; block], 2));
        }
        assert!(
            receipts(&mut control).iter().any(|receipt| matches!(
                receipt,
                Receipt::Finished { clock: Some(clock), .. }
                    if clock.content_frame == target.capture.end_frame + latency
                        && clock.performance_frame - origin.performance_frame
                            == clock.content_frame - origin.content_frame
            )),
            "block={block}"
        );
    }
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
    assert_delayed_output_rejected(recording_target());
}

// Keep the two real callback fixtures in separate test frames. Each fixture
// remains large enough that reserving both in one debug frame overflows the
// default test-thread stack before the refusal can be exercised.
#[inline(never)]
fn recording_target() -> Target {
    let (_, _, _, target) = fixture(48_000);
    target
}

#[inline(never)]
fn assert_delayed_output_rejected(target: Target) {
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
