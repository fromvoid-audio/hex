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
