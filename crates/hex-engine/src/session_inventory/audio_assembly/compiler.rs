//! Actual paired Sampler factories needed by Arrangement AND Session runtime.
//! No provider factory is recreated/adopted and no opaque branch is qualified.
use super::*;
impl SessionAssembly {
    pub(super) fn compiler_factory_backing_bytes(
        project: &hex_project::Project,
    ) -> Result<usize, AudioStorageError> {
        let count = project
            .tracks()
            .iter()
            .filter_map(|track| track.instrument())
            .filter(|device| !device.bypassed())
            .try_fold(0usize, |n, device| {
                if device.state().type_id() != hex_devices::sampler::state::TYPE_ID {
                    return Err(AudioStorageError::UnsupportedGraph);
                }
                n.checked_add(1).ok_or(AudioStorageError::Overflow)
            })?;
        std::alloc::Layout::array::<PreparedMutableTrackInstrument>(count)
            .map_err(|_| AudioStorageError::Overflow)?;
        count
            .checked_mul(mem::size_of::<PreparedMutableTrackInstrument>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub(crate) fn construct_compiler_factories(
        &mut self,
        project: &hex_project::Project,
        contexts: &OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != AssemblyPhase::Reserved
            || self.unused_mutable.capacity() != 0
            || self.inventory.is_some()
            || self.pending_mutable.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let backing = Self::compiler_factory_backing_bytes(project)?;
        let count = backing / mem::size_of::<PreparedMutableTrackInstrument>();
        // Validate every actual source BEFORE reserving/cloning a factory.
        // The concrete Sampler currently inherits Unchanged retargeting. A
        // future changed provider law must be explicitly integrated here.
        for track in project.tracks() {
            if let Some(device) = track.instrument().filter(|device| !device.bypassed()) {
                let owner = contexts
                    .owner_for_device(device.id())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let prepared = owner
                    .sampler_prepared_on_control(
                        track.id(),
                        device.id(),
                        device.state(),
                        project.sample_rate(),
                    )
                    .map_err(|_| AudioStorageError::SourceMismatch)?
                    .ok_or(AudioStorageError::SourceMismatch)?;
                if !matches!(
                    PreparedMutableInstrumentRuntime::prepare_history_retarget_target(prepared)
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                    InstrumentHistoryRetargetPreparation::Unchanged
                ) {
                    return Err(AudioStorageError::UnsupportedGraph);
                }
            }
        }
        self.unused_mutable
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        if bytes(&self.unused_mutable)? != backing {
            return Err(AudioStorageError::ReservationExceeded);
        }
        for track in project.tracks() {
            if let Some(device) = track.instrument().filter(|device| !device.bypassed()) {
                let owner = contexts
                    .owner_for_device(device.id())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let factory = owner
                    .sampler_factory_on_control()
                    .ok_or(AudioStorageError::SourceMismatch)?;
                // Reserved capacity and fixed typed scalar fields: no fallible
                // work/callback between this paired clone and persistent push.
                self.unused_mutable.push(PreparedMutableTrackInstrument {
                    track: track.id(),
                    factory: PreparedTrackRuntimeFactory::Sampler(factory),
                    history_retarget_target: None,
                    history_retarget_requires_quiescence: false,
                });
            }
        }
        Ok(())
    }
}
