use crate::{
    Command, Created, DeviceId, Editor, Invalid, Project, SourceId, TrackId,
    default_instrument_state,
};
fn editor() -> Editor {
    Editor::new(Project::new(48000).unwrap(), 32).unwrap()
}
fn track(e: &mut Editor, name: &str) -> TrackId {
    let Created::Track(id) = e
        .execute(Command::AddMidiTrack { name: name.into() })
        .unwrap()
    else {
        panic!("track")
    };
    id
}
fn instrument(e: &mut Editor, track: TrackId) -> DeviceId {
    let Created::Device(id) = e
        .execute(Command::InsertInstrument {
            track,
            state: default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    id
}
fn source(e: &mut Editor, name: &str) -> SourceId {
    let Created::NativeSource(id) = e
        .execute(Command::CreateNativeSource {
            name: name.into(),
            devices: vec![default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    id
}
#[test]
fn instrument_creation_registers_once_and_unlink_delete_reuse_preserve_source_and_local_effects() {
    let mut e = editor();
    let a = track(&mut e, "first");
    let id = instrument(&mut e, a);
    let retained = e.project().track(a).unwrap().source_id().unwrap();
    assert_eq!(e.project().native_sources().len(), 1);
    let Created::Device(effect) = e
        .execute(Command::InsertDevice {
            track: a,
            before: None,
            state: default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let original = e.project().native_source(retained).unwrap().clone();
    e.execute(Command::BindTrackSource {
        track: a,
        source: None,
    })
    .unwrap();
    assert!(e.project().track(a).unwrap().instrument().is_none());
    assert_eq!(e.project().track(a).unwrap().devices()[0].id(), effect);
    assert_eq!(e.project().native_source(retained), Some(&original));
    assert!(e.undo());
    assert_eq!(e.project().track(a).unwrap().source_id(), Some(retained));
    assert!(e.redo());
    e.execute(Command::RemoveTrack(a)).unwrap();
    assert_eq!(e.project().native_source(retained), Some(&original));
    let b = track(&mut e, "reused");
    e.execute(Command::BindTrackSource {
        track: b,
        source: Some(retained),
    })
    .unwrap();
    assert_eq!(e.project().track(b).unwrap().instrument().unwrap().id(), id);
    assert!(e.project().track(b).unwrap().devices().is_empty());
    let saved = e.project().snapshot();
    assert_eq!(Project::from_snapshot(saved).unwrap(), *e.project());
    let bound = e.project().clone();
    e.execute(Command::RemoveTrack(b)).unwrap();
    assert_eq!(e.project().native_source(retained), Some(&original));
    assert!(e.undo());
    assert_eq!(e.project(), &bound);
    assert!(e.redo());
    assert!(e.project().track(b).is_none());
    assert_eq!(e.project().native_source(retained), Some(&original));
    assert!(e.undo());
    let before = e.project().clone();
    assert!(e.execute(Command::RemoveNativeSource(retained)).is_err());
    assert_eq!(e.project(), &before);
    e.execute(Command::RemoveInstrument { track: b }).unwrap();
    assert_eq!(e.project().native_source(retained), Some(&original));
    e.execute(Command::RemoveNativeSource(retained)).unwrap();
    assert!(e.project().native_source(retained).is_none());
    assert!(e.undo());
    assert!(e.undo());
    assert_eq!(e.project().track(b).unwrap().source_id(), Some(retained));
}
#[test]
fn source_edits_replace_bound_view_atomically_without_changing_history_or_local_effects() {
    let mut e = editor();
    let a = track(&mut e, "synth");
    let id = instrument(&mut e, a);
    let source = e.project().track(a).unwrap().source_id().unwrap();
    let Created::Device(effect) = e
        .execute(Command::InsertDevice {
            track: a,
            before: None,
            state: default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let before = e.project().clone();
    let state = default_instrument_state()
        .unwrap()
        .with_output_gain_millidb(-6000)
        .unwrap();
    e.execute(Command::ReplaceDeviceState {
        device: id,
        state: state.clone(),
    })
    .unwrap();
    assert_eq!(
        e.project().track(a).unwrap().instrument().unwrap().state(),
        &state
    );
    assert_eq!(
        e.project()
            .native_source(source)
            .unwrap()
            .instrument()
            .state(),
        &state
    );
    assert_eq!(
        before.track(a).unwrap().instrument().unwrap().state(),
        &default_instrument_state().unwrap()
    );
    assert!(before.instrument_history_delta(e.project()).is_some());
    assert_eq!(
        e.project().track(a).unwrap().devices(),
        before.track(a).unwrap().devices()
    );
    assert!(e.undo());
    assert_eq!(
        e.project().native_source(source),
        before.native_source(source)
    );
    assert!(e.redo());
    let source_before = e.project().native_source(source).unwrap().clone();
    e.execute(Command::SetDeviceBypass {
        device: effect,
        bypassed: true,
    })
    .unwrap();
    assert_eq!(e.project().native_source(source), Some(&source_before));
    assert!(
        e.execute_prepared(
            Command::RenameNativeSource {
                source,
                name: "refused".into()
            },
            |_| Err::<(), _>(Invalid("preparation refused"))
        )
        .is_err()
    );
    assert_eq!(e.project().native_source(source), Some(&source_before));
}
#[test]
fn reassignment_and_unsupported_bindings_are_explicit_atomic_and_identity_scoped() {
    let mut e = editor();
    let a = track(&mut e, "a");
    let b = track(&mut e, "b");
    instrument(&mut e, a);
    let original = e.project().track(a).unwrap().source_id().unwrap();
    let other = source(&mut e, "other");
    e.execute(Command::BindTrackSource {
        track: b,
        source: Some(other),
    })
    .unwrap();
    let before = e.project().clone();
    assert!(
        e.execute(Command::BindTrackSource {
            track: a,
            source: Some(other)
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
    let Created::NativeSource(chain) = e
        .execute(Command::CreateNativeSource {
            name: "chain".into(),
            devices: vec![default_instrument_state().unwrap(); 2],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("chain")
    };
    let before = e.project().clone();
    assert!(
        e.execute(Command::BindTrackSource {
            track: a,
            source: Some(chain)
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
    e.execute(Command::BindTrackSource {
        track: b,
        source: None,
    })
    .unwrap();
    e.execute(Command::BindTrackSource {
        track: a,
        source: Some(other),
    })
    .unwrap();
    assert!(e.project().native_source(original).is_some());
    assert_eq!(e.project().track(a).unwrap().source_id(), Some(other));
    let mut forged = e.project().snapshot();
    forged.tracks[1].instrument = forged.tracks[0].instrument.clone();
    forged.tracks[1].instrument_position = Some(0);
    assert!(Project::from_snapshot(forged).is_err());
    let mut forged = e.project().snapshot();
    forged.tracks[0].instrument.as_mut().unwrap().bypassed = true;
    assert!(
        Project::from_snapshot(forged).is_err(),
        "a projection cannot overwrite authoritative state"
    );
}
#[test]
fn legacy_inline_instruments_gain_source_identity_without_losing_devices_or_local_placement() {
    let mut e = editor();
    let a = track(&mut e, "Old Name");
    let id = instrument(&mut e, a);
    let Created::Device(effect) = e
        .execute(Command::InsertDevice {
            track: a,
            before: Some(id),
            state: default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let mut old = e.project().snapshot();
    old.native_sources.clear();
    let old_cursor = old.next_id;
    assert!(Project::from_snapshot(old.clone()).is_err());
    let upgraded = Project::from_legacy_snapshot(old).unwrap();
    let source = upgraded.track(a).unwrap().source_id().unwrap();
    assert_eq!(source.value(), old_cursor);
    assert_eq!(upgraded.native_source(source).unwrap().name(), "Old Name");
    assert_eq!(upgraded.track(a).unwrap().instrument().unwrap().id(), id);
    assert_eq!(upgraded.track(a).unwrap().instrument_position(), Some(1));
    assert_eq!(upgraded.track(a).unwrap().devices()[0].id(), effect);
    assert_eq!(
        Project::from_legacy_snapshot(upgraded.snapshot()).unwrap(),
        upgraded,
        "layered migration does not allocate again"
    );
    let mut exhausted = upgraded.snapshot();
    exhausted.native_sources.clear();
    exhausted.next_id = u64::MAX;
    assert!(Project::from_legacy_snapshot(exhausted).is_err());
}
#[test]
fn explicit_track_duplication_copies_source_but_instrument_move_preserves_its_identity() {
    let mut e = editor();
    let a = track(&mut e, "a");
    let b = track(&mut e, "b");
    let id = instrument(&mut e, a);
    let original = e.project().track(a).unwrap().source_id().unwrap();
    e.execute(Command::MoveTrackDevice {
        device: id,
        track: b,
        before: None,
    })
    .unwrap();
    assert!(e.project().track(a).unwrap().source_id().is_none());
    assert_eq!(e.project().track(b).unwrap().source_id(), Some(original));
    e.execute(Command::DuplicateTracks {
        tracks: crate::TrackSelectionIds::new(&[b]).unwrap(),
        without_clips: true,
    })
    .unwrap();
    let copy = e.project().tracks().last().unwrap();
    let copied = copy.source_id().unwrap();
    assert_ne!(copied, original);
    assert_ne!(copy.instrument().unwrap().id(), id);
    assert_eq!(
        copy.instrument().unwrap().state(),
        e.project()
            .native_source(original)
            .unwrap()
            .instrument()
            .state()
    );
    assert_eq!(e.project().native_sources().len(), 2);
}
