//! Nonhosting desktop boundary on targets without the macOS arm64 VST3 host.
//!
//! No source, worker, endpoint, capture or restoration is admitted here. Built-in
//! graphs keep the ordinary compiler. Foreign-effect graphs and native operations
//! refuse explicitly; saving leaves canonical opaque state in its existing owner.
use super::*;
use hex_audio::prepared::{PublicationControl, PublicationPolicy};
use hex_engine::PreparedRevision;
use hex_session::service::{
    PluginEffectControl, PluginEffectPreparation, PluginEffectPreparationState, PluginStateCapture,
    PluginStateSaveInput, RequestCancellation, ResidentEffectPreparation,
};

const UNAVAILABLE: &str = "vst3 hosting requires macos arm64";

#[derive(Default)]
pub(super) struct Plugins {
    // No native pool can be acquired on this target.
    pub(super) pool_held: bool,
}
impl Plugins {
    pub fn set_diagnostics(&mut self, _: Option<hex_diagnostics::DiagnosticsHandle>) {}
    pub fn initialize(&mut self) {}
    pub fn wants_profile(&self) -> bool {
        false
    }
    pub fn control_held(&self) -> bool {
        false
    }
    pub fn refuse_profile(&mut self, _: String) {}
    pub fn fresh_output(&mut self) {}
    pub fn output_closed(&mut self) {}
    pub fn configure(
        &mut self,
        _: &mut PublicationControl,
        _: crate::audio_policy::PluginPolicy,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn idle(&self) -> bool {
        true
    }
    pub(super) fn can_retain_output_for_document(
        &self,
        _: &PublicationControl,
        _: u64,
        incoming: ResidentEffectPreparation<'_>,
    ) -> bool {
        !incoming.requires_plugin_profile()
    }
    pub(super) fn retire_document_sources(&mut self, _generation: u64) {}
    pub fn ready(&self) -> bool {
        true
    }
    pub fn take_degraded_change(&mut self) -> Option<(u64, u64, Vec<u64>)> {
        None
    }
    pub fn prepare(
        &mut self,
        _: &mut WorkerPreparedOutput,
        _: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn finish(&mut self, _: &Arc<PluginEffectControl>, _: bool) {}
    pub fn poll(&mut self, _: &mut WorkerPreparedOutput, _: Option<AudioSourceContext>) {}
    pub fn poll_latency_restart(
        &mut self,
        _: &mut WorkerPreparedOutput,
        _: AudioSourceContext,
    ) -> Option<hex_audio::prepared::AppliedAudioSource> {
        None
    }
    pub fn preparing_edit(&self, _: AudioSourceContext) -> bool {
        false
    }
    pub fn finish_edit(&mut self, _: AudioSourceContext, _: bool) {}
    pub fn prepare_restore(
        &mut self,
        _: &mut WorkerPreparedOutput,
        _: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        Ok(None)
    }
    pub fn restore_ready(&self) -> bool {
        false
    }
    pub fn cancel_restore(&mut self, _: Option<&mut PublicationControl>) {}
    pub fn restore_policy(
        &self,
        _: AudioSourceContext,
    ) -> Result<Option<PublicationPolicy>, String> {
        Ok(None)
    }
    pub fn restore_submitted(&mut self, _: u64) {}
    pub fn poll_restore(&mut self, _: &mut PublicationControl, _: Option<AudioSourceContext>) {}
    pub fn request_reopen(&mut self, _: &CanonicalAudioSourceSnapshot) {}
    pub fn reopen_ready(&self) -> Option<AudioSourceContext> {
        None
    }
    pub fn hold_reopen(&self, _: AudioSourceContext) -> bool {
        false
    }
    pub fn check_reopen_publication(
        &self,
        _: AudioSourceContext,
        restoring: bool,
    ) -> Result<(), String> {
        if restoring {
            Err(UNAVAILABLE.into())
        } else {
            Ok(())
        }
    }
    pub fn finish_reopen(&mut self, _: AudioSourceContext, _: bool) -> bool {
        false
    }
    pub fn poll_reopen(
        &mut self,
        _: &mut WorkerPreparedOutput,
        _: Option<&CanonicalAudioSourceSnapshot>,
    ) {
    }
    pub fn compile(
        &mut self,
        _: Option<&mut WorkerPreparedOutput>,
        input: ResidentEffectPreparation<'_>,
        _: Option<&CanonicalAudioSourceSnapshot>,
        _: Option<&RequestCancellation>,
        _: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        // No native work is admitted here. Document replacement advances
        // intent when queued, so its prior-intent token must not be reused to
        // cancel ordinary compilation. Session owns those admission checks.
        input.visit_plugin_devices(|_, _, _| Err(UNAVAILABLE.into()))?;
        Ok(None)
    }
}

impl NativeAudioSink {
    pub(in crate::native_audio) fn capture_plugin_save(
        &mut self,
        _: PluginStateSaveInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        // No hosted source exists. Session's empty-capture contract retains the
        // complete canonical opaque payload; this never claims a fresh capture.
        Ok(Vec::new())
    }
}
