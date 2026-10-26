use super::*;
use hex_project::{Command, Created};

fn fixture() -> (Session, DeviceId, Arc<PreparedRevision>) {
    let mut session = Session::new(48_000, 32, 0).unwrap();
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::AddTrack {
                name: "test".into(),
            },
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    let state =
        hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.eq8")).unwrap();
    let (Created::Device(device), prepared) = session
        .command_prepared_revision(
            Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    (session, device, prepared)
}
fn values(frequency: f32, gain: f32) -> DeviceParameterSnapshot {
    DeviceParameterSnapshot::new(&[
        (
            ParameterId::from_static(18),
            ParameterValue::Float(frequency),
        ),
        (ParameterId::from_static(19), ParameterValue::Float(gain)),
    ])
    .unwrap()
}
fn ack(request: GestureRequest) -> GestureAck {
    GestureAck {
        preview_retained: false,
        source_lost: false,
        key: request.key(),
        phase: request.phase(),
        sequence: request.sequence(),
        accepted: true,
        reason: None,
    }
}

#[test]
fn snapshot_atomic_preview_commit_cancel_noop_and_history() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let mut machine = GestureMachine::default();
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginSnapshot {
                    key,
                    device,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    let change = machine.handle(
        &mut session,
        GestureRequest::ChangeSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 1,
            snapshot: values(1500.0, 6.0),
        },
        Some(&prepared),
        1,
    );
    assert!(change.ack.accepted);
    assert_ne!(change.preview_project.unwrap().as_ref(), &original);
    assert_eq!(session.project(), &original);
    let cancel = machine.handle(
        &mut session,
        GestureRequest::Cancel { key },
        Some(&prepared),
        1,
    );
    assert!(cancel.ack.accepted);
    assert!(Arc::ptr_eq(cancel.playback.as_ref().unwrap(), &prepared));
    let key = GestureKey {
        generation: 1,
        token: 2,
    };
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginSnapshot {
                    key,
                    device,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::CommitSnapshot {
                    touched_parameters: crate::device_gesture::TouchedParameters::default(),
                    key,
                    sequence: 1,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .canonical_changed
    );
    let committed = session.project().clone();
    let encoded = hex_storage::encode(&committed).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap(), committed);
    assert!(session.undo());
    assert_eq!(session.project(), &original);
    assert!(session.redo());
    assert_eq!(session.project(), &committed);
    session
        .commit_device_snapshot(device, &values(1500.0, 6.0), Some(&prepared))
        .unwrap();
    assert!(session.undo());
    assert_eq!(session.project(), &original);
}

#[test]
fn snapshot_stale_domain_and_parameter_refusal_preserves_overlay() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let mut machine = GestureMachine::default();
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    assert!(
        !machine
            .handle(
                &mut session,
                GestureRequest::BeginSnapshot {
                    key,
                    device,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                2
            )
            .ack
            .accepted
    );
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginSnapshot {
                    key,
                    device,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    for (sequence, snapshot) in [
        (0, values(1500.0, 6.0)),
        (1, values(-1.0, 6.0)),
        (
            1,
            DeviceParameterSnapshot::new(&[(
                ParameterId::from_static(18),
                ParameterValue::Float(1500.0),
            )])
            .unwrap(),
        ),
    ] {
        assert!(
            !machine
                .handle(
                    &mut session,
                    GestureRequest::ChangeSnapshot {
                        touched_parameters: TouchedParameters::default(),
                        key,
                        sequence,
                        snapshot
                    },
                    Some(&prepared),
                    1
                )
                .ack
                .accepted
        );
    }
    assert_eq!(session.project(), &original);
    assert_eq!(machine.snapshot().unwrap().sequence, 0);
}

#[test]
fn snapshot_save_finalization_rebases_latest_unrelated_state() {
    let (mut session, device, prepared) = fixture();
    let mut machine = GestureMachine::default();
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginSnapshot {
                    key,
                    device,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::ChangeSnapshot {
                    touched_parameters: crate::device_gesture::TouchedParameters::default(),
                    key,
                    sequence: 1,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    let (_, unrelated) = session
        .command_prepared_revision(
            Command::SetDeviceBypass {
                device,
                bypassed: true,
            },
            Some(&prepared),
        )
        .unwrap();
    assert!(
        machine
            .rebase(&mut session, Some(&unrelated), 1)
            .unwrap()
            .ack
            .accepted
    );
    // The service's explicit Save/Export barrier calls finish before encoding.
    assert!(
        machine
            .finish(&mut session, Some(&unrelated), 1)
            .unwrap()
            .ack
            .accepted
    );
    assert!(machine.snapshot().is_none());
    let saved = hex_storage::decode(&hex_storage::encode(session.project()).unwrap()).unwrap();
    assert!(saved.device(device).unwrap().bypassed());
    assert_eq!(&saved, session.project());
    assert_eq!(
        saved.device(device).unwrap(),
        session.project().device(device).unwrap()
    );
    assert!(session.undo());
    // Undo only the stroke; the independent bypass command remains authored.
    assert_ne!(
        session.project().device(device).unwrap().state(),
        saved.device(device).unwrap().state()
    );
    assert!(session.project().device(device).unwrap().bypassed());
    assert!(session.undo());
    assert!(!session.project().device(device).unwrap().bypassed());
}

#[test]
fn snapshot_control_storage_and_coalescing_are_finitely_bounded() {
    use std::{hint::black_box, mem::size_of, time::Instant};
    let (_, device, _) = fixture();
    let entries: Vec<_> = (1..=64)
        .map(|id| (ParameterId::from_static(id), ParameterValue::Float(0.5)))
        .collect();
    let snapshot = DeviceParameterSnapshot::new(&entries).unwrap();
    let mut client = GestureClient::default();
    client.begin_snapshot(1, device, snapshot).unwrap();
    let start = Instant::now();
    for _ in 0..100_000 {
        client.change_snapshot(black_box(snapshot)).unwrap();
    }
    println!(
        "snapshot={} request={} overlay={} client={} service_request={} queue_slots=32 queue_payload_bytes={} coalesce100k_ns={}",
        size_of::<DeviceParameterSnapshot>(),
        size_of::<GestureRequest>(),
        size_of::<GestureSnapshot>(),
        size_of::<GestureClient>(),
        size_of::<crate::service::Request>(),
        32 * size_of::<crate::service::Request>(),
        start.elapsed().as_nanos()
    );
    // The largest inline intent is now the 256-member frozen gain cohort.
    // It stays control-side; the same 32-slot FIFO and coalescing rules apply.
    assert!(size_of::<GestureRequest>() <= 3200);
    assert!(size_of::<crate::service::Request>() <= 3200);
    assert!(32 * size_of::<crate::service::Request>() <= 102_400);
    assert!(size_of::<GestureClient>() < 20_480);
    assert_eq!(client.snapshot().unwrap().sequence, 100_000);
    assert!(client.latest_change.is_some());
    assert!(client.in_flight.is_none());
}

#[test]
fn snapshot_removed_owner_and_rapid_reversal_do_not_commit_stale_values() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let mut machine = GestureMachine::default();
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    machine.handle(
        &mut session,
        GestureRequest::BeginSnapshot {
            key,
            device,
            snapshot: values(1500.0, 6.0),
        },
        Some(&prepared),
        1,
    );
    let a = machine.handle(
        &mut session,
        GestureRequest::ChangeSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 1,
            snapshot: values(1500.0, 6.0),
        },
        Some(&prepared),
        1,
    );
    let b = machine.handle(
        &mut session,
        GestureRequest::ChangeSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 2,
            snapshot: values(1200.0, 2.0),
        },
        Some(&prepared),
        1,
    );
    assert!(a.ack.accepted && b.ack.accepted);
    assert!(
        !machine
            .handle(
                &mut session,
                GestureRequest::ChangeSnapshot {
                    touched_parameters: crate::device_gesture::TouchedParameters::default(),
                    key,
                    sequence: 1,
                    snapshot: values(1500.0, 6.0)
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    assert_eq!(session.project(), &original);
    let (_, removed) = session
        .command_prepared_revision(Command::RemoveDevice(device), Some(&prepared))
        .unwrap();
    assert!(machine.rebase(&mut session, Some(&removed), 1).is_some());
    assert!(machine.snapshot().is_none());
    assert!(
        !machine
            .handle(
                &mut session,
                GestureRequest::CommitSnapshot {
                    touched_parameters: crate::device_gesture::TouchedParameters::default(),
                    key,
                    sequence: 3,
                    snapshot: values(1600.0, 8.0)
                },
                Some(&removed),
                1
            )
            .ack
            .accepted
    );
    assert!(session.project().device(device).is_none());
}

#[test]
fn snapshot_coalesces_and_retries_queue_refusal_without_losing_terminal() {
    let (_, device, _) = fixture();
    let mut client = GestureClient::default();
    client
        .begin_snapshot(1, device, values(1000.0, 0.0))
        .unwrap();
    let begin = client.next_request().unwrap();
    assert_eq!(client.next_request(), Some(begin));
    client.mark_submitted();
    assert!(client.acknowledge(&ack(begin)));
    for index in 1..=100 {
        client
            .change_snapshot(values(1000.0 + index as f32, 1.0))
            .unwrap();
    }
    let change = client.next_request().unwrap();
    assert!(
        matches!(change, GestureRequest::ChangeSnapshot { sequence: 100, snapshot, .. } if snapshot == values(1100.0, 1.0))
    );
    client.finish_snapshot(values(1200.0, 2.0)).unwrap();
    assert_eq!(client.next_request(), Some(change));
    client.mark_submitted();
    assert!(client.acknowledge(&ack(change)));
    assert!(matches!(
        client.next_request(),
        Some(GestureRequest::CommitSnapshot { sequence: 101, .. })
    ));
    assert!(client.change_snapshot(values(1300.0, 3.0)).is_err());
    assert!(
        client
            .change_pair(ParameterPairValue {
                first: ParameterValue::Float(1.0),
                second: ParameterValue::Float(1.0)
            })
            .is_err()
    );
}

#[test]
fn actual_harmonic_stroke_is_one_history_edit_and_hidden_partials_survive_reopen() {
    let mut session = Session::new(48000, 32, 0).unwrap();
    let (Created::Track(track), prepared) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "harmonics".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
            None,
        )
        .unwrap()
    else {
        panic!()
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let (_, prepared) = session
        .command_prepared_revision(
            Command::ReplaceDeviceState { device, state },
            Some(&prepared),
        )
        .unwrap();
    let original = session.project().clone();
    let spectrum = |amplitude: f32| {
        DeviceParameterSnapshot::new(&std::array::from_fn::<_, 64, _>(|i| {
            (
                ParameterId::from_static(93 + i as u32),
                ParameterValue::Float(if i == 63 { amplitude } else { 0.0 }),
            )
        }))
        .unwrap()
    };
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    for request in [
        GestureRequest::BeginSnapshot {
            key,
            device,
            snapshot: spectrum(0.0),
        },
        GestureRequest::ChangeSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 1,
            snapshot: spectrum(0.25),
        },
        GestureRequest::ChangeSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 2,
            snapshot: spectrum(0.75),
        },
        GestureRequest::CommitSnapshot {
            touched_parameters: crate::device_gesture::TouchedParameters::default(),
            key,
            sequence: 3,
            snapshot: spectrum(0.5),
        },
    ] {
        assert!(
            machine
                .handle(&mut session, request, Some(&prepared), 1)
                .ack
                .accepted
        );
    }
    let authored = session.project().clone();
    assert!(session.undo());
    assert_eq!(session.project(), &original);
    assert!(session.redo());
    assert_eq!(session.project(), &authored);
    let current = session.project().device(device).unwrap().state();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        current,
        ParameterId::from_static(349),
        ParameterValue::Choice(0),
    )
    .unwrap();
    session
        .command_prepared_revision(Command::ReplaceDeviceState { device, state }, None)
        .unwrap();
    let reopened = hex_storage::decode(&hex_storage::encode(session.project()).unwrap()).unwrap();
    assert_eq!(&reopened, session.project());
    let view = hex_engine::native_device_view_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        reopened.device(device).unwrap().state(),
    )
    .unwrap();
    assert_eq!(
        view.parameters
            .iter()
            .find(|p| p.descriptor.id.value() == 156)
            .unwrap()
            .value,
        ParameterValue::Float(0.5)
    );
}
