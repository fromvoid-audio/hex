//! Actual constructor-born scalar factories for the closed CURRENT class.
use super::*;
use crate::native_device_resources::current_sampler::CurrentSamplerMetadata;
use hex_devices::sampler::PreparedSamplerRuntime;

pub(super) struct FactoryBinding {
    track: TrackId,
    address: usize,
}
impl SessionAssembly {
    pub(crate) fn reserve_current_sampler_factories(
        &mut self,
        metadata: &CurrentSamplerMetadata,
        contexts: &crate::owned_shared::OwnedShared<
            crate::shared_analytic::SharedAnalyticContextTable,
        >,
    ) -> Result<(), AudioStorageError> {
        if self.phase != AssemblyPhase::Reserved
            || !self.unused_mutable.is_empty()
            || self.unused_mutable.capacity() != 0
            || self.sampler_factories.capacity() != 0
            || self.sampler_midi_scope.is_some()
            || self.sampler_context_address.is_some()
            || contexts.tuning().frequencies() != metadata.tuning.frequencies()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.sampler_midi_scope = Some(metadata.id);
        self.sampler_context_address = Some(Arc::as_ptr(contexts.arc()) as usize);
        let count = metadata.configurations().len();
        self.unused_mutable
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        self.sampler_factories
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        for config in metadata.configurations() {
            self.unused_mutable.push(PreparedMutableTrackInstrument {
                automation: None,
                resource_factory_inline_bound: None,
                track: config.track,
                factory: Box::new(config.prepared.clone()),
                history_retarget_target: None,
                history_retarget_requires_quiescence: false,
            });
            let factory = &self.unused_mutable[self.unused_mutable.len() - 1];
            self.sampler_factories.push(FactoryBinding {
                track: config.track,
                address: factory.factory.as_ref() as *const dyn PreparedMutableInstrumentRuntime
                    as *const () as usize,
            });
        }
        self.current_sampler_bytes(None)?;
        Ok(())
    }
    pub(super) fn current_sampler_values<'a>(
        &'a self,
        external: Option<&'a PreparedSessionInventory>,
    ) -> Result<&'a Vec<PreparedMutableTrackInstrument>, AudioStorageError> {
        let values = self
            .inventory
            .as_ref()
            .or(external)
            .map(|i| &i.mutable_instruments)
            .unwrap_or(&self.unused_mutable);
        if (self.inventory.is_some() || external.is_some()) && self.unused_mutable.capacity() != 0
            || values.len() != self.sampler_factories.len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (value, binding) in values.iter().zip(&self.sampler_factories) {
            if value.automation.is_some()
                || value.track != binding.track
                || value.history_retarget_target.is_some()
                || value.history_retarget_requires_quiescence
                || value.factory.as_ref() as *const dyn PreparedMutableInstrumentRuntime
                    as *const () as usize
                    != binding.address
                || !value.factory.as_any().is::<PreparedSamplerRuntime>()
                || mem::size_of_val(value.factory.as_ref())
                    != mem::size_of::<PreparedSamplerRuntime>()
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        Ok(values)
    }
    pub(super) fn current_sampler_bytes(
        &self,
        external: Option<&PreparedSessionInventory>,
    ) -> Result<usize, AudioStorageError> {
        let values = self.current_sampler_values(external)?;
        bytes(values)?
            .checked_add(bytes(&self.sampler_factories)?)
            .and_then(|n| {
                n.checked_add(
                    values
                        .len()
                        .checked_mul(mem::size_of::<PreparedSamplerRuntime>())?,
                )
            })
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn visit_current_sampler_factories<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a PreparedSessionInventory>,
    ) -> Result<(), AudioStorageError> {
        for value in self.current_sampler_values(external)? {
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(&value.factory),
                crate::audio_storage::Location("current-sampler-factory", 0),
                mem::size_of::<PreparedSamplerRuntime>(),
            )?;
        }
        Ok(())
    }
}
