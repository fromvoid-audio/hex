//! Independent stopped monitor sources use the original Root's worker and
//! endpoint custody. Only immutable graph state is restored into each worker.
use super::*;
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_engine::{PausedMonitorForeignBinding, PausedTrackMonitor};
use hex_session::service::{PausedInputMonitorRequest, RequestCancellation};
use std::time::{Duration, Instant};

const PREPARATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound all waits for one request on the dedicated Session worker. The
/// independent transport, document cancellation and output faults can stop it
/// earlier. Actual SDK construction retains its own native timeout and custody.
pub(in crate::native_audio) struct PausedMonitorPreparation<'a> {
    scope: (AppliedAudioSource, u64),
    cancellation: Option<&'a RequestCancellation>,
    deadline: Instant,
}

impl<'a> PausedMonitorPreparation<'a> {
    pub(in crate::native_audio) fn new(
        request: PausedInputMonitorRequest,
        cancellation: Option<&'a RequestCancellation>,
    ) -> Self {
        Self {
            scope: (
                AppliedAudioSource {
                    context: request.source.context,
                    output_identity: request.source.output_identity,
                    audio_revision: request.source.audio_revision,
                },
                request.transport_epoch,
            ),
            cancellation,
            deadline: Instant::now() + PREPARATION_TIMEOUT,
        }
    }

    fn wait<T>(
        &self,
        control: &mut PublicationControl,
        mut attempt: impl FnMut(&mut PublicationControl) -> Result<Option<T>, String>,
    ) -> Result<T, String> {
        loop {
            if self
                .cancellation
                .is_some_and(RequestCancellation::is_cancelled)
                || control.status.paused_input_source() != Some(self.scope)
            {
                return Err("paused vst3 monitor source changed".into());
            }
            if Instant::now() >= self.deadline {
                return Err("paused vst3 monitor preparation timed out".into());
            }
            match attempt(control) {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native preparation is busy"
                            | "native source is busy"
                            | "native endpoint publication is busy"
                            | "native effect preparation is busy"
                            | "native effect selection is busy"
                    ) => {}
                Err(error) => return Err(bounded_error(error)),
            }
            edits::poll_control(control);
        }
    }
}

impl Plugins {
    /// Only after the monitor owner has been destroyed on control can its
    /// bindings retire. Actual endpoint returns and worker joins stay in Root.
    pub(in crate::native_audio) fn reap_paused_monitor(
        &mut self,
        control: &mut PublicationControl,
    ) {
        self.paused_monitor_sources.retain(|&binding| {
            let _ = control.retire_native_source_preparation(binding);
            control.recover_native_source_preparation(binding).is_none()
        });
    }

    pub(in crate::native_audio) fn prepare_paused_monitor_effects(
        &mut self,
        control: &mut PublicationControl,
        preparation: &PausedMonitorPreparation<'_>,
        revision: &PreparedRevision,
    ) -> Result<Vec<PausedMonitorForeignBinding>, String> {
        if !self.paused_monitor_sources.is_empty() {
            preparation.wait(control, |control| {
                self.reap_paused_monitor(control);
                Ok(self.paused_monitor_sources.is_empty().then_some(()))
            })?;
        }
        let nodes = || PausedTrackMonitor::foreign_effect_nodes_on_control(revision);
        let count = nodes().count();
        if count == 0 {
            return Ok(Vec::new());
        }
        let host = self
            .host
            .as_ref()
            .ok_or("qualified vst3 helper unavailable")?;
        if !self.configured || count > self.paused_monitor_sources.capacity() {
            return Err("vst3 monitor sources exceed the reserved capacity".into());
        }
        if !control.has_vacant_native_source_slots_on_control(count) {
            return Err("vst3 monitor source capacity is unavailable".into());
        }
        let mut replacements = Vec::new();
        replacements
            .try_reserve_exact(count)
            .map_err(|_| "vst3 monitor effect storage unavailable")?;
        if replacements.capacity() != count {
            return Err("vst3 monitor effect storage differs from its reservation".into());
        }
        for node in nodes() {
            let Some(effect) = node.foreign_processor() else {
                return Err("paused vst3 monitor effect identity differs".into());
            };
            if effect.is_instrument_profile()
                || node.automation.foreign().is_some()
                || node.modulation.is_some()
            {
                return Err("paused vst3 monitoring requires an effect without source-bound automation or modulation".into());
            }
            let state = node
                .configuration
                .as_deref()
                .ok_or("paused vst3 monitor effect state unavailable")?;
            let original = effect
                .binding()
                .ok_or("paused vst3 monitor original binding unavailable")?;
            let metadata = preparation.wait(control, |control| {
                control.native_source_metadata(original).map(Some)
            })?;
            let binding = preparation.wait(control, |control| {
                host.begin_on_control(
                    control,
                    metadata.selection(),
                    node.owner,
                    node.device_id,
                    original.graph,
                    node.sample_rate,
                    PluginProfile::Effect,
                    Some(state.clone()),
                )
                .map(Some)
            })?;
            self.paused_monitor_sources.push(binding);
            preparation.wait(control, |control| {
                match control.native_source_preparation_progress(binding) {
                    Some(Progress::Facts(_)) => host.complete_on_control(
                        control,
                        binding,
                        EffectPresentationPlan::AllowUnknown,
                    )?,
                    Some(Progress::Prepared { .. }) => control
                        .start_native_effect(binding)
                        .map_err(str::to_owned)?,
                    Some(Progress::Effect { .. }) => return Ok(Some(())),
                    Some(Progress::Failed | Progress::Retiring | Progress::Complete) => {
                        return Err(control
                            .try_native_source_preparation_error(binding)
                            .map_err(str::to_owned)?
                            .unwrap_or_else(|| "paused vst3 effect preparation refused".into()));
                    }
                    Some(Progress::Preparing) | None => {}
                }
                Ok(None)
            })?;
            let replacement = preparation.wait(control, |control| {
                control
                    .with_native_effect_definition(&[binding], |definition| {
                        definition
                            .prepared_processor_on_control(node)
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| "paused vst3 monitor restored state differs".into())
                    })
                    .map(Some)
            })?;
            replacements.push(PausedMonitorForeignBinding {
                original,
                replacement,
            });
        }
        replacements.sort_unstable_by_key(|effect| effect.original);
        Ok(replacements)
    }

    pub(in crate::native_audio) fn install_paused_monitor_effects(
        &self,
        control: &mut PublicationControl,
        preparation: &PausedMonitorPreparation<'_>,
    ) -> Result<(), String> {
        let (source, epoch) = preparation.scope;
        for &binding in &self.paused_monitor_sources {
            preparation.wait(control, |control| {
                control
                    .publish_paused_monitor_native_source_endpoint(binding, source, epoch)
                    .map(|()| Some(()))
                    .map_err(str::to_owned)
            })?;
            preparation.wait(control, |control| {
                if let Some((loaned, live)) = control.native_source_endpoint_status(binding)
                    && (!loaned || !live)
                {
                    return Err("paused vst3 monitor endpoint returned or stopped".into());
                }
                Ok((control.native_source_endpoint_installed(binding) == Some(true)).then_some(()))
            })?;
        }
        Ok(())
    }
}
