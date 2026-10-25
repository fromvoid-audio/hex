//! Original worker custody while its unique callback is loaned through Root's
//! existing carrier. There is no public constructor, clone or worker extraction.
use super::*;

pub(crate) struct NativeEffectControl {
    worker: NativeWorker,
    configuration: Configuration,
    storage: NativeEffectStorage,
    source_metadata: EffectSourceMetadata,
}
impl NativeEffect {
    #[allow(clippy::result_large_err)] // Refusal retains the entire original pair, without an error-path allocation.
    pub(crate) fn loan_on_control(
        self,
    ) -> Result<(NativeEffectControl, NativeEffectCallback), Self> {
        // Physical return preserves custody, not processing eligibility. Stop
        // and rejected delivery revoke this exact transport permanently; only
        // retained native quiescence/resume can construct a fresh paired epoch.
        if self.callback.transport.is_revoked() {
            return Err(self);
        }
        let Some(storage) = self.constructor_storage_on_control() else {
            return Err(self);
        };
        let control = NativeEffectControl {
            configuration: self.callback.configuration.clone(),
            worker: self.worker,
            storage,
            source_metadata: self.source_metadata,
        };
        Ok((control, self.callback))
    }
}
impl NativeEffectControl {
    pub(crate) fn transport_is_live_on_control(&self) -> bool {
        self.worker.transport_is_live_on_control()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_published_blocks(&self) -> u64 {
        self.worker.published_blocks_on_control()
    }
    pub(crate) fn binding(&self) -> Binding {
        self.configuration.binding
    }
    pub(crate) fn identity(&self) -> ProcessorInstanceIdentity {
        self.configuration.identity
    }
    pub(crate) fn prepared(&self) -> PreparedForeignProcessor {
        self.configuration.prepared
    }
    pub(crate) fn configuration(&self) -> &NativeDeviceState {
        &self.configuration.state
    }
    pub(crate) fn storage(&self) -> NativeEffectStorage {
        self.storage
    }
    pub(crate) fn constructor_storage_on_control(&self) -> Option<NativeEffectStorage> {
        // Only loan_on_control constructs this unique control, after inspecting
        // the original paired callback. Its backing cannot resize while loaned;
        // no worker extraction or source handoff is exposed from this owner.
        (self.worker.retains_original_source_on_control()
            && self.worker.binding() == self.binding()
            && self.worker.quantum_frames() == self.prepared().quantum_frames())
        .then_some(self.storage)
    }
    pub(crate) fn matches_callback(&self, callback: &NativeEffectCallback) -> bool {
        callback.matches_worker(&self.worker)
            && callback.binding() == self.binding()
            && callback.identity() == self.identity()
    }
    #[allow(clippy::result_large_err)] // A mismatched original return remains wholly owned by the caller.
    pub(crate) fn reunite(
        self,
        callback: NativeEffectCallback,
    ) -> Result<NativeEffect, (Self, NativeEffectCallback)> {
        if !self.matches_callback(&callback) {
            return Err((self, callback));
        }
        Ok(NativeEffect {
            worker: self.worker,
            callback,
            source_metadata: self.source_metadata,
        })
    }
}

pub(crate) enum NativeEffectReference<'a> {
    Unsplit(&'a NativeEffect),
    Loaned(&'a NativeEffectControl),
}
impl NativeEffectReference<'_> {
    pub(crate) fn source_metadata(&self) -> &EffectSourceMetadata {
        match self {
            Self::Unsplit(effect) => &effect.source_metadata,
            Self::Loaned(control) => &control.source_metadata,
        }
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn completed_blocks_on_control(&self) -> u64 {
        match self {
            Self::Unsplit(effect) => effect.worker.published_blocks_on_control(),
            Self::Loaned(control) => control.worker.published_blocks_on_control(),
        }
    }
    pub(crate) fn transport_is_live_on_control(&self) -> bool {
        match self {
            Self::Unsplit(effect) => !effect.callback.transport.is_revoked(),
            Self::Loaned(control) => control.transport_is_live_on_control(),
        }
    }
    pub(crate) fn matches_prepared(&self, node: &hex_lattice::PreparedRuntimeInstance) -> bool {
        let config = self.configuration();
        node.owner == config.identity.owner()
            && node.device_id == config.identity.device_id()
            && node.device_type == config.state.type_id()
            && node.sample_rate == config.sample_rate
            && matches!(&node.definition,
                Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if *prepared == config.prepared)
            && node
                .configuration
                .as_deref()
                .is_some_and(|state| state.processing_eq(&config.state))
    }
    pub(crate) fn binding(&self) -> Binding {
        self.configuration().binding
    }
    pub(crate) fn identity(&self) -> ProcessorInstanceIdentity {
        self.configuration().identity
    }
    fn configuration(&self) -> &Configuration {
        match self {
            Self::Unsplit(effect) => &effect.callback.configuration,
            Self::Loaned(control) => &control.configuration,
        }
    }
}
impl<'a> NativeEffectDefinition<'a> {
    pub(crate) fn from_root_sources(
        sources: impl IntoIterator<Item = NativeEffectReference<'a>>,
    ) -> Result<Self, DeviceContractError> {
        Self::from_selected(sources.into_iter().map(|source| match source {
            NativeEffectReference::Unsplit(effect) => (
                &effect.callback.configuration,
                effect.callback.matches_worker(&effect.worker),
            ),
            NativeEffectReference::Loaned(control) => (
                &control.configuration,
                control.worker.binding() == control.binding(),
            ),
        }))
    }
}
