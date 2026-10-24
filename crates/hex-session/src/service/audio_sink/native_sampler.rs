//! Synchronous original-source borrow on the canonical Session worker.
use super::super::sampler_export::Admission;
use super::*;
use hex_engine::{
    AudioCandidatePreparation, AudioCandidateReservationQuote, AudioStorageReservation,
};

/// Conservative native custody reservation; these scalars grant no authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeSamplerReservationHold {
    pub retained_bytes: usize,
    pub construction_bytes: usize,
    pub frames: u64,
    pub current_context: Option<hex_engine::AudioSourceContext>,
}
/// A nonowning retry key for unchanged control-side capacity waits. A changed
/// key only requests fresh admission; it never grants space or source custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSamplerRetryFacts {
    native: NativeSamplerReservationHold,
    export_runtime: usize,
    export_present: bool,
    outbound_job: Option<u64>,
}
/// The actual committed or still-staged transaction, borrowed only on control.
/// Neither variant projects an owning source or a temporary committed store.
#[derive(Clone, Copy)]
pub(in crate::service) enum NativeSamplerSources<'a> {
    Committed(&'a hex_engine::native_device_resources::ResidentDeviceAssets),
    Staged(&'a hex_engine::native_device_resources::StagedNativeAssetView<'a>),
}
impl NativeSamplerSources<'_> {
    fn source(&self, asset: hex_project::AssetId) -> Option<hex_engine::OwnedAudioSourceView<'_>> {
        match self {
            Self::Committed(store) => store.source(asset),
            Self::Staged(stage) => stage.source(asset),
        }
    }
    pub(in crate::service) fn inspect(
        self,
        phase: &hex_engine::native_device_resources::DeviceResourcePhase<'_>,
        limits: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<
        hex_engine::native_device_resources::DeviceResourceInventory,
        hex_engine::native_device_resources::ResourcePreparationError,
    > {
        use hex_engine::native_device_resources::{self as resources, DeviceResourcePhase};
        match self {
            Self::Committed(store) => {
                let stores = std::iter::once(store)
                    .chain(phase.stores.iter().copied())
                    .collect::<Vec<_>>();
                let actual = DeviceResourcePhase {
                    registered: phase.registered,
                    stores: &stores,
                    selections: phase.selections,
                    outbound: phase.outbound,
                    legacy: phase.legacy,
                    checked_runtime_bytes: phase.checked_runtime_bytes,
                    construction_bytes: phase.construction_bytes,
                };
                resources::inspect_device_resource_union(&actual, &actual, limits)
            }
            Self::Staged(stage) => stage.inspect(phase, phase, limits),
        }
    }
}
/// This borrow cannot be cloned, retained by a sink or obtained from an Event.
pub struct NativeSamplerInput<'a> {
    pub(in crate::service) project: &'a Project,
    pub(in crate::service) media:
        &'a std::collections::HashMap<hex_project::AssetId, crate::ResidentAsset>,
    pub(in crate::service) originals: NativeSamplerSources<'a>,
    pub(in crate::service) reservation: Option<crate::ResidentNativeReservation>,
    pub(in crate::service) audio_mode: crate::DocumentAudioMode,
    pub(in crate::service) max_frames: u64,
    pub(in crate::service) shutdown: &'a AtomicBool,
    pub(in crate::service) candidate:
        Option<hex_engine::native_device_resources::ControlSamplerSelection<'a>>,
    pub(in crate::service) context: hex_engine::AudioSourceContext,
    pub(in crate::service) admission: Admission<'a>,
}
impl NativeSamplerInput<'_> {
    pub fn retry_facts(&self) -> NativeSamplerRetryFacts {
        NativeSamplerRetryFacts {
            native: self.admission.native,
            export_runtime: self.admission.runtime_bytes,
            export_present: self.admission.export.is_some(),
            outbound_job: self.admission.outbound.map(|o| o.job),
        }
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
    }
    pub fn requires_sampler(&self) -> bool {
        hex_engine::native_device_resources::requires_private_sampler_sources(self.project)
    }
    fn needed(&self, asset: hex_project::AssetId) -> bool {
        let project = self.project;
        project.tracks().iter().any(|t| t.clips().iter().any(|c| c.referenced_assets().any(|source| source == asset))
            || t.instrument().is_some_and(|d| d.state().asset_references().iter().any(|r| r.asset.value() == asset.value())))
            || project.session_cells().iter().any(|c| matches!(c.content(), hex_project::SessionCellContent::Audio(c) if c.asset() == asset))
    }
    fn sampler_asset(&self, asset: hex_project::AssetId) -> bool {
        self.project
            .tracks()
            .iter()
            .filter_map(|t| t.instrument())
            .any(|d| {
                d.state()
                    .asset_references()
                    .iter()
                    .any(|r| r.asset.value() == asset.value())
            })
    }
    pub fn quote(
        &self,
    ) -> Result<
        (
            AudioCandidateReservationQuote,
            AudioStorageReservation,
            usize,
            u64,
        ),
        String,
    > {
        self.quote_inner(None)
    }
    pub fn quote_with_effects(
        &self,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<
        (
            AudioCandidateReservationQuote,
            AudioStorageReservation,
            usize,
            u64,
        ),
        String,
    > {
        self.quote_inner(Some(effects))
    }
    fn quote_inner(
        &self,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<
        (
            AudioCandidateReservationQuote,
            AudioStorageReservation,
            usize,
            u64,
        ),
        String,
    > {
        if !self.requires_sampler()
            || self.audio_mode != crate::DocumentAudioMode::Resident
            || self.reservation.is_none()
        {
            return Err("sampler native playback requires an explicit resident reservation".into());
        }
        let mut count = 0usize;
        let mut frames = 0u64;
        for asset in self.project.assets().filter(|a| self.needed(a.id())) {
            let length = if self.sampler_asset(asset.id()) {
                let source = self
                    .originals
                    .source(asset.id())
                    .ok_or("sampler original source is unavailable")?;
                source
                    .resident_frames()
                    .ok_or("sampler native source must be resident")?
                    .len()
            } else {
                self.media
                    .get(&asset.id())
                    .ok_or("resident source is unavailable")?
                    .audio
                    .frames()
                    .len()
            };
            frames = frames
                .checked_add(u64::try_from(length).map_err(|_| "source frame count overflow")?)
                .ok_or("source frame count overflow")?;
            count = count.checked_add(1).ok_or("source count overflow")?;
        }
        let quote = effects
            .map_or_else(
                || AudioCandidateReservationQuote::for_project(self.project, count),
                |effects| effects.quote(self.project, count),
            )
            .map_err(|e| format!("sampler constructor quote refused: {e:?}").to_lowercase())?;
        let pcm = usize::try_from(frames)
            .ok()
            .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
            .ok_or("sampler pcm reservation overflow")?;
        let storage = quote
            .storage_reservation(pcm)
            .map_err(|e| format!("sampler reservation refused: {e:?}").to_lowercase())?;
        // The native constructor retains one additional finite PCM root for
        // each transformed clip. Reserve physical frames before any copy/work.
        frames = frames
            .checked_add(quote.prepared_pcm_frames)
            .ok_or("prepared audio frame reservation overflow")?;
        Ok((quote, storage, count, frames))
    }
    /// Reserve additional native copy/control together with actual Session and
    /// export owners before copying. The existing physical frame cap is shared.
    pub fn admit_additional(
        &self,
        hold: NativeSamplerReservationHold,
        actual_native: NativeSamplerReservationHold,
    ) -> Result<(), String> {
        let mut admission = self.admission;
        // Sink has atomically suspended old forwarding and refreshed its actual
        // obligations after the worker's earlier observation. Never reuse that
        // potentially stale native sample for admission.
        admission.native = actual_native;
        admission.native.retained_bytes = admission
            .native
            .retained_bytes
            .checked_add(hold.retained_bytes)
            .ok_or("native retained reservation overflow")?;
        admission.native.construction_bytes = admission
            .native
            .construction_bytes
            .checked_add(hold.construction_bytes)
            .ok_or("native peak reservation overflow")?;
        admission.native.frames = admission
            .native
            .frames
            .checked_add(hold.frames)
            .ok_or("native frame reservation overflow")?;
        let legacy = self
            .media
            .values()
            .flat_map(|m| [&m.source_audio, &m.audio])
            .collect::<Vec<_>>();
        admission
            .check_native_originals(
                self.originals,
                &legacy,
                self.reservation,
                self.max_frames,
                self.candidate,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Original sampler PCM is copied directly into persistent constructor
    /// custody; ordinary audio clips retain their existing resident derivative.
    pub fn prepare(
        &self,
        catalog: u64,
        cell: &mut AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.copy_sources(cell)?;
        let limits = self
            .reservation
            .ok_or(hex_engine::AudioStorageError::ReservationExceeded)?;
        cell.prepare_current_sampler_on_control(
            self.project,
            catalog,
            self.context,
            limits.max_scan_frames,
            self.shutdown,
        )
    }
    pub fn prepare_with_effects(
        &self,
        catalog: u64,
        cell: &mut AudioCandidatePreparation,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.copy_sources(cell)?;
        let limits = self
            .reservation
            .ok_or(hex_engine::AudioStorageError::ReservationExceeded)?;
        effects.prepare(
            cell,
            self.project,
            catalog,
            self.context,
            Some((limits.max_scan_frames, self.shutdown)),
        )
    }
    fn copy_sources(
        &self,
        cell: &mut AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        for asset in self.project.assets().filter(|a| self.needed(a.id())) {
            if self.sampler_asset(asset.id()) {
                cell.copy_original_source_on_control(
                    asset.id(),
                    self.originals
                        .source(asset.id())
                        .ok_or(hex_engine::AudioStorageError::SourceMismatch)?,
                )?;
            } else {
                cell.copy_resident_source_on_control(
                    asset.id(),
                    &self
                        .media
                        .get(&asset.id())
                        .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
                        .audio,
                )?;
            }
        }
        Ok(())
    }
}
pub(in crate::service) fn drive(
    audio: Option<&mut dyn WorkerAudioSink>,
    session: &Session,
    context: hex_engine::AudioSourceContext,
    lane: &mut super::super::export_lane::ExportLane,
    current: Option<hex_engine::native_device_resources::ControlSamplerSelection<'_>>,
) {
    lane.poll();
    if let Some(audio) = audio {
        lane.native_hold = audio.native_sampler_hold();
        if audio.resident_publication_enabled() {
            return;
        }
        audio.native_sampler_input(NativeSamplerInput {
            project: session.project(),
            media: &session.media,
            originals: NativeSamplerSources::Committed(&session.native_assets),
            reservation: session.native_reservation,
            audio_mode: session.audio_mode,
            max_frames: session.max_resident_frames,
            shutdown: &session.shutdown,
            candidate: None,
            context,
            admission: lane.admission(current),
        });
        lane.native_hold = audio.native_sampler_hold();
    }
}
