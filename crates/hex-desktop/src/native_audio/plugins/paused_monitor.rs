//! Independent stopped monitor sources use the original Root's worker and
//! endpoint custody. Only immutable graph state is restored into each worker.
use super::*;
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_engine::{PausedMonitorForeignBinding, PausedTrackMonitor};
use hex_session::service::{PausedInputMonitorRequest, RequestCancellation};
use std::time::{Duration, Instant};

pub(in crate::native_audio) const PREPARATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Explicit physical input preparation retains its request boundary. Ordinary
/// paused notes advance the same construction and installation one poll at a time.
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
                Err(error) if paused_monitor_busy(&error) => {}
                Err(error) => return Err(bounded_error(error)),
            }
            edits::poll_control(control);
        }
    }
}

pub(in crate::native_audio) fn paused_monitor_busy(error: &str) -> bool {
    matches!(
        error,
        "native preparation is busy"
            | "native source is busy"
            | "native endpoint publication is busy"
            | "native effect preparation is busy"
            | "native effect selection is busy"
    )
}

pub(in crate::native_audio) struct PausedEffects {
    replacements: Vec<PausedMonitorForeignBinding>,
    count: usize,
    preparing: Option<Binding>,
    installation: PausedEffectInstallation,
}

#[derive(Default)]
struct PausedEffectInstallation {
    installed: usize,
    submitted: bool,
}

impl PausedEffects {
    pub(in crate::native_audio) fn bindings(&self) -> &[PausedMonitorForeignBinding] {
        &self.replacements
    }

    pub(in crate::native_audio) fn poll(
        &mut self,
        plugins: &mut Plugins,
        control: &mut PublicationControl,
        revision: &PreparedRevision,
        scope: (AppliedAudioSource, u64),
    ) -> Result<bool, String> {
        if self.replacements.len() < self.count {
            self.prepare_one(plugins, control, revision)?;
            return Ok(false);
        }
        self.installation
            .poll(control, scope, &plugins.paused_monitor_sources)
    }

    fn prepare_one(
        &mut self,
        plugins: &mut Plugins,
        control: &mut PublicationControl,
        revision: &PreparedRevision,
    ) -> Result<(), String> {
        let host = plugins
            .host
            .as_ref()
            .ok_or("qualified vst3 helper unavailable")?;
        let node = PausedTrackMonitor::foreign_effect_nodes_on_control(revision)
            .nth(self.replacements.len())
            .ok_or("paused vst3 monitor graph changed")?;
        let effect = node
            .foreign_processor()
            .ok_or("paused vst3 monitor effect identity differs")?;
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
        let Some(binding) = self.preparing else {
            let metadata = control.native_source_metadata(original)?;
            let binding = host.begin_on_control(
                control,
                metadata.selection(),
                node.owner,
                node.device_id,
                original.graph,
                node.sample_rate,
                PluginProfile::Effect,
                Some(state.clone()),
                None,
            )?;
            plugins.paused_monitor_sources.push(binding);
            self.preparing = Some(binding);
            return Ok(());
        };
        match control.native_source_preparation_progress(binding) {
            Some(Progress::Facts(_)) => {
                host.complete_on_control(control, binding, EffectPresentationPlan::AllowUnknown)?
            }
            Some(Progress::Prepared { .. }) => control.start_native_effect(binding)?,
            Some(Progress::Effect { .. }) => {
                let replacement =
                    control.with_native_effect_definition(&[binding], |definition| {
                        definition
                            .prepared_processor_on_control(node)
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| "paused vst3 monitor restored state differs".into())
                    })?;
                self.replacements.push(PausedMonitorForeignBinding {
                    original,
                    replacement,
                });
                self.preparing = None;
                if self.replacements.len() == self.count {
                    self.replacements
                        .sort_unstable_by_key(|effect| effect.original);
                }
            }
            Some(Progress::Failed | Progress::Retiring | Progress::Complete) => {
                return Err(control
                    .try_native_source_preparation_error(binding)?
                    .unwrap_or_else(|| "paused vst3 effect preparation refused".into()));
            }
            Some(Progress::Preparing) | None => {}
        }
        Ok(())
    }
}

impl PausedEffectInstallation {
    fn poll(
        &mut self,
        control: &mut PublicationControl,
        (source, epoch): (AppliedAudioSource, u64),
        bindings: &[Binding],
    ) -> Result<bool, String> {
        let Some(&binding) = bindings.get(self.installed) else {
            return Ok(true);
        };
        if !self.submitted {
            control.publish_paused_monitor_native_source_endpoint(binding, source, epoch)?;
            self.submitted = true;
            return Ok(false);
        }
        if let Some((loaned, live)) = control.native_source_endpoint_status(binding)
            && (!loaned || !live)
        {
            return Err("paused vst3 monitor endpoint returned or stopped".into());
        }
        if control.native_source_endpoint_installed(binding) == Some(true) {
            self.installed += 1;
            self.submitted = false;
        }
        Ok(self.installed == bindings.len())
    }
}

impl Plugins {
    pub(in crate::native_audio) fn report_paused_note_refusal(
        &self,
        scope: (AppliedAudioSource, u64),
        error: String,
    ) {
        if let Some(log) = &self.diagnostics {
            let (source, epoch) = scope;
            let _ = log.record_typed(
                hex_diagnostics::Level::Warn,
                hex_diagnostics::Category::Audio,
                &format!(
                    "vst3_pause v=1 stage=monitor_refused revision={} epoch={epoch} reason={}",
                    source.audio_revision,
                    bounded_error(error),
                ),
            );
        }
    }
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

    pub(in crate::native_audio) fn begin_paused_effects(
        &self,
        control: &PublicationControl,
        revision: &PreparedRevision,
    ) -> Result<PausedEffects, String> {
        if !self.paused_monitor_sources.is_empty() {
            return Err("native source is busy".into());
        }
        let count = PausedTrackMonitor::foreign_effect_nodes_on_control(revision).count();
        let mut replacements = Vec::new();
        if count != 0 {
            self.host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?;
            if !self.configured || count > self.paused_monitor_sources.capacity() {
                return Err("vst3 monitor sources exceed the reserved capacity".into());
            }
            if !control.has_vacant_native_source_slots_on_control(count) {
                return Err("vst3 monitor source capacity is unavailable".into());
            }
            replacements
                .try_reserve_exact(count)
                .map_err(|_| "vst3 monitor effect storage unavailable")?;
            if replacements.capacity() != count {
                return Err("vst3 monitor effect storage differs from its reservation".into());
            }
        }
        Ok(PausedEffects {
            replacements,
            count,
            preparing: None,
            installation: Default::default(),
        })
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
        let mut effects = self.begin_paused_effects(control, revision)?;
        preparation.wait(control, |control| {
            if effects.replacements.len() == effects.count {
                return Ok(Some(()));
            }
            effects.prepare_one(self, control, revision)?;
            Ok(None)
        })?;
        Ok(effects.replacements)
    }

    pub(in crate::native_audio) fn install_paused_monitor_effects(
        &self,
        control: &mut PublicationControl,
        preparation: &PausedMonitorPreparation<'_>,
    ) -> Result<(), String> {
        let mut installation = PausedEffectInstallation::default();
        preparation.wait(control, |control| {
            installation
                .poll(control, preparation.scope, &self.paused_monitor_sources)
                .map(|ready| ready.then_some(()))
        })
    }
}
