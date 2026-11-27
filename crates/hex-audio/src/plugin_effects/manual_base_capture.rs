//! Isolated manual and saved-state base capture on one ordinary processor. The
//! original Root source supplies immutable identity and configuration facts.
//! A second Root cell owns the scratch child through exact disposal recovery.
use super::{
    Binding, EffectSourceMetadata, Hosting, NativeDeviceState, PluginProfile, state_approval,
};
use crate::{
    NativeSourcePreparationProgress as Progress,
    prepared::{AppliedAudioSource, PublicationControl},
};
use hex_lattice::ProcessorInstanceIdentity;
use hex_plugin_host::{
    protocol::{FlushDisposition, PresentationPlan},
    transport::{MAX_PARAMETER_POINTS, ParameterPoint, Parameters},
};
use std::collections::VecDeque;

pub(crate) struct BaseCaptureSource {
    pub metadata: EffectSourceMetadata,
    pub identity: ProcessorInstanceIdentity,
    pub profile: PluginProfile,
    pub presentation: PresentationPlan,
    pub prepared: hex_lattice::PreparedForeignProcessor,
    pub canonical_is_constructor_state: bool,
}

enum Phase {
    Prepare,
    Flush,
    Capture,
    Retire(Option<Result<NativeDeviceState, ManualBaseCaptureError>>),
    Approve(CompletedManualBase),
    Finished,
}

struct FlushPlan {
    cohorts: VecDeque<Parameters>,
    exact_readback: bool,
}

impl FlushPlan {
    fn manual_point(id: u32, normalized: f64) -> Result<Self, String> {
        let mut parameters = Parameters::default();
        parameters
            .push(ParameterPoint {
                id,
                frame: 0,
                normalized,
            })
            .map_err(|_| "native manual base input is invalid")?;
        Ok(Self {
            cohorts: VecDeque::from([parameters]),
            exact_readback: false,
        })
    }

    fn ordered(cohorts: &[Vec<(u32, f64)>]) -> Result<Self, String> {
        let mut plan = Self {
            cohorts: VecDeque::new(),
            exact_readback: true,
        };
        let mut remaining = MAX_PARAMETER_POINTS;
        if cohorts.is_empty() || cohorts.len() > remaining {
            return Err("native replay base cohort count is invalid".into());
        }
        plan.cohorts
            .try_reserve_exact(cohorts.len())
            .map_err(|_| "native replay base cohort allocation failed")?;
        for cohort in cohorts {
            remaining = remaining
                .checked_sub(cohort.len())
                .filter(|_| !cohort.is_empty())
                .ok_or("native replay base input exceeds its point bound")?;
            let mut parameters = Parameters::default();
            for &(id, normalized) in cohort {
                parameters
                    .push(ParameterPoint {
                        id,
                        frame: 0,
                        normalized,
                    })
                    .map_err(|_| "native replay base input is invalid")?;
            }
            if parameters.as_slice().len() != cohort.len() {
                return Err("native replay base cohort repeats a parameter".into());
            }
            plan.cohorts.push_back(parameters);
        }
        Ok(plan)
    }
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
    flushes: FlushPlan,
    before_fingerprint: state_approval::StateFingerprint,
    manual_point_id: Option<u32>,
    flushed_normalized: Option<f64>,
    phase: Phase,
}

/// Failure reported when an isolated capture reaches its terminal.
#[derive(Debug)]
pub enum ManualBaseCaptureError {
    /// The applied source or transport fence changed and the scratch source was recovered.
    SourceChanged,
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ManualBaseCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceChanged => f.write_str("native manual base capture source changed"),
            Self::Cancelled => f.write_str("native manual base capture cancelled"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ManualBaseCaptureError {}

/// Constructed only after this module has completed and reaped the actual
/// scratch capture. Its fields cannot be assembled by adapters or Root callers.
pub(crate) struct CompletedManualBase {
    binding: Binding,
    source: (AppliedAudioSource, u64, bool),
    state: NativeDeviceState,
    manual_flush: Option<state_approval::ManualFlushObservation>,
}
impl CompletedManualBase {
    pub(crate) fn binding(&self) -> Binding {
        self.binding
    }
    pub(crate) fn state(&self) -> &NativeDeviceState {
        &self.state
    }
    pub(crate) fn manual_flush(&self) -> Option<state_approval::ManualFlushObservation> {
        self.manual_flush
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
            FlushPlan::manual_point(id, normalized)?,
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
            FlushPlan::manual_point(id, normalized)?,
            false,
        )
    }

    /// Replay ordered native editor cohorts in a private sibling restored
    /// from the installed source's exact canonical base. Every cohort requires
    /// exact SDK readback before the next can run. The live program stays on
    /// the original worker. A returned capture grants state preparation only;
    /// the caller still owns editor-input conflict checks and publication.
    pub fn begin_ordered_manual_base_capture_on_control(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        cohorts: &[Vec<(u32, f64)>],
        playing: bool,
    ) -> Result<ManualBaseCapture, String> {
        self.begin_parameter_base_capture(
            output,
            source,
            binding,
            canonical,
            FlushPlan::ordered(cohorts)?,
            playing,
        )
    }

    fn begin_parameter_base_capture(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        flushes: FlushPlan,
        playing: bool,
    ) -> Result<ManualBaseCapture, String> {
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
            None,
        )?;
        let before_fingerprint = state_approval::state_fingerprint(canonical);
        let manual_point_id = (!flushes.exact_readback
            && flushes.cohorts.len() == 1
            && flushes.cohorts[0].as_slice().len() == 1)
            .then(|| flushes.cohorts[0].as_slice()[0].id);
        Ok(ManualBaseCapture {
            binding: scratch,
            output_identity: source.output_identity,
            source: Some((source, transport_epoch, playing)),
            original_binding: Some(binding),
            identity,
            presentation: original.presentation,
            flushes,
            before_fingerprint,
            manual_point_id,
            flushed_normalized: None,
            phase: Phase::Prepare,
        })
    }
    /// Restore fresh captured state in a private Root sibling, then put only
    /// armed targets back to their source-bound canonical bases. Completed
    /// isolated facts supply the current bases after an original-worker edit;
    /// otherwise the canonical state must match the constructor's frozen bases.
    /// Other component/controller bytes remain part of the actual capture. Save owns
    /// the canonical context and cancellation, without a live-delivery claim.
    #[allow(clippy::too_many_arguments)] // Exact saved source/state plus optional isolated canonical facts.
    pub fn begin_saved_base_capture_on_control(
        &self,
        output: &mut PublicationControl,
        binding: Binding,
        canonical: &NativeDeviceState,
        captured: &NativeDeviceState,
        selection: &super::AuthoredSelection,
        canonical_bases: Option<&super::CapturedCanonicalBases>,
    ) -> Result<ManualBaseCapture, String> {
        let identity = Self::saved_identity_on_control(canonical)?;
        if Self::saved_identity_on_control(captured)? != identity {
            return Err("native saved capture identity differs".into());
        }
        let (rate, original) = output.native_saved_base_capture_source(binding, canonical)?;
        if canonical_bases.is_none() && !original.canonical_is_constructor_state {
            return Err("native saved current canonical bases are required".into());
        }
        let (class, ids) = selection.parameters();
        if class != identity.class || original.metadata.authored_base_values().count() != ids.len()
        {
            return Err("native saved base selection differs".into());
        }
        if let Some(facts) = canonical_bases {
            if facts.binding() != binding
                || facts.prepared() != original.prepared
                || facts.sample_rate() != rate
                || !facts.canonical_state().processing_eq(canonical)
                || facts.values().count() != ids.len()
            {
                return Err("native saved canonical base facts differ".into());
            }
            match output.native_source_approves_processing_state_on_control(
                binding,
                facts.canonical_state(),
            ) {
                Some(true) => {}
                Some(false) => return Err("native saved canonical base facts differ".into()),
                None => return Err("native preparation is busy".into()),
            }
        }
        let mut parameters = Parameters::default();
        let mut restored = canonical_bases.map(|facts| facts.values());
        for (&expected, (id, original_value)) in
            ids.iter().zip(original.metadata.authored_base_values())
        {
            if expected != id {
                return Err("native saved base parameter differs".into());
            }
            let normalized = match restored.as_mut() {
                Some(values) => {
                    let (restored_id, value) = values
                        .next()
                        .ok_or("native saved canonical base is missing")?;
                    if restored_id != id {
                        return Err("native saved canonical parameter differs".into());
                    }
                    value
                }
                None => original_value,
            };
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
            None,
        )?;
        Ok(ManualBaseCapture {
            binding: scratch,
            output_identity: binding.root,
            source: None,
            original_binding: None,
            identity,
            presentation: original.presentation,
            flushes: FlushPlan {
                cohorts: VecDeque::from([parameters]),
                exact_readback: true,
            },
            before_fingerprint: state_approval::state_fingerprint(canonical),
            manual_point_id: None,
            flushed_normalized: None,
            phase: Phase::Prepare,
        })
    }
}

impl ManualBaseCapture {
    fn interruption(
        source: Option<(AppliedAudioSource, u64, bool)>,
        status: &crate::prepared::Status,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<ManualBaseCaptureError> {
        if is_cancelled() {
            Some(ManualBaseCaptureError::Cancelled)
        } else {
            source
                .is_some_and(|(source, epoch, playing)| {
                    status.native_parameter_source(playing) != Some((source, epoch))
                })
                .then_some(ManualBaseCaptureError::SourceChanged)
        }
    }

    /// Advance one control operation. A captured envelope is returned once,
    /// only after the exact scratch child has been reaped and its Root cell
    /// recovered. Cancelled, stale and refused captures still drain custody.
    pub fn poll_on_control(
        &mut self,
        output: &mut PublicationControl,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Result<NativeDeviceState, ManualBaseCaptureError>> {
        use ManualBaseCaptureError::Failed;
        if matches!(self.phase, Phase::Finished)
            || output.output_identity() != Some(self.output_identity)
        {
            return None;
        }
        if let Phase::Approve(captured) = &self.phase {
            if let Some(error) = Self::interruption(self.source, &output.status, is_cancelled) {
                self.phase = Phase::Finished;
                return Some(Err(error));
            }
            let result = output.approve_native_manual_base_on_control(captured);
            if matches!(result, Err("native preparation is busy")) {
                return None;
            }
            let result = Self::interruption(self.source, &output.status, is_cancelled)
                .map_or_else(|| result.map_err(|error| Failed(error.into())), Err);
            self.phase = Phase::Finished;
            return Some(result);
        }
        if !matches!(self.phase, Phase::Retire(Some(Err(_))))
            && let Some(error) = Self::interruption(self.source, &output.status, is_cancelled)
        {
            self.phase = Phase::Retire(Some(Err(error)));
        }
        let progress = output.native_source_preparation_progress(self.binding);
        if matches!(progress, Some(Progress::Failed)) {
            self.phase = Phase::Finished;
            return Some(Err(Failed("native manual base disposal failed".into())));
        }
        if matches!(progress, Some(Progress::Retiring | Progress::Complete))
            && !matches!(self.phase, Phase::Retire(_))
        {
            self.phase = Phase::Retire(Some(Err(Failed(
                "native manual base preparation refused".into(),
            ))));
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
                (Some(error), _) => Err(Failed(error.to_string())),
                (None, Some(Ok(state))) => {
                    Self::interruption(self.source, &output.status, is_cancelled)
                        .map_or(Ok(state), Err)
                }
                (None, Some(Err(error))) => Err(error),
                (None, None) => Err(Failed(
                    "native manual base capture result is unavailable".into(),
                )),
            };
            match (result, self.original_binding, self.source) {
                (Ok(state), Some(binding), Some(source)) => {
                    let manual_flush = self.manual_point_id.zip(self.flushed_normalized).map(
                        |(id, flushed_normalized)| state_approval::ManualFlushObservation {
                            id,
                            flushed_normalized,
                            before_fingerprint: self.before_fingerprint,
                            after_fingerprint: state_approval::state_fingerprint(&state),
                        },
                    );
                    self.phase = Phase::Approve(CompletedManualBase {
                        binding,
                        source,
                        state,
                        manual_flush,
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
        let mut interruption = None;
        let mut cancelled = || {
            if interruption.is_none() {
                interruption = Self::interruption(self.source, &status, is_cancelled);
            }
            interruption.is_some()
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
            (Phase::Flush, Some(Progress::Facts(_))) => match self.flushes.cohorts.front() {
                Some(points) => output
                    .flush_native_source_parameters_on_control(self.binding, points, &mut cancelled)
                    .and_then(|reply| {
                        let verified = reply.filter(|reply| {
                            reply.disposition == FlushDisposition::Applied
                                && points.as_slice().iter().all(|expected| {
                                    reply.as_slice().iter().any(|value| {
                                        value.id == expected.id
                                            && (!self.flushes.exact_readback
                                                || value.normalized == expected.normalized)
                                    })
                                })
                        });
                        let reply =
                            verified.ok_or("native manual base input was not acknowledged")?;
                        if let Some(id) = self.manual_point_id {
                            self.flushed_normalized = reply
                                .as_slice()
                                .iter()
                                .find(|value| value.id == id)
                                .map(|value| value.normalized);
                        }
                        Ok(())
                    })
                    .map(|()| {
                        self.flushes.cohorts.pop_front();
                        if self.flushes.cohorts.is_empty() {
                            Phase::Capture
                        } else {
                            Phase::Flush
                        }
                    }),
                None => Err("native manual base input is unavailable"),
            },
            (Phase::Capture, Some(Progress::Facts(_))) => output
                .capture_native_source_state_on_control(self.binding, &mut cancelled)
                .map(|state| {
                    let result = state
                        .ok_or_else(|| Failed("native manual base capture refused".into()))
                        .and_then(|state| {
                            if Hosting::saved_identity_on_control(&state).map_err(Failed)?
                                == self.identity
                            {
                                Ok(state)
                            } else {
                                Err(Failed("native manual base capture identity differs".into()))
                            }
                        });
                    Phase::Retire(Some(result))
                }),
            _ => return None,
        };
        match (interruption, next) {
            (Some(error), _) => self.phase = Phase::Retire(Some(Err(error))),
            (None, Ok(phase)) => self.phase = phase,
            (None, Err("native preparation is busy")) => {}
            (None, Err(error)) => self.phase = Phase::Retire(Some(Err(Failed(error.into())))),
        }
        None
    }
}
