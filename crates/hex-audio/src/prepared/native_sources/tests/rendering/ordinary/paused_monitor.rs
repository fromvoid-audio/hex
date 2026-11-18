//! The two real cohorts share Root and the host acceptance frontier.
use super::*;
use crate::prepared::input_monitor::MonitorStatus;
use hex_engine::{PausedMonitorForeignBinding, PausedTrackMonitor};
use hex_lattice::{PreparedForeignProcessor, PreparedProcessorFactory};

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
