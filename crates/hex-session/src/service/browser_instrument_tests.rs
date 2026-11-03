//! Browser Enter resolves native defaults only on the worker and preserves edit publication.
use super::*;
use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
use hex_lattice_contract::{DeviceTypeId, ParameterValue};

fn event(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("event: worker event absent: {error}"),
    }
}

fn request(revision: u64, kind: &'static str) -> Request {
    Request::InsertBrowserInstrumentTrack {
        project_generation: 0,
        revision,
        class: hex_engine::NativeDeviceClass::Instrument,
        type_id: DeviceTypeId::from_static(kind),
    }
}

#[test]
fn browser_enter_worker_selected_defaults_created_track_and_one_undo_redo() {
    let service = Service::new(48_000, 32, 0).unwrap();
    let original = event(&service).state;
    for kind in [
        "hex.instrument.fm4",
        "hex.instrument.sine",
        "hex.instrument.sampler",
    ] {
        let before = if kind == "hex.instrument.fm4" {
            Arc::clone(&original)
        } else {
            service.submit(Request::Undo).unwrap();
            let before = event(&service);
            assert_eq!(before.state.project.tracks(), original.project.tracks());
            before.state
        };
        service.submit(request(before.revision, kind)).unwrap();
        let added = event(&service);
        assert!(added.error.is_none(), "{:?}", added.error);
        let track = &added.state.project.tracks()[0];
        assert!(matches!(added.created, Some(hex_project::Created::Track(id)) if id == track.id()));
        assert_eq!(track.name(), kind.trim_start_matches("hex.instrument."));
        assert!(track.name_is_automatic());
        assert_eq!(track.ordered_device_chain().count(), 1);
        assert_eq!(
            track.instrument().unwrap().state(),
            &hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                DeviceTypeId::from_static(kind)
            )
            .unwrap()
        );
        assert!(added.playback.is_some());
        service.submit(Request::Undo).unwrap();
        let undone = event(&service);
        assert_eq!(undone.state.project.tracks(), original.project.tracks());
        service.submit(Request::Redo).unwrap();
        let redone = event(&service);
        assert_eq!(redone.state.project, added.state.project);
    }
}

#[test]
fn browser_enter_worker_refuses_unknown_wrong_class_and_stale_without_mutation() {
    let service = Service::new(48_000, 32, 0).unwrap();
    let initial = event(&service).state;
    for (generation, revision, class, kind) in [
        (
            0,
            initial.revision,
            hex_engine::NativeDeviceClass::Instrument,
            "missing.instrument",
        ),
        (
            0,
            initial.revision,
            hex_engine::NativeDeviceClass::Instrument,
            "hex.utility",
        ),
        (
            0,
            initial.revision,
            hex_engine::NativeDeviceClass::Effect,
            "hex.utility",
        ),
        (
            1,
            initial.revision,
            hex_engine::NativeDeviceClass::Instrument,
            "hex.instrument.sine",
        ),
        (
            0,
            initial.revision + 1,
            hex_engine::NativeDeviceClass::Instrument,
            "hex.instrument.sine",
        ),
    ] {
        service
            .submit(Request::InsertBrowserInstrumentTrack {
                project_generation: generation,
                revision,
                class,
                type_id: DeviceTypeId::from_static(kind),
            })
            .unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.project);
        assert_eq!(refused.state.revision, initial.revision);
        assert_eq!(refused.state.can_undo, initial.can_undo);
        assert_eq!(refused.state.can_redo, initial.can_redo);
        assert!(refused.created.is_none());
        assert!(refused.playback.is_none());
        assert!(Arc::ptr_eq(
            refused.state.canonical_audio_source.as_ref().unwrap(),
            initial.canonical_audio_source.as_ref().unwrap()
        ));
    }
}

#[test]
fn browser_enter_worker_refusal_preserves_active_gesture_and_history() {
    let service = Service::new(48_000, 32, 0).unwrap();
    event(&service);
    service.submit(request(0, "hex.instrument.sine")).unwrap();
    let initial = event(&service).state;
    let key = GestureKey {
        generation: 0,
        token: 37,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain {
                track: initial.project.tracks()[0].id(),
            },
        }))
        .unwrap();
    assert!(event(&service).gesture_ack.unwrap().accepted);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }))
        .unwrap();
    let preview = event(&service);
    assert!(preview.gesture_ack.unwrap().accepted);
    for (revision, kind) in [
        (initial.revision + 1, "hex.instrument.sine"),
        (initial.revision, "missing.instrument"),
        (initial.revision, "hex.instrument.fm4"),
    ] {
        service.submit(request(revision, kind)).unwrap();
        let refused = event(&service);
        assert!(refused.error.is_some());
        assert!(
            refused.gesture_ack.is_none(),
            "refusal cannot finalize the live draft"
        );
        assert!(refused.active_device_gesture.is_some());
        assert_eq!(refused.state.project, initial.project);
        assert_eq!(refused.state.revision, initial.revision);
        assert_eq!(refused.state.can_undo, initial.can_undo);
        assert!(refused.playback.is_none());
    }
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    let cancelled = event(&service);
    assert!(cancelled.gesture_ack.unwrap().accepted);
    assert_eq!(cancelled.state.project, initial.project);
    assert_eq!(cancelled.state.revision, initial.revision);
    service
        .submit(request(cancelled.state.revision, "hex.instrument.fm4"))
        .unwrap();
    let inserted = event(&service);
    assert!(inserted.error.is_none());
    assert_eq!(inserted.state.project.tracks().len(), 2);
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        event(&service).state.project.tracks(),
        initial.project.tracks()
    );
}

#[test]
fn existing_track_drops_rename_automatic_names_with_instruments_only() {
    use hex_engine::NativeDeviceClass::{Effect, Instrument};
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let drop = |editor: &mut hex_project::Editor, track, class, kind: &'static str| {
        let instrument = editor
            .project()
            .track(track)
            .and_then(hex_project::Track::instrument)
            .map(hex_project::Device::id);
        let command = arrangement_device_command(
            editor.project(),
            class,
            DeviceTypeId::from_static(kind),
            ArrangementDeviceDestination::ExistingTrack { track, instrument },
        )
        .unwrap();
        if class == Effect {
            assert!(matches!(command, Command::InsertDevice { .. }));
        }
        editor.execute(command).unwrap();
        let track = editor.project().track(track).unwrap();
        (track.name().to_owned(), track.name_is_automatic())
    };
    let Ok(hex_project::Created::Track(default)) =
        editor.execute(Command::InsertDefaultTrack { before_track: None })
    else {
        panic!("default track")
    };
    assert_eq!(
        drop(&mut editor, default, Effect, "hex.utility"),
        ("track 1".into(), true)
    );
    assert_eq!(
        drop(&mut editor, default, Instrument, "hex.instrument.sampler"),
        ("sampler".into(), true)
    );
    assert_eq!(
        drop(&mut editor, default, Instrument, "hex.instrument.sine"),
        ("sine".into(), true)
    );
    let Ok(hex_project::Created::Track(custom)) = editor.execute(Command::AddTrack {
        name: "track 2".into(),
    }) else {
        panic!("custom track")
    };
    assert_eq!(
        drop(&mut editor, custom, Instrument, "hex.instrument.sampler"),
        ("track 2".into(), false)
    );
}
