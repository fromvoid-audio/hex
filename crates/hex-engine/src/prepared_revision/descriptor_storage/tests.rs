use super::*;
use crate::{
    AudioStorageLedgerReservation, AudioStorageRole, prepared_revision::assembly::RevisionAssembly,
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{Command, Created};
use std::collections::BTreeMap;

#[test]
fn actual_mutable_descriptors_charge_nested_capacities_and_retain_all_canonical_aliases() {
    for mono in [false, true] {
        let Some((bank, mut editor)) = super::super::resource_tests::source_fixture(mono) else {
            eprintln!("unsupported actual mutable descriptor storage");
            return;
        };
        eprintln!("SUPPORTED actual mutable descriptor storage mono={mono}");
        let track = editor.project().tracks()[0].id();
        super::super::resource_tests::musical_clips(&mut editor, track);
        let Created::Device(effect) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        editor
            .execute(Command::SetDeviceBypass {
                device: effect,
                bypassed: true,
            })
            .unwrap();
        let prepare = || {
            PreparedRevision::prepare_with_audio_transforms_and_device_resources(
                editor.project(),
                &BTreeMap::new(),
                crate::SharedSineTable::new_on_control(),
                None,
                &crate::audio_transform::AudioTransformPreparation::default(),
                crate::native_device_resources::DeviceResourcePreparation {
                    bank: &bank,
                    max_scope_metadata_bytes: 65_536,
                },
            )
            .unwrap()
        };
        let mut first = prepare();
        let mut second = prepare();
        assert_eq!(first.mutable_track_revisions.len(), 1);
        assert!(first.mutable_track_revisions == second.mutable_track_revisions);
        assert!(first.mutable_track_revisions[0].effects[0].bypassed);
        assert_ne!(
            first.mutable_track_revisions.as_ptr(),
            second.mutable_track_revisions.as_ptr()
        );
        second.mutable_track_revisions[0]
            .effects
            .try_reserve_exact(7)
            .unwrap();
        second.mutable_track_revisions[0]
            .arrangement_midi_sources
            .try_reserve_exact(11)
            .unwrap();
        fn independent(rows: &Vec<PreparedMutableTrackRevision>) -> usize {
            rows.capacity() * size_of::<PreparedMutableTrackRevision>()
                + rows
                    .iter()
                    .map(|r| {
                        r.effects.capacity() * size_of::<PreparedDeviceIdentity>()
                            + r.arrangement_midi_sources.capacity()
                                * size_of::<PreparedArrangementMidiSource>()
                    })
                    .sum::<usize>()
        }
        let expected = independent(&first.mutable_track_revisions)
            + independent(&second.mutable_track_revisions);
        let mut old = RevisionAssembly::default();
        let mut next = RevisionAssembly::default();
        old.mutable_descriptors_for_test(&mut first.mutable_track_revisions);
        next.mutable_descriptors_for_test(&mut second.mutable_track_revisions);
        // Retain actual moved fields, then retire the other concrete compiler
        // products under their original bank. No raw owner is adopted.
        drop((first, second));
        let budget = AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 128,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        };
        let collect = |limits| -> Result<_, AudioStorageError> {
            let mut l = AudioRootStorageLedger::try_new_on_control(limits)?;
            l.root(&old, AudioStorageRole::Predecessor)?;
            l.root(&next, AudioStorageRole::Current)?;
            l.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)?;
            l.borrow_editor_native_states_on_control(&editor)?;
            l.borrow_editor_midi_sources_on_control(&editor)?;
            let before = l.summary()?;
            assert!(l.validate_selected_shared_handles_on_control().is_err());
            old.visit_audio_storage(&mut l)?;
            assert!(l.validate_selected_shared_handles_on_control().is_err());
            next.visit_audio_storage(&mut l)?;
            l.validate_selected_shared_handles_on_control()?;
            let after = l.summary()?;
            assert_eq!(after.data_bytes - before.data_bytes, expected);
            assert_eq!(after.data_allocations - before.data_allocations, 6);
            next.visit_audio_storage(&mut l)?;
            assert_eq!(l.summary()?, after);
            assert!(after.require_complete().is_err());
            Ok(after)
        };
        let summary = collect(budget).unwrap();
        let mut exact = budget;
        exact.max_retained_bytes = summary.retained_bytes;
        exact.max_construction_peak_bytes = summary.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), summary);
        exact.max_retained_bytes -= 1;
        assert_eq!(collect(exact), Err(AudioStorageError::ReservationExceeded));
        let quoted = old.capacity().unwrap().mutable_descriptor_backing_bytes
            + next.capacity().unwrap().mutable_descriptor_backing_bytes;
        assert_eq!(quoted, expected);
        old.clear_descriptor_custody_on_control().unwrap();
        {
            let mut l = AudioRootStorageLedger::try_new_on_control(budget).unwrap();
            l.borrow_editor_native_states_on_control(&editor).unwrap();
            l.borrow_editor_midi_sources_on_control(&editor).unwrap();
            next.visit_audio_storage(&mut l).unwrap();
            l.validate_selected_shared_handles_on_control().unwrap();
        }
        next.clear_descriptor_custody_on_control().unwrap();
        assert_eq!(old.capacity().unwrap().mutable_descriptor_backing_bytes, 0);
        assert_eq!(next.capacity().unwrap().mutable_descriptor_backing_bytes, 0);
        let mut l = AudioRootStorageLedger::try_new_on_control(budget).unwrap();
        l.borrow_editor_native_states_on_control(&editor).unwrap();
        l.borrow_editor_midi_sources_on_control(&editor).unwrap();
        l.validate_selected_shared_handles_on_control().unwrap();
        assert!(crate::audio_preparation::validate_constructor_project(editor.project()).is_err());
    }
}
