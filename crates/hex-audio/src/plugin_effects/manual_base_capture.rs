//! Isolated manual and saved-state base capture on one ordinary processor. The
//! original Root source supplies immutable identity and configuration facts.
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
    pub profile: PluginProfile,
    pub presentation: PresentationPlan,
    pub prepared: hex_lattice::PreparedForeignProcessor,
}

enum Phase {
    Prepare,
    Flush,
    Capture,
    Retire(Option<Result<NativeDeviceState, String>>),
    Approve(CompletedManualBase),
    Finished,
}

/// One bounded in-flight scratch source, with no callback endpoint or graph
/// publication. Poll on the same control lane even after cancellation until it
/// returns a terminal. If the output itself closes, its Root retains disposal.
#[must_use = "poll the capture through its disposal terminal"]
pub struct ManualBaseCapture {
    binding: Binding,
    output_identity: u64,
    source: Option<(AppliedAudioSource, u64, bool)>,
    original_binding: Option<Binding>,
    identity: hex_plugin_state::Identity,
    presentation: PresentationPlan,
    parameters: Parameters,
    phase: Phase,
}

/// Constructed only after this module has completed and reaped the actual
/// scratch capture. Its fields cannot be assembled by adapters or Root callers.
pub(crate) struct CompletedManualBase {
    binding: Binding,
    source: (AppliedAudioSource, u64, bool),
    state: NativeDeviceState,
}
impl CompletedManualBase {
    pub(crate) fn binding(&self) -> Binding {
        self.binding
    }
    pub(crate) fn state(&self) -> &NativeDeviceState {
        &self.state
    }
    pub(crate) fn matches_source(&self, status: &crate::prepared::Status) -> bool {
        status.native_parameter_source(self.source.2) == Some((self.source.0, self.source.1))
    }
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
        id: u32,
        normalized: f64,
    ) -> Result<ManualBaseCapture, String> {
        self.begin_parameter_base_capture(
            output,
            source,
            binding,
            canonical,
            ParameterPoint {
                id,
                frame: 0,
                normalized,
            },
            true,
        )
    }

    /// Isolate one stopped manual edit from canonical state. The exact idle
    /// source and epoch must remain current through capture and disposal.
    pub fn begin_idle_manual_base_capture_on_control(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        id: u32,
        normalized: f64,
    ) -> Result<ManualBaseCapture, String> {
        self.begin_parameter_base_capture(
            output,
            source,
            binding,
            canonical,
            ParameterPoint {
                id,
                frame: 0,
                normalized,
            },
            false,
        )
    }

    fn begin_parameter_base_capture(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        point: ParameterPoint,
        playing: bool,
    ) -> Result<ManualBaseCapture, String> {
        let mut parameters = Parameters::default();
        parameters
            .push(point)
            .map_err(|_| "native manual base input is invalid")?;
        let identity = Self::saved_identity_on_control(canonical)?;
        let (rate, transport_epoch, original) =
            output.native_base_capture_source(source, binding, canonical, playing)?;
        let scratch = self.begin_on_control(
            output,
            original.metadata.selection(),
            original.identity.owner(),
            original.identity.device_id(),
            binding.graph,
            rate,
            original.profile,
            Some(canonical.clone()),
        )?;
        Ok(ManualBaseCapture {
            binding: scratch,
            output_identity: source.output_identity,
            source: Some((source, transport_epoch, playing)),
            original_binding: Some(binding),
            identity,
            presentation: original.presentation,
            parameters,
            phase: Phase::Prepare,
        })
    }
    /// Restore fresh captured state in a private Root sibling, then put only
    /// reading targets back to their source-bound canonical bases. Other
    /// component/controller bytes remain part of the actual capture. Save owns
    /// the canonical context and cancellation, without a live-delivery claim.
    pub fn begin_saved_base_capture_on_control(
        &self,
        output: &mut PublicationControl,
        binding: Binding,
        canonical: &NativeDeviceState,
        captured: &NativeDeviceState,
        selection: &super::AuthoredSelection,
    ) -> Result<ManualBaseCapture, String> {
        let identity = Self::saved_identity_on_control(canonical)?;
        if Self::saved_identity_on_control(captured)? != identity {
            return Err("native saved capture identity differs".into());
        }
        let (rate, original) = output.native_saved_base_capture_source(binding, canonical)?;
        let (class, ids) = selection.parameters();
        if class != identity.class || original.metadata.authored_base_values().count() != ids.len()
        {
            return Err("native saved base selection differs".into());
        }
        let mut parameters = Parameters::default();
        for (&expected, (id, normalized)) in
            ids.iter().zip(original.metadata.authored_base_values())
        {
            if expected != id {
                return Err("native saved base parameter differs".into());
            }
            parameters
                .push(ParameterPoint {
                    id,
                    frame: 0,
                    normalized,
                })
                .map_err(|_| "native saved base value is invalid")?;
        }
        let scratch = self.begin_on_control(
            output,
            original.metadata.selection(),
            original.identity.owner(),
            original.identity.device_id(),
            binding.graph,
            rate,
            original.profile,
            Some(captured.clone()),
        )?;
        Ok(ManualBaseCapture {
            binding: scratch,
            output_identity: binding.root,
            source: None,
            original_binding: None,
            identity,
            presentation: original.presentation,
            parameters,
            phase: Phase::Prepare,
        })
    }
}

impl ManualBaseCapture {
    fn source_matches(&self, status: &crate::prepared::Status) -> bool {
        self.source.is_none_or(|(source, epoch, playing)| {
            status.native_parameter_source(playing) == Some((source, epoch))
        })
    }

    /// Advance one control operation. A captured envelope is returned once,
    /// only after the exact scratch child has been reaped and its Root cell
    /// recovered. Cancelled, stale and refused captures still drain custody.
    pub fn poll_on_control(
        &mut self,
        output: &mut PublicationControl,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Result<NativeDeviceState, String>> {
        if matches!(self.phase, Phase::Finished)
            || output.output_identity() != Some(self.output_identity)
        {
            return None;
        }
        if let Phase::Approve(captured) = &self.phase {
            if is_cancelled() || !captured.matches_source(&output.status) {
                self.phase = Phase::Finished;
                return Some(Err("native manual base capture cancelled".into()));
            }
            let result = output.approve_native_manual_base_on_control(captured);
            if matches!(result, Err("native preparation is busy")) {
                return None;
            }
            let result = result.map_err(str::to_owned).and_then(|state| {
                if is_cancelled() || !captured.matches_source(&output.status) {
                    Err("native manual base capture cancelled".into())
                } else {
                    Ok(state)
                }
            });
            self.phase = Phase::Finished;
            return Some(result);
        }
        if !matches!(self.phase, Phase::Retire(Some(Err(_))))
            && (is_cancelled() || !self.source_matches(&output.status))
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
                (None, Some(Ok(_))) if is_cancelled() || !self.source_matches(&output.status) => {
                    Err("native manual base capture cancelled".into())
                }
                (None, Some(result)) => result,
                (None, None) => Err("native manual base capture result is unavailable".into()),
            };
            match (result, self.original_binding, self.source) {
                (Ok(state), Some(binding), Some(source)) => {
                    self.phase = Phase::Approve(CompletedManualBase {
                        binding,
                        source,
                        state,
                    });
                    return None;
                }
                (result, _, _) => {
                    self.phase = Phase::Finished;
                    return Some(result);
                }
            }
        }

        // Recheck source changes during bounded native I/O without taking
        // locks or borrowing the publication endpoint from the cancellation.
        let status = std::sync::Arc::clone(&output.status);
        let mut cancelled = || is_cancelled() || !self.source_matches(&status);
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
                            // Manual gestures allow SDK normalization. Save
                            // requires readback of every exact frozen base.
                            reply.disposition == FlushDisposition::Applied
                                && self.parameters.as_slice().iter().all(|expected| {
                                    reply.as_slice().iter().any(|value| {
                                        value.id == expected.id
                                            && (self.source.is_some()
                                                || value.normalized == expected.normalized)
                                    })
                                })
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
