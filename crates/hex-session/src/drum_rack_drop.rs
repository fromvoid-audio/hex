//! Control-side session handler for dropping a sample file or built-in native
//! instrument onto a stable Drum Rack pad (`#3476`).
//!
//! Validates captured `(project_generation, revision, device, pad_id)` against
//! live session state, imports dropped sample files through the existing
//! `hex_media` decoder + `ResidentDeviceAssets` lease path (never a parallel
//! sample storage owner), commits atomic single-step Undo/Redo commands, and
//! returns exact completion acknowledgements while keeping prior pads and media
//! intact on stale targets, unsupported files, failed imports, or cancellation.

#[cfg(test)]
mod tests;

use crate::{
    Session, SessionError, native_assets::NativeAssetDestination, prepare_with_waveforms,
    resident_import::decode_resident_import_cancellable,
};
use hex_engine::native_device_resources::ResourcePreparationError;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{AssetId, Command, DeviceId};
use std::{
    num::NonZeroU64,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Payload dropped onto one Drum Rack pad by the UI/caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrumRackPadDropInput {
    NativeInstrument {
        type_id: DeviceTypeId,
        pad_name: Option<String>,
    },
    SampleFile {
        path: PathBuf,
        pad_name: Option<String>,
    },
}

/// Captured drop request targeting one Drum Rack pad at an exact project snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrumRackPadDropRequest {
    pub project_generation: u64,
    pub expected_revision: u64,
    pub device: DeviceId,
    pub pad_id: NonZeroU64,
    pub input: DrumRackPadDropInput,
}

/// Exact session completion receipt for one Drum Rack pad drop request.
pub struct DrumRackPadDropReceipt {
    pub project_generation: u64,
    pub revision: u64,
    pub device: DeviceId,
    pub pad_id: NonZeroU64,
    pub imported_asset: Option<AssetId>,
    pub prepared_revision: Arc<hex_engine::PreparedRevision>,
    pub prepared_resources: Option<hex_engine::native_device_resources::PreparedDeviceResources>,
}

impl Session {
    /// Admit a sample-file or native-instrument drop onto one stable Drum Rack
    /// pad in a single canonical Undo/Redo step with exact generation/revision
    /// validation and cancellation custody.
    pub fn drop_onto_drum_rack_pad(
        &mut self,
        project_generation: u64,
        request: DrumRackPadDropRequest,
        cancel: &AtomicBool,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<DrumRackPadDropReceipt, SessionError> {
        if cancel.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            return Err(ResourcePreparationError::Cancelled.into());
        }
        if request.project_generation != project_generation
            || request.expected_revision != self.history_revision()
        {
            return Err(SessionError::Invalid(
                "drum rack drop target is stale; project changed",
            ));
        }
        let expected_state = self
            .project()
            .device(request.device)
            .ok_or(SessionError::Invalid("drum rack device no longer exists"))?
            .state()
            .clone();

        match request.input {
            DrumRackPadDropInput::NativeInstrument { type_id, pad_name } => {
                let next_state = hex_engine::drum_rack_drop::build_pad_instrument_state(
                    &expected_state,
                    request.pad_id,
                    type_id,
                    pad_name.as_deref(),
                )
                .map_err(|_| SessionError::Invalid("unsupported drum rack pad instrument drop"))?;
                if cancel.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
                    return Err(ResourcePreparationError::Cancelled.into());
                }
                let (_, (prepared, resources)) = self.execute_prepared_device_state_drop(
                    Command::ReplaceDeviceState {
                        device: request.device,
                        state: next_state,
                    },
                    cancel,
                    previous,
                )?;
                Ok(DrumRackPadDropReceipt {
                    project_generation,
                    revision: self.history_revision(),
                    device: request.device,
                    pad_id: request.pad_id,
                    imported_asset: None,
                    prepared_revision: prepared,
                    prepared_resources: resources,
                })
            }
            DrumRackPadDropInput::SampleFile { path, pad_name } => {
                let decoded = decode_resident_import_cancellable(&path, cancel)?;
                if cancel.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
                    return Err(ResourcePreparationError::Cancelled.into());
                }
                let frames = u64::try_from(decoded.frames())
                    .map_err(|_| SessionError::Invalid("sample frame count exceeds range"))?;
                let next_asset = self.project().peek_next_asset_id()?;
                let derived_name = pad_name.or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                });
                let next_state = hex_engine::drum_rack_drop::build_pad_sampler_state(
                    &expected_state,
                    request.pad_id,
                    next_asset,
                    frames,
                    derived_name.as_deref(),
                )
                .map_err(|_| SessionError::Invalid("could not bind sample to drum rack pad"))?;
                let max_scan = self.native_reservation.map_or(0, |r| r.max_scan_frames);
                let (created, (prepared, resources)) = self.import_device_asset_prepared(
                    decoded,
                    cancel,
                    NativeAssetDestination::DeviceStateUpdate {
                        device: request.device,
                        expected_state,
                        expected_asset: next_asset,
                        next_state,
                    },
                    |candidate, media, table, transforms, waveforms, stage, _media_assets| {
                        let resources =
                            hex_engine::native_device_resources::PreparedDeviceResources::prepare_staged_on_control(
                                stage,
                                candidate,
                                20_000_000,
                                cancel,
                            )?;
                        let prepared = media.prepare_with_device_resources_and_catalog(
                            candidate,
                            table,
                            previous.map(|p| p.session().as_ref()),
                            transforms,
                            &resources,
                            max_scan,
                            cancel,
                            None,
                            None,
                            None,
                        )?;
                        let _ = waveforms;
                        Ok((Arc::new(prepared), resources))
                    },
                    |_, _, _| Ok(()),
                )?;
                let imported_asset = match created {
                    hex_project::Created::Asset(asset) => Some(asset),
                    _ => None,
                };
                Ok(DrumRackPadDropReceipt {
                    project_generation,
                    revision: self.history_revision(),
                    device: request.device,
                    pad_id: request.pad_id,
                    imported_asset,
                    prepared_revision: prepared,
                    prepared_resources: Some(resources),
                })
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn execute_prepared_device_state_drop(
        &mut self,
        command: Command,
        cancel: &AtomicBool,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<
        (
            hex_project::Created,
            (
                Arc<hex_engine::PreparedRevision>,
                Option<hex_engine::native_device_resources::PreparedDeviceResources>,
            ),
        ),
        SessionError,
    > {
        let media = self.media_bank();
        let table = self.sine_table();
        let transforms = &self.audio_transforms;
        let waveforms = &self.audio_clip_waveforms;
        let native_assets = &self.native_assets;
        let max_scan = self.native_reservation.map_or(0, |r| r.max_scan_frames);
        let has_native_assets = self.native_reservation.is_some();

        self.editor
            .execute_prepared_transition(command, |_old, candidate| {
                if cancel.load(Ordering::Acquire) {
                    return Err(ResourcePreparationError::Cancelled.into());
                }
                if has_native_assets
                    && candidate.tracks().iter().any(|t| {
                        t.receives_source_audio()
                            && t.instrument()
                                .is_some_and(|d| !d.state().asset_references().is_empty())
                    })
                {
                    let resources =
                        hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
                            native_assets,
                            candidate,
                            20_000_000,
                            cancel,
                        )?;
                    let prepared = media.prepare_with_device_resources_and_catalog(
                        candidate,
                        table,
                        previous.map(|p| p.session().as_ref()),
                        transforms,
                        &resources,
                        max_scan,
                        cancel,
                        None,
                        None,
                        None,
                    )?;
                    Ok((Arc::new(prepared), Some(resources)))
                } else {
                    let prepared = prepare_with_waveforms(
                        candidate,
                        &media,
                        table,
                        previous.map(|p| p.session().as_ref()),
                        transforms,
                        waveforms,
                    )?;
                    Ok((Arc::new(prepared), None))
                }
            })
    }
}
