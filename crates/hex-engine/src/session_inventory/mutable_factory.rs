//! Preserve the actual closed sampler constructor through source preparation.
//! Opaque provider factories retain their existing behavior and stay Unknown.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, audio_storage::Owner, owned_shared::OwnedShared,
};

pub(crate) enum PreparedTrackRuntimeFactory {
    Opaque(Box<dyn PreparedMutableInstrumentRuntime>),
    Sampler(OwnedShared<hex_devices::sampler::PreparedSamplerRuntime>),
}

impl PreparedTrackRuntimeFactory {
    pub(super) fn as_ref(&self) -> &dyn PreparedMutableInstrumentRuntime {
        match self {
            Self::Opaque(factory) => factory.as_ref(),
            Self::Sampler(factory) => factory.as_ref(),
        }
    }
    pub(super) fn visit_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        match self {
            Self::Opaque(_) => Err(AudioStorageError::UnknownStorage),
            Self::Sampler(factory) => {
                if factory.control_storage_declaration()
                    != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                        bytes: std::mem::size_of::<hex_devices::sampler::PreparedSamplerRuntime>(),
                    })
                {
                    return Err(AudioStorageError::UnknownStorage);
                }
                ledger.observe_shared(factory.arc(), factory.parts(), Owner::exclusive(factory))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AudioStorageLedgerReservation, AudioStorageRole, shared_analytic::resource_storage,
    };
    use hex_lattice::{
        NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    };

    #[test]
    fn actual_source_factory_field_and_context_table_share_one_factory_and_retain_pcm() {
        let Some((source, owner, editor)) =
            crate::shared_analytic::sampler_tests::retained_fixture()
        else {
            return;
        };
        let track = editor.project().tracks().first().unwrap().id();
        let entries = vec![PreparedMutableTrackInstrument {
            track,
            factory: PreparedTrackRuntimeFactory::Sampler(
                owner.sampler_factory_on_control().unwrap(),
            ),
            history_retarget_target: None,
            history_retarget_requires_quiescence: false,
        }];
        let table = crate::shared_analytic::sampler_tests::retained_table_fixture(owner);
        let reservation = AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 16,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&source, AudioStorageRole::Caller)
            .unwrap();
        resource_storage::visit_table(&table, &mut ledger, AudioStorageRole::Current).unwrap();
        assert!(ledger.validate_closed_shared_handles().is_err());
        let before = ledger.summary().unwrap();
        ledger.root(&entries, AudioStorageRole::Caller).unwrap();
        ledger.vec(&entries).unwrap();
        for entry in &entries {
            entry.factory.visit_storage(&mut ledger).unwrap();
        }
        ledger.validate_closed_shared_handles().unwrap();
        let summary = ledger.summary().unwrap();
        // Only the actual Vec backing is new; its inline factory handle is
        // already inside each source row, not another physical allocation.
        assert_eq!(summary.data_allocations, before.data_allocations + 1);
        assert_eq!(
            summary.data_bytes - before.data_bytes,
            std::mem::size_of_val(&entries)
                + entries.capacity() * std::mem::size_of::<PreparedMutableTrackInstrument>()
        );
        assert!(summary.require_complete().is_err());
        drop(ledger);
        drop(source);
        drop(editor);
        let mut pair = NativeInstrumentRuntimePair::new(
            entries[0].factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 32,
                max_tail_frames: 44_100 * 600,
            },
        )
        .unwrap();
        let mut output = [[0.0; 2]; 6];
        pair.begin_block();
        pair.process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }],
            &mut output,
            table.owner(0).unwrap().resources(),
        )
        .unwrap();
        pair.commit_block();
        assert_eq!(&output[..4], &crate::native_device_resources::tests::PCM);
        assert_eq!(&output[4..], &[[0.0; 2]; 2]);
        // Concrete type/value alone cannot upgrade a different or raw owner.
        let PreparedTrackRuntimeFactory::Sampler(factory) = &entries[0].factory else {
            panic!("concrete factory")
        };
        let opaque = PreparedTrackRuntimeFactory::Opaque(Box::new(factory.as_ref().clone()));
        let raw =
            PreparedTrackRuntimeFactory::Sampler(OwnedShared::untracked(Arc::clone(factory.arc())));
        let mut refused = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        assert_eq!(
            opaque.visit_storage(&mut refused),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            raw.visit_storage(&mut refused),
            Err(AudioStorageError::UnknownStorage)
        );
    }
}
