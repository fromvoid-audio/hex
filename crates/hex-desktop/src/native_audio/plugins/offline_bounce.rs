//! Private VST3 processors for one isolated Arrangement bounce item.
use super::*;
use hex_audio::plugin_effects::{NativeEffect, NativeEffectCallbacks, OfflineEffectPreparation};
use hex_audio::plugin_worker::NativeWorker;
use hex_engine::AudioCandidateEffects;
use hex_export::{ExportControl, ExportError, ForeignBounceRenderer};
use hex_session::service::OfflineBounceSource;

impl Plugins {
    pub(in crate::native_audio) fn prepare_offline_bounce(
        &self,
        input: ResidentEffectPreparation<'_>,
        control: &ExportControl,
        capacity: usize,
    ) -> Result<Option<Box<dyn OfflineBounceSource>>, String> {
        if !input.requires_plugin_profile() {
            return Ok(None);
        }
        let mut count = 0;
        input.visit_plugin_devices(|_, _, _| {
            count += 1;
            if count > capacity {
                return Err("offline vst3 processor limit reached".into());
            }
            Ok(())
        })?;
        if count == 0 {
            return Err("offline vst3 processor unavailable".into());
        }
        let catalog = hex_plugin_catalog::Catalog::load(
            self.catalog.as_deref().ok_or("vst3 catalog unavailable")?,
        )?;
        let host = self
            .host
            .as_ref()
            .ok_or("qualified vst3 helper unavailable")?;
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(count)
            .map_err(|_| "offline vst3 processor storage unavailable")?;
        let mut bytes = 0_usize;
        // Serial processors may extend one another's tails. Sum the known
        // lengths conservatively, including when a group contains branches.
        let mut tail = Some(0_u64);
        input.visit_plugin_devices(|destination, device, state| {
            if control.is_cancelled() {
                return Err("bounce cancelled".into());
            }
            let (profile, role) = if destination.is_instrument() {
                (
                    PluginProfile::Instrument,
                    hex_plugin_catalog::PluginRole::Instrument,
                )
            } else {
                (
                    PluginProfile::Effect,
                    hex_plugin_catalog::PluginRole::Effect,
                )
            };
            let identity = Hosting::saved_identity_on_control(state)?;
            let source = catalog.resolve_plugin(
                identity.class.bytes(),
                identity.binary.executable_sha256,
                identity.binary.info_sha256,
                role,
            )?;
            let authored = super::edits::requested_selection(&input, device)?;
            let effect = host.prepare_offline_effect_on_control(
                EffectSelection {
                    module: &source.module,
                    executable: &source.executable_relative_path,
                    cid: source.cid,
                    executable_sha256: &source.executable_sha256,
                    info_sha256: &source.info_sha256,
                },
                destination.owner(),
                device,
                input.sample_rate(),
                profile,
                state,
                authored.as_ref(),
                input
                    .plugin_device_bypassed(device)
                    .ok_or("offline vst3 device is missing")?,
            )?;
            bytes = bytes
                .checked_add(effect.retained_data_bytes())
                .ok_or("offline vst3 worker storage unavailable")?;
            tail = tail
                .zip(effect.finite_tail_frames())
                .and_then(|(sum, frames)| sum.checked_add(frames));
            effects.push(effect);
            Ok(())
        })?;
        if control.is_cancelled() {
            return Err("bounce cancelled".into());
        }
        let quantum = effects
            .first()
            .map(|effect| usize::from(effect.prepared().quantum_frames()))
            .ok_or("offline vst3 processor unavailable")?;
        Ok(Some(Box::new(PrivateProcessors {
            preparing: Some(effects),
            effects: None,
            running: None,
            bytes,
            quantum,
            tail,
        })))
    }
}

struct PrivateProcessors {
    preparing: Option<Vec<OfflineEffectPreparation>>,
    effects: Option<Vec<NativeEffect>>,
    running: Option<(Vec<NativeWorker>, NativeEffectCallbacks)>,
    bytes: usize,
    quantum: usize,
    tail: Option<u64>,
}

impl OfflineBounceSource for PrivateProcessors {
    fn retained_data_bytes(&self) -> usize {
        self.bytes
    }

    fn prepare(
        &self,
        compile: &mut dyn FnMut(
            &AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<PreparedRevision, String> {
        let effects = self
            .preparing
            .as_deref()
            .ok_or("offline vst3 source already prepared")?;
        OfflineEffectPreparation::prepare_revision_on_control(effects, compile)
    }
    fn finalize_on_control(
        &mut self,
        revision: &PreparedRevision,
        control: &ExportControl,
    ) -> Result<(), String> {
        let observation = hex_engine::PreparedPlaybackObservation::prepare_on_control(revision)
            .map_err(|e| e.to_string())?;
        let sources = self
            .preparing
            .take()
            .ok_or("offline vst3 source already finalized")?;
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(sources.len())
            .map_err(|_| "offline vst3 final storage unavailable")?;
        let mut bytes = 0_usize;
        for source in sources {
            let effect = source.complete_on_control(&observation, || control.is_cancelled())?;
            bytes = effect
                .constructor_storage_on_control()
                .and_then(|s| s.data_bytes())
                .and_then(|n| bytes.checked_add(n))
                .ok_or("offline vst3 final storage unavailable")?;
            effects.push(effect);
        }
        self.bytes = self.bytes.max(bytes);
        self.effects = Some(effects);
        Ok(())
    }
}

impl ForeignBounceRenderer for PrivateProcessors {
    fn max_block_frames(&self) -> usize {
        self.quantum
    }
    fn tail_is_drained(&self, frames_since_end: u64) -> bool {
        self.tail.is_some_and(|tail| frames_since_end >= tail)
    }

    fn render_block(
        &mut self,
        playback: &mut hex_engine::SessionPlayback,
        output: &mut [hex_engine::Frame],
        pages: Option<&mut dyn hex_engine::MediaPages>,
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        if let Some(effects) = self.effects.take() {
            self.running = Some(
                NativeEffectCallbacks::from_effects_on_control(effects)
                    .map_err(|_| ExportError::Invalid("offline vst3 endpoint unavailable"))?,
            );
        }
        let (workers, callbacks) = self
            .running
            .as_mut()
            .ok_or(ExportError::Invalid("offline vst3 source unavailable"))?;
        callbacks
            .begin_host_callback(output.len())
            .map_err(|_| ExportError::Invalid("offline vst3 block refused"))?;
        if let Err(error) =
            playback.render_with_foreign_processors(output, pages, None, None, callbacks)
        {
            let _ = callbacks.finish_host_callback(false);
            return Err(ExportError::SessionRender(error));
        }
        callbacks
            .finish_host_callback(true)
            .map_err(|_| ExportError::Invalid("offline vst3 block failed"))?;
        callbacks
            .wait_offline_on_control(workers, || control.is_cancelled())
            .map_err(|error| {
                if control.is_cancelled() {
                    ExportError::Cancelled
                } else {
                    ExportError::Media(error.to_string())
                }
            })
    }

    fn finish(&mut self, _: &ExportControl) -> Result<(), ExportError> {
        let (mut workers, _callbacks) = self.running.take().ok_or(ExportError::Invalid(
            "offline vst3 render produced no blocks",
        ))?;
        let mut failed = false;
        for worker in &mut workers {
            failed |= worker.retire_on_control().map_or(true, |retired| {
                retired.failure.is_some() || !retired.native.status.success()
            });
        }
        if failed {
            return Err(ExportError::Invalid(
                "offline vst3 worker retirement failed",
            ));
        }
        Ok(())
    }
}
