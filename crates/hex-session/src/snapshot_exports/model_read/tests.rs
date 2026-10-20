use crate::snapshot_exports::*;
use hex_project::{Command, Created};

#[test]
fn retained_model_queries_follow_exact_snapshot_until_last_read_ack() {
    let mut session = crate::Session::new(48_000, 8, 1024).unwrap();
    let Created::Track(track) = session
        .editor
        .execute(Command::AddTrack {
            name: "captured track".into(),
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    let Created::ReturnTrack(bus) = session
        .editor
        .execute(Command::AddReturnTrack {
            name: "captured return".into(),
        })
        .unwrap()
    else {
        panic!("expected return");
    };
    let make_view = |session: &crate::Session, revision| {
        Some(crate::service::view_state_owned(
            session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            1,
            false,
            None,
        ))
    };
    let (mut family, producer) =
        SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 4,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap();
    let mut source = make_view(&session, 7);
    let first = family
        .insert_event_read_on_control(&producer, &mut source)
        .unwrap();
    let second = family.derive_read(&first).unwrap();
    let third = family.derive_read(&first).unwrap();
    let fourth = family.derive_read(&first).unwrap();
    assert!(source.is_none());
    session
        .editor
        .execute(Command::RenameTrack {
            track,
            name: "current track".into(),
        })
        .unwrap();
    session
        .editor
        .execute(Command::RemoveReturnTrack(bus))
        .unwrap();
    let mut replacement = make_view(&session, 8);
    assert!(matches!(
        family.insert_event_read_on_control(&producer, &mut replacement),
        Err(ExportRecordError::Reservation)
    ));
    assert!(replacement.is_some());
    for read in [&first, &second, &third, &fourth] {
        assert_eq!(
            family.read_with(read, |view| {
                let model = view.model();
                let captured = model.track(track).unwrap();
                let returned = model.returns().next().unwrap();
                assert_eq!(captured.name(), "captured track");
                assert_eq!(returned.id(), bus);
                assert_eq!(returned.name(), "captured return");
                assert_eq!(model.tracks().len(), 1);
                assert_eq!(captured.clips().len(), 0);
                assert_eq!(captured.gain_points().len(), 0);
                (view.revision(), captured.id(), model.sample_rate())
            }),
            Ok((7, track, 48_000))
        );
    }
    family.release_read(first).unwrap();
    family.release_read(second).unwrap();
    family.release_read(third).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(
        family
            .try_inventory_guard_on_control()
            .unwrap()
            .snapshot_count(),
        1
    );
    let next = family
        .insert_event_read_on_control(&producer, &mut replacement)
        .unwrap();
    assert_eq!(
        family.read_with(&next, |view| {
            assert_eq!(view.model().track(track).unwrap().name(), "current track");
            assert_eq!(view.model().returns().len(), 0);
            view.revision()
        }),
        Ok(8)
    );
    // The last old reader remains on revision 7 after replacement and abandon.
    family.abandon_on_control(&producer).unwrap();
    assert_eq!(family.read_with(&fourth, |view| view.revision()), Ok(7));
    family.release_read(fourth).unwrap();
    family.release_read(next).unwrap();
    for _ in 0..4 {
        if family.cleanup_one_on_control() == Ok(SnapshotFamilyCleanup::Disposed) {
            return;
        }
    }
    panic!("all actual readers were acknowledged but family did not retire");
}

#[test]
fn sampler_control_scalars_follow_retained_read_after_current_state_replacement() {
    use hex_lattice_contract::{ParameterId, ParameterValue};
    let mut session = crate::Session::new(48_000, 8, 0).unwrap();
    let empty = hex_engine::empty_sampler_state_on_control().unwrap();
    let Created::Track(track) = session
        .editor
        .execute(Command::InsertTrackWithDevice {
            name: "retained sampler controls".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: empty.clone(),
        })
        .unwrap()
    else {
        panic!("actual test instrument track");
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (mut family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 4,
        max_bookkeeping_bytes: 16_384,
    })
    .unwrap();
    let make_view = |session: &crate::Session, revision| {
        Some(crate::service::view_state_owned(
            session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            0,
            false,
            None,
        ))
    };
    let mut initial = make_view(&session, 0);
    let old = family
        .insert_event_read_on_control(&key, &mut initial)
        .unwrap();
    let loaded = hex_engine::sampler_replacement_state_on_control(&empty, 16)
        .unwrap()
        .state;
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: loaded,
        })
        .unwrap();
    let mut next = make_view(&session, 1);
    let new = family
        .insert_event_read_on_control(&key, &mut next)
        .unwrap();
    let old_values = family
        .read_with(&old, |read| read.sampler_parameters(device))
        .unwrap()
        .unwrap();
    assert!(old_values.iter().all(|p| !p.enabled));
    let next_values = family
        .read_with(&new, |read| read.sampler_parameters(device))
        .unwrap()
        .unwrap();
    assert_eq!(
        next_values
            .iter()
            .find(|p| p.descriptor.id == ParameterId::from_static(18))
            .unwrap()
            .value,
        ParameterValue::Float(16.0)
    );
    assert!(next_values.iter().any(|p| p.enabled));
    assert_eq!(
        family
            .inspect_queries_on_control(|fields| fields.snapshot_count())
            .unwrap(),
        2
    );
    family.release_read(old).unwrap();
    family.cleanup_one_on_control().unwrap();
    assert_eq!(
        family
            .inspect_queries_on_control(|fields| fields.snapshot_count())
            .unwrap(),
        1
    );
    // Copy display facts are intentionally usable AFTER the owning read ends.
    // They contain no model/state handle capable of retaining the old source.
    assert!(old_values.iter().all(|p| !p.enabled));
    family.release_read(new).unwrap();
    family.cleanup_one_on_control().unwrap();
    assert_eq!(
        family
            .inspect_queries_on_control(|fields| fields.snapshot_count())
            .unwrap(),
        0
    );
}
