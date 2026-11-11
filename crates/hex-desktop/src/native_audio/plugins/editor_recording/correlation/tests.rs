use super::*;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::AudioSourceContext;
use std::num::NonZeroU64;

fn capture(parameter: u32) -> Correlation {
    let context = AudioSourceContext {
        project_generation: 3,
        editor_revision: 5,
    };
    let effect = hex_engine::NativeDeviceClass::Effect;
    let utility = hex_engine::native_device_catalog_for_class(effect)
        .unwrap()
        .into_iter()
        .find(|device| device.type_id.as_str() == Ok("hex.utility"))
        .unwrap()
        .type_id;
    let state = hex_engine::default_native_device_state_for_class(effect, utility).unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
    let hex_project::Created::Device(device) = editor
        .execute(hex_project::Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("test device identity")
    };
    let target = Vst3CaptureTarget {
        device,
        class: [9; 16],
        parameter,
        binding: hex_audio::plugin_effects::EffectBinding {
            root: 11,
            instance: 13,
            graph: 17,
            epoch: 19,
        },
    };
    let output = AppliedAudioSource {
        context,
        output_identity: 11,
        audio_revision: 23,
    };
    Correlation::new(
        Target {
            source: output,
            binding: target.binding,
            transport_epoch: 29,
            scope: NonZeroU64::MIN,
            quantum_frames: 8,
        },
        GestureKey {
            generation: 3,
            token: 31,
        },
        target,
        DeviceAutomationSource {
            target: DeviceCaptureTarget::Vst3(target),
            sample_rate: 48_000,
            output_identity: 11,
            audio_revision: 23,
            transport_epoch: 29,
            section_loop: None,
        },
    )
    .unwrap()
}

fn ordered(
    capture: &mut Correlation,
    kind: ParameterEditKind,
    sequence: u64,
    gesture: u64,
    id: u32,
) {
    capture
        .ordered(ParameterEdit {
            sequence,
            gesture,
            kind,
            id,
            normalized: 0.25,
            first_frame: 1000,
        })
        .unwrap();
}

fn actual(sequence: u64, begin: u64, value: Vst3ApplicationValue) -> Fact {
    Fact {
        sequence,
        begin,
        value,
        origin: DraftClock {
            content_frame: sequence,
            performance_frame: sequence + 64,
            loop_pass: 1,
        },
        main_frame: sequence + 576,
    }
}

#[test]
fn delayed_order_uses_exact_raw_zero_and_exclusive_end() {
    use ParameterEditKind::{Begin, End, Perform};
    let mut capture = capture(0);
    // Empty gesture and unrelated edits must not claim the one recorded take.
    for (kind, sequence, gesture, id) in [
        (Begin, 1, 1, 0),
        (End, 2, 1, 0),
        (Begin, 3, 3, 11),
        (Perform, 4, 3, 11),
        (End, 5, 3, 11),
    ] {
        ordered(&mut capture, kind, sequence, gesture, id);
    }
    let point = actual(7, 6, Vst3ApplicationValue::Value(0.75));
    assert!(matches!(
        capture.observe_fact(point, false).unwrap(),
        Match::WaitingForOrder
    ));
    ordered(&mut capture, Begin, 6, 6, 0);
    ordered(&mut capture, Perform, 7, 6, 0);
    let Match::Observed(Observation::Delivered(delivery)) =
        capture.observe_fact(point, false).unwrap()
    else {
        panic!("one original Main point")
    };
    assert_eq!(delivery.target.parameter, 0);
    assert_eq!(delivery.application.origin, point.origin);
    assert_eq!(
        delivery.application.value,
        Vst3ApplicationValue::Value(0.75)
    );
    assert!(capture.observe_fact(point, false).is_err());
    let terminal = actual(8, 6, Vst3ApplicationValue::Restored);
    assert!(matches!(
        capture.observe_fact(terminal, false).unwrap(),
        Match::WaitingForOrder
    ));
    ordered(&mut capture, End, 8, 6, 0);
    assert!(matches!(
        capture.observe_fact(terminal, false).unwrap(),
        Match::Observed(Observation::Restored(_))
    ));
    assert!(matches!(
        capture.observe_fact(terminal, false).unwrap(),
        Match::Unrelated
    ));
    assert!(capture.abort().is_none());
}

#[test]
fn retirement_preserves_unread_main_points_but_does_not_invent_an_end() {
    let mut capture = capture(7);
    let point = actual(12, 11, Vst3ApplicationValue::Value(0.5));
    assert!(matches!(
        capture.observe_fact(point, false).unwrap(),
        Match::WaitingForOrder
    ));
    assert!(matches!(
        capture.observe_fact(point, true).unwrap(),
        Match::Observed(Observation::Delivered(_))
    ));
    assert!(matches!(
        capture.abort(),
        Some(Observation::Interrupted { sequence: 13, .. })
    ));
}

#[test]
fn stale_begin_and_bounded_order_cannot_replace_a_confirmed_take() {
    use ParameterEditKind::{Begin, End, Perform};
    let mut capture = capture(0);
    let stale = actual(2, 1, Vst3ApplicationValue::Value(0.5));
    ordered(&mut capture, Begin, 3, 3, 0);
    ordered(&mut capture, Perform, 4, 3, 0);
    ordered(&mut capture, End, 5, 3, 0);
    assert!(capture.observe_fact(stale, false).is_err());
    for index in 1..MAX_ORDERED_GESTURES as u64 {
        let begin = 3 + 3 * index;
        ordered(&mut capture, Begin, begin, begin, 0);
        ordered(&mut capture, Perform, begin + 1, begin, 0);
        ordered(&mut capture, End, begin + 2, begin, 0);
    }
    let sequence = 3 + 3 * MAX_ORDERED_GESTURES as u64;
    assert!(
        capture
            .ordered(ParameterEdit {
                sequence,
                gesture: sequence,
                id: 0,
                ..Default::default()
            })
            .is_err()
    );
    let actual = actual(4, 3, Vst3ApplicationValue::Value(0.75));
    assert!(matches!(
        capture.observe_fact(actual, false).unwrap(),
        Match::Observed(Observation::Delivered(_))
    ));
    assert!(matches!(
        capture.abort(),
        Some(Observation::Interrupted { .. })
    ));
}
