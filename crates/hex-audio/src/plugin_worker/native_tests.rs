//! Original SDK source/worker custody; no app graph or physical device claim.
use super::*;

fn original_instance() -> hex_plugin_host::native::Prepared {
    original_instance_for_binding(Binding {
        root: 1,
        instance: 2,
        graph: 3,
        epoch: 4,
    })
}

pub(crate) fn original_instance_for_binding(binding: Binding) -> hex_plugin_host::native::Prepared {
    use hex_plugin_host::{native::*, protocol::PresentationLatency};
    use hex_plugin_scan::macos::build_identity::InstanceBuild;
    use hex_plugin_state::ClassId;
    use std::path::PathBuf;
    let receipt = PathBuf::from(std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap());
    let fixtures = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let build = InstanceBuild::inspect(&receipt).unwrap();
    let source = Source::inspect_on_control(
        &fixtures
            .parent()
            .unwrap()
            .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3"),
        "Contents/MacOS/fixture",
        ClassId::new([
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ])
        .unwrap(),
    )
    .unwrap();
    let presentation = PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    };
    Prepared::prepare_on_control(
        &build,
        source,
        Options {
            preparation: Preparation {
                binding,
                sample_rate: 48000.0,
                quantum: 8,
                input_channels: 2,
                output_channels: 2,
                limits: Limits {
                    startup: Duration::from_secs(3),
                    exchange: Duration::from_secs(1),
                    retirement: Duration::from_secs(3),
                },
            },
            presentation,
        },
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_retirement_preserves_failure_and_requires_observed_reap() {
    for cancelled in 0..3 {
        let instance = original_instance();
        let cancel = instance.cancellation_on_control();
        let (mut worker, mut callback) = NativeWorker::start_on_control(instance).unwrap();
        if cancelled == 0 {
            for block in 0..4 {
                callback
                    .stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8])
                    .unwrap();
                callback.finish(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while worker.published_blocks_on_control() < block + 1 {
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
            }
        } else {
            cancel.cancel_on_control();
            if cancelled == 2 {
                // Exercise an actual worker-side native error as well as
                // cancellation observed only by the joining stop operation.
                callback
                    .stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8])
                    .unwrap();
                callback.finish(true).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while !worker.reaper.is_finished() {
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
            }
        }
        let retired = worker.retire_on_control().unwrap();
        if cancelled == 0 {
            assert!(retired.failure.is_none());
            assert!(retired.native.status.success());
            let terminal = retired.native.terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (4, 32));
            assert!(terminal.closed && terminal.unloaded);
        } else {
            assert!(matches!(
                retired.failure,
                Some(hex_plugin_host::Error::Cancelled)
            ));
            assert!(retired.native.terminal.is_none());
            assert_eq!(worker.published_blocks_on_control(), 0);
        }
        assert!(worker.retire_on_control().is_err());
        assert!(worker.quiesce_on_control().is_err());
        assert!(
            callback
                .stage(&[[1.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .is_err(),
            "actual native retirement never grants another renderable endpoint"
        );
        // This still-live callback carrier has not returned merely because
        // native retirement completed. Its matching worker retains the queues.
        assert!(worker.owns_callback(&callback));
        drop(callback);
        drop(worker);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_returns_same_instance_for_fresh_epoch() {
    let mut instance = original_instance();
    let presentation = hex_plugin_host::protocol::PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    };
    let child = instance.child_id_on_control(); // diagnostic; typed lease retains custody
    for epoch in 0..2_u64 {
        let binding = instance.configuration().binding;
        let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
        let (_, unrelated, _) = channel(binding, 8).unwrap();
        assert!(owner.owns_callback(&callback));
        assert!(!owner.owns_callback(&unrelated));
        for n in 0..4_u64 {
            let mut output = [[9.0; 2]; 8];
            callback
                .stage(&[[0.5, -0.25]; 8], &[], &mut output)
                .unwrap();
            callback.finish(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while owner.published_blocks_on_control() < epoch * 4 + n + 1 {
                assert!(std::time::Instant::now() < deadline);
                thread::yield_now();
            }
            let gain = 0.5 + (epoch * 4 + n.saturating_sub(2)) as f32 * 0.125;
            assert_eq!(
                output,
                if n < 2 {
                    [[0.0; 2]; 8]
                } else {
                    [[0.5 * gain, -0.25 * gain]; 8]
                }
            );
        }
        let stopped = owner.quiesce_on_control().unwrap();
        assert_eq!(stopped.completed_blocks(), (epoch + 1) * 4);
        assert_eq!(stopped.child_id_on_control(), child);
        assert!(owner.quiesce_on_control().is_err());
        assert!(
            callback
                .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
                .is_err()
        );
        drop(callback); // actual callback endpoint returned before old control rings
        drop(owner);
        instance = stopped
            .resume_on_control(
                Binding {
                    graph: binding.graph + 1,
                    epoch: binding.epoch + 1,
                    ..binding
                },
                Some(presentation),
            )
            .unwrap();
    }
    let retired = instance.stop_and_reap_on_control().unwrap();
    assert!(retired.status.success());
    let terminal = retired.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (8, 64));
    assert!(NativeWorker::start_on_control(instance).is_err());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_worker_cancellation_cannot_return_a_reusable_source() {
    let instance = original_instance();
    let cancellation = instance.cancellation_on_control();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    cancellation.cancel_on_control();
    callback
        .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
        .unwrap();
    callback.finish(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !owner.reaper.is_finished() {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    assert!(matches!(
        owner.quiesce_on_control(),
        Err(hex_plugin_host::Error::Cancelled)
    ));
    assert!(owner.quiesce_on_control().is_err());
    assert!(
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .is_err()
    );
    drop(callback);
    drop(owner);
    let instance = original_instance();
    instance.cancellation_on_control().cancel_on_control();
    assert!(matches!(
        NativeWorker::start_on_control(instance),
        Err(hex_plugin_host::Error::Cancelled)
    ));
}
