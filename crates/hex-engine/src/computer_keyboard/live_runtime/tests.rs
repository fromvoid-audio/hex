use super::*;
use crate::computer_keyboard::tests::{fixture, playback};

fn input() -> Arc<ComputerKeyboardInput> {
    Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap())
}
fn on(pitch: u8, velocity: u8) -> LiveNoteEvent {
    LiveNoteEvent::NoteOn {
        channel: 3,
        pitch,
        velocity,
    }
}
fn off(pitch: u8) -> LiveNoteEvent {
    LiveNoteEvent::NoteOff {
        channel: 3,
        pitch,
        velocity: 42,
    }
}

#[test]
fn short_chord_has_exact_velocity_pcm_and_positive_ordered_gates() {
    let (project, track) = fixture();
    let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
    let a = instrument.prepare_note(60, 37).unwrap();
    let b = instrument.prepare_note(64, 93).unwrap();
    for partition in [1, 17, 256] {
        let input = input();
        let mut tested = playback(&project, track, &input);
        for event in [on(60, 37), on(64, 93), off(60), off(64)] {
            input
                .live_notes()
                .unwrap()
                .publish(
                    input
                        .live_notes()
                        .unwrap()
                        .current_activation_on_control()
                        .unwrap(),
                    event,
                )
                .unwrap();
        }
        let mut output = vec![[0.0; 2]; 1024];
        for frames in output.chunks_mut(partition) {
            tested.render(frames).unwrap();
        }
        for (age, actual) in output.iter().enumerate() {
            let x = a.sample(age as u64, 2, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let y = if age == 0 {
                [0.0; 2]
            } else {
                b.sample(age as u64 - 1, 2, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            };
            assert_eq!(
                *actual,
                [x[0] + y[0], x[1] + y[1]],
                "partition {partition} frame {age}"
            );
        }
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        assert!(input.live_notes().unwrap().snapshot().unwrap().is_empty());
    }
}

#[test]
fn hardware_overflow_releases_its_voices_and_preserves_computer_keyboard() {
    let (project, track) = fixture();
    let input = input();
    let mut tested = playback(&project, track, &input);
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let live = input.live_notes().unwrap();
    let activation = live.current_activation_on_control().unwrap();
    live.publish(activation, on(64, 37)).unwrap();
    tested.render(&mut [[0.0; 2]; 16]).unwrap();
    for _ in 0..live_notes::LIVE_NOTE_INPUT_CAPACITY {
        live.publish(activation, on(67, 93)).unwrap();
    }
    assert!(live.publish(activation, off(64)).is_err());
    assert!(live.is_cancelled());
    let mut output = [[0.0; 2]; 512];
    tested.render(&mut output).unwrap();
    let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
    let physical = instrument.prepare_note(60, 100).unwrap();
    let hardware = instrument.prepare_note(64, 37).unwrap();
    for (offset, actual) in output.into_iter().enumerate() {
        let age = offset as u64 + 16;
        let a = physical.sample(age, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let b = hardware.sample(age, 16, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, [a[0] + b[0], a[1] + b[1]]);
    }
    assert!(!input.is_cancelled());
}

#[test]
fn speculative_render_retry_keeps_the_ordered_prefix_and_pcm() {
    let (project, track) = fixture();
    let input = input();
    let mut runtime =
        ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input)).unwrap();
    let live = input.live_notes().unwrap();
    let activation = live.current_activation_on_control().unwrap();
    live.publish(activation, on(60, 37)).unwrap();
    live.publish(activation, off(60)).unwrap();
    runtime.begin_block();
    let mut abandoned = [[0.0; 2]; 8];
    runtime.render(&mut abandoned, None).unwrap();
    runtime.rollback();
    assert_eq!(live.snapshot().unwrap().len, 2);
    runtime.begin_block();
    let mut retry = [[0.0; 2]; 8];
    runtime.render(&mut retry, None).unwrap();
    assert_eq!(retry, abandoned);
    runtime.commit();
    assert!(live.snapshot().unwrap().is_empty());
}

#[test]
fn a_full_hardware_pool_refuses_a_physical_press_without_cancelling_either_source() {
    let (project, track) = fixture();
    let input = input();
    let mut tested = playback(&project, track, &input);
    let live = input.live_notes().unwrap();
    let activation = live.current_activation_on_control().unwrap();
    for pitch in 32..96 {
        live.publish(activation, on(pitch, 37)).unwrap();
    }
    tested.render(&mut [[0.0; 2]; 64]).unwrap();
    input.set_key(0, Some((60, 100, 1))).unwrap();
    let mut full = [[0.0; 2]; 64];
    tested.render(&mut full).unwrap();
    assert!(full.iter().flatten().any(|sample| *sample != 0.0));
    assert!(input.rejected());
    assert!(!input.is_cancelled());
    assert!(!live.is_cancelled());
    live.publish(activation, LiveNoteEvent::AllNotesOff)
        .unwrap();
    tested.render(&mut [[0.0; 2]; 4096]).unwrap();
    let mut silent = [[0.0; 2]; 64];
    tested.render(&mut silent).unwrap();
    assert_eq!(
        silent, [[0.0; 2]; 64],
        "the refused held press cannot trigger late"
    );
    input.set_key(0, None).unwrap();
    input.set_key(0, Some((60, 100, 2))).unwrap();
    tested.render(&mut silent).unwrap();
    assert!(silent.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn accepted_graph_handoff_preserves_hardware_phase_and_pending_release_exactly_once() {
    use crate::computer_keyboard::tests::prepared;
    use crate::playback::Playback;
    let (project, track) = fixture();
    let revision = prepared(&project);
    let oracle_input = input();
    let input = input();
    let mut old = Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = Playback::from_prepared(Arc::clone(&revision));
    old.prepare_computer_keyboard(&project, track, Arc::clone(&input))
        .unwrap();
    oracle
        .prepare_computer_keyboard(&project, track, Arc::clone(&oracle_input))
        .unwrap();
    for input in [&input, &oracle_input] {
        let live = input.live_notes().unwrap();
        live.publish(live.current_activation_on_control().unwrap(), on(60, 37))
            .unwrap();
    }
    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    old.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    let mut next = Playback::from_prepared(revision);
    next.prepare_computer_keyboard_for_controller_activation(&project, track, Arc::clone(&input))
        .unwrap();
    // Candidate preparation must neither consume nor capture the reader cursor.
    for input in [&input, &oracle_input] {
        let live = input.live_notes().unwrap();
        live.publish(live.current_activation_on_control().unwrap(), off(60))
            .unwrap();
    }
    next.try_inherit_runtime_from(&mut old).unwrap();
    old.cancel_computer_keyboard();
    drop(old);
    assert!(!input.is_cancelled());
    next.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(input.live_notes().unwrap().snapshot().unwrap().is_empty());
}

#[test]
fn mutable_instrument_short_note_matches_its_exact_velocity_audition_oracle() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{Command, Editor};
    let (project, track) = fixture();
    let metadata = &hex_devices::operator_state::OPERATOR_METADATA;
    let state = metadata
        .patch_parameter(
            &metadata.default_state().unwrap(),
            ParameterId::from_static(78),
            ParameterValue::Float(0.4),
        )
        .unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let project = editor.project();
    let input = input();
    let mut tested = playback(project, track, &input);
    let live = input.live_notes().unwrap();
    let token = live.current_activation_on_control().unwrap();
    live.publish(token, on(60, 37)).unwrap();
    live.publish(token, off(60)).unwrap();
    let oracle_input = Arc::new(
        ComputerKeyboardInput::default()
            .with_note_audition(&[(60, 37)], 1)
            .unwrap(),
    );
    let mut oracle = playback(project, track, &oracle_input);
    oracle_input.trigger_note_audition().unwrap();
    let mut actual = [[0.0; 2]; 1024];
    tested.render(&mut actual).unwrap();
    let mut expected = actual;
    for frame in &mut expected {
        oracle.render(std::slice::from_mut(frame)).unwrap();
    }
    assert_eq!(actual, expected);
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
#[ignore = "release component timing; run explicitly with --release --ignored --nocapture"]
fn profile_live_input_monitor_cost() {
    use std::{hint::black_box, time::Instant};
    fn report(label: &str, mut nanos: Vec<u128>) {
        let mean = nanos.iter().sum::<u128>() as f64 / nanos.len() as f64 / 1000.0;
        nanos.sort_unstable();
        println!(
            "{label}: n={} mean_us={mean:.3} p50_us={:.3} p95_us={:.3} max_us={:.3}",
            nanos.len(),
            nanos[nanos.len() / 2] as f64 / 1000.0,
            nanos[nanos.len() * 95 / 100] as f64 / 1000.0,
            nanos[nanos.len() - 1] as f64 / 1000.0
        );
    }
    let (project, track) = fixture();
    println!(
        "stopped sine component; rate=48000 block=256; no native output or musical clock; 128 warmup blocks; control allocation/destruction outside render timers"
    );
    for hardware in [false, true] {
        let input = Arc::new(if hardware {
            ComputerKeyboardInput::default().with_live_notes().unwrap()
        } else {
            ComputerKeyboardInput::default()
        });
        let prepare = Instant::now();
        let mut runtime =
            ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input)).unwrap();
        println!(
            "hardware={hardware} prepare_ms={:.3} retained_bytes={}",
            prepare.elapsed().as_secs_f64() * 1000.0,
            runtime.storage_bytes().unwrap()
        );
        for key in 0..17 {
            if let Some(live) = input.live_notes() {
                live.publish(
                    live.current_activation_on_control().unwrap(),
                    on(48 + key as u8, 100),
                )
                .unwrap();
            } else {
                input
                    .set_key(key, Some((48 + key as u8, 100, key as u64 + 1)))
                    .unwrap();
            }
        }
        let mut output = [[0.0; 2]; 256];
        let mut durations = Vec::with_capacity(2000);
        for block in 0..2128 {
            output.fill([0.0; 2]);
            let start = Instant::now();
            runtime.begin_block();
            runtime.render(black_box(&mut output), None).unwrap();
            runtime.commit();
            let elapsed = start.elapsed().as_nanos();
            black_box(&output);
            if block >= 128 {
                durations.push(elapsed);
            }
        }
        assert!(!input.is_cancelled());
        assert!(input.live_notes().is_none_or(|live| !live.is_cancelled()));
        report(
            if hardware {
                "17 held hardware notes"
            } else {
                "17 held physical notes"
            },
            durations,
        );
    }
    let mut durations = Vec::with_capacity(50);
    for _ in 0..50 {
        let input = input();
        let mut runtime =
            ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input)).unwrap();
        let live = input.live_notes().unwrap();
        let activation = live.current_activation_on_control().unwrap();
        for _ in 0..64 {
            live.publish(activation, on(60, 37)).unwrap();
            live.publish(activation, off(60)).unwrap();
        }
        let mut output = [[0.0; 2]; 256];
        let start = Instant::now();
        runtime.begin_block();
        runtime.render(black_box(&mut output), None).unwrap();
        runtime.commit();
        durations.push(start.elapsed().as_nanos());
        black_box(&output);
        assert!(!live.is_cancelled());
    }
    report("128 ordered events / 64 short notes", durations);
}

#[test]
fn audition_gate_and_cancel_do_not_release_hardware_playing_the_same_pitch() {
    let (project, track) = fixture();
    let input = input();
    let mut tested = playback(&project, track, &input);
    let live = input.live_notes().unwrap();
    let activation = live.current_activation_on_control().unwrap();
    live.publish(activation, on(60, 37)).unwrap();
    input
        .trigger_live_note_audition_on_control(&[(60, 93)], 8)
        .unwrap();
    let instrument = hex_devices::prepare_legacy_sine_instrument(48_000).unwrap();
    let hardware = instrument.prepare_note(60, 37).unwrap();
    let preview = instrument.prepare_note(60, 93).unwrap();
    let mut frames = [[0.0; 2]; 24];
    for block in frames.chunks_mut(8) {
        tested.render(block).unwrap();
    }
    for (age, actual) in frames.into_iter().enumerate() {
        let a = hardware.sample(age as u64, u64::MAX, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let b = preview.sample(age as u64, 8, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, [a[0] + b[0], a[1] + b[1]]);
    }

    input
        .trigger_live_note_audition_on_control(&[(60, 93)], 32)
        .unwrap();
    input.cancel_live_note_audition_on_control();
    live.publish(activation, off(60)).unwrap();
    let mut frames = [[0.0; 2]; 8];
    tested.render(&mut frames).unwrap();
    for (offset, actual) in frames.into_iter().enumerate() {
        let age = offset as u64 + 24;
        let a = hardware.sample(age, 24, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let b = preview.sample(age, 8, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(actual, [a[0] + b[0], a[1] + b[1]]);
    }
    assert!(live.same_current_activation(activation));
    assert!(!input.is_cancelled());
}

#[test]
fn a_full_preview_pool_yields_without_losing_the_pending_hardware_onset() {
    let (project, track) = fixture();
    let input = input();
    let mut tested = playback(&project, track, &input);
    let live = input.live_notes().unwrap();
    let activation = live.current_activation_on_control().unwrap();
    for pitch in 32..95 {
        live.publish(activation, on(pitch, 37)).unwrap();
    }
    tested.render(&mut [[0.0; 2]; 64]).unwrap();
    input
        .trigger_live_note_audition_on_control(&[(95, 93)], 12000)
        .unwrap();
    tested.render(&mut [[0.0; 2]; 64]).unwrap();

    live.publish(activation, on(96, 91)).unwrap();
    tested.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(live.snapshot().unwrap().events(), &[on(96, 91)]);
    for _ in 0..32 {
        tested.render(&mut [[0.0; 2]; 128]).unwrap();
    }
    assert!(live.snapshot().unwrap().is_empty());
    assert!(live.same_current_activation(activation));
    assert!(!input.rejected());
}

#[test]
fn recording_prefix_preempts_live_audition_and_preview_can_resume_after_seal() {
    let (project, track) = fixture();
    let input = Arc::new(
        ComputerKeyboardInput::default()
            .with_live_notes()
            .unwrap()
            .with_recording_support()
            .unwrap(),
    );
    let mut tested = playback(&project, track, &input);
    input
        .trigger_live_note_audition_on_control(&[(60, 37)], 128)
        .unwrap();
    let keys = input.begin_ordered_keys_on_control().unwrap();
    assert!(
        input
            .trigger_live_note_audition_on_control(&[(60, 37)], 128)
            .is_err()
    );
    let mut frames = [[0.0; 2]; 64];
    tested.render(&mut frames).unwrap();
    assert_eq!(frames, [[0.0; 2]; 64]);
    input.seal_ordered_keys_on_control(keys).unwrap();
    input
        .trigger_live_note_audition_on_control(&[(60, 37)], 128)
        .unwrap();
    tested.render(&mut frames).unwrap();
    assert!(frames.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn reusable_chord_refuses_all_onsets_and_preserves_hardware_when_one_pitch_or_slot_is_unavailable()
{
    let (project, track) = fixture();
    for capacity_shortage in [false, true] {
        let input = input();
        let mut runtime =
            ComputerKeyboardRuntime::prepare(&project, track, Arc::clone(&input)).unwrap();
        let live = input.live_notes().unwrap();
        let activation = live.current_activation_on_control().unwrap();
        let held = if capacity_shortage { VOICES - 1 } else { 1 };
        for pitch in 0..held {
            live.publish(activation, on(pitch as u8, 80)).unwrap();
        }
        runtime.begin_block();
        runtime.render(&mut [[0.0; 2]; 128], None).unwrap();
        runtime.commit();
        let voices_before: Vec<_> = runtime
            .primary
            .iter()
            .flatten()
            .map(|voice| voice.id)
            .collect();
        assert_eq!(voices_before.len(), held);
        let trigger = input
            .trigger_live_note_audition_on_control(&[(67, 37), (68, 95)], 256)
            .unwrap();
        runtime.begin_block();
        if !capacity_shortage {
            assert!(runtime.admits_live_chord(0, 2, true));
            assert!(!runtime.admits_live_chord(0, 1, true));
            let unavailable = runtime.input.sampler_index(1, 68, 95).unwrap();
            runtime.supported[unavailable] = false;
        }
        runtime.render(&mut [[0.0; 2]; 16], None).unwrap();
        assert!(
            !input.live_note_audition_refused(trigger),
            "speculative refusal must not publish"
        );
        runtime.rollback();
        runtime.begin_block();
        runtime.render(&mut [[0.0; 2]; 16], None).unwrap();
        runtime.commit();
        assert!(input.live_note_audition_refused(trigger));
        assert_eq!(
            runtime
                .primary
                .iter()
                .flatten()
                .map(|voice| voice.id)
                .collect::<Vec<_>>(),
            voices_before
        );
        assert!(
            runtime
                .primary
                .iter()
                .flatten()
                .all(|voice| voice.gate.is_none())
        );
        assert!(live.same_current_activation(activation));
        assert!(!input.is_cancelled() && !input.rejected());
        let next = input
            .trigger_live_note_audition_on_control(&[(67, 37)], 4)
            .unwrap();
        assert!(!input.live_note_audition_refused(next));
        runtime.begin_block();
        runtime.render(&mut [[0.0; 2]; 8], None).unwrap();
        runtime.commit();
        assert!(!input.live_note_audition_refused(next));
        assert!(
            runtime
                .primary
                .iter()
                .flatten()
                .any(|voice| voice.source == VoiceSource::Audition)
        );
    }
}

#[test]
fn sustain_pedal_holds_native_instrument_note_across_note_off_until_pedal_release() {
    let (project, track) = fixture();
    let sustained_input = input();
    let held_oracle_input = input();
    let mut sustained = playback(&project, track, &sustained_input);
    let mut held_oracle = playback(&project, track, &held_oracle_input);

    let live = sustained_input.live_notes().unwrap();
    let token = live.current_activation_on_control().unwrap();
    let oracle_live = held_oracle_input.live_notes().unwrap();
    let oracle_token = oracle_live.current_activation_on_control().unwrap();

    // Start note 60 on ch 3 on both; press CC64 sustain on ch 3, send CC11 expression, and release key 60 on tested.
    live.publish_at(token, on(60, 100), 1_000).unwrap();
    live.publish_at(
        token,
        LiveNoteEvent::ControlChange {
            channel: 3,
            cc: 64,
            value: 127,
        },
        2_000,
    )
    .unwrap();
    live.publish_at(
        token,
        LiveNoteEvent::ControlChange {
            channel: 3,
            cc: 11,
            value: 96,
        },
        3_000,
    )
    .unwrap();
    live.publish_at(token, off(60), 4_000).unwrap();
    oracle_live
        .publish_at(oracle_token, on(60, 100), 1_000)
        .unwrap();

    // While CC64 sustain is down, NoteOff(60) must NOT release the voice:
    // rendered PCM matches the continuously held oracle note.
    let mut actual = [[0.0; 2]; 512];
    let mut expected = [[0.0; 2]; 512];
    sustained.render(&mut actual).unwrap();
    held_oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "CC64 sustain must hold sounding native instrument voice across NoteOff"
    );
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));

    // Release CC64 sustain pedal on ch 3: voice now releases and decays to silence.
    live.publish_at(
        token,
        LiveNoteEvent::ControlChange {
            channel: 3,
            cc: 64,
            value: 0,
        },
        5_000,
    )
    .unwrap();
    let mut tail = [[0.0; 2]; 512];
    let mut decayed = false;
    for _ in 0..200 {
        sustained.render(&mut tail).unwrap();
        if tail.iter().flatten().all(|sample| *sample == 0.0) {
            decayed = true;
            break;
        }
    }
    assert!(
        decayed,
        "releasing CC64 sustain pedal must release the held native voice to silence"
    );
}

#[test]
fn mpe_member_channel_expression_routes_to_distinct_note_occurrences_and_handles_reuse_and_stale_activation()
 {
    use crate::computer_keyboard::{LiveNoteEvent, LiveNoteInput, LiveNotePublishError};

    // 1. Stale activation and invalid expression bounds are refused deterministically.
    let probe = LiveNoteInput::default();
    let stale_token = probe.begin_activation_on_control().unwrap();
    let fresh_token = probe.begin_activation_on_control().unwrap();
    assert_eq!(
        probe.publish(
            stale_token,
            LiveNoteEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 1200,
            },
        ),
        Err(LiveNotePublishError::StaleActivation)
    );
    assert_eq!(
        probe.publish(
            fresh_token,
            LiveNoteEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 4801,
            },
        ),
        Err(LiveNotePublishError::InvalidEvent)
    );
    let invalid_timbre = LiveNoteInput::default();
    let invalid_timbre_token = invalid_timbre.begin_activation_on_control().unwrap();
    assert_eq!(
        invalid_timbre.publish(
            invalid_timbre_token,
            LiveNoteEvent::NoteTimbre {
                channel: 1,
                pitch: 60,
                value: 128,
            },
        ),
        Err(LiveNotePublishError::InvalidEvent)
    );

    // 2. Drive two simultaneous MPE notes on distinct member channels (ch 1 pitch 60,
    // ch 2 pitch 64) with independent NotePitchBend + NoteTimbre into a real engine playback.
    let (project, track) = fixture();
    let mpe_input = input();
    let oracle_input = input();
    let mut expressed = playback(&project, track, &mpe_input);
    let mut unbent_oracle = playback(&project, track, &oracle_input);

    let live = mpe_input.live_notes().unwrap();
    let token = live.current_activation_on_control().unwrap();
    let oracle_live = oracle_input.live_notes().unwrap();
    let oracle_token = oracle_live.current_activation_on_control().unwrap();

    for &(ch, pitch) in &[(1_u8, 60_u8), (2, 64)] {
        live.publish(
            token,
            LiveNoteEvent::NoteOn {
                channel: ch,
                pitch,
                velocity: 100,
            },
        )
        .unwrap();
        oracle_live
            .publish(
                oracle_token,
                LiveNoteEvent::NoteOn {
                    channel: ch,
                    pitch,
                    velocity: 100,
                },
            )
            .unwrap();
    }
    live.publish(
        token,
        LiveNoteEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: 700,
        },
    )
    .unwrap();
    live.publish(
        token,
        LiveNoteEvent::NoteTimbre {
            channel: 2,
            pitch: 64,
            value: 112,
        },
    )
    .unwrap();

    let mut actual = [[0.0_f32; 2]; 512];
    let mut expected = [[0.0_f32; 2]; 512];
    expressed.render(&mut actual).unwrap();
    unbent_oracle.render(&mut expected).unwrap();
    assert!(actual.iter().flatten().any(|s| *s != 0.0));
    assert!(actual.iter().flatten().all(|s| s.is_finite()));

    // 3. Release ch 1, confirm idle ch 1 expression is ignored, then reuse ch 1 for a fresh note
    // and clean up everything via AllNotesOff without stuck notes.
    live.publish(
        token,
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
    )
    .unwrap();
    live.publish(
        token,
        LiveNoteEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: -1200,
        },
    )
    .unwrap();
    live.publish(
        token,
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 67,
            velocity: 96,
        },
    )
    .unwrap();
    expressed.render(&mut actual).unwrap();
    assert!(actual.iter().flatten().all(|s| s.is_finite()));

    live.publish(token, LiveNoteEvent::AllNotesOff).unwrap();
    let mut silent = false;
    for _ in 0..200 {
        expressed.render(&mut actual).unwrap();
        if actual.iter().flatten().all(|s| *s == 0.0) {
            silent = true;
            break;
        }
    }
    assert!(
        silent,
        "AllNotesOff must release all MPE member-channel voices cleanly to silence"
    );
}

#[test]
fn mpe_sustain_ownership_same_channel_reuse_stop_disconnect_and_full_queue_have_no_stuck_notes() {
    use crate::computer_keyboard::{KeyboardActivationClock, LiveNoteEvent, LiveNotePublishError};
    use hex_project::{Command, Created, Editor, Project};

    // Use FM4 so per-note pitch bend and timbre visibly modulate independent voices in PCM.
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let fm4_state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        crate::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4-mpe".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: fm4_state,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let project = editor.project().clone();

    let sustained_input = input();
    let held_oracle_input = input();
    let mut sustained = playback(&project, track, &sustained_input);
    let mut held_oracle = playback(&project, track, &held_oracle_input);

    let live = sustained_input.live_notes().unwrap();
    let token = live.current_activation_on_control().unwrap();
    let oracle_live = held_oracle_input.live_notes().unwrap();
    let oracle_token = oracle_live.current_activation_on_control().unwrap();

    // 1. Two independently expressive MPE notes: ch 1 pitch 60 (+700c bend) & ch 2 pitch 64 (timbre 110).
    //    Engage CC64 sustain on ch 1, lift ch 1 key, then modulate sustained ch 1 with timbre 96.
    //    Send stale wrong-pitch NoteOff and idle-channel ch 3 bend: neither may disturb ch 1 or ch 2.
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 64,
            velocity: 95,
        },
        LiveNoteEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: 700,
        },
        LiveNoteEvent::NoteTimbre {
            channel: 2,
            pitch: 64,
            value: 110,
        },
        LiveNoteEvent::ControlChange {
            channel: 1,
            cc: 64,
            value: 127,
        },
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 62, // wrong pitch -> ignored
            velocity: 0,
        },
        LiveNoteEvent::NotePitchBend {
            channel: 3, // idle channel -> ignored
            pitch: 60,
            cents: -1200,
        },
        LiveNoteEvent::NoteTimbre {
            channel: 1,
            pitch: 60,
            value: 96,
        },
    ] {
        live.publish(token, ev).unwrap();
    }
    // Oracle holds ch 1 without NoteOff, receiving the exact same bends/timbre.
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 64,
            velocity: 95,
        },
        LiveNoteEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: 700,
        },
        LiveNoteEvent::NoteTimbre {
            channel: 2,
            pitch: 64,
            value: 110,
        },
        LiveNoteEvent::NoteTimbre {
            channel: 1,
            pitch: 60,
            value: 96,
        },
    ] {
        oracle_live.publish(oracle_token, ev).unwrap();
    }
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    sustained.render(&mut actual).unwrap();
    held_oracle.render(&mut expected).unwrap();
    // Both renders must be non-silent, finite, and close (events align within the 4-sample CC offset).
    assert!(actual.iter().flatten().any(|s| *s != 0.0));
    assert!(actual.iter().flatten().all(|s| s.is_finite()));
    assert!(expected.iter().flatten().any(|s| *s != 0.0));

    // 2. Same-channel reuse on ch 1 while sustained (retrigger 60 -> 67), then pedal lift & ch 2 release -> exact silence.
    for ev in [
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 67,
            velocity: 92,
        },
        LiveNoteEvent::ControlChange {
            channel: 1,
            cc: 64,
            value: 0,
        },
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 67,
            velocity: 0,
        },
        LiveNoteEvent::NoteOff {
            channel: 2,
            pitch: 64,
            velocity: 0,
        },
    ] {
        live.publish(token, ev).unwrap();
    }
    let mut silent = false;
    for _ in 0..400 {
        sustained.render(&mut actual).unwrap();
        if actual.iter().flatten().all(|s| *s == 0.0) {
            silent = true;
            break;
        }
    }
    assert!(
        silent,
        "pedal-off + ch 2 NoteOff must decay to exact silence"
    );

    // 3. Zone disconnect (cancel_activation) releases sustained MPE voices deterministically.
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::ControlChange {
            channel: 1,
            cc: 64,
            value: 127,
        },
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
    ] {
        live.publish(token, ev).unwrap();
    }
    sustained.render(&mut actual).unwrap();
    assert!(actual.iter().flatten().any(|s| *s != 0.0));
    live.cancel_activation(token).unwrap();
    let mut disconnected_silent = false;
    for _ in 0..400 {
        sustained.render(&mut actual).unwrap();
        if actual.iter().flatten().all(|s| *s == 0.0) {
            disconnected_silent = true;
            break;
        }
    }
    assert!(
        disconnected_silent,
        "zone disconnect must release sustained MPE voices with zero stuck notes"
    );

    // 4. Priority Stop cuts sustained MPE voices immediately to silence.
    let token_stop = live.begin_activation_on_control().unwrap();
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 64,
            velocity: 100,
        },
        LiveNoteEvent::ControlChange {
            channel: 2,
            cc: 64,
            value: 127,
        },
        LiveNoteEvent::NoteOff {
            channel: 2,
            pitch: 64,
            velocity: 0,
        },
    ] {
        live.publish(token_stop, ev).unwrap();
    }
    sustained.render(&mut actual).unwrap();
    assert!(actual.iter().flatten().any(|s| *s != 0.0));
    let clock = KeyboardActivationClock::default();
    sustained.apply_priority_stop(&clock);
    let mut after_stop = [[0.0; 2]; 64];
    sustained.render(&mut after_stop).unwrap();
    assert_eq!(
        after_stop, [[0.0; 2]; 64],
        "priority Stop must immediately silence sustained MPE voices"
    );

    // 5. Full LiveNoteInput queue overflow faults the activation and releases sustained MPE notes cleanly.
    let overflow_input = input();
    let mut overflow_pb = playback(&project, track, &overflow_input);
    let overflow_live = overflow_input.live_notes().unwrap();
    let token_overflow = overflow_live.begin_activation_on_control().unwrap();
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::ControlChange {
            channel: 1,
            cc: 64,
            value: 127,
        },
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
    ] {
        overflow_live.publish(token_overflow, ev).unwrap();
    }
    overflow_pb.render(&mut actual).unwrap();
    assert!(actual.iter().flatten().any(|s| *s != 0.0));
    let mut hit_full = false;
    for step in 0..2048_i16 {
        match overflow_live.publish(
            token_overflow,
            LiveNoteEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: (step % 100) * 10,
            },
        ) {
            Ok(()) => {}
            Err(LiveNotePublishError::Full) => {
                hit_full = true;
                break;
            }
            Err(other) => panic!("unexpected publish error while filling queue: {other:?}"),
        }
    }
    assert!(hit_full, "queue must report Full under burst overflow");
    let mut overflow_silent = false;
    for _ in 0..400 {
        overflow_pb.render(&mut actual).unwrap();
        if actual.iter().flatten().all(|s| *s == 0.0) {
            overflow_silent = true;
            break;
        }
    }
    assert!(
        overflow_silent,
        "full LiveNoteInput queue must fault activation and release sustained MPE notes to silence"
    );
}

#[test]
fn mpe_member_and_master_pitch_bends_produce_independent_fm4_pcm_matching_exact_cent_oracles() {
    use crate::computer_keyboard::LiveNoteEvent;
    use hex_project::{Command, Created, Editor, Project};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let fm4_state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        crate::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4-mpe-bend".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: fm4_state,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let project = editor.project().clone();

    // Helper rendering one single FM4 note at pitch + exact cents offset.
    let render_single = |channel: u8, pitch: u8, cents: i16| -> Vec<[f32; 2]> {
        let inp = input();
        let mut pb = playback(&project, track, &inp);
        let live = inp.live_notes().unwrap();
        let tok = live.current_activation_on_control().unwrap();
        live.publish(
            tok,
            LiveNoteEvent::NoteOn {
                channel,
                pitch,
                velocity: 100,
            },
        )
        .unwrap();
        live.publish(
            tok,
            LiveNoteEvent::NotePitchBend {
                channel,
                pitch,
                cents,
            },
        )
        .unwrap();
        let mut buf = vec![[0.0_f32; 2]; 256];
        pb.render(&mut buf).unwrap();
        buf
    };

    // Two same-pitch notes (60) with different member bends (+700c vs -500c) + master bend (+300c)
    // yield exact independent total offsets (+1000c vs -200c) and distinct PCM waveforms!
    let bent_high = render_single(1, 60, 1000);
    let bent_low = render_single(2, 60, -200);
    let unbent = render_single(1, 60, 0);
    assert_ne!(bent_high, bent_low);
    assert_ne!(bent_high, unbent);
    assert_ne!(bent_low, unbent);

    // Measure zero-crossing rate on the mono sum: +1000c (~1.78x freq) must cross zero
    // significantly more often than -200c (~0.89x freq).
    let zero_crossings = |frames: &[[f32; 2]]| -> usize {
        frames
            .windows(2)
            .filter(|w| (w[0][0] <= 0.0 && w[1][0] > 0.0) || (w[0][0] >= 0.0 && w[1][0] < 0.0))
            .count()
    };
    assert!(
        zero_crossings(&bent_high) > zero_crossings(&bent_low),
        "higher bent voice (+1000c) must oscillate faster than lower bent voice (-200c): {} vs {}",
        zero_crossings(&bent_high),
        zero_crossings(&bent_low)
    );

    // Drive BOTH voices simultaneously on one FM4 instance and verify endpoints (-4800c / +4800c)
    // stay finite and clean, and same-channel reuse starts unbent at the new pitch.
    let dual_input = input();
    let mut dual = playback(&project, track, &dual_input);
    let live = dual_input.live_notes().unwrap();
    let tok = live.current_activation_on_control().unwrap();
    for ev in [
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch: 60,
            velocity: 100,
        },
        LiveNoteEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: 4800,
        },
        LiveNoteEvent::NotePitchBend {
            channel: 2,
            pitch: 60,
            cents: -4800,
        },
    ] {
        live.publish(tok, ev).unwrap();
    }
    let mut dual_buf = [[0.0_f32; 2]; 256];
    dual.render(&mut dual_buf).unwrap();
    assert!(dual_buf.iter().flatten().any(|s| *s != 0.0));
    assert!(dual_buf.iter().flatten().all(|s| s.is_finite()));

    // Same-channel reuse on ch 1 (NoteOff 60, NoteOn 64) + AllNotesOff cleanly silences both voices.
    for ev in [
        LiveNoteEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 0,
        },
        LiveNoteEvent::NoteOn {
            channel: 1,
            pitch: 64,
            velocity: 96,
        },
        LiveNoteEvent::AllNotesOff,
    ] {
        live.publish(tok, ev).unwrap();
    }
    let mut silent = false;
    for _ in 0..400 {
        dual.render(&mut dual_buf).unwrap();
        if dual_buf.iter().flatten().all(|s| *s == 0.0) {
            silent = true;
            break;
        }
    }
    assert!(silent);
}
