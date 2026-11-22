//! Return routing, device ownership, history and schema-25 service acceptance.
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, TrackDeviceRef};
use hex_session::service::{Event, Request, Service};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("service event missing: {error}"),
        }
    }
}

fn edit(service: &Service, command: Command) -> Event {
    service.submit(Request::Edit(command)).expect("submit edit");
    let event = wait(service);
    assert!(event.error.is_none(), "edit rejected: {:?}", event.error);
    assert!(
        event.playback.is_some(),
        "accepted edit must publish playback"
    );
    event
}

fn rendered(event: &mut Event) -> Vec<[f32; 2]> {
    let mut playback = event.playback.take().expect("compiled playback");
    playback.apply(hex_engine::playback::PlaybackCommand::PlayFrom(0));
    let mut frames = [[0.0; 2]; 256];
    playback.render(&mut frames).expect("render playback");
    frames.to_vec()
}

fn temp_project_path() -> (std::path::PathBuf, TempDir) {
    let directory = TempDir::new();
    (directory.0.join("return-project.hex"), directory)
}

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-session-return-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn return_workflow_routes_pcm_edits_devices_history_and_current_schema_roundtrip() {
    use hex_engine::NativeDeviceClass;

    let service = Service::new(8_000, 16, 0).expect("service");
    let initial = wait(&service);
    assert!(initial.playback.is_some());
    let track_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("midi track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).expect("clip length"),
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("midi clip expected")
    };
    let note_event = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 120,
        },
    );
    let dry = rendered(&mut { note_event });
    assert!(dry.iter().any(|frame| frame[0].abs() > 0.001));

    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "send bus".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return track expected")
    };
    let sent_event = edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        },
    );
    assert_eq!(
        sent_event
            .state
            .project
            .track(track)
            .expect("source track")
            .sends()[0]
            .gain(),
        0.5
    );
    let sent_pcm = rendered(&mut { sent_event });
    assert_ne!(sent_pcm, dry, "the compiled send contributes to Main PCM");

    service.submit(Request::Undo).expect("undo send");
    let send_undone = wait(&service);
    assert!(send_undone.error.is_none(), "{:?}", send_undone.error);
    assert!(
        send_undone
            .state
            .project
            .track(track)
            .unwrap()
            .sends()
            .is_empty()
    );
    assert_eq!(rendered(&mut { send_undone }), dry);
    service.submit(Request::Redo).expect("redo send");
    let send_redone = wait(&service);
    assert!(send_redone.error.is_none(), "{:?}", send_redone.error);
    assert_eq!(
        send_redone.state.project.track(track).unwrap().sends()[0].gain(),
        0.5
    );

    let utility = DeviceTypeId::from_static("hex.utility");
    let utility_state = || {
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, utility)
            .expect("Utility default state")
    };
    let first_insert = edit(
        &service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: utility_state(),
        },
    );
    let Some(Created::Device(first)) = first_insert.created else {
        panic!("first return effect expected")
    };
    let patched = {
        service
            .submit(Request::SetDeviceParameter {
                device: first,
                parameter: ParameterId::from_static(1),
                value: ParameterValue::Float(2.0),
            })
            .expect("patch Utility gain");
        wait(&service)
    };
    assert!(
        patched.error.is_none(),
        "return Utility patch: {:?}",
        patched.error
    );
    assert!(patched.playback.is_some());
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            patched.state.project.device(first).unwrap().state(),
            ParameterId::from_static(1),
        ),
        Ok(ParameterValue::Float(2.0))
    );
    let gained_pcm = rendered(&mut { patched });
    assert_ne!(
        gained_pcm, sent_pcm,
        "return Utility gain changes rendered PCM"
    );

    let second_insert = edit(
        &service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: utility_state(),
        },
    );
    let Some(Created::Device(second)) = second_insert.created else {
        panic!("second return effect expected")
    };
    let inverted = {
        service
            .submit(Request::SetDeviceParameter {
                device: second,
                parameter: ParameterId::from_static(5),
                value: ParameterValue::Bool(true),
            })
            .expect("patch Utility polarity");
        wait(&service)
    };
    assert!(
        inverted.error.is_none(),
        "return Utility patch: {:?}",
        inverted.error
    );
    assert!(inverted.playback.is_some());
    let reordered = edit(
        &service,
        Command::MoveReturnDevice {
            device: second,
            before: Some(first),
        },
    );
    let return_devices = reordered
        .state
        .project
        .return_tracks()
        .iter()
        .find(|candidate| candidate.id() == return_track)
        .expect("return owner")
        .devices();
    assert_eq!(
        return_devices
            .iter()
            .map(|device| device.id())
            .collect::<Vec<_>>(),
        vec![second, first],
        "same-return reorder follows stable device identity"
    );
    let ordered_pcm = rendered(&mut { reordered });

    let bypassed = edit(
        &service,
        Command::SetDeviceBypass {
            device: second,
            bypassed: true,
        },
    );
    assert!(bypassed.state.project.device(second).unwrap().bypassed());
    let bypassed_pcm = rendered(&mut { bypassed });
    assert_ne!(
        bypassed_pcm, ordered_pcm,
        "bypass changes the return contribution"
    );
    service.submit(Request::Undo).expect("undo bypass");
    let unbypassed = wait(&service);
    assert!(!unbypassed.state.project.device(second).unwrap().bypassed());
    assert_eq!(rendered(&mut { unbypassed }), ordered_pcm);
    service.submit(Request::Redo).expect("redo bypass");
    let bypass_redone = wait(&service);
    assert!(
        bypass_redone
            .state
            .project
            .device(second)
            .unwrap()
            .bypassed()
    );

    let removed = edit(&service, Command::RemoveDevice(first));
    assert!(removed.state.project.device(first).is_none());
    assert_eq!(removed.state.project.return_tracks()[0].devices().len(), 1);
    let removed_pcm = rendered(&mut { removed });
    assert_eq!(
        removed_pcm, sent_pcm,
        "removing Utility restores plain send routing"
    );
    service
        .submit(Request::Undo)
        .expect("undo return effect removal");
    let removal_undone = wait(&service);
    assert!(removal_undone.state.project.device(first).is_some());
    assert!(removal_undone.playback.is_some());
    service
        .submit(Request::Redo)
        .expect("redo return effect removal");
    let final_state = wait(&service);
    assert!(final_state.error.is_none(), "{:?}", final_state.error);
    assert!(final_state.state.project.device(first).is_none());
    assert!(final_state.playback.is_some());

    // Existing regular-track ownership remains one movable instrument slot.
    let regular_utility = edit(
        &service,
        Command::InsertDevice {
            track,
            before: None,
            state: utility_state(),
        },
    );
    let Some(Created::Device(regular_effect)) = regular_utility.created else {
        panic!("regular effect expected")
    };
    let instrument = regular_utility
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .expect("one default instrument")
        .id();
    let instrument_moved = edit(
        &service,
        Command::MoveTrackDevice {
            device: instrument,
            track,
            before: None,
        },
    );
    let chain = instrument_moved
        .state
        .project
        .track(track)
        .unwrap()
        .ordered_device_chain()
        .collect::<Vec<_>>();
    assert!(matches!(chain[0], TrackDeviceRef::Effect(device) if device.id() == regular_effect));
    assert!(matches!(chain[1], TrackDeviceRef::Instrument(device) if device.id() == instrument));
    assert_eq!(
        chain
            .iter()
            .filter(|node| matches!(node, TrackDeviceRef::Instrument(_)))
            .count(),
        1
    );
    let expected_pcm = rendered(&mut { instrument_moved });

    let (path, _directory) = temp_project_path();
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save schema 25 project");
    let saved = wait(&service);
    assert!(saved.error.is_none(), "save: {:?}", saved.error);
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);
    let bytes = fs::read(&path).expect("saved document bytes");
    let document: serde_json::Value = serde_json::from_slice(&bytes).expect("project json");
    assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(document["routing"], serde_json::json!([]));
    assert!(document["import_provenance"].is_null());
    let persisted = hex_storage::load(&path).expect("load schema 25 project");
    assert_eq!(persisted, *saved.state.project);
    assert_eq!(persisted.return_tracks().len(), 1);
    assert_eq!(persisted.track(track).unwrap().sends()[0].gain(), 0.5);
    assert_eq!(persisted.return_tracks()[0].devices()[0].id(), second);
    assert!(persisted.device(second).unwrap().bypassed());
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            persisted.device(second).unwrap().state(),
            ParameterId::from_static(5),
        ),
        Ok(ParameterValue::Bool(true))
    );

    let reopened_service = Service::new(8_000, 16, 0).expect("reopen service");
    wait(&reopened_service);
    reopened_service
        .submit(Request::Open(path))
        .expect("open saved return project");
    let reopened = wait(&reopened_service);
    assert!(reopened.error.is_none(), "open: {:?}", reopened.error);
    assert!(reopened.open_complete && reopened.project_replaced);
    assert_eq!(reopened.state.project, saved.state.project);
    assert!(!reopened.state.unsaved);
    assert_eq!(rendered(&mut { reopened }), expected_pcm);
}

#[test]
fn removing_return_with_active_parameter_gesture_cancels_stale_owner() {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};

    let service = Service::new(8_000, 16, 0).expect("service");
    wait(&service);
    let track_event = edit(
        &service,
        Command::AddTrack {
            name: "regular source".into(),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "return".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return expected")
    };
    let inserted = edit(
        &service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .expect("Utility state"),
        },
    );
    let Some(Created::Device(device)) = inserted.created else {
        panic!("return device expected")
    };
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: ParameterId::from_static(1),
            },
        }))
        .expect("begin return device edit");
    let begun = wait(&service);
    assert!(
        begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.reason.is_none())
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }))
        .expect("preview return device edit");
    let preview = wait(&service);
    assert!(preview.device_preview.is_some());
    assert_eq!(
        preview
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
        DeviceTypeId::from_static("hex.utility")
    );
    service
        .submit(Request::Edit(Command::RemoveReturnTrack(return_track)))
        .expect("remove active return owner");
    let removed = wait(&service);
    assert!(
        removed.error.is_none(),
        "remove return: {:?}",
        removed.error
    );
    assert!(removed.state.project.return_tracks().is_empty());
    assert!(removed.state.project.device(device).is_none());
    assert!(removed.device_preview.is_none());
    assert!(removed.active_device_gesture.is_none());
    assert!(removed.gesture_ack.as_ref().is_some_and(|ack| {
        ack.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("removed or replaced"))
    }));
    assert!(removed.state.project.track(track).is_some());
}

#[test]
fn eq_eight_processes_on_regular_and_return_device_chains() {
    use hex_engine::NativeDeviceClass;

    let service = Service::new(48_000, 16, 0).expect("service");
    wait(&service);
    let track_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "eq source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).expect("clip length"),
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("clip expected")
    };
    let note_event = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).expect("note duration"),
            pitch: 60,
            channel: 0,
            velocity: 120,
        },
    );
    let dry = rendered(&mut { note_event });

    let mut eq = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )
    .expect("eq eight default state");
    for (parameter, value) in [
        (ParameterId::from_static(1), ParameterValue::Bool(true)),
        (ParameterId::from_static(3), ParameterValue::Float(261.6)),
        (ParameterId::from_static(4), ParameterValue::Float(12.0)),
    ] {
        eq = hex_engine::patch_native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            &eq,
            parameter,
            value,
        )
        .expect("eq eight parameter");
    }
    let regular_insert = edit(
        &service,
        Command::InsertDevice {
            track,
            before: None,
            state: eq.clone(),
        },
    );
    let regular_pcm = rendered(&mut { regular_insert });
    assert_ne!(regular_pcm, dry, "regular-track EQ must affect playback");

    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "eq return".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return track expected")
    };
    let sent_event = edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        },
    );
    let sent_without_eq = rendered(&mut { sent_event });
    let return_insert = edit(
        &service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: eq,
        },
    );
    let return_pcm = rendered(&mut { return_insert });
    assert_ne!(
        return_pcm, sent_without_eq,
        "registered EQ must process Return contributions"
    );
}

#[test]
fn return_send_gesture_previews_transiently_and_save_barrier_commits_once() {
    use hex_session::device_gesture::{GestureKey, GesturePhase, GestureRequest, GestureTarget};

    let service = Service::new(8_000, 16, 0).expect("service");
    wait(&service);
    let track_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("midi track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).expect("clip length"),
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("midi clip expected")
    };
    edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 120,
        },
    );
    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "send bus".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return track expected")
    };
    let canonical_send = edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.25,
        },
    );
    let canonical_revision = canonical_send.state.revision;
    let canonical_pcm = rendered(&mut { canonical_send });

    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let target = GestureTarget::ReturnSend {
        track,
        return_track,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target,
        }))
        .expect("begin send gesture");
    let begun = wait(&service);
    assert!(
        begun.gesture_ack.as_ref().is_some_and(|ack| {
            ack.key == key && ack.phase == GesturePhase::Begin && ack.accepted
        })
    );
    assert_eq!(
        begun.active_device_gesture.map(|snapshot| snapshot.target),
        Some(target)
    );

    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.75),
        }))
        .expect("preview send level");
    let preview = wait(&service);
    assert!(preview.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    assert_eq!(preview.state.revision, canonical_revision);
    assert!(preview.state.unsaved);
    assert_eq!(
        preview.state.project.track(track).unwrap().sends()[0].gain(),
        0.25,
        "preview cannot mutate canonical project state"
    );
    let overlay = preview
        .device_preview
        .as_ref()
        .expect("prepared send preview");
    assert_eq!(
        overlay.state.project.track(track).unwrap().sends()[0].gain(),
        0.75
    );
    assert_ne!(
        rendered(&mut { preview }),
        canonical_pcm,
        "preview graph changes audible PCM without a canonical edit"
    );

    let (path, _directory) = temp_project_path();
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save finalizes the active numeric gesture");
    let saved = wait(&service);
    assert!(saved.error.is_none(), "save: {:?}", saved.error);
    assert!(saved.save_complete && !saved.state.unsaved);
    assert!(saved.gesture_ack.as_ref().is_some_and(|ack| {
        ack.key == key && ack.phase == GesturePhase::Commit && ack.accepted
    }));
    assert_eq!(saved.state.revision, canonical_revision + 1);
    assert_eq!(
        saved.state.project.track(track).unwrap().sends()[0].gain(),
        0.75
    );
    assert_eq!(
        hex_storage::load(&path).expect("saved canonical send project"),
        *saved.state.project
    );

    service
        .submit(Request::Undo)
        .expect("undo one send gesture");
    let undone = wait(&service);
    assert!(undone.error.is_none(), "undo: {:?}", undone.error);
    assert_eq!(
        undone.state.project.track(track).unwrap().sends()[0].gain(),
        0.25,
        "gesture produces one canonical undo step"
    );
    service
        .submit(Request::Redo)
        .expect("redo one send gesture");
    let redone = wait(&service);
    assert_eq!(
        redone.state.project.track(track).unwrap().sends()[0].gain(),
        0.75
    );
}

#[test]
fn return_send_gesture_cancel_rebases_after_same_target_and_unrelated_edits() {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};

    let service = Service::new(8_000, 16, 0).expect("service");
    wait(&service);
    let track_event = edit(
        &service,
        Command::AddTrack {
            name: "source".into(),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "send bus".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return expected")
    };
    edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.25,
        },
    );
    let target = GestureTarget::ReturnSend {
        track,
        return_track,
    };
    let first_key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key: first_key,
            target,
        }))
        .expect("begin send gesture");
    let _ = wait(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key: first_key,
            sequence: 1,
            value: ParameterValue::Float(0.75),
        }))
        .expect("preview send");
    let _ = wait(&service);

    let unrelated = edit(
        &service,
        Command::ReturnGain {
            return_track,
            gain: 1.5,
        },
    );
    assert_eq!(unrelated.state.project.return_tracks()[0].gain(), 1.5);
    assert_eq!(
        unrelated.state.project.track(track).unwrap().sends()[0].gain(),
        0.25
    );
    assert_eq!(
        unrelated
            .device_preview
            .as_ref()
            .expect("rebased send overlay")
            .state
            .project
            .track(track)
            .unwrap()
            .sends()[0]
            .gain(),
        0.75
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel {
            key: first_key,
        }))
        .expect("cancel after unrelated edit");
    let canceled_unrelated = wait(&service);
    assert!(
        canceled_unrelated
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted)
    );
    assert_eq!(
        canceled_unrelated.state.project.return_tracks()[0].gain(),
        1.5
    );
    assert_eq!(
        canceled_unrelated
            .state
            .project
            .track(track)
            .unwrap()
            .sends()[0]
            .gain(),
        0.25,
        "cancel restores latest canonical state, including unrelated changes"
    );

    let second_key = GestureKey {
        generation: 0,
        token: 2,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key: second_key,
            target,
        }))
        .expect("begin second send gesture");
    let _ = wait(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key: second_key,
            sequence: 1,
            value: ParameterValue::Float(0.8),
        }))
        .expect("preview second send");
    let _ = wait(&service);
    let same_target = edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        },
    );
    assert_eq!(
        same_target.state.project.track(track).unwrap().sends()[0].gain(),
        0.5
    );
    assert_eq!(
        same_target
            .device_preview
            .as_ref()
            .expect("same-target edit rebases active overlay")
            .state
            .project
            .track(track)
            .unwrap()
            .sends()[0]
            .gain(),
        0.8
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel {
            key: second_key,
        }))
        .expect("cancel after same-target edit");
    let canceled_same_target = wait(&service);
    assert_eq!(
        canceled_same_target
            .state
            .project
            .track(track)
            .unwrap()
            .sends()[0]
            .gain(),
        0.5,
        "cancel cannot undo or resurrect over a newer canonical send edit"
    );
}

#[test]
fn rejected_send_gesture_values_and_removed_targets_never_change_canonical_state() {
    use hex_session::device_gesture::{GestureKey, GesturePhase, GestureRequest, GestureTarget};

    let service = Service::new(8_000, 16, 0).expect("service");
    wait(&service);
    let track_event = edit(
        &service,
        Command::AddTrack {
            name: "source".into(),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let return_event = edit(
        &service,
        Command::AddReturnTrack {
            name: "send bus".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = return_event.created else {
        panic!("return expected")
    };
    let saved_revision = edit(
        &service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.25,
        },
    )
    .state
    .revision;
    let (path, _directory) = temp_project_path();
    service
        .submit(Request::SaveNew(path))
        .expect("establish clean project");
    let saved = wait(&service);
    assert!(saved.save_complete && !saved.state.unsaved);
    let target = GestureTarget::ReturnSend {
        track,
        return_track,
    };
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target,
        }))
        .expect("begin invalid-value gesture");
    let _ = wait(&service);
    for (sequence, value) in [f32::NAN, f32::INFINITY, -0.01, 2.01]
        .into_iter()
        .enumerate()
    {
        service
            .submit(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: sequence as u64 + 1,
                value: ParameterValue::Float(value),
            }))
            .expect("submit invalid send preview");
        let event = wait(&service);
        assert!(event.gesture_ack.as_ref().is_some_and(|ack| !ack.accepted));
        assert!(event.device_preview.is_none());
        assert_eq!(event.state.revision, saved_revision);
        assert!(!event.state.unsaved);
        assert_eq!(event.active_device_gesture.unwrap().sequence, 0);
    }
    service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 1,
            value: ParameterValue::Float(f32::INFINITY),
        }))
        .expect("submit rejected terminal value");
    let rejected = wait(&service);
    assert!(
        rejected
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| { ack.phase == GesturePhase::Commit && !ack.accepted })
    );
    assert!(rejected.active_device_gesture.is_none());
    assert_eq!(rejected.state.revision, saved_revision);
    assert!(!rejected.state.unsaved);
    assert_eq!(
        rejected.state.project.track(track).unwrap().sends()[0].gain(),
        0.25
    );

    let remove_key = GestureKey {
        generation: 0,
        token: 2,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key: remove_key,
            target,
        }))
        .expect("begin removal gesture");
    let _ = wait(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key: remove_key,
            sequence: 1,
            value: ParameterValue::Float(0.9),
        }))
        .expect("preview before target removal");
    let _ = wait(&service);
    let removed = edit(&service, Command::RemoveReturnTrack(return_track));
    assert!(removed.state.project.return_tracks().is_empty());
    assert!(
        removed
            .state
            .project
            .track(track)
            .unwrap()
            .sends()
            .is_empty()
    );
    assert!(removed.active_device_gesture.is_none());
    service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key: remove_key,
            sequence: 2,
            value: ParameterValue::Float(0.9),
        }))
        .expect("submit stale send terminal");
    let stale = wait(&service);
    assert!(stale.gesture_ack.as_ref().is_some_and(|ack| !ack.accepted));
    assert!(stale.state.project.return_tracks().is_empty());
    assert!(stale.state.project.track(track).unwrap().sends().is_empty());
}
