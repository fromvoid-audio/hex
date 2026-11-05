//! Real source-fenced native input and checked host delivery; no device deadline claim.
use super::*;
use crate::native_parameter_input::{AbortReason, Receipt, Target};
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_engine::{Audio, PreparedRevision, SineTable, playback::Playback};
use hex_lattice::NativeTransientValue;
use hex_lattice_contract::{
    DeviceAutomationTarget, HostGain, NativeDeviceMetadata, ParameterValue,
};
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
pub(crate) fn fixture(limiter: bool) -> (prepared::Control, prepared::Callback, Target) {
    fixture_with_setup(limiter, |_| {})
}
fn fixture_with_setup(
    limiter: bool,
    setup: impl FnOnce(&mut prepared::Callback),
) -> (prepared::Control, prepared::Callback, Target) {
    fixture_with_target(
        limiter,
        DeviceAutomationTarget::Parameter(parameter::GAIN),
        false,
        setup,
    )
}
fn fixture_with_target(
    limiter: bool,
    input: DeviceAutomationTarget,
    bypassed: bool,
    setup: impl FnOnce(&mut prepared::Callback),
) -> (prepared::Control, prepared::Callback, Target) {
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
    if bypassed {
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
    }
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
            input,
        )
        .unwrap(),
    );
    let input = revision.native_parameter_input_target().unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    setup(&mut callback);
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
pub(crate) fn value(gain: f32) -> NativeTransientValue {
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
fn output_trim_clear_receipt_does_not_end_ramp_and_seek_discards_it() {
    let (mut control, mut callback, target) =
        fixture_with_target(false, DeviceAutomationTarget::HostOutputGain, false, |_| {});
    let gain = HostGain::from_db(-6.0).unwrap();
    control
        .native_parameter_input
        .begin(target, NativeTransientValue::HostOutputGain(gain))
        .unwrap();
    let mut pcm = [[0.0; 2]; 64];
    for block in 0..4 {
        let rendered = render(&mut callback, &mut pcm);
        if block == 0 {
            assert!(pcm[0][0] < 0.25 && pcm[0][0] > 0.249);
        }
        assert!(control.native_parameter_input.try_receipt().is_none());
        assert!(write(&mut callback, &mut pcm, &rendered, 2));
        if block == 0 {
            assert!(matches!(
                control.native_parameter_input.try_receipt(),
                Some(Receipt::Applied { sequence: 1, .. })
            ));
        }
    }
    assert_eq!(pcm[63], [0.25 * gain.linear(), -0.5 * gain.linear()]);
    control.native_parameter_input.release().unwrap();
    let rendered = render(&mut callback, &mut pcm[..1]);
    let first = pcm[0];
    assert!(first[0] > 0.25 * gain.linear() && first[0] < 0.25);
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(write(&mut callback, &mut pcm[..1], &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Finished { .. })
    ));
    let rendered = render(&mut callback, &mut pcm);
    assert!(pcm[0][0] > first[0] && pcm[63][0] < 0.25);
    assert!(write(&mut callback, &mut pcm, &rendered, 2));
    control
        .command(hex_engine::PlaybackCommand::Seek(1024))
        .unwrap();
    for _ in 0..4 {
        let rendered = render(&mut callback, &mut pcm);
        assert!(write(&mut callback, &mut pcm, &rendered, 2));
    }
    assert_eq!(pcm[63], [0.25, -0.5]);
    assert!(control.native_parameter_input.try_receipt().is_none());
    shutdown(&mut control, &mut callback);
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
fn render_fault_aborts_a_racing_native_parameter_gesture() {
    let (mut control, mut callback, target) = fixture(false);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control
        .status
        .render_fault
        .store(true, std::sync::atomic::Ordering::Release);
    let mut source = [[0.0; 2]; 8];
    let _ = render(&mut callback, &mut source);
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            ..
        })
    ));
    shutdown(&mut control, &mut callback);
}

#[test]
fn device_error_aborts_a_queued_native_parameter_gesture() {
    let (mut control, mut callback, target) = fixture(false);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control
        .status
        .device_fault
        .store(true, std::sync::atomic::Ordering::Release);
    counted(|| callback.begin_block());
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            ..
        })
    ));
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

#[test]
fn aborted_native_gesture_late_changes_retire_credits_without_repeating_its_terminal() {
    let (mut control, mut callback, mut target) = fixture(false);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    for _ in 1..64 {
        control.native_parameter_input.change(value(0.75)).unwrap();
    }
    control.command(hex_engine::PlaybackCommand::Stop).unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    // Leave Aborted unread behind the original normal receipts. Control still
    // owns the gesture and may use each returned normal credit for a late Set.
    // Every such Set owes exactly one Superseded, never another Aborted.
    let mut seen = [false; 72];
    for expected in 1..=8 {
        let Some(Receipt::Superseded { sequence, .. }) =
            control.native_parameter_input.try_receipt()
        else {
            panic!("normal credit")
        };
        assert_eq!(sequence, expected);
        seen[usize::try_from(sequence - 1).unwrap()] = true;
        control.native_parameter_input.change(value(0.5)).unwrap();
        let rendered = render(&mut callback, &mut source);
        assert!(rendered.is_ok());
        assert!(write(&mut callback, &mut source, &rendered, 2));
        assert!(
            !control.status.native_parameter_input_fault.load(Acquire),
            "late commands cannot exhaust reserved terminal receipt capacity"
        );
    }
    let mut terminals = 0;
    while let Some(receipt) = control.native_parameter_input.try_receipt() {
        match receipt {
            Receipt::Superseded { sequence, .. } => {
                let index = usize::try_from(sequence - 1).unwrap();
                assert!(!seen[index], "each issued credit retires once");
                seen[index] = true;
            }
            Receipt::Aborted {
                target: actual,
                reason: AbortReason::TransportChanged,
            } => {
                assert_eq!(actual, target);
                terminals += 1;
            }
            _ => panic!("a stopped gesture cannot be applied or finished"),
        }
    }
    assert_eq!(seen, [true; 72]);
    assert_eq!(terminals, 1);
    target.gesture_serial += 1;
    target.transport_epoch = control.status.transport_epoch.load(Acquire);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control.native_parameter_input.cancel().unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::Cancelled,
            ..
        })
    ));
    assert!(control.native_parameter_input.try_receipt().is_none());
    shutdown(&mut control, &mut callback);
}

#[test]
fn stopped_native_begin_release_aborts_once_and_releases_publication_admission() {
    let (mut control, mut callback, mut target) = fixture(false);
    control.command(hex_engine::PlaybackCommand::Stop).unwrap();
    let mut source = [[0.0; 2]; 1];
    // A queued Stop renders its 128-frame fade before it stops (#4151).
    for _ in 0..256 {
        let rendered = render(&mut callback, &mut source);
        assert!(rendered.is_ok());
        assert!(write(&mut callback, &mut source, &rendered, 2));
        if !control.status.playing.load(Acquire) {
            break;
        }
    }
    assert!(!control.status.playing.load(Acquire));
    target.transport_epoch = control.status.transport_epoch.load(Acquire);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control.native_parameter_input.release().unwrap();
    assert!(!control.status.publication_reserve_for_test());
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert_eq!(source, [[0.0; 2]]);
    assert!(
        !control.status.publication_reserve_for_test(),
        "terminal remains control-owned until drained"
    );
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Aborted { target: actual, reason: AbortReason::TransportChanged }) if actual == target)
    );
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(control.native_parameter_input.target().is_none());
    assert!(
        control.status.publication_reserve_for_test(),
        "draining the terminal releases the existing shared reservation"
    );
    control.status.publication_clear_for_test();
    shutdown(&mut control, &mut callback);
}

#[test]
fn native_begin_rejects_each_stale_source_field_without_consuming_admission_or_credit() {
    let (mut control, mut callback, target) = fixture(false);
    for field in 0..4 {
        let mut stale = target;
        match field {
            0 => stale.source.output_identity += 1,
            1 => stale.source.context.project_generation += 1,
            2 => stale.source.context.editor_revision += 1,
            _ => stale.source.audio_revision += 1,
        }
        assert!(
            control
                .native_parameter_input
                .begin(stale, value(0.5))
                .is_err()
        );
        assert!(control.native_parameter_input.target().is_none());
        assert!(control.native_parameter_input.try_receipt().is_none());
        assert!(
            control.status.publication_reserve_for_test(),
            "stale field {field} must not retain shared publication admission"
        );
        control.status.publication_clear_for_test();
    }
    // The identical valid gesture is still usable: no failed Begin consumed its
    // normal credit or advanced the monotonically increasing gesture serial.
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control.native_parameter_input.cancel().unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::Cancelled,
            ..
        })
    ));
    assert!(control.native_parameter_input.try_receipt().is_none());
    shutdown(&mut control, &mut callback);
}

#[test]
fn seek_revokes_a_delayed_native_marker_and_resumes_with_base_pcm() {
    let (mut control, mut callback, mut target) = fixture(true);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    let mut source = [[0.0; 2]; 64];
    let rendered = render(&mut callback, &mut source[..32]);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source[..32], &rendered, 2));
    assert_eq!(source[..32], [[0.0; 2]; 32]);
    assert!(
        control.native_parameter_input.try_receipt().is_none(),
        "actual Set is processed but its frame 304 has not reached the sink"
    );
    control
        .command(hex_engine::PlaybackCommand::Seek(0))
        .unwrap();
    control.command(hex_engine::PlaybackCommand::Play).unwrap();
    let rendered = render(&mut callback, &mut source[..1]);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source[..1], &rendered, 2));
    assert_eq!(source[0], [0.0; 2]);
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Superseded { target: actual, sequence: 1 }) if actual == target)
    );
    assert!(
        !control.status.publication_reserve_for_test(),
        "normal credit alone does not retire the terminal reservation"
    );
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Aborted { target: actual, reason: AbortReason::TransportChanged }) if actual == target)
    );
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(control.native_parameter_input.target().is_none());
    assert!(control.status.publication_reserve_for_test());
    control.status.publication_clear_for_test();
    // Seek(0) resets the real Limiter. The first 288 output frames are silent;
    // the following frames are the original base-gain PCM, not the old Set.
    let mut at = 1;
    for requested in [19, 37, 1, 64].into_iter().cycle() {
        if at == 320 {
            break;
        }
        let len = requested.min(320 - at);
        let rendered = render(&mut callback, &mut source[..len]);
        assert!(rendered.is_ok());
        assert!(write(&mut callback, &mut source[..len], &rendered, 2));
        for (offset, actual) in source[..len].iter().enumerate() {
            assert_eq!(
                *actual,
                if at + offset < 288 {
                    [0.0; 2]
                } else {
                    [0.25, -0.5]
                }
            );
        }
        assert!(
            control.native_parameter_input.try_receipt().is_none(),
            "a pre-seek marker can never become Applied after resume"
        );
        at += len;
    }
    // A fresh Begin proves that the one issued normal credit was returned too.
    target.gesture_serial += 1;
    target.transport_epoch = control.status.transport_epoch.load(Acquire);
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control.native_parameter_input.cancel().unwrap();
    let rendered = render(&mut callback, &mut source[..1]);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source[..1], &rendered, 2));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(matches!(
        control.native_parameter_input.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::Cancelled,
            ..
        })
    ));
    assert!(control.native_parameter_input.try_receipt().is_none());
    shutdown(&mut control, &mut callback);
}

#[test]
fn native_begin_release_refuses_delayed_output_profile_before_engine_application() {
    let mut transport = None;
    let (mut control, mut callback, target) = fixture_with_setup(false, |callback| {
        transport = Some(
            callback
                .prepare_output_transport_on_control(1, 1, 1, 16)
                .unwrap(),
        );
    });
    let (_transport_control, mut worker) = transport.unwrap();
    // Complete the real warmup request on control, outside measured scopes. No
    // native parameter clock is derived from this worker's delayed output.
    let request = worker.receive().unwrap().unwrap();
    worker.complete(request).unwrap();
    control
        .native_parameter_input
        .begin(target, value(0.5))
        .unwrap();
    control.native_parameter_input.release().unwrap();
    let mut source = [[0.0; 2]; 16];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Superseded { target: actual, sequence: 1 }) if actual == target)
    );
    assert!(
        matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Aborted { target: actual, reason: AbortReason::UnsupportedOutput }) if actual == target)
    );
    assert!(
        control.native_parameter_input.try_receipt().is_none(),
        "delayed worker output cannot confirm native Set or Clear"
    );
    assert!(control.native_parameter_input.target().is_none());
    assert!(control.status.publication_reserve_for_test());
    control.status.publication_clear_for_test();
    shutdown(&mut control, &mut callback);
}

#[test]
fn bypassed_host_trims_abort_live_requests_without_holding_publication() {
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut control, mut callback, binding) = fixture_with_target(false, target, true, |_| {});
        let gain = HostGain::from_db(-6.0).unwrap();
        let value = match target {
            DeviceAutomationTarget::HostInputGain => NativeTransientValue::HostInputGain(gain),
            DeviceAutomationTarget::HostOutputGain => NativeTransientValue::HostOutputGain(gain),
            _ => panic!("host target fixture"),
        };
        control
            .native_parameter_input
            .begin(binding, value)
            .unwrap();
        control.native_parameter_input.release().unwrap();
        let mut source = [[0.0; 2]; 16];
        let rendered = render(&mut callback, &mut source);
        assert!(rendered.is_ok());
        assert_eq!(
            source,
            [[0.25, -0.5]; 16],
            "host trim bypass preserves ordinary PCM"
        );
        assert!(write(&mut callback, &mut source, &rendered, 2));
        assert!(!control.status.publication_reserve_for_test());
        assert!(
            matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Superseded { target: actual, sequence: 1 }) if actual == binding)
        );
        assert!(
            matches!(control.native_parameter_input.try_receipt(), Some(Receipt::Aborted { target: actual, reason: AbortReason::TargetUnavailable }) if actual == binding)
        );
        assert!(control.native_parameter_input.try_receipt().is_none());
        assert!(control.native_parameter_input.target().is_none());
        assert!(
            control.status.publication_reserve_for_test(),
            "the undeliverable target cannot trap publication admission"
        );
        control.status.publication_clear_for_test();
        shutdown(&mut control, &mut callback);
    }
}

#[test]
fn bypassed_provider_gain_still_applies_and_finishes_clear_without_changing_dry_pcm() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut control, mut callback, binding) = fixture_with_target(false, target, true, |_| {});
    let value = value(0.5);
    control
        .native_parameter_input
        .begin(binding, value)
        .unwrap();
    let mut source = [[0.0; 2]; 1];
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert_eq!(
        source,
        [[0.25, -0.5]],
        "bypassed provider advances only its internal controls/history"
    );
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let Some(Receipt::Applied {
        target: actual,
        sequence: 1,
        application,
    }) = control.native_parameter_input.try_receipt()
    else {
        panic!("applied provider input under bypass")
    };
    assert_eq!(actual, binding);
    assert_eq!(
        application.operation,
        hex_lattice::NativeTransientOperation::Set(value)
    );
    control.native_parameter_input.release().unwrap();
    let rendered = render(&mut callback, &mut source);
    assert!(rendered.is_ok());
    assert_eq!(source, [[0.25, -0.5]]);
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(write(&mut callback, &mut source, &rendered, 2));
    let Some(Receipt::Finished {
        target: actual,
        application,
    }) = control.native_parameter_input.try_receipt()
    else {
        panic!("delivered provider clear under bypass")
    };
    assert_eq!(actual, binding);
    assert_eq!(
        application.operation,
        hex_lattice::NativeTransientOperation::Clear(target)
    );
    assert!(control.native_parameter_input.try_receipt().is_none());
    assert!(control.native_parameter_input.target().is_none());
    assert!(control.status.publication_reserve_for_test());
    control.status.publication_clear_for_test();
    shutdown(&mut control, &mut callback);
}
