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
/// This borrow cannot be cloned, retained by a sink or obtained from an Event.
pub struct NativeSamplerInput<'a> {
    session: &'a Session,
    context: hex_engine::AudioSourceContext,
    admission: Admission<'a>,
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
        self.session.project().sample_rate()
    }
    pub fn requires_sampler(&self) -> bool {
        hex_engine::native_device_resources::requires_private_sampler_sources(
            self.session.project(),
        )
    }
    fn needed(&self, asset: hex_project::AssetId) -> bool {
        let project = self.session.project();
        project.tracks().iter().any(|t| t.clips().iter().any(|c| c.asset() == asset)
            || t.instrument().is_some_and(|d| d.state().asset_references().iter().any(|r| r.asset.value() == asset.value())))
            || project.session_cells().iter().any(|c| matches!(c.content(), hex_project::SessionCellContent::Audio(c) if c.asset() == asset))
    }
    fn sampler_asset(&self, asset: hex_project::AssetId) -> bool {
        self.session
            .project()
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
        if !self.requires_sampler()
            || self.session.audio_mode() != crate::DocumentAudioMode::Resident
            || self.session.native_reservation.is_none()
        {
            return Err("sampler native playback requires an explicit resident reservation".into());
        }
        let mut count = 0usize;
        let mut frames = 0u64;
        for asset in self
            .session
            .project()
            .assets()
            .filter(|a| self.needed(a.id()))
        {
            let length = if self.sampler_asset(asset.id()) {
                let source = self
                    .session
                    .native_source(asset.id())
                    .ok_or("sampler original source is unavailable")?;
                source
                    .resident_frames()
                    .ok_or("sampler native source must be resident")?
                    .len()
            } else {
                self.session
                    .media
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
        let quote = AudioCandidateReservationQuote::for_project(self.session.project(), count)
            .map_err(|e| format!("sampler constructor quote refused: {e:?}").to_lowercase())?;
        let pcm = usize::try_from(frames)
            .ok()
            .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
            .ok_or("sampler pcm reservation overflow")?;
        let storage = quote
            .storage_reservation(pcm)
            .map_err(|e| format!("sampler reservation refused: {e:?}").to_lowercase())?;
        Ok((quote, storage, count, frames))
    }
    /// Reserve additional native copy/control together with actual Session and
    /// export owners before copying. The existing physical frame cap is shared.
    pub fn admit_additional(&self, hold: NativeSamplerReservationHold) -> Result<(), String> {
        let mut admission = self.admission;
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
        admission
            .check_legacy_stage(self.session, &[])
            .map_err(|e| e.to_string())
    }
    /// Original sampler PCM is copied directly into persistent constructor
    /// custody; ordinary audio clips retain their existing resident derivative.
    pub fn prepare(
        &self,
        catalog: u64,
        cell: &mut AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        for asset in self
            .session
            .project()
            .assets()
            .filter(|a| self.needed(a.id()))
        {
            if self.sampler_asset(asset.id()) {
                cell.copy_original_source_on_control(
                    asset.id(),
                    self.session
                        .native_source(asset.id())
                        .ok_or(hex_engine::AudioStorageError::SourceMismatch)?,
                )?;
            } else {
                cell.copy_resident_source_on_control(
                    asset.id(),
                    &self
                        .session
                        .media
                        .get(&asset.id())
                        .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
                        .audio,
                )?;
            }
        }
        let limits = self
            .session
            .native_reservation
            .ok_or(hex_engine::AudioStorageError::ReservationExceeded)?;
        cell.prepare_current_sampler_on_control(
            self.session.project(),
            catalog,
            self.context,
            limits.max_scan_frames,
            &self.session.shutdown,
        )
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
        audio.native_sampler_input(NativeSamplerInput {
            session,
            context,
            admission: lane.admission(current),
        });
        lane.native_hold = audio.native_sampler_hold();
    }
}
