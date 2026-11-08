//! Actual private modulation backing, charged after matching its immutable
//! prepared recipe. Inline runtime fields are already in runtime-entry storage.
use crate::{
    AudioRootStorageLedger, AudioStorageError,
    audio_storage::{Location, Owner},
};
use hex_lattice::{NativeRuntimeChain, NativeRuntimeEntryStorage, PreparedRuntimeInstance};

pub(super) fn visit<'a>(
    entry: &NativeRuntimeEntryStorage<'_>,
    prepared: &PreparedRuntimeInstance,
    chain: &'a NativeRuntimeChain,
    index: usize,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if !entry.matches_prepared_runtime_storage(prepared) {
        return Err(AudioStorageError::SourceMismatch);
    }
    let bytes = entry
        .modulation_storage_bytes()
        .ok_or(AudioStorageError::Overflow)?;
    if bytes != 0 {
        ledger.bytes(
            Owner::exclusive(chain),
            Location("modulation-runtime-backing", index),
            bytes,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        ModulationSourceId, PreparedLfoSource, PreparedModulation, PreparedModulationState,
        ProcessorChainOwner, StereoMatrix,
    };
    use hex_lattice_contract::NativeDeviceMetadata;
    use std::sync::Arc;

    fn instance(frames: usize, frequency: f64) -> PreparedRuntimeInstance {
        let source = PreparedLfoSource::free(
            ModulationSourceId::new(1).unwrap(),
            48_000.0,
            frequency,
            0.0,
        )
        .unwrap();
        PreparedRuntimeInstance {
            modulation: Some(
                PreparedModulationState::new(
                    vec![source],
                    PreparedModulation::new(&[], &[], 1).unwrap(),
                    frames,
                )
                .unwrap(),
            ),
            automation: Default::default(),
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: hex_devices::utility::UTILITY_TYPE_ID,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(
                hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            )),
            definition: None,
        }
    }
    fn ledger<'a>() -> AudioRootStorageLedger<'a> {
        AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 4,
            handle_rows: 0,
            root_rows: 0,
            max_retained_bytes: 1_048_576,
            max_construction_peak_bytes: 1_048_576,
        })
        .unwrap()
    }
    fn bytes(chain: &NativeRuntimeChain, prepared: &PreparedRuntimeInstance) -> usize {
        let mut ledger = ledger();
        let view = chain.storage_view();
        let entry = view.entries().next().unwrap();
        visit(&entry, prepared, chain, 0, &mut ledger).unwrap();
        ledger.summary().unwrap().data_bytes
    }
    #[test]
    fn actual_buffer_capacity_is_counted_and_committed_phase_is_not_recipe_data() {
        let short = instance(17, 7.0);
        let long = instance(128, 7.0);
        let mut short_chain = NativeRuntimeChain::new_prepared(vec![short.clone()]).unwrap();
        let long_chain = NativeRuntimeChain::new_prepared(vec![long.clone()]).unwrap();
        let original = bytes(&short_chain, &short);
        assert_eq!(
            bytes(&long_chain, &long) - original,
            (128 - 17) * std::mem::size_of::<f32>()
        );
        short_chain.begin_block();
        short_chain.process_block(&mut [[0.1; 2]; 97]).unwrap();
        short_chain.commit_block();
        assert_eq!(bytes(&short_chain, &short), original);
    }
    #[test]
    fn changed_recipe_is_refused_before_any_backing_is_recorded() {
        let prepared = instance(17, 7.0);
        let chain = NativeRuntimeChain::new_prepared(vec![prepared.clone()]).unwrap();
        for other in [instance(18, 7.0), instance(17, 11.0)] {
            let mut changed = prepared.clone();
            changed.modulation = other.modulation;
            let mut ledger = ledger();
            let view = chain.storage_view();
            let entry = view.entries().next().unwrap();
            assert_eq!(
                visit(&entry, &changed, &chain, 0, &mut ledger),
                Err(AudioStorageError::SourceMismatch)
            );
            assert_eq!(ledger.summary().unwrap().data_bytes, 0);
        }
    }
}
