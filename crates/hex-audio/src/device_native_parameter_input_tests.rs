//! Real source-fenced native input and checked host delivery; no device deadline claim.
use super::*;
use crate::native_parameter_input::{AbortReason, Receipt, Target};
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, PreparedRevision, SineTable, playback::Playback};
use hex_lattice::NativeTransientValue;
use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
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
fn render(
    callback: &mut prepared::Callback,
    source: &mut [Frame],
) -> Result<(), hex_engine::Invalid> {
    counted(|| {
        callback.begin_block();
        callback.render(source)
    })
}
fn write(
    callback: &mut prepared::Callback,
    source: &mut [Frame],
    rendered: &Result<(), hex_engine::Invalid>,
    channels: usize,
) -> bool {
    let mut host = [0.0_f32; 128];
    assert!(source.len() <= 64);
    counted(|| {
        finish_host_chunk(
            callback,
            &mut host[..source.len() * 2],
            source,
            channels,
            rendered,
            0,
            Instant::now(),
        )
    })
}
fn fixture(limiter: bool) -> (prepared::Control, prepared::Callback, Target) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "native input".into(),
            path: "original-native-input.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    if limiter {
        let definition = &hex_devices::limiter::DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(2),
            )
            .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
    }
    let revision = Arc::new(
        PreparedRevision::prepare_for_native_parameter_input(
            editor.project(),
            &BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap())]),
            Arc::new(SineTable::new()),
            None,
            device,
            DeviceAutomationTarget::Parameter(parameter::GAIN),
        )
        .unwrap(),
    );
    let input = revision.native_parameter_input_target().unwrap();
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
    control
        .publish_with_admission_and_source_on_control(
            Playback::try_from_prepared_with_session(revision).unwrap(),
            prepared::PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 11,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    let mut source = [[0.0; 2]; 16];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let target = Target {
        source: control.status.recording_source().unwrap(),
        input,
        transport_epoch: control.status.transport_epoch.load(Acquire),
        gesture_serial: 1,
    };
    (control, callback, target)
}
fn value(gain: f32) -> NativeTransientValue {
    NativeTransientValue::Parameter {
        id: parameter::GAIN,
        value: ParameterValue::Float(gain),
    }
}
fn shutdown(control: &mut prepared::Control, callback: &mut prepared::Callback) {
    control.request_shutdown();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(callback, &mut source, &rendered, 2));
    control.reclaim();
}
#[test]
fn native_input_receipt_waits_for_actual_delayed_pcm_and_clear_waits_for_sink() {
    let (mut control, mut callback, target) = fixture(true);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    let mut at = 16;
    let mut source = [[0.0; 2]; 64];
    while at < 304 {
        let len = (304 - at).min(37);
        let rendered = render(&mut callback, &mut source[..len]);
        assert!(rendered.is_ok());
        assert!(control.native_parameter_input.try_receipt().is_none());
        assert!(write(&mut callback, &mut source[..len], &rendered, 2));
        for (offset, frame) in source[..len].iter().enumerate() {
            assert_eq!(
                *frame,
                if at + offset < 288 {
                    [0.0; 2]
                } else {
                    [0.25, -0.5]
                }
            );
        }
        assert!(control.native_parameter_input.try_receipt().is_none());
        at += len;
    }
    let rendered = render(&mut callback, &mut source[..1]);
    assert!(rendered.is_ok());
    assert_eq!(source[0], [0.125, -0.25]);
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(write(&mut callback, &mut source[..1], &rendered, 2));
    let Some(Receipt::Applied {
        target: actual,
        sequence: 1,
        application,
    }) = control.native_parameter_input.try_receipt()
    else {
        panic!("actual application")
    };
    assert_eq!(actual, target);
    assert_eq!(application.performance_frame, 16);
    assert_eq!(application.arrangement_frame, 16);
    assert_eq!(application.remaining_latency_frames, 288);
    control.native_parameter_input.release().unwrap();
    let rendered = render(&mut callback, &mut source[..1]);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source[..1], &rendered, 2));
    assert!(
        control.native_parameter_input.try_receipt().is_none(),
        "clear is processed but not yet delivered"
    );
    control.native_parameter_input.cancel().unwrap();
    let rendered = render(&mut callback, &mut source[..1]);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source[..1], &rendered, 2));
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Aborted { target: actual, reason: AbortReason::Cancelled }) if actual == target)
    );
    assert!(
        control.native_parameter_input.try_receipt().is_none(),
        "cancel revokes still-undelivered clear"
    );
    shutdown(&mut control, &mut callback);
}
#[test]
fn native_input_cancel_wins_after_release_render_before_the_sink_write() {
    let (mut control, mut callback, target) = fixture(false);
    let mut stale = target;
    stale.source.context.editor_revision += 1;
    assert!(
        control
            .native_parameter_input
            .begin(stale, value(0.5))
            .is_err()
    );
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Applied { sequence: 1, .. })
    ));
    control.native_parameter_input.release().unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert_eq!(source, [[0.25, -0.5]]);
    control.native_parameter_input.cancel().unwrap();
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::Cancelled,
            ..
        })
    ));
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(control.native_parameter_input.target().is_none());
    shutdown(&mut control, &mut callback);
}
#[test]
fn native_input_reserves_cancel_under_full_normal_pressure_and_retires_each_credit_once() {
    let (mut control, mut callback, mut target) = fixture(false);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    for _ in 1..64 {
        control.native_parameter_input.change(value(0.75)).unwrap();
    }
    assert!(control.native_parameter_input.change(value(0.5)).is_err());
    control.native_parameter_input.release().unwrap();
    control.native_parameter_input.cancel().unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let mut seen = [false; 64];
    let mut aborted = 0;
    while let Some(receipt) = control.native_parameter_input.try_receipt() {
        match receipt {
            Receipt::Superseded { sequence, .. } => {
                let index = usize::try_from(sequence - 1).unwrap();
                assert!(!seen[index]);
                seen[index] = true;
            }
            Receipt::Aborted {
                reason: AbortReason::Cancelled,
                ..
            } => aborted += 1,
            _ => panic!("cancelled work cannot be delivered"),
        }
    }
    assert_eq!(seen, [true; 64]);
    assert_eq!(aborted, 1);
    target.gesture_serial += 1;
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    for _ in 0..1025 {
        let rendered = render(&mut callback, &mut source);
        assert!(rendered.is_ok());
        assert!(write(&mut callback, &mut source, &rendered, 2));
        assert!(matches!(
            control.native_parameter_input.try_receipt(),
            Some(Receipt::Applied { .. })
        ));
        control.native_parameter_input.change(value(0.5)).unwrap();
    }
    control.native_parameter_input.cancel().unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    while control.native_parameter_input.try_receipt().is_some() {}
    shutdown(&mut control, &mut callback);
}

#[test]
fn native_input_successful_release_observes_actual_restoration_and_failure_never_applies() {
    let (mut control, mut callback, target) = fixture(false);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Applied { .. })
    ));
    control.native_parameter_input.release().unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert_eq!(source, [[0.25, -0.5]]);
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let Some(Receipt::Finished { application, .. }) = control.native_parameter_input.try_receipt()
    else {
        panic!("delivered restoration")
    };
    assert_eq!(
        application.operation,
        hex_lattice::NativeTransientOperation::Clear(target.input.target())
    );
    assert_eq!(application.performance_frame, 17);
    assert!(control.native_parameter_input.target().is_none());
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert_eq!(source, [[0.25, -0.5]]);
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let mut next = target;
    next.gesture_serial += 1;
    control
        .native_parameter_input
        .begin(next, value(0.25))
        .unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(!write(&mut callback, &mut source, &rendered, 1));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            ..
        })
    ));
    assert!(control.native_parameter_input.try_receipt().is_none());
    shutdown(&mut control, &mut callback);
}
