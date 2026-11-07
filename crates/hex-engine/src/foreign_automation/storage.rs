//! Constructor declarations and exact runtime handles for prepared versions.
//! This borrowed control inventory never clones or transfers program custody.
use crate::{
    AudioRootStorageLedger, AudioStorageError,
    audio_storage::{Location, Owner},
};
use hex_lattice::PreparedForeignAutomation;

pub(crate) fn declare<'a>(
    program: &'a PreparedForeignAutomation,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    for (owner, bytes) in program.storage_owners() {
        ledger.shared(
            owner,
            Some((bytes, 0)),
            Owner::exclusive(owner),
            crate::audio_only_inputs::handle()?,
            None,
        )?;
    }
    Ok(())
}

pub(crate) fn runtime<'a>(
    program: &'a PreparedForeignAutomation,
    prepared: &PreparedForeignAutomation,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if !program.shares_storage_with(prepared) {
        return Err(AudioStorageError::SourceMismatch);
    }
    for (owner, _) in program.storage_owners() {
        ledger.existing_shared(
            owner,
            Owner::exclusive(owner),
            Location("foreign-automation-runtime", 0),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    struct Program;
    impl hex_lattice::ForeignAutomationProgram for Program {
        fn render_quantum(
            &self,
            _: &[Option<u64>],
            _: &[u16],
            _: &mut [hex_lattice_contract::ForeignParameterPoint],
        ) -> Result<usize, hex_lattice_contract::DeviceContractError> {
            Ok(0)
        }
    }
    #[test]
    fn retained_data_deduplicates_but_every_prepared_and_runtime_handle_is_required() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let make = |key| {
            PreparedForeignAutomation::new(binding, NonZeroU64::new(key).unwrap(), Program, 0)
                .unwrap()
        };
        let previous = make(1);
        let mut current = make(2);
        current.retain_previous_on_control(&previous).unwrap();
        let runtime_copy = current.clone();
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 32,
                handle_rows: 32,
                root_rows: 1,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            })
            .unwrap();
        declare(&previous, &mut ledger).unwrap();
        declare(&current, &mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_allocations, 2);
        assert!(
            ledger.validate_closed_shared_handles().is_err(),
            "the two runtime handles are still undeclared"
        );
        runtime(&runtime_copy, &current, &mut ledger).unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.data_allocations, 2);
        assert_eq!(summary.unresolved_handles, 5);
        assert_eq!(summary.data_bytes, current.storage_bytes());
        ledger.validate_closed_shared_handles().unwrap();
        println!(
            "retained program inventory: {} allocations, {} handles, {} bytes",
            summary.data_allocations, summary.unresolved_handles, summary.data_bytes
        );
        let unrelated = make(2);
        assert_eq!(
            runtime(&unrelated, &current, &mut ledger),
            Err(AudioStorageError::SourceMismatch)
        );
    }
}
