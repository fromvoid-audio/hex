//! The two real cohorts share Root and the host acceptance frontier.
use super::*;
use crate::prepared::input_monitor::MonitorStatus;
use hex_engine::{PausedMonitorForeignBinding, PausedTrackMonitor};
use hex_lattice::{PreparedForeignProcessor, PreparedProcessorFactory};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn paused_notes_use_original_generator_and_private_effect_after_host_acceptance() {
    const DRIVER: &str = "HEX_ORDINARY_PAUSED_NOTE_BANKS";
    let outcome = std::env::var(DRIVER);
    if outcome.is_err() {
        for case in ["resume", "reject"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                ])
                .env(DRIVER, case)
                .env("RUST_MIN_STACK", "16777216")
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
                .env("HEX_ORIGINAL_TAIL_INSTANCE_VARIANT", "0")
                .output()
                .unwrap();
            assert!(
                result.status.success()
                    && String::from_utf8_lossy(&result.stdout).contains("1 passed"),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    }
    use hex_engine::{PausedSourceMonitor, computer_keyboard::ComputerKeyboardInput};
    const Q: usize = 32;
    let _serial = serial();
    let mut f = fixture_with_authored_mode_and_capacity("processing", 0, Q as u16, &[], true, 3);
    let track = f.editor.project().tracks()[0].id();
    let original_instrument = f.binding;
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Main,
        f.editor.project().next_identity(),
    )
    .unwrap();
    let original = source_for_profile(&mut f.control, identity, "tail", Q as u16, None);
    let state = f
        .control
        .native_source_prepared_state(original)
        .unwrap_or_else(|| {
            panic!(
                "effect state: {:?}, {:?}",
                f.control.native_source_preparation_progress(original),
                f.control.try_native_source_preparation_error(original)
            )
        });
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })
        .unwrap();
    let authored = f.editor.project().tracks()[0].midi_clips()[0].notes()[0].id();
    f.editor
        .execute(Command::UpdateMidiNote {
            note: authored,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let Created::Imported { asset, .. } = f
        .editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "marker".into(),
            path: "pause-marker.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("Arrangement marker")
    };
    f.media.insert(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap(),
    );
    f.control.publish_native_source_endpoint(original).unwrap();
    physical_block::<Q>(&mut f);
    let mut playback = compile_with_bindings(&f, &[original_instrument, original]);
    let prepared = Arc::clone(playback.prepared_revision().unwrap());
    let keys = Arc::new(ComputerKeyboardInput::default());
    playback
        .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(&keys))
        .unwrap();
    assert!(playback.defer_hosted_paused_source_monitor_on_control());
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    assert_eq!(physical_block::<Q>(&mut f), [[0.0; 2]; Q]);
    assert!(f.control.status.paused_input_source().is_some());
    assert!(f.control.status.paused_note_source().is_none());
    let root = f.control.native_candidate_root();
    let committed = |f: &mut OrdinaryFixture, binding| {
        let mut result = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            result = callback
                .native_sources
                .test_committed_frames(binding)
                .or_else(|| {
                    callback
                        .paused_native_sources
                        .test_committed_frames(binding)
                });
        });
        result.unwrap()
    };
    let render = |f: &mut OrdinaryFixture, private: Option<Binding>| {
        let output = physical_block::<Q>(f);
        for binding in [Some(original_instrument), Some(original), private]
            .into_iter()
            .flatten()
        {
            replacement::wait_for_blocks(&root, binding, committed(f, binding) / Q as u64);
        }
        output
    };
    f.control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..12 {
        render(&mut f, None);
    }
    f.control.command(PlaybackCommand::Pause).unwrap();
    for _ in 0..4 {
        render(&mut f, None);
    }
    let (source, epoch) = f.control.status.paused_note_source().unwrap();
    let position = f.control.status.position.load(Acquire);
    let frozen_effect = committed(&mut f, original);
    assert!(position > 0 && frozen_effect > 0);
    assert_eq!(render(&mut f, None), [[0.0; 2]; Q]);
    assert_eq!(committed(&mut f, original), frozen_effect);

    let private = source_for_profile(&mut f.control, identity, "tail", Q as u16, Some(state));
    let original_facts = PausedTrackMonitor::foreign_effect_nodes_on_control(&prepared)
        .next()
        .unwrap()
        .foreign_processor()
        .unwrap();
    let prepare_monitor = |resources: &PreparedDeviceResources| {
        PausedSourceMonitor::prepare_admitted_on_control(
            Arc::clone(&prepared),
            Some(resources),
            &[PausedMonitorForeignBinding {
                original,
                replacement: original_facts.with_binding(private).unwrap(),
            }],
            4096,
            |_| Ok::<_, ()>(()),
        )
        .unwrap()
    };
    let (missing, missing_hold) = prepare_monitor(&f.resources);
    let missing = f
        .control
        .publish_paused_note_monitor(missing, source, epoch + 1)
        .expect_err("a stale transport epoch returns the owner");
    assert!(!missing_hold.released_on_control());
    assert!(
        f.control
            .publish_paused_note_monitor(missing, source, epoch)
            .is_ok()
    );
    let (monitor, hold) = prepare_monitor(&f.resources);
    let monitor = f
        .control
        .publish_paused_note_monitor(monitor, source, epoch)
        .expect_err("the bounded worker queue returns a second owner");
    assert!(!hold.released_on_control());
    assert_eq!(render(&mut f, None), [[0.0; 2]; Q]);
    assert_eq!(
        f.control.paused_note_monitor_status(),
        MonitorStatus::Unsupported
    );
    render(&mut f, None);
    f.control.reclaim();
    assert!(missing_hold.released_on_control());

    f.control
        .publish_paused_monitor_native_source_endpoint(private, source, epoch)
        .unwrap();
    assert!(
        f.control
            .publish_paused_note_monitor(monitor, source, epoch)
            .is_ok()
    );
    let generator_before = committed(&mut f, original_instrument);
    // Drain the authored note-off through the original instrument and the
    // private effect. Neither may expose the stereo Arrangement marker.
    for _ in 0..7 {
        assert!(
            render(&mut f, Some(private))
                .iter()
                .all(|frame| frame[0] == frame[1])
        );
    }
    keys.set_key(0, Some((60, 100, 1))).unwrap();
    assert!((0..8).any(|_| {
        render(&mut f, Some(private))
            .iter()
            .any(|frame| frame[0].abs() > 0.001)
    }));
    assert_eq!(
        f.control.paused_note_monitor_status(),
        MonitorStatus::Running
    );
    assert_eq!(f.control.input_monitor.status(), MonitorStatus::Idle);
    assert_eq!(f.control.status.position.load(Acquire), position);
    assert_eq!(committed(&mut f, original), frozen_effect);
    assert!(committed(&mut f, original_instrument) > generator_before);
    let private_before = committed(&mut f, private);
    keys.set_key(0, None).unwrap();
    if outcome.as_deref() == Ok("reject") {
        let generator = committed(&mut f, original_instrument);
        let mut output = [[0.0; 2]; Q];
        let mut host = [1.0_f32; Q * 2];
        allocation_free(|| {
            f.data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(Q).unwrap();
                let rendered = callback.render(&mut output);
                assert!(rendered.is_ok());
                output[0][0] = f32::NAN;
                crate::device::finish_host_chunk(
                    callback,
                    &mut host,
                    &mut output,
                    2,
                    &rendered,
                    0,
                    Instant::now(),
                );
            })
        });
        assert_eq!(host, [0.0; Q * 2]);
        assert_eq!(committed(&mut f, original_instrument), generator);
        assert_eq!(committed(&mut f, private), private_before);
        assert_eq!(
            f.control.paused_note_monitor_status(),
            MonitorStatus::OutputFault
        );
    } else {
        for _ in 0..8 {
            render(&mut f, Some(private));
        }
        assert_eq!(render(&mut f, Some(private)), [[0.0; 2]; Q]);
        f.control.command(PlaybackCommand::Play).unwrap();
        assert!((0..5).any(|_| {
            render(&mut f, Some(private))
                .iter()
                .any(|frame| frame[0] != frame[1])
        }));
        assert!(committed(&mut f, original) > frozen_effect);
    }
    physical_block::<Q>(&mut f);
    f.control.reclaim();
    assert!(hold.released_on_control());

    let (closing, closing_hold) = prepare_monitor(&f.resources);
    f.control.request_shutdown();
    assert!(f.control.status.paused_note_source().is_none());
    let closing = f
        .control
        .publish_paused_note_monitor(closing, source, epoch)
        .expect_err("a closed output returns the monitor owner");
    assert!(!closing_hold.released_on_control());
    drop(closing);
    assert!(closing_hold.released_on_control());
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    for binding in [original_instrument, original, private] {
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    }
    drop(control);
    finish(&mut retirement);
}

fn processor(playback: &Playback) -> PreparedForeignProcessor {
    let mut nodes = playback.foreign_nodes_on_control();
    let Some(PreparedProcessorFactory::Foreign(processor)) = nodes.next().unwrap().definition
    else {
        panic!("prepared effect")
    };
    assert!(nodes.next().is_none());
    processor
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn paused_monitor_uses_private_native_endpoint_and_contains_host_refusal() {
    run_native_monitor_fixture(exercise_paused_monitor_host_refusal);
}

// Both prepared processor banks and their callback carrier exceed the default
// test stack. Size only the thread that runs the signed fixture.
fn run_native_monitor_fixture(exercise: fn()) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(exercise)
        .unwrap()
        .join()
        .unwrap();
}

fn exercise_paused_monitor_host_refusal() {
    let _serial = serial();
    let mut f = fixture_at("processing", 0);
    let root = f.control.native_candidate_root();
    let playback = compile(&f, true);
    let prepared = Arc::clone(playback.prepared_revision().unwrap());
    let track = f.editor.project().tracks()[0].id();
    let original = processor(&playback).binding().unwrap();
    assert_eq!(f.binding, original);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
    let (source, epoch) = f.control.status.paused_input_source().unwrap();

    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        original.instance,
    )
    .unwrap();
    let state = f.control.native_source_prepared_state(original).unwrap();
    let private = source_for_profile(&mut f.control, identity, "processing", 8, Some(state));
    assert_ne!(original, private);
    f.binding = private;
    let private_processor = processor(&compile(&f, true));
    f.binding = original;
    let (monitor, hold) = PausedTrackMonitor::prepare_admitted_with_foreign_effects_on_control(
        prepared,
        Some(&f.resources),
        track,
        &[PausedMonitorForeignBinding {
            original,
            replacement: private_processor,
        }],
        4096,
        |_| Ok::<_, ()>(()),
    )
    .unwrap();
    assert!(
        f.control
            .publish_paused_monitor_native_source_endpoint(private, source, epoch + 1)
            .is_err()
    );
    f.control
        .publish_paused_monitor_native_source_endpoint(private, source, epoch)
        .unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });

    let (mut input, stream) = crate::input_capture::test_monitor_source();
    assert_eq!(input.submit(&[0.25; 512]), 2);
    assert!(
        f.control
            .input_monitor
            .publish_paused(stream, monitor, source, epoch)
            .is_ok()
    );
    for completed in 1..=3 {
        let output = physical_block::<8>(&mut f);
        replacement::wait_for_blocks(&root, private, completed);
        let expected = if completed <= 2 { [0.0; 2] } else { [0.125; 2] };
        assert_eq!(output, [expected; 8]);
    }
    assert_eq!(
        f.control.status.paused_input_source(),
        Some((source, epoch))
    );
    assert_eq!(f.control.input_monitor.status(), MonitorStatus::Running);
    assert_eq!(root.native_source_completed_blocks(original), Some(0));

    f.control.input_monitor.release_paused();
    assert_eq!(physical_block::<8>(&mut f), [[0.15625; 2]; 8]);
    replacement::wait_for_blocks(&root, private, 4);
    // The host sees invalid output after the monitor has staged its next
    // segment. The copied output is silenced without dispatching that input.
    let mut rejected = [[0.0; 2]; 8];
    let mut host = [1.0_f32; 16];
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.begin_native_effect_callback(8).unwrap();
            let result = callback.render(&mut rejected);
            assert!(result.is_ok() && rejected.iter().any(|frame| frame[0] > 0.0));
            rejected[0][0] = f32::NAN;
            assert!(crate::device::finish_host_chunk(
                callback,
                &mut host,
                &mut rejected,
                2,
                &result,
                0,
                Instant::now()
            ));
            assert!(!callback.native_effects_revoked());
        })
    });
    assert_eq!(host, [0.0; 16]);
    assert_eq!(f.control.input_monitor.status(), MonitorStatus::OutputFault);
    assert_eq!(root.native_source_completed_blocks(private), Some(4));
    assert!(f.control.retire_native_source_preparation(private));
    assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
    f.control.reclaim();
    assert!(f.control.input_monitor.reclaim().is_some());
    assert!(hold.released_on_control());
    let stopped = recover(&mut f.control, private);
    assert!(stopped.error.is_none() && stopped.cleanup_error.is_none());
    assert!(
        matches!(stopped.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some_and(|terminal| terminal.blocks == 4))
    );

    f.control.command(PlaybackCommand::Play).unwrap();
    for completed in 1..=3 {
        let output = physical_block::<8>(&mut f);
        replacement::wait_for_blocks(&root, original, completed);
        let expected = if completed <= 2 {
            [0.0; 2]
        } else {
            [0.125, -0.25]
        };
        assert_eq!(output, [expected; 8]);
    }
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    let returned = recover(&mut control, original);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    drop(control);
    finish(&mut retirement);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn shutdown_returns_both_native_banks_through_the_same_root() {
    run_native_monitor_fixture(exercise_dual_bank_shutdown);
}

fn exercise_dual_bank_shutdown() {
    let _serial = serial();
    let mut f = fixture_at("processing", 0);
    let playback = compile(&f, true);
    let original = f.binding;
    let track = f.editor.project().tracks()[0].id();
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    assert_eq!(physical_block::<8>(&mut f), [[0.0; 2]; 8]);
    let (source, epoch) = f.control.status.paused_input_source().unwrap();
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        original.instance,
    )
    .unwrap();
    let state = f.control.native_source_prepared_state(original).unwrap();
    let private = source_for_profile(&mut f.control, identity, "processing", 8, Some(state));
    f.control
        .publish_paused_monitor_native_source_endpoint(private, source, epoch)
        .unwrap();
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.native_sources.has_endpoints());
            assert!(callback.paused_native_sources.has_endpoints());
        })
    });
    let OrdinaryFixture {
        mut control,
        data,
        errors,
        retirement,
        ..
    } = f;
    let mut retirement = close_backend(data, errors, retirement);
    for binding in [original, private] {
        let returned = recover(&mut control, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some_and(|terminal| terminal.blocks == 0))
        );
    }
    drop(control);
    finish(&mut retirement);
}
