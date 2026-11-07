use super::*;
use hex_project::{Command, Created};
use hex_storage::automation_draft::{DraftOutcome, DraftReadLimits, DraftReader};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-vst3-draft-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("capture.hex-automation-draft")
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, Vst3CaptureTarget, DraftProvenance, CaptureClock) {
    let mut session = Session::new(48_000, 8, 4096).unwrap();
    let state = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [6; 32],
                info_sha256: [7; 32],
            },
        },
        component: Some(&[42; 12]),
        controller: Some(&[]),
    })
    .unwrap();
    // Canonical/scalar bridge fixture, not an admitted SDK or output endpoint.
    let Created::Device(device) = session
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let target = Vst3CaptureTarget {
        device,
        class: [9; 16],
        parameter: 0,
        binding: ForeignProcessorBinding {
            root: 9,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
    };
    let stamp = DraftProvenance {
        sample_rate: 48_000,
        output_identity: 9,
        project_generation: 0,
        editor_revision: 2,
        audio_revision: 7,
        transport_epoch: 1,
        gesture_serial: 1,
    };
    (
        session,
        target,
        stamp,
        CaptureClock {
            sample_rate: 48_000,
            loop_region: None,
        },
    )
}
fn application(
    target: Vst3CaptureTarget,
    frame: u64,
    value: Vst3ApplicationValue,
) -> Vst3ParameterApplication {
    Vst3ParameterApplication {
        binding: target.binding,
        parameter: target.parameter,
        origin: DraftClock {
            performance_frame: frame + 8,
            content_frame: frame,
            loop_pass: 0,
        },
        value,
    }
}
fn read(path: &Path) -> DraftReader {
    DraftReader::open(
        path,
        DraftReadLimits {
            max_source_bytes: 64 * 1024,
        },
    )
    .unwrap()
}

#[test]
fn exact_scope_zero_target_rounding_clocks_and_exclusive_restore_use_the_real_disk_worker() {
    let files = Files::new();
    let path = files.path();
    let (session, target, stamp, clock) = fixture();
    let (mut input, worker) =
        DeviceDraftInput::start_vst3_on_control(&session, None, path.clone(), target, stamp, clock)
            .unwrap();
    let first = application(target, 8, Vst3ApplicationValue::Value(1.0 / 3.0));
    input
        .append_vst3_delivered(target, stamp, &[(1, first)])
        .unwrap()
        .unwrap();
    assert!(input.finish_vst3_delivered(target, stamp, first).is_err());
    let end = application(target, 12, Vst3ApplicationValue::Restored);
    input.finish_vst3_delivered(target, stamp, end).unwrap();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_none());
    assert_eq!(outcome.admitted_events, 1);
    let mut reader = read(&path);
    assert_eq!(reader.source(), session.project());
    assert_eq!(
        reader
            .target()
            .vst3_class_in_source(reader.source())
            .unwrap(),
        Some(target.class)
    );
    assert_eq!(
        reader.target(),
        DraftTarget::Device {
            device: target.device,
            target: DeviceAutomationTarget::Vst3 { parameter: 0 }
        }
    );
    assert_eq!(reader.outcome(), DraftOutcome::Finished(end.origin));
    let mut page = [None; 2];
    assert_eq!(reader.read_events(0, &mut page).unwrap(), 1);
    let event = page[0].unwrap();
    assert_eq!(event.sequence, 1);
    assert_eq!(
        (
            event.performance_frame,
            event.content_frame,
            event.loop_pass
        ),
        (16, 8, 0)
    );
    assert_eq!(event.value.get(), (1.0_f64 / 3.0) as f32);
    assert!((f64::from(event.value.get()) - 1.0 / 3.0).abs() <= 2_f64.powi(-25));
}

#[test]
fn scope_target_value_sequence_and_terminal_refusals_keep_the_entire_batch_retryable() {
    let files = Files::new();
    let path = files.path();
    let (session, target, stamp, clock) = fixture();
    for invalid in [
        Vst3CaptureTarget {
            class: [8; 16],
            ..target
        },
        Vst3CaptureTarget {
            binding: ForeignProcessorBinding {
                root: 8,
                ..target.binding
            },
            ..target
        },
        Vst3CaptureTarget {
            parameter: u32::MAX,
            ..target
        },
    ] {
        assert!(
            DeviceDraftInput::start_vst3_on_control(
                &session,
                None,
                path.clone(),
                invalid,
                stamp,
                clock
            )
            .is_err()
        );
        assert!(!path.exists());
    }
    let (mut input, worker) =
        DeviceDraftInput::start_vst3_on_control(&session, None, path.clone(), target, stamp, clock)
            .unwrap();
    let valid = application(target, 8, Vst3ApplicationValue::Value(0.5));
    let stale = DraftProvenance {
        audio_revision: stamp.audio_revision + 1,
        ..stamp
    };
    assert!(
        input
            .append_vst3_delivered(target, stale, &[(1, valid)])
            .is_err()
    );
    for invalid in [
        Vst3ParameterApplication {
            binding: ForeignProcessorBinding {
                epoch: 5,
                ..target.binding
            },
            ..valid
        },
        Vst3ParameterApplication {
            parameter: 1,
            ..valid
        },
        Vst3ParameterApplication {
            value: Vst3ApplicationValue::Restored,
            ..valid
        },
    ] {
        assert!(
            input
                .append_vst3_delivered(target, stamp, &[(1, invalid)])
                .is_err()
        );
    }
    for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        assert!(
            input
                .append_vst3_delivered(
                    target,
                    stamp,
                    &[
                        (1, valid),
                        (
                            2,
                            Vst3ParameterApplication {
                                value: Vst3ApplicationValue::Value(value),
                                ..valid
                            }
                        )
                    ]
                )
                .is_err()
        );
    }
    assert!(
        input
            .append_vst3_delivered(target, stamp, &[(1, valid), (1, valid)])
            .is_err()
    );
    assert_eq!(input.last_sequence, 0);
    assert_eq!(input.progress().accepted_events, 0);
    input
        .append_vst3_delivered(target, stamp, &[(1, valid)])
        .unwrap()
        .unwrap();
    let end = application(target, 12, Vst3ApplicationValue::Restored);
    assert!(input.finish_vst3_delivered(target, stale, end).is_err());
    assert!(
        input
            .finish_vst3_delivered(
                target,
                stamp,
                Vst3ParameterApplication {
                    parameter: 1,
                    ..end
                }
            )
            .is_err()
    );
    input.finish_vst3_delivered(target, stamp, end).unwrap();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_none());
    assert_eq!(outcome.admitted_events, 1);
    assert_eq!(read(&path).progress().durable_events, 1);
}

#[test]
fn three_shared_loans_backpressure_without_losing_source_or_advancing_the_watermark() {
    let files = Files::new();
    let path = files.path();
    let (session, target, stamp, clock) = fixture();
    let (mut input, worker) =
        DeviceDraftInput::start_vst3_on_control(&session, None, path.clone(), target, stamp, clock)
            .unwrap();
    for sequence in 1..=3 {
        input
            .append_vst3_delivered(
                target,
                stamp,
                &[(
                    sequence,
                    application(target, sequence + 7, Vst3ApplicationValue::Value(0.5)),
                )],
            )
            .unwrap()
            .unwrap();
    }
    let pending = [(
        4,
        application(target, 11, Vst3ApplicationValue::Value(0.75)),
    )];
    let retained = pending;
    assert_eq!(
        input
            .append_vst3_delivered(target, stamp, &pending)
            .unwrap(),
        None
    );
    assert_eq!(pending, retained);
    assert_eq!(input.last_sequence, 3);
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut acknowledgements = 0;
    while acknowledgements < 3 {
        if input.poll().is_some() {
            acknowledgements += 1
        } else {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    input
        .append_vst3_delivered(target, stamp, &pending)
        .unwrap()
        .unwrap();
    input
        .finish_vst3_delivered(
            target,
            stamp,
            application(target, 12, Vst3ApplicationValue::Restored),
        )
        .unwrap();
    let outcome = worker.join_on_control().unwrap();
    assert!(outcome.failure.is_none());
    assert_eq!(outcome.admitted_events, 4);
    let mut reader = read(&path);
    let mut page = [None; 5];
    assert_eq!(reader.read_events(0, &mut page).unwrap(), 4);
    assert_eq!(
        page[..4]
            .iter()
            .map(|event| event.unwrap().sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(page[3].unwrap().value.get(), 0.75);
}
