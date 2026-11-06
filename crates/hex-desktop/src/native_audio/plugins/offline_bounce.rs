//! Private instrument processing for one resident Arrangement bounce.
use super::*;
use hex_audio::plugin_effects::{NativeEffect, NativeEffectCallbacks, NativeEffectDefinition};
use hex_audio::plugin_worker::NativeWorker;
use hex_engine::AudioCandidateEffects;
use hex_export::{ExportControl, ExportError, ForeignBounceRenderer};
use hex_session::service::OfflineBounceSource;

impl Plugins {
    pub(in crate::native_audio) fn prepare_offline_bounce(
        &self,
        input: ResidentEffectPreparation<'_>,
    ) -> Result<Option<Box<dyn OfflineBounceSource>>, String> {
        if !input.requires_plugin_profile() {
            return Ok(None);
        }
        let mut selected = None;
        input.visit_plugin_devices(|destination, device, state| {
            if !destination.is_instrument() || selected.is_some() {
                return Err(
                    "offline bounce requires one vst3 instrument without vst3 effects".into(),
                );
            }
            selected = Some((destination, device, state));
            Ok(())
        })?;
        let (destination, device, state) = selected.ok_or("offline vst3 instrument unavailable")?;
        let identity = Hosting::saved_identity_on_control(state)?;
        let source = hex_plugin_catalog::Catalog::load(
            self.catalog.as_deref().ok_or("vst3 catalog unavailable")?,
        )?
        .resolve_plugin(
            identity.class.bytes(),
            identity.binary.executable_sha256,
            identity.binary.info_sha256,
            hex_plugin_catalog::PluginRole::Instrument,
        )?;
        let effect = self
            .host
            .as_ref()
            .ok_or("qualified vst3 helper unavailable")?
            .prepare_offline_instrument_on_control(
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
                state,
            )?;
        let bytes = effect
            .constructor_storage_on_control()
            .and_then(|s| s.data_bytes())
            .ok_or("offline vst3 worker storage unavailable")?;
        let quantum = usize::from(effect.prepared().quantum_frames());
        let tail = effect.finite_tail_frames();
        Ok(Some(Box::new(PrivateInstrument {
            effect: Some(effect),
            running: None,
            bytes,
            quantum,
            tail,
        })))
    }
}

struct PrivateInstrument {
    effect: Option<NativeEffect>,
    running: Option<(Vec<NativeWorker>, NativeEffectCallbacks)>,
    bytes: usize,
    quantum: usize,
    tail: Option<u64>,
}

impl OfflineBounceSource for PrivateInstrument {
    fn retained_data_bytes(&self) -> usize {
        self.bytes
    }

    fn prepare(
        &self,
        compile: &mut dyn FnMut(
            &AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<PreparedRevision, String> {
        let effect = self
            .effect
            .as_ref()
            .ok_or("offline vst3 source already rendering")?;
        let definition =
            NativeEffectDefinition::new(std::slice::from_ref(effect)).map_err(|e| e.to_string())?;
        AudioCandidateEffects::with_host_definitions(&[&definition], compile)
            .map_err(|e| e.to_string())
    }
}

impl ForeignBounceRenderer for PrivateInstrument {
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
        if let Some(effect) = self.effect.take() {
            self.running = Some(
                NativeEffectCallbacks::from_effects_on_control(vec![effect])
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
        for worker in &mut workers {
            let retired = worker
                .retire_on_control()
                .map_err(|_| ExportError::Invalid("offline vst3 worker retirement failed"))?;
            if retired.failure.is_some() || !retired.native.status.success() {
                return Err(ExportError::Invalid(
                    "offline vst3 worker retirement failed",
                ));
            }
        }
        Ok(())
    }
}
