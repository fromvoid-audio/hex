//! Isolated manual base capture for one exact applied ordinary effect. The
//! original worker supplies immutable identity and configuration facts only.
//! A second Root cell owns the scratch child through exact disposal recovery.
use super::{Binding, EffectSourceMetadata, Hosting, NativeDeviceState, PluginProfile};
use crate::{
    NativeSourcePreparationProgress as Progress,
    prepared::{AppliedAudioSource, PublicationControl},
};
use hex_lattice::ProcessorInstanceIdentity;
use hex_plugin_host::{
    protocol::{FlushDisposition, PresentationPlan},
    transport::{ParameterPoint, Parameters},
};

pub(crate) struct BaseCaptureSource {
    pub metadata: EffectSourceMetadata,
    pub identity: ProcessorInstanceIdentity,
    pub presentation: PresentationPlan,
}

enum Phase {
    Prepare,
    Flush,
    Capture,
    Retire(Option<Result<NativeDeviceState, String>>),
    Finished,
}

/// One bounded in-flight scratch source, with no callback endpoint or graph
/// publication. Poll on the same control lane even after cancellation until it
/// returns a terminal. If the output itself closes, its Root retains disposal.
#[must_use = "poll the capture through its disposal terminal"]
pub struct ManualBaseCapture {
    binding: Binding,
    source: AppliedAudioSource,
    identity: hex_plugin_state::Identity,
    presentation: PresentationPlan,
    id: u32,
    parameters: Parameters,
    phase: Phase,
}

impl Hosting {
    /// Restore the exact canonical processing state into a private sibling of
    /// the installed source. The caller validates the editable SDK row before
    /// admission. Scratch preparation revalidates the original binary and the
    /// helper validates the parameter again on its own controller/processor.
    pub fn begin_manual_base_capture_on_control(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        point: ParameterPoint,
    ) -> Result<ManualBaseCapture, String> {
        if point.frame != 0 {
            return Err("native manual base input requires frame zero".into());
        }
        let mut parameters = Parameters::default();
        parameters
            .push(point)
            .map_err(|_| "native manual base input is invalid")?;
        let identity = Self::saved_identity_on_control(canonical)?;
        let (rate, original) = output.native_base_capture_source(source, binding, canonical)?;
        let scratch = self.begin_on_control(
            output,
            original.metadata.selection(),
            original.identity.owner(),
            original.identity.device_id(),
            binding.graph,
            rate,
            PluginProfile::Effect,
            Some(canonical.clone()),
        )?;
        Ok(ManualBaseCapture {
            binding: scratch,
            source,
            identity,
            presentation: original.presentation,
            id: point.id,
            parameters,
            phase: Phase::Prepare,
        })
    }
}

impl ManualBaseCapture {
    /// Advance one control operation. A captured envelope is returned once,
    /// only after the exact scratch child has been reaped and its Root cell
    /// recovered. Cancelled, stale and refused captures still drain custody.
    pub fn poll_on_control(
        &mut self,
        output: &mut PublicationControl,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Result<NativeDeviceState, String>> {
        if matches!(self.phase, Phase::Finished)
            || output.output_identity() != Some(self.source.output_identity)
        {
            return None;
        }
        if !matches!(self.phase, Phase::Retire(Some(Err(_))))
            && (is_cancelled() || !output.native_base_capture_is_current(self.source))
        {
            self.phase = Phase::Retire(Some(Err("native manual base capture cancelled".into())));
        }
        let progress = output.native_source_preparation_progress(self.binding);
        if matches!(progress, Some(Progress::Failed)) {
            self.phase = Phase::Finished;
            return Some(Err("native manual base disposal failed".into()));
        }
        if matches!(progress, Some(Progress::Retiring | Progress::Complete))
            && !matches!(self.phase, Phase::Retire(_))
        {
            self.phase = Phase::Retire(Some(Err("native manual base preparation refused".into())));
        }
        if let Phase::Retire(result) = &mut self.phase {
            if !matches!(progress, Some(Progress::Complete)) {
                if !matches!(progress, Some(Progress::Retiring)) {
                    output.retire_native_source_preparation(self.binding);
                }
                return None;
            }
            let report = output.recover_native_source_preparation(self.binding)?;
            let result = match (report.error.or(report.cleanup_error), result.take()) {
                (Some(error), _) => Err(error.to_string()),
                (None, Some(Ok(_)))
                    if is_cancelled() || !output.native_base_capture_is_current(self.source) =>
                {
                    Err("native manual base capture cancelled".into())
                }
                (None, Some(result)) => result,
                (None, None) => Err("native manual base capture result is unavailable".into()),
            };
            self.phase = Phase::Finished;
            return Some(result);
        }

        // Recheck source changes during bounded native I/O without taking
        // locks or borrowing the publication endpoint from the cancellation.
        let status = std::sync::Arc::clone(&output.status);
        let mut cancelled = || {
            is_cancelled()
                || !status.playing.load(std::sync::atomic::Ordering::Acquire)
                || status.native_effect_source() != Some(self.source)
        };
        let next = match (&self.phase, progress) {
            (Phase::Prepare, Some(Progress::Facts(_))) => output
                .complete_native_source_preparation(self.binding, self.presentation)
                .map(|()| Phase::Prepare),
            (Phase::Prepare, Some(Progress::Prepared { .. })) => output
                .start_native_effect_with_authored_selection(self.binding, None)
                .map(|()| Phase::Prepare),
            (Phase::Prepare, Some(Progress::Effect { .. })) => output
                .quiesce_native_source_preparation(self.binding, self.binding.graph)
                .map(|binding| {
                    self.binding = binding;
                    Phase::Flush
                }),
            (Phase::Flush, Some(Progress::Facts(_))) => output
                .flush_native_source_parameters_on_control(
                    self.binding,
                    &self.parameters,
                    &mut cancelled,
                )
                .and_then(|reply| {
                    reply
                        .filter(|reply| {
                            reply.disposition == FlushDisposition::Applied
                                && reply.as_slice().iter().any(|value| value.id == self.id)
                        })
                        .map(|_| Phase::Capture)
                        .ok_or("native manual base input was not acknowledged")
                }),
            (Phase::Capture, Some(Progress::Facts(_))) => output
                .capture_native_source_state_on_control(self.binding, &mut cancelled)
                .map(|state| {
                    let result = state
                        .ok_or_else(|| "native manual base capture refused".to_owned())
                        .and_then(|state| {
                            if Hosting::saved_identity_on_control(&state)? == self.identity {
                                Ok(state)
                            } else {
                                Err("native manual base capture identity differs".into())
                            }
                        });
                    Phase::Retire(Some(result))
                }),
            _ => return None,
        };
        match next {
            Ok(phase) => self.phase = phase,
            Err("native preparation is busy") => {}
            Err(error) => self.phase = Phase::Retire(Some(Err(error.into()))),
        }
        None
    }
}
