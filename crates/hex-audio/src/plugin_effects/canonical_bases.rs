//! Exact restored SDK bases for a prospective canonical graph. A private Root
//! sibling supplies the facts and is reaped before they return. The original
//! worker/endpoint is never restored, stopped, loaned or replaced here.
use super::*;
use crate::{
    NativeSourcePreparationProgress as Progress,
    prepared::{AppliedAudioSource, PublicationControl, StoppedEditFence},
};

enum Phase {
    Prepare,
    Retire(Result<(), String>),
    Verify,
    Finished,
}

/// Immutable observations, not worker/endpoint or publication authority.
/// Construction is private to the completed SDK query. Callers retain this
/// bounded state handle only through the corresponding control preparation.
pub struct CapturedCanonicalBases {
    binding: Binding,
    source: (AppliedAudioSource, u64, bool),
    fence: Option<StoppedEditFence>,
    canonical: NativeDeviceState,
    prepared: PreparedForeignProcessor,
    sample_rate: u32,
    pub(super) capabilities: automation::SourceCapabilities,
}
impl CapturedCanonicalBases {
    pub(crate) fn matches_output_on_control(&self, output: &PublicationControl) -> bool {
        output.canonical_source_matches_on_control(self.source, self.fence)
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn canonical_state(&self) -> &NativeDeviceState {
        &self.canonical
    }
    pub fn prepared(&self) -> PreparedForeignProcessor {
        self.prepared
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn parameters(&self) -> impl Iterator<Item = hex_lattice::ForeignParameterCapability> + '_ {
        self.capabilities.parameters().iter().copied()
    }

    pub fn source(&self) -> (AppliedAudioSource, u64, bool) {
        self.source
    }
    pub fn values(&self) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.capabilities
            .parameters()
            .iter()
            .map(|parameter| (parameter.id, f64::from_bits(parameter.normalized_base_bits)))
    }
}

/// One private query in the existing fixed Root source pool. Poll through its
/// terminal even after cancellation; output closure retains disposal in Root.
#[must_use = "poll the canonical base query through its disposal terminal"]
pub struct CanonicalBasesCapture {
    scratch: Binding,
    original: Binding,
    identity: ProcessorInstanceIdentity,
    source: (AppliedAudioSource, u64, bool),
    fence: Option<StoppedEditFence>,
    canonical: Option<NativeDeviceState>,
    selection: AuthoredSelection,
    presentation: hex_plugin_host::protocol::PresentationPlan,
    prepared: PreparedForeignProcessor,
    sample_rate: u32,
    steps: [u32; hex_plugin_host::transport::MAX_PARAMETER_POINTS],
    values: automation::CanonicalParameterBases,
    phase: Phase,
}

impl Hosting {
    /// Restore and inspect only the private sibling. The requested state must
    /// be the original canonical configuration or carry its private-capture
    /// approval; copied opaque bytes cannot authorize a different source.
    pub fn begin_canonical_bases_capture_on_control(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        selection: &AuthoredSelection,
        playing: bool,
    ) -> Result<CanonicalBasesCapture, String> {
        self.begin_canonical_query(output, source, binding, canonical, selection, playing, None)
    }

    /// Query a renewed original source after its ordinary graph returned.
    /// The exact acknowledged restore fence must remain held through scratch
    /// disposal and processing approval. This grants no graph admission.
    pub fn begin_stopped_canonical_bases_capture_on_control(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
        binding: Binding,
        canonical: &NativeDeviceState,
        selection: &AuthoredSelection,
    ) -> Result<CanonicalBasesCapture, String> {
        self.begin_canonical_query(
            output,
            source,
            binding,
            canonical,
            selection,
            false,
            Some(fence),
        )
    }

    #[allow(clippy::too_many_arguments)] // Exact source plus its optional held restore fence.
    fn begin_canonical_query(
        &self,
        output: &mut PublicationControl,
        source: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        selection: &AuthoredSelection,
        playing: bool,
        fence: Option<StoppedEditFence>,
    ) -> Result<CanonicalBasesCapture, String> {
        let identity = Self::saved_identity_on_control(canonical)?;
        if selection.parameters().0 != identity.class {
            return Err("native canonical base class differs".into());
        }
        let (rate, epoch, original) = match fence {
            Some(fence) => {
                output.native_fenced_base_capture_source(source, fence, binding, canonical)?
            }
            None => output.native_base_capture_source(source, binding, canonical, playing)?,
        };
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
        Ok(CanonicalBasesCapture {
            scratch,
            original: binding,
            identity: original.identity,
            source: (source, epoch, playing),
            fence,
            canonical: Some(canonical.clone()),
            selection: selection.clone(),
            presentation: original.presentation,
            prepared: original.prepared,
            sample_rate: rate,
            steps: [0; hex_plugin_host::transport::MAX_PARAMETER_POINTS],
            values: [None; hex_plugin_host::transport::MAX_PARAMETER_POINTS],
            phase: Phase::Prepare,
        })
    }
}

impl CanonicalBasesCapture {
    #[cfg(test)]
    pub(crate) fn awaiting_processing_approval(&self) -> bool {
        matches!(self.phase, Phase::Verify)
    }
    pub fn retained_control_metadata_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(
            self.canonical
                .as_ref()
                .map_or(Some(0), NativeDeviceState::retained_control_metadata_bytes)?,
        )
    }
    fn source_matches(&self, output: &PublicationControl) -> bool {
        output.canonical_source_matches_on_control(self.source, self.fence)
    }

    /// Return once, after disposal/recovery and a final source check. Capacity,
    /// SDK metadata and compatibility refusals retain scratch custody until
    /// that terminal. No callback or SDK work runs on the caller's audio lane.
    pub fn poll_on_control(
        &mut self,
        output: &mut PublicationControl,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Result<CapturedCanonicalBases, String>> {
        if matches!(self.phase, Phase::Finished)
            || output.output_identity() != Some(self.source.0.output_identity)
        {
            return None;
        }
        if matches!(self.phase, Phase::Verify) {
            return self.qualify_processing(output, is_cancelled);
        }
        if !matches!(self.phase, Phase::Retire(Err(_)))
            && (is_cancelled() || !self.source_matches(output))
        {
            self.phase = Phase::Retire(Err("native canonical base query cancelled".into()));
        }
        let progress = output.native_source_preparation_progress(self.scratch);
        if matches!(progress, Some(Progress::Failed)) {
            self.phase = Phase::Finished;
            return Some(Err("native canonical base disposal failed".into()));
        }
        if matches!(progress, Some(Progress::Retiring | Progress::Complete))
            && !matches!(self.phase, Phase::Retire(_))
        {
            self.phase = Phase::Retire(Err("native canonical base preparation refused".into()));
        }
        if matches!(self.phase, Phase::Retire(_)) {
            if progress != Some(Progress::Complete) {
                if progress != Some(Progress::Retiring) {
                    output.retire_native_source_preparation(self.scratch);
                }
                return None;
            }
            let report = output.recover_native_source_preparation(self.scratch)?;
            let Phase::Retire(result) = std::mem::replace(&mut self.phase, Phase::Finished) else {
                return None;
            };
            if let Some(error) = report.error.or(report.cleanup_error) {
                return Some(Err(error.to_string()));
            }
            if let Err(error) = result {
                return Some(Err(error));
            }
            if is_cancelled() || !self.source_matches(output) {
                return Some(Err("native canonical base query cancelled".into()));
            }
            self.phase = Phase::Verify;
            return None;
        }
        let next = match progress {
            Some(Progress::Facts(_)) => {
                output.complete_native_source_preparation(self.scratch, self.presentation)
            }
            Some(Progress::Prepared { .. }) => output
                .start_native_effect_with_authored_selection(self.scratch, Some(&self.selection)),
            Some(Progress::Effect { prepared }) => self
                .read_bases(output, prepared)
                .map(|()| self.phase = Phase::Retire(Ok(()))),
            _ => return None,
        };
        if let Err(error) = next
            && !matches!(
                error,
                "native preparation is busy" | "native source is busy"
            )
        {
            self.phase = Phase::Retire(Err(error.into()));
        }
        None
    }

    fn qualify_processing(
        &mut self,
        output: &PublicationControl,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Result<CapturedCanonicalBases, String>> {
        if is_cancelled() || !self.source_matches(output) {
            self.phase = Phase::Finished;
            return Some(Err("native canonical base query cancelled".into()));
        }
        let Some(canonical) = self.canonical.as_ref() else {
            self.phase = Phase::Finished;
            return Some(Err("native canonical base state is unavailable".into()));
        };
        let mut facts = CapturedCanonicalBases {
            binding: self.original,
            source: self.source,
            fence: self.fence,
            canonical: canonical.clone(),
            prepared: self.prepared,
            sample_rate: self.sample_rate,
            capabilities: match automation::SourceCapabilities::from_canonical(
                self.selection.parameters().0,
                &self.values,
                &self.steps,
            ) {
                Ok(capabilities) => capabilities,
                Err(error) => {
                    self.phase = Phase::Finished;
                    return Some(Err(error.into()));
                }
            },
        };
        match output.approve_native_canonical_processing_on_control(&facts) {
            Err("native preparation is busy") => None,
            result => {
                self.phase = Phase::Finished;
                let result = result.map_err(str::to_owned).and_then(|state| {
                    if is_cancelled() || !self.source_matches(output) {
                        return Err("native canonical base query cancelled".into());
                    }
                    facts.canonical = state;
                    self.canonical = None;
                    Ok(facts)
                });
                Some(result)
            }
        }
    }

    fn read_bases(
        &mut self,
        output: &PublicationControl,
        prepared: PreparedForeignProcessor,
    ) -> Result<(), &'static str> {
        if !compatible(self.prepared, prepared, self.original) {
            return Err("native canonical base processing metadata differs");
        }
        let metadata = output
            .native_source_metadata(self.scratch)
            .map_err(|error| match error.as_str() {
                "native preparation is busy" => "native preparation is busy",
                "native source is busy" => "native source is busy",
                _ => "native canonical base metadata is unavailable",
            })?;
        self.values = copy_values(
            self.selection.parameters().1,
            metadata.authored_base_values(),
        )?;
        let canonical = self
            .canonical
            .as_ref()
            .ok_or("native canonical base state is unavailable")?;
        let identity = self.identity;
        let steps = output
            .with_native_effect_definition(&[self.scratch], |definition| {
                use hex_lattice::NativeDeviceDefinition;
                let context = hex_lattice::DevicePreparationContext {
                    sample_rate: self.sample_rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                };
                let source = definition
                    .foreign_automation_source(identity, canonical, &context)
                    .map_err(|_| "native canonical capabilities are unavailable".to_owned())?
                    .ok_or("native canonical capabilities are unavailable")?;
                if source.class != self.selection.parameters().0.bytes()
                    || source.parameters.len() != self.selection.parameters().1.len()
                {
                    return Err("native canonical capabilities differ".into());
                }
                let mut steps = [0; hex_plugin_host::transport::MAX_PARAMETER_POINTS];
                for ((slot, parameter), &expected) in steps
                    .iter_mut()
                    .zip(source.parameters)
                    .zip(self.selection.parameters().1)
                {
                    if parameter.id != expected {
                        return Err("native canonical parameter differs".into());
                    }
                    *slot = parameter.steps;
                }
                Ok(steps)
            })
            .map_err(|error| match error.as_str() {
                "native effect preparation is busy" | "native effect selection is busy" => {
                    "native preparation is busy"
                }
                _ => "native canonical capabilities are unavailable",
            })?;
        self.steps = steps;
        Ok(())
    }
}

fn compatible(
    original: PreparedForeignProcessor,
    sibling: PreparedForeignProcessor,
    binding: Binding,
) -> bool {
    sibling
        .with_binding(binding)
        .is_ok_and(|sibling| sibling == original)
}

fn copy_values(
    expected: &[u32],
    mut values: impl Iterator<Item = (u32, f64)>,
) -> Result<automation::CanonicalParameterBases, &'static str> {
    let mut result = [None; hex_plugin_host::transport::MAX_PARAMETER_POINTS];
    if expected.len() > result.len() {
        return Err("native canonical base selection exceeds capacity");
    }
    for (slot, &expected) in result.iter_mut().zip(expected) {
        let (id, value) = values.next().ok_or("native canonical base value missing")?;
        if id != expected || !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("native canonical base value differs");
        }
        *slot = Some((id, value.to_bits()));
    }
    if values.next().is_some() {
        return Err("native canonical base selection differs");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restored_values_preserve_exact_precision_and_refuse_incomplete_or_invalid_metadata() {
        let precise = f64::from_bits(0x3fe0000000000001);
        let values = copy_values(&[0, 7], [(0, precise), (7, 1.0)].into_iter()).unwrap();
        assert_eq!(values[0], Some((0, precise.to_bits())));
        assert_eq!(values[1], Some((7, 1.0f64.to_bits())));
        assert!(values[2..].iter().all(Option::is_none));
        for values in [vec![], vec![(7, 0.5)], vec![(0, 0.5), (7, 0.5)]] {
            assert!(copy_values(&[0], values.into_iter()).is_err());
        }
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(copy_values(&[0], [(0, value)].into_iter()).is_err());
        }
        assert!(copy_values(&[0; 65], std::iter::repeat((0, 0.5))).is_err());
    }
    #[test]
    fn only_the_private_binding_difference_is_ignored_in_processing_compatibility() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let original = PreparedForeignProcessor::new(8, 0, 16)
            .unwrap()
            .with_binding(binding)
            .unwrap();
        let sibling = original
            .with_binding(Binding {
                instance: 9,
                ..binding
            })
            .unwrap();
        assert!(compatible(original, sibling, binding));
        for different in [
            PreparedForeignProcessor::new(16, 0, 16).unwrap(),
            PreparedForeignProcessor::new(8, 1, 16).unwrap(),
            PreparedForeignProcessor::new(8, 0, 8).unwrap(),
            sibling.with_instrument_profile(),
            sibling.with_output_tail(hex_lattice::OutputTail::Finite { max_tail_frames: 1 }),
        ] {
            assert!(!compatible(original, different, binding));
        }
    }
}
