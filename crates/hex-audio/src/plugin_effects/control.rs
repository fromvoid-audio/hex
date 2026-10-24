//! Original worker custody while its unique callback is loaned through Root's
//! existing carrier. There is no public constructor, clone or worker extraction.
use super::*;

pub(crate) struct NativeEffectControl {
    worker: NativeWorker,
    configuration: Configuration,
    storage: NativeEffectStorage,
}
impl NativeEffect {
    #[allow(clippy::result_large_err)] // Refusal retains the entire original pair, without an error-path allocation.
    pub(crate) fn loan_on_control(
        self,
    ) -> Result<(NativeEffectControl, NativeEffectCallback), Self> {
        let Some(storage) = self.constructor_storage_on_control() else {
            return Err(self);
        };
        let control = NativeEffectControl {
            configuration: self.callback.configuration.clone(),
            worker: self.worker,
            storage,
        };
        Ok((control, self.callback))
    }
}
impl NativeEffectControl {
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
        })
    }
}

pub(crate) enum NativeEffectReference<'a> {
    Unsplit(&'a NativeEffect),
    Loaned(&'a NativeEffectControl),
}
impl NativeEffectReference<'_> {
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
