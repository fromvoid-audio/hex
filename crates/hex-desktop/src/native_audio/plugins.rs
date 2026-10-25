//! Canonical-worker adapter only. Records contain request correlation and exact
//! Root bindings; Root retains every actual child, worker, pair and return.
use super::*;
use hex_audio::plugin_effects::EffectBinding as Binding;
use hex_audio::{
    NativeSourcePreparationProgress as Progress,
    plugin_effects::{EffectSelection, Hosting},
};
use hex_engine::{PreparedRevision, ProcessorChainOwner};
use hex_session::service::{NativeDeviceState, ResidentEffectPreparation};
use hex_session::service::{
    PluginEffectControl, PluginEffectPreparation, PluginEffectPreparationState,
};
mod restore;
use restore::Restore;

struct Source {
    binding: Binding,
    owner: ProcessorChainOwner,
    device: u64,
    operation: Arc<PluginEffectControl>,
    accepted: bool,
    retiring: bool,
    queued: bool,
}
struct Pending {
    operation: Arc<PluginEffectControl>,
    binding: Binding,
    context: AudioSourceContext,
    result: Option<Result<NativeDeviceState, String>>,
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) struct FactsGate {
    pub(super) reached: mpsc::SyncSender<()>,
    pub(super) resume: mpsc::Receiver<()>,
}
#[derive(Default)]
pub(super) struct Plugins {
    host: Option<Hosting>,
    initialized: bool,
    unavailable: Option<String>,
    profile_failed: bool,
    configured: bool,
    pub(super) pool_held: bool,
    sources: Vec<Source>,
    pending: Option<Pending>,
    restore: Option<Restore>,
    #[cfg(all(test, feature = "allocation-probe"))]
    presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) completed: Option<Arc<std::sync::atomic::AtomicU64>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) queued: Option<Arc<AtomicBool>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) records: Option<Arc<std::sync::atomic::AtomicUsize>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) facts_gate: Option<FactsGate>,
}
impl Plugins {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn with_fixture(
        host: Hosting,
        presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    ) -> Self {
        Self {
            host: Some(host),
            initialized: true,
            presentation,
            ..Self::default()
        }
    }
    pub fn initialize(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        match Hosting::current_on_control() {
            Ok(host) => self.host = Some(host),
            Err(error) => self.unavailable = Some(error),
        }
    }
    pub fn wants_profile(&self) -> bool {
        self.host.is_some() && !self.profile_failed
    }
    pub fn control_held(&self) -> bool {
        self.host.is_some() || self.sources.capacity() != 0 || self.pending.is_some()
    }
    pub fn refuse_profile(&mut self, reason: String) {
        self.profile_failed = true;
        self.configured = false;
        self.unavailable = Some(reason);
    }
    /// A fresh exclusive output Lease proves destruction of the prior Root.
    /// Preserve an unresolved request's terminal rather than silently restarting it.
    pub fn fresh_output(&mut self) {
        self.output_closed();
        // Session already completed an accepted submission. A fresh exclusive
        // Lease proves the old Root/gate retired even if its final ACK was lost
        // to output replacement. Unsubmitted requests still need their refusal.
        if self.restore.as_ref().is_some_and(Restore::submitted) {
            self.restore = None;
        }
        self.sources.clear();
        self.pool_held = false;
    }
    pub fn output_closed(&mut self) {
        self.configured = false;
        if let Some(restore) = self.restore.as_mut() {
            restore.output_closed();
        }
        if let Some(pending) = self.pending.as_mut() {
            pending.result = Some(Err("vst3 insertion cancelled · output changed".into()));
        }
    }
    pub fn configure(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        policy: crate::audio_policy::PluginPolicy,
    ) -> Result<(), String> {
        if !self.wants_profile() {
            return Ok(());
        }
        if control
            .requested_physical_callback_frames_on_control()
            .is_none()
        {
            return Err("vst3 output profile is unavailable".into());
        }
        self.sources
            .try_reserve_exact(policy.capacity)
            .map_err(|_| "vst3 source records unavailable")?;
        if self.sources.capacity() != policy.capacity {
            return Err("vst3 source record capacity differs".into());
        }
        let control_bytes = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<Source>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| n.checked_add(self.host.as_ref()?.control_data_bytes()?))
            .and_then(|n| n.checked_add(8192))
            .ok_or("vst3 control data overflow")?;
        if control_bytes > crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES {
            return Err("vst3 control data reservation exceeded".into());
        }
        control
            .configure_native_source_preparation(policy.capacity, policy.source)
            .map_err(str::to_owned)?;
        self.configured = true;
        self.pool_held = true;
        Ok(())
    }
    pub fn ready(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.result.is_some())
    }
    pub fn prepare(
        &mut self,
        output: &mut WorkerPreparedOutput,
        input: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        if let Some(pending) = self.pending.as_ref() {
            if !Arc::ptr_eq(&pending.operation, &input.insertion.control)
                || pending.context != input.insertion.context
            {
                return Err("another vst3 source operation is pending".into());
            }
        } else {
            if self.restore.is_some() {
                return Err("stopped vst3 restoration is pending".into());
            }
            if !self.configured {
                return Err(self
                    .unavailable
                    .clone()
                    .unwrap_or_else(|| "vst3 hosting unavailable for this output".into()));
            }
            if self.sources.len() >= self.sources.capacity() {
                return Err(
                    "vst3 source/history capacity is full · close the project before loading more"
                        .into(),
                );
            }
            if input.insertion.control.is_cancelled() {
                return Err("vst3 insertion cancelled".into());
            }
            let host = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?;
            let control = output.publication.control().ok_or("native output closed")?;
            let selection = &input.insertion.selection;
            let owner = input.insertion.destination.owner();
            let graph = input
                .insertion
                .context
                .editor_revision
                .checked_add(1)
                .ok_or("vst3 graph revision exhausted")?;
            let binding = host.begin_on_control(
                control,
                EffectSelection {
                    module: &selection.module,
                    executable: &selection.executable,
                    cid: selection.cid,
                    executable_sha256: &selection.executable_sha256,
                    info_sha256: &selection.info_sha256,
                },
                owner,
                input.device_id,
                graph,
                output.sample_rate,
                None,
            )?;
            self.sources.push(Source {
                binding,
                owner,
                device: input.device_id,
                operation: Arc::clone(&input.insertion.control),
                accepted: false,
                retiring: false,
                queued: false,
            });
            self.pending = Some(Pending {
                operation: Arc::clone(&input.insertion.control),
                binding,
                context: input.insertion.context,
                result: None,
            });
        }
        self.poll(output, Some(input.insertion.context));
        match self.pending.as_ref().and_then(|p| p.result.as_ref()) {
            Some(Ok(state)) => Ok(PluginEffectPreparationState::Ready(state.clone())),
            Some(Err(error)) => Err(error.clone()),
            None => Ok(PluginEffectPreparationState::Preparing),
        }
    }
    pub fn finish(&mut self, operation: &Arc<PluginEffectControl>, accepted: bool) {
        if self
            .pending
            .as_ref()
            .is_none_or(|p| !Arc::ptr_eq(&p.operation, operation))
        {
            return;
        }
        let pending = self.pending.take();
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|s| Arc::ptr_eq(&s.operation, operation))
        {
            source.accepted = accepted
                && pending
                    .as_ref()
                    .is_some_and(|p| matches!(p.result, Some(Ok(_))));
            source.retiring |= !source.accepted;
        }
    }
    pub fn poll(&mut self, output: &mut WorkerPreparedOutput, context: Option<AudioSourceContext>) {
        let Some(control) = output.publication.control() else {
            self.output_closed();
            return;
        };
        self.poll_restore(control, context);
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(observed) = self.completed.as_ref() {
            let count = self
                .sources
                .iter()
                .filter(|s| s.accepted)
                .filter_map(|s| control.native_source_completed_blocks(s.binding))
                .min();
            if let Some(count) = count {
                observed.store(count, std::sync::atomic::Ordering::Release);
            }
        }
        if let Some(pending) = self.pending.as_mut() {
            if pending.result.is_none() {
                if pending.operation.is_cancelled()
                    || context.is_none_or(|current| {
                        current.project_generation != pending.context.project_generation
                    })
                {
                    pending.result = Some(Err("vst3 insertion cancelled · source changed".into()));
                } else if let Some(source) = self
                    .sources
                    .iter_mut()
                    .find(|s| s.binding == pending.binding)
                {
                    #[cfg(all(test, feature = "allocation-probe"))]
                    if matches!(
                        control.native_source_preparation_progress(source.binding),
                        Some(Progress::Facts(_))
                    ) && let Some(gate) = self.facts_gate.take()
                    {
                        assert!(
                            gate.reached.send(()).is_ok(),
                            "test facts gate disconnected"
                        );
                        assert!(
                            gate.resume
                                .recv_timeout(std::time::Duration::from_secs(10))
                                .is_ok(),
                            "test facts gate was not released"
                        );
                    }
                    #[cfg(not(all(test, feature = "allocation-probe")))]
                    let presentation = None;
                    #[cfg(all(test, feature = "allocation-probe"))]
                    let presentation = self.presentation;
                    let result = self
                        .host
                        .as_ref()
                        .ok_or_else(|| "qualified vst3 helper unavailable".to_owned())
                        .and_then(|host| advance_source(host, presentation, control, source));
                    match result {
                        Ok(Some(state)) => pending.result = Some(Ok(state)),
                        Ok(None) => {}
                        Err(error) => pending.result = Some(Err(bounded_error(error))),
                    }
                } else {
                    pending.result = Some(Err("original vst3 source unavailable".into()));
                }
            }
        }
        // Refused operations retain their original binding until actual report
        // recovery. Accepted sources stay warm for canonical Undo/Redo, bounded
        // by this document's finite source pool, until output retirement.
        let mut index = 0;
        // Renewal's bounded indices refer to this original record set. Leave
        // refused records in control custody until the epoch operation retires;
        // removing one here would shift a later source past its renewal stage.
        while self.restore.is_none() && index < self.sources.len() {
            let source = &self.sources[index];
            if source.retiring {
                let _ = control.retire_native_source_preparation(source.binding);
                if control
                    .recover_native_source_preparation(source.binding)
                    .is_some()
                {
                    self.sources.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        {
            if let Some(queued) = &self.queued {
                queued.store(
                    self.sources.iter().any(|s| s.queued && !s.accepted),
                    std::sync::atomic::Ordering::Release,
                );
            }
            if let Some(records) = &self.records {
                records.store(self.sources.len(), std::sync::atomic::Ordering::Release);
            }
        }
    }
    pub fn compile(
        &self,
        output: &mut WorkerPreparedOutput,
        input: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(self.sources.len())
            .map_err(|_| "vst3 compiler selection unavailable")?;
        input.visit_plugin_effects(|owner, device| {
            let source = self
                .sources
                .iter()
                .find(|s| s.owner == owner && s.device == device && !s.retiring)
                .ok_or("original vst3 source unavailable for this device")?;
            bindings.push(source.binding);
            Ok(())
        })?;
        if bindings.is_empty() {
            return Ok(None);
        }
        let control = output.publication.control().ok_or("native output closed")?;
        Hosting::prepare_on_control(control, &bindings, prepare).map(Some)
    }
}

fn bounded_error(error: String) -> String {
    if error.capacity() <= 8192 {
        return error;
    }
    // Root retains the original complete diagnostic; UI correlation keeps only
    // a bounded explanation while its actual owner reaches disposal/recovery.
    error.chars().take(1024).collect()
}

/// The same original preparation and endpoint ACK protocol serves insertion and
/// stopped renewal. Returning captured state never admits a canonical graph.
fn advance_source(
    host: &Hosting,
    presentation: Option<hex_audio::plugin_effects::EffectPresentation>,
    control: &mut hex_audio::prepared::PublicationControl,
    source: &mut Source,
) -> Result<Option<NativeDeviceState>, String> {
    match control.native_source_preparation_progress(source.binding) {
        Some(Progress::Facts(_)) => {
            match host.complete_on_control(control, source.binding, presentation) {
                Ok(()) => {}
                // Root has not taken or changed this source when Work is busy.
                // Retain its exact operation and re-poll through the existing
                // worker loop; contention is neither native ACK nor refusal.
                Err(reason) if reason == "native preparation is busy" => return Ok(None),
                Err(reason) => return Err(reason),
            }
        }
        Some(Progress::Prepared { .. }) => match control.start_native_effect(source.binding) {
            Ok(()) => {}
            Err("native preparation is busy") => return Ok(None),
            Err(reason) => return Err(reason.to_owned()),
        },
        Some(Progress::Effect { .. }) => {
            if let Some((loaned, live)) = control.native_source_endpoint_status(source.binding) {
                if !live || (source.queued && !loaned) {
                    return Err("vst3 endpoint returned or stopped before publication".into());
                }
            }
            if !source.queued {
                match control.publish_native_source_endpoint(source.binding) {
                    Ok(()) => source.queued = true,
                    Err("native endpoint publication is busy" | "native preparation is busy") => {
                        return Ok(None);
                    }
                    Err(reason) => return Err(reason.to_owned()),
                }
            }
            if control.native_source_endpoint_installed(source.binding) == Some(true) {
                return Ok(control.native_source_prepared_state(source.binding));
            }
        }
        Some(Progress::Failed | Progress::Complete | Progress::Retiring) => {
            return match control.try_native_source_preparation_error(source.binding) {
                Ok(Some(reason)) => Err(reason),
                Ok(None) => Err("vst3 preparation refused".into()),
                Err("native preparation is busy") => Ok(None),
                Err(reason) => Err(reason.to_owned()),
            };
        }
        Some(Progress::Preparing) | None => {}
    }
    Ok(None)
}
