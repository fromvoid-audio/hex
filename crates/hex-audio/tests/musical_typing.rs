use hex_audio::prepared;
use hex_engine::{
    PlaybackCommand,
    live_input::{PreparedTypingArm, TypingTarget},
    typing_protocol::{Action, Token},
};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn fixture() -> (prepared::Control, prepared::Callback, TypingTarget) {
    fixture_kind(hex_devices_type("hex.instrument.sine"), &[])
}
fn hex_devices_type(value: &'static str) -> hex_lattice_contract::DeviceTypeId {
    hex_lattice_contract::DeviceTypeId::from_static(value)
}
fn fixture_kind(
    kind: hex_lattice_contract::DeviceTypeId,
    parameters: &[(u32, hex_lattice_contract::ParameterValue)],
) -> (prepared::Control, prepared::Callback, TypingTarget) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "typing".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let target = TypingTarget {
        project_generation: 0,
        project_revision: 1,
        track,
        device,
    };
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        kind,
    )
    .unwrap();
    for (id, value) in parameters {
        state = hex_engine::patch_native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            &state,
            hex_lattice_contract::ParameterId::from_static(*id),
            *value,
        )
        .unwrap();
    }
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    playback
        .prepare_typing_input_with_bridge(
            PreparedTypingArm::prepare(editor.project(), target).unwrap(),
            1,
            prepared::input_bridge_storage_bytes().unwrap(),
        )
        .unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    callback.finish_typing_output(true);
    (control, callback, target)
}
fn on(target: TypingTarget, serial: u64) -> Action {
    Action::On {
        owner: target.owner().unwrap(),
        token: Token { epoch: 1, serial },
        key: 9,
        pitch: 69,
        velocity: 100,
    }
}
fn off(action: Action) -> Action {
    let Action::On { owner, token, .. } = action else {
        panic!("on")
    };
    Action::Off { owner, token }
}

#[test]
fn consumed_queue_ticket_is_not_an_ack_and_only_accepted_output_publishes_actual_frame() {
    let (mut control, mut callback, target) = fixture();
    let action = on(target, 1);
    let ticket = control.typing_with_ticket(action).unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert!(control.status.typing.snapshot().unwrap().applications[9].is_none());
    let mut pcm = [[0.0; 2]; 256];
    callback.render(&mut pcm).unwrap();
    assert!(control.status.typing.snapshot().unwrap().applications[9].is_none());
    assert!(hex_audio::typing::output_is_finite(&pcm, 2));
    callback.finish_typing_output(true);
    let application = control.status.typing.snapshot().unwrap().applications[9].unwrap();
    assert_eq!(application.ticket, ticket);
    assert_eq!(application.action, action);
    assert_eq!(application.performance_frame, 1);
    assert_eq!(application.result, Ok(()));
    for (age, frame) in pcm.iter().enumerate() {
        let expected = (std::f64::consts::TAU * 440.0 * age as f64 / 48_000.0).sin() * 0.1 * 100.0
            / 127.0
            * (age as f64 / 240.0).min(1.0);
        assert!((f64::from(frame[0]) - expected).abs() < 1e-6);
        assert_eq!(frame[0], frame[1]);
    }
}

#[test]
fn rejected_endpoint_cuts_input_and_cannot_later_publish_an_old_success() {
    let (mut control, mut callback, target) = fixture();
    control.typing_with_ticket(on(target, 1)).unwrap();
    callback.begin_block();
    let mut pcm = [[0.0; 2]; 256];
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(false);
    let snapshot = control.status.typing.snapshot().unwrap();
    assert!(!snapshot.armed);
    assert_eq!(snapshot.cut_through_epoch, 1);
    assert!(snapshot.applications[9].is_none());
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    assert!(control.status.typing.snapshot().unwrap().applications[9].is_none());
    assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn full_real_ring_cannot_lose_an_owned_release_or_redirect_it() {
    let (mut control, mut callback, target) = fixture();
    let action = on(target, 1);
    control.typing_with_ticket(action).unwrap();
    callback.begin_block();
    let mut pcm = [[0.0; 2]; 256];
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    for _ in 0..32 {
        control.command(PlaybackCommand::Play).unwrap();
    }
    assert_eq!(control.typing_with_ticket(off(action)), Err(off(action)));
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    let snapshot = control.status.typing.snapshot().unwrap();
    assert_eq!(snapshot.owner, target.owner().unwrap());
    assert!(!snapshot.armed);
    assert_eq!(snapshot.cut_through_epoch, 1);
    for _ in 0..10 {
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        callback.finish_typing_output(true);
    }
    assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn pause_in_the_same_prefix_cancels_unapplied_onsets_instead_of_resurrecting_them() {
    let (mut control, mut callback, target) = fixture();
    let action = on(target, 1);
    let ticket = control.typing_with_ticket(action).unwrap();
    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    let mut pcm = [[0.0; 2]; 256];
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    let application = control.status.typing.snapshot().unwrap().applications[9].unwrap();
    assert_eq!(application.ticket, ticket);
    assert_eq!(
        application.result,
        Err(hex_engine::typing_protocol::Refusal::NotPlaying)
    );
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn final_channel_validation_accepts_representable_mono_extrema_and_refuses_nonfinite_stereo() {
    assert!(hex_audio::typing::output_is_finite(&[[0.5, -0.5]], 1));
    assert!(hex_audio::typing::output_is_finite(
        &[[f32::MAX, f32::MAX]],
        1
    ));
    assert!(hex_audio::typing::output_is_finite(
        &[[f32::MAX, f32::MAX]],
        2
    ));
    assert!(!hex_audio::typing::output_is_finite(&[[f32::NAN, 0.0]], 2));
    assert!(!hex_audio::typing::output_is_finite(&[[0.0; 2]], 0));
}

#[cfg(feature = "allocation-probe")]
#[test]
fn actual_ring_callback_and_output_acknowledgment_allocate_nothing() {
    use hex_lattice_contract::ParameterValue;
    for (kind, parameters) in [
        ("hex.instrument.sine", Vec::new()),
        ("hex.instrument.fm4", Vec::new()),
        ("hex.instrument.operator", Vec::new()),
        (
            "hex.instrument.operator",
            vec![(82, ParameterValue::Bool(true))],
        ),
        (
            "hex.instrument.operator",
            vec![(78, ParameterValue::Float(0.25))],
        ),
        (
            "hex.instrument.operator",
            vec![
                (82, ParameterValue::Bool(true)),
                (78, ParameterValue::Float(0.25)),
            ],
        ),
    ] {
        let (mut control, mut callback, target) = fixture_kind(hex_devices_type(kind), &parameters);
        let mut pcm = [[0.0; 2]; 256];
        let counts = allocation_counter::measure(|| {
            for serial in 1..=64 {
                let action = on(target, serial);
                control.typing_with_ticket(action).unwrap();
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
                callback.finish_typing_output(true);
                control.typing_with_ticket(off(action)).unwrap();
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
                callback.finish_typing_output(true);
                for _ in 0..10 {
                    callback.begin_block();
                    callback.render(&mut pcm).unwrap();
                    callback.finish_typing_output(true);
                }
            }
        });
        assert_eq!(counts.count_total, 0);
        assert_eq!(counts.bytes_total, 0);
        assert_eq!(counts.count_current, 0);
        assert_eq!(counts.bytes_current, 0);
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn fixed_bridge_charge_covers_actual_owned_allocations_and_container_storage() {
    let mut channel = None;
    let counts = allocation_counter::measure(|| {
        channel = Some(prepared::channel(48_000).unwrap());
    });
    let declared = prepared::input_bridge_storage_bytes().unwrap();
    let containers =
        std::mem::size_of::<prepared::Control>() + std::mem::size_of::<prepared::Callback>();
    assert!(
        declared >= counts.bytes_current as usize + containers,
        "declared={declared} heap={} containers={containers}",
        counts.bytes_current
    );
    println!(
        "typing bridge: declared={declared} measured_heap={} containers={containers}",
        counts.bytes_current
    );
    drop(channel);
}

#[test]
fn mutable_capacity_refusal_does_not_steal_or_change_any_existing_voice() {
    use hex_lattice_contract::ParameterValue;
    let parameters = [(82, ParameterValue::Bool(true))];
    let (mut control, mut callback, target) =
        fixture_kind(hex_devices_type("hex.instrument.operator"), &parameters);
    let (mut oracle_control, mut oracle, oracle_target) =
        fixture_kind(hex_devices_type("hex.instrument.operator"), &parameters);
    let mut actual = [[0.0; 2]; 1];
    let mut expected = actual;
    for serial in 1..=16 {
        control.typing_with_ticket(on(target, serial)).unwrap();
        oracle_control
            .typing_with_ticket(on(oracle_target, serial))
            .unwrap();
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        callback.finish_typing_output(true);
        oracle.begin_block();
        oracle.render(&mut expected).unwrap();
        oracle.finish_typing_output(true);
        assert_eq!(actual, expected);
        assert!(
            control
                .status
                .typing
                .snapshot()
                .unwrap()
                .last_application
                .unwrap()
                .result
                .is_ok()
        );
    }
    control.typing_with_ticket(on(target, 17)).unwrap();
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    callback.finish_typing_output(true);
    oracle.begin_block();
    oracle.render(&mut expected).unwrap();
    oracle.finish_typing_output(true);
    assert_eq!(
        actual, expected,
        "capacity refusal must retain every original LFO/phase/gate"
    );
    assert_eq!(
        control
            .status
            .typing
            .snapshot()
            .unwrap()
            .last_application
            .unwrap()
            .result,
        Err(hex_engine::typing_protocol::Refusal::Capacity)
    );
}

#[test]
fn held_release_during_pause_is_applied_before_the_first_resume_sample() {
    let (mut control, mut callback, target) = fixture();
    let action = on(target, 1);
    control.typing_with_ticket(action).unwrap();
    let mut pcm = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    let on_frame = control
        .status
        .typing
        .snapshot()
        .unwrap()
        .last_application
        .unwrap()
        .performance_frame;
    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    let frozen = control.status.performance_frame.load(Acquire);
    control.typing_with_ticket(off(action)).unwrap();
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(control.status.performance_frame.load(Acquire), frozen);
    assert!(matches!(
        control
            .status
            .typing
            .snapshot()
            .unwrap()
            .last_application
            .unwrap()
            .action,
        Action::On { .. }
    ));
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut pcm).unwrap();
    callback.finish_typing_output(true);
    let release = control
        .status
        .typing
        .snapshot()
        .unwrap()
        .last_application
        .unwrap();
    assert!(matches!(release.action, Action::Off { .. }));
    assert_eq!(release.performance_frame, frozen);
    let age_at_off = frozen - on_frame;
    for (offset, frame) in pcm.iter().enumerate() {
        let age = age_at_off + offset as u64;
        let attack_at_off = (age_at_off as f64 / 240.0).min(1.0);
        let decay = (1.0 - offset as f64 / 2400.0).max(0.0);
        let expected = (std::f64::consts::TAU * 440.0 * age as f64 / 48000.0).sin() * 0.1 * 100.0
            / 127.0
            * attack_at_off
            * decay;
        assert!(
            (frame[0] as f64 - expected).abs() < 1e-6,
            "resume release oracle at {offset}"
        );
    }
}

#[test]
#[ignore = "manual Linux timing probe; not native deadline acceptance"]
fn profile_real_typing_callback_and_committed_output_receipts() {
    use hex_lattice_contract::ParameterValue;
    for (kind, parameters) in [
        ("hex.instrument.sine", Vec::new()),
        (
            "hex.instrument.operator",
            vec![
                (82, ParameterValue::Bool(true)),
                (78, ParameterValue::Float(0.25)),
            ],
        ),
    ] {
        let (mut control, mut callback, target) = fixture_kind(hex_devices_type(kind), &parameters);
        for key in 0..12 {
            control
                .typing_with_ticket(Action::On {
                    owner: target.owner().unwrap(),
                    token: Token {
                        epoch: 1,
                        serial: key as u64 + 1,
                    },
                    key,
                    pitch: 60 + key,
                    velocity: 100,
                })
                .unwrap();
        }
        let mut pcm = [[0.0; 2]; 128];
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        callback.finish_typing_output(true);
        let mut samples = vec![0u128; 2000];
        for sample in &mut samples {
            let start = std::time::Instant::now();
            callback.begin_block();
            callback.render(std::hint::black_box(&mut pcm)).unwrap();
            callback.finish_typing_output(true);
            *sample = start.elapsed().as_nanos();
            assert!(pcm.iter().flatten().all(|value| value.is_finite()));
        }
        samples.sort_unstable();
        println!(
            "linux typing profile kind={kind} voices=12 rate=48000 block=128 samples={} p50_ns={} p99_ns={} max_ns={}",
            samples.len(),
            samples[1000],
            samples[1980],
            samples[1999]
        );
    }
}
