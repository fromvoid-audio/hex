//! Retain the actual constructor route with the pair; no mutable pair escape.
use crate::session_inventory::PreparedTrackRuntimeFactory;
use hex_lattice::{
    AnalyticInstrumentHistory, NativeInstrumentAnalyticSeed, NativeInstrumentClock,
    NativeInstrumentEvent, NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime, StereoFrame,
};

pub(crate) struct RuntimePair {
    pair: NativeInstrumentRuntimePair,
    origin: Origin,
}
enum Origin {
    Opaque,
    Sampler,
}
impl std::ops::Deref for RuntimePair {
    type Target = NativeInstrumentRuntimePair;
    fn deref(&self) -> &Self::Target {
        &self.pair
    }
}
impl RuntimePair {
    pub(super) fn new(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        Ok(Self {
            pair: NativeInstrumentRuntimePair::new(prepared, limits)?,
            origin: Origin::Opaque,
        })
    }
    pub(super) fn from_factory(
        factory: &PreparedTrackRuntimeFactory,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Self, NativeInstrumentRuntimeError> {
        match factory {
            PreparedTrackRuntimeFactory::Opaque(factory) => Self::new(factory.as_ref(), limits),
            PreparedTrackRuntimeFactory::Sampler(factory) => Ok(Self {
                pair: NativeInstrumentRuntimePair::new(factory.as_ref(), limits)?,
                origin: Origin::Sampler,
            }),
        }
    }
    pub(super) fn qualified_sampler_pair(&self) -> Option<&NativeInstrumentRuntimePair> {
        matches!(self.origin, Origin::Sampler).then_some(&self.pair)
    }
    pub(super) fn visit_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        self.visit_storage_with_gate(ledger, native_box_layout_known())
    }
    fn visit_storage_with_gate<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        supported: bool,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        let pair = self
            .qualified_sampler_pair()
            .ok_or(crate::AudioStorageError::UnknownStorage)?;
        if !supported {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        let boxes = pair.runtime_box_observations();
        let layout = hex_devices::sampler::PreparedSamplerRuntime::runtime_box_layout_on_control();
        if layout.size() == 0
            || boxes[0].same_allocation(&boxes[1])
            || boxes[0].same_owning_field(&boxes[1])
            || boxes
                .iter()
                .any(|observation| observation.pointee_layout() != layout)
        {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        for (index, observation) in boxes.iter().enumerate() {
            ledger.bytes(
                Owner::exclusive(pair),
                Location("sampler-runtime-box", index),
                observation.pointee_layout().size(),
            )?;
        }
        Ok(())
    }
    pub(super) fn begin_block(&mut self) {
        self.pair.begin_block()
    }
    pub(super) fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.pair
            .process_block_with_clock(events, frames, clock, _resources)
    }
    pub(super) fn commit_block(&mut self) {
        self.pair.commit_block()
    }
    pub(super) fn rollback_block(&mut self) {
        self.pair.rollback_block()
    }
    pub(super) fn reset(&mut self) {
        self.pair.reset()
    }
    pub(super) fn inherit_from(
        &mut self,
        previous: &Self,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.pair.inherit_from(&previous.pair)
    }
    pub(super) fn import_analytic_history(
        &mut self,
        history: &AnalyticInstrumentHistory<'_>,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.pair.import_analytic_history(history)
    }
    pub(super) fn import_analytic(
        &mut self,
        notes: &[NativeInstrumentAnalyticSeed],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.pair.import_analytic(notes)
    }
    pub(super) fn retarget_from(
        &mut self,
        previous: &Self,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        self.pair.retarget_from(&previous.pair, proof)
    }
}

#[cfg(test)]
mod tests;

fn native_box_layout_known() -> bool {
    let target = env!("HEX_SAMPLER_BOX_LAYOUT_TARGET");
    env!("HEX_SAMPLER_BOX_LAYOUT_KNOWN") == "1"
        && env!("HEX_SAMPLER_BOX_CONSTRUCTOR_FAMILY") == "sampler-runtime-sized-box-new-v1"
        && ((cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) && target == "x86_64-unknown-linux-gnu")
            || (cfg!(all(target_arch = "aarch64", target_os = "macos"))
                && target == "aarch64-apple-darwin"))
}
