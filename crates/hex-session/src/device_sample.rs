//! Actual original decoder custody and canonical Sampler assignment staging.
//! No model/media publication, registry permission or callback preparation.
use crate::{
    DecodedOriginal, OriginalAudio, ResidentResourcePlanReservation, Session, SessionError,
};
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageRole, OwnedAudioSource,
    native_device_resources::{DeviceResourceBank, ResourcePreparationError},
};
use hex_lattice_contract::NativeDeviceState;
use hex_project::{AssetId, Command, DeviceId, Project};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

/// One worker-owned decoded original and bounded current assignment metadata.
/// It is not Clone and exposes no owning PCM/Arc. The completed graph must
/// inventory this still-live temporary before committing media/model/audio.
pub struct PreparedDeviceSample {
    device: DeviceId,
    expected_state: NativeDeviceState,
    assignment: hex_engine::SamplerSampleAssignment,
    path: PathBuf,
    source: OriginalAudio,
    source_frames: u64,
}
/// Explicit bounds for worker-only summary scanning and its retained source
/// handle. Whole graph metadata/peak accounting is still required by the host.
#[derive(Clone, Copy, Debug)]
pub struct DeviceSampleWaveformLimits {
    pub bins: usize,
    pub scan_frames: usize,
    pub source_metadata_bytes: usize,
}
impl Session {
    /// Resolve the actual captured instrument on the existing control worker
    /// before touching the file. A service caller must also check its document
    /// generation/revision stamps. This immutable Session borrow freezes this
    /// target during decode; the returned staging still needs full prepared
    /// admission before one atomic model/media/audio publication.
    pub fn prepare_device_sample_on_worker(
        &self,
        device: DeviceId,
        expected_state: &NativeDeviceState,
        path: impl AsRef<Path>,
        limits: hex_media::Limits,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<PreparedDeviceSample, SessionError> {
        if cancel.is_cancelled() {
            return Err(hex_media::ImportError::Cancelled.into());
        }
        let target = self
            .project()
            .device(device)
            .filter(|target| {
                target.state() == expected_state
                    && self.project().tracks().iter().any(|track| {
                        track
                            .instrument()
                            .is_some_and(|instrument| instrument.id() == device)
                    })
            })
            .ok_or(SessionError::Invalid(
                "sample loading canceled · target instrument changed",
            ))?;
        hex_engine::validate_sampler_state_on_control(target.state())?;
        PreparedDeviceSample::decode_on_worker(device, target.state(), path, limits, cancel)
    }
}
impl PreparedDeviceSample {
    /// Prepare real peaks and deep-zoom backing from this candidate's same
    /// original resource root. No PCM derivative, fabricated peaks or UI/audio
    /// scanning. The still-live decoder, returned bank and waveform each have
    /// actual owner fields which must join the full pending/current union.
    pub fn prepare_resources_and_waveform_on_worker(
        &self,
        session: &Session,
        candidate: &Project,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        limits: DeviceSampleWaveformLimits,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<
        (DeviceResourceBank, hex_media::waveform::Waveform),
        hex_media::DeviceSourceImportError,
    > {
        let bank = self.prepare_resource_bank_for_candidate_on_control(
            session,
            candidate,
            assets,
            reservation,
            cancel,
        )?;
        let reference = candidate
            .device(self.device)
            .and_then(|device| device.state().asset_references().first())
            .ok_or(ResourcePreparationError::InvalidSource)?;
        let asset = candidate
            .assets()
            .find(|asset| asset.id().value() == reference.asset.value())
            .ok_or(ResourcePreparationError::InvalidSource)?
            .id();
        let source = bank.borrow_original_source_on_control(asset, limits.source_metadata_bytes)?;
        let waveform = hex_media::waveform::summarize_original(
            &source,
            limits.bins,
            limits.scan_frames,
            limits.source_metadata_bytes,
            cancel,
        )?;
        Ok((bank, waveform))
    }
    pub fn decode_on_worker(
        device: DeviceId,
        current: &NativeDeviceState,
        path: impl AsRef<Path>,
        limits: hex_media::Limits,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<Self, SessionError> {
        let decoded = DecodedOriginal::decode_file(path, limits, cancel)?;
        if cancel.is_cancelled() {
            return Err(hex_media::ImportError::Cancelled.into());
        }
        Self::from_decoded_on_control(device, current, decoded)
    }
    pub fn from_native_on_control(
        device: DeviceId,
        current: &NativeDeviceState,
        path: PathBuf,
        source: OwnedAudioSource,
    ) -> Result<Self, SessionError> {
        Self::from_decoded_on_control(device, current, DecodedOriginal::from_native(path, source)?)
    }
    fn from_decoded_on_control(
        device: DeviceId,
        current: &NativeDeviceState,
        decoded: DecodedOriginal,
    ) -> Result<Self, SessionError> {
        let assignment =
            hex_engine::sampler_replacement_state_on_control(current, decoded.frames())?;
        let source_frames = u64::try_from(decoded.frames()).map_err(|_| {
            SessionError::Invalid("device source frame count exceeds project range")
        })?;
        let (path, source) = decoded.into_device_parts_on_control();
        Ok(Self {
            device,
            expected_state: current.clone(),
            assignment,
            path,
            source,
            source_frames,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn source_sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }
    pub fn source_frames(&self) -> u64 {
        self.source_frames
    }

    /// Metadata only. A host must execute this through prepared canonical
    /// editing with actual media/graph admission, not commit it by itself.
    pub fn assignment_command_on_control(&self) -> Command {
        Command::ImportDeviceAsset {
            device: self.device,
            expected_state: self.expected_state.clone(),
            state: self.assignment.state.clone(),
            slot: self.assignment.slot,
            path: self.path.clone(),
            source_sample_rate: self.source.sample_rate(),
            source_frames: self.source_frames,
        }
    }
    pub fn validate_target_on_control(&self, project: &Project) -> Result<(), SessionError> {
        if project
            .device(self.device)
            .is_none_or(|device| device.state() != &self.expected_state)
            || !project.tracks().iter().any(|track| {
                track
                    .instrument()
                    .is_some_and(|device| device.id() == self.device)
            })
        {
            return Err(SessionError::Invalid(
                "sample loading canceled · target instrument changed",
            ));
        }
        Ok(())
    }
    /// Actual pending decoder/assignment children, including both distinct
    /// canonical state FIELDS and path capacity. The worker's enclosing staging
    /// slot/body, other roots and original input/temporary peak remain separate.
    pub fn visit_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            device,
            expected_state,
            assignment,
            path,
            source: _,
            source_frames,
        } = self;
        let hex_engine::SamplerSampleAssignment { state, slot } = assignment;
        let _: (&DeviceId, &hex_lattice_contract::DeviceAssetSlot, &u64) =
            (device, slot, source_frames);
        ledger.borrow_canonical_native_state_on_control(expected_state)?;
        ledger.borrow_canonical_native_state_on_control(state)?;
        ledger.borrow_metadata_path_backing_on_control(path)?;
        match &self.source {
            OriginalAudio::Stereo(audio) => ledger.borrow_embedded_native_audio_on_control(audio),
            OriginalAudio::Mono(source) => ledger.borrow_embedded_native_source_on_control(source),
        }
    }
    /// PCM fields only; outer staging/path/state metadata and complete graph
    /// custody/peak/publication/retirement are separate mandatory obligations.
    pub fn visit_original_pcm_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        match &self.source {
            OriginalAudio::Stereo(audio) => ledger.borrow_native_audio_on_control(audio, role),
            OriginalAudio::Mono(source) => ledger.borrow_native_source_on_control(source, role),
        }
    }
    /// Prepare in the SAME bank/catalog family using the actual candidate
    /// Asset binding. Neither the candidate nor media is published here.
    pub fn prepare_resource_bank_for_candidate_on_control(
        &self,
        session: &Session,
        candidate: &Project,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        self.prepare_resource_bank_with_sources_on_control(
            session.project(),
            candidate,
            &crate::resource_plan::producer::ResidentResourceSources::new(&session.media),
            assets,
            reservation,
            cancel,
        )
    }
    /// Internal binding facts for the actual editor prepared-transition source
    /// and candidate. The media projection borrows no editor/whole Session.
    /// This is not a native reservation, install or publication operation.
    pub(crate) fn prepare_resource_bank_with_sources_on_control(
        &self,
        source: &Project,
        candidate: &Project,
        sources: &crate::resource_plan::producer::ResidentResourceSources<'_>,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        self.resolve_resource_plan_with_sources_on_control(
            source,
            candidate,
            sources,
            assets,
            reservation,
            cancel,
        )?
        .as_engine_plan()
        .prepare_on_control(reservation, cancel)
    }
    /// Actual assignment/source checks precede resolving borrowed input rows.
    /// This view is not worker/request freshness or an aggregate reservation;
    /// the caller must retain the actual transition and preparation custody.
    pub(crate) fn resolve_resource_plan_with_sources_on_control<'scope>(
        &'scope self,
        source: &Project,
        candidate: &Project,
        sources: &'scope crate::resource_plan::producer::ResidentResourceSources<'_>,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<
        crate::resource_plan::producer::ResolvedOriginalResources<'scope>,
        ResourcePreparationError,
    > {
        self.validate_target_on_control(source)
            .map_err(|_| ResourcePreparationError::InvalidSource)?;
        if candidate.sample_rate() != source.sample_rate() {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let state = candidate
            .device(self.device)
            .ok_or(ResourcePreparationError::InvalidSource)?
            .state();
        let expected = &self.assignment.state;
        if state.type_id() != expected.type_id()
            || state.state_version() != expected.state_version()
            || state.payload() != expected.payload()
            || state.input_gain_millidb() != expected.input_gain_millidb()
            || state.output_gain_millidb() != expected.output_gain_millidb()
            || state.asset_references().len() != 1
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let reference = &state.asset_references()[0];
        if reference.slot != self.assignment.slot {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let asset = candidate
            .assets()
            .find(|asset| asset.id().value() == reference.asset.value())
            .ok_or(ResourcePreparationError::InvalidSource)?;
        if asset.path() != self.path {
            return Err(ResourcePreparationError::InvalidSource);
        }
        crate::resource_plan::validate_candidate_assets(
            candidate,
            assets,
            reservation
                .max_preparation_metadata_bytes
                .min(reservation.max_construction_peak_bytes),
            cancel,
        )?;
        sources.resolve(
            assets,
            Some((asset, &self.source)),
            reservation.max_preparation_metadata_bytes,
            cancel,
        )
    }
}

#[cfg(test)]
mod tests;
