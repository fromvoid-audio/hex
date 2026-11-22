use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};

#[test]
fn toggle_is_ordered_and_undoable_for_all_device_owners() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let effect =
        || NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, vec![]).unwrap();
    let Created::Device(instrument) = editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let Created::Device(local) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: effect(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(return_effect) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: effect(),
        })
        .unwrap()
    else {
        panic!("return effect")
    };
    let Created::Device(main_effect) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: effect(),
        })
        .unwrap()
    else {
        panic!("main effect")
    };
    for device in [instrument, local, return_effect, main_effect] {
        let before = editor.project().device(device).unwrap().clone();
        assert!(!before.bypassed());
        editor
            .execute(Command::ToggleDeviceBypass { device })
            .unwrap();
        assert!(editor.project().device(device).unwrap().bypassed());
        editor
            .execute(Command::ToggleDeviceBypass { device })
            .unwrap();
        assert_eq!(editor.project().device(device).unwrap(), &before);
        assert!(editor.undo());
        assert!(editor.project().device(device).unwrap().bypassed());
        assert!(editor.undo());
        assert_eq!(editor.project().device(device).unwrap(), &before);
        assert!(editor.redo());
        assert!(editor.project().device(device).unwrap().bypassed());
        assert!(editor.redo());
        assert_eq!(editor.project().device(device).unwrap(), &before);
    }
}

#[test]
fn refused_or_stale_toggle_preserves_project_and_redo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let state =
        NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, vec![]).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device")
    };
    let before = editor.project().clone();
    assert!(
        editor
            .execute_prepared(Command::ToggleDeviceBypass { device }, |_| Err::<(), _>(
                hex_project::Invalid("refused")
            ))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let removed = editor.project().clone();
    assert!(
        editor
            .execute(Command::ToggleDeviceBypass { device })
            .is_err()
    );
    assert_eq!(editor.project(), &removed);
    assert!(editor.undo());
    assert!(editor.can_redo());
    assert!(
        editor
            .execute_prepared(Command::ToggleDeviceBypass { device }, |_| Err::<(), _>(
                hex_project::Invalid("refused")
            ))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn toggling_shared_source_updates_all_bindings_without_changing_identity() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(first) = editor
        .execute(Command::AddTrack {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Track(second) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = editor
        .execute(Command::InsertInstrument {
            track: first,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let source = editor.project().track(first).unwrap().source_id().unwrap();
    editor
        .execute(Command::BindTrackSource {
            track: second,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::ToggleDeviceBypass { device })
        .unwrap();
    for track in [first, second] {
        let row = editor.project().track(track).unwrap();
        assert_eq!(row.source_id(), Some(source));
        assert_eq!(row.instrument().unwrap().id(), device);
        assert!(row.instrument().unwrap().bypassed());
    }
    assert!(editor.undo());
    for track in [first, second] {
        assert!(
            !editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .bypassed()
        );
    }
}
