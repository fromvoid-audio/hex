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
    Restore,
    ReadQuiescent,
    Retire(Result<(), String>),
    Verify,
    Finished,
}

struct ColdFallback {
    hosting: Hosting,
    metadata: EffectSourceMetadata,
    owner: hex_lattice::ProcessorChainOwner,
    device_id: u64,
    graph: u64,
    rate: u32,
    profile: PluginProfile,
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

    /// Project a verified subset of already-captured canonical bases for the
    /// exact same binding, prepared profile, sample rate, and canonical state.
    pub fn project_selection(&self, selection: &AuthoredSelection) -> Result<Self, &'static str> {
        let (class, ids) = selection.parameters();
        if self.capabilities.class() != Some(class) {
            return Err("native canonical base class differs");
        }
        let mut values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
        let mut steps = [0; MAX_FOREIGN_AUTOMATION_TARGETS];
        let source = self.capabilities.parameters();
        for ((value_slot, step_slot), &id) in values.iter_mut().zip(steps.iter_mut()).zip(ids) {
            let index = source
                .binary_search_by_key(&id, |parameter| parameter.id)
                .map_err(|_| "native canonical base value missing")?;
            let parameter = source[index];
            *value_slot = Some((parameter.id, parameter.normalized_base_bits));
            *step_slot = parameter.steps;
        }
        let capabilities = automation::SourceCapabilities::from_canonical(class, &values, &steps)?;
        Ok(Self {
            binding: self.binding,
            source: self.source,
            fence: self.fence,
            canonical: self.canonical.clone(),
            prepared: self.prepared,
            sample_rate: self.sample_rate,
            capabilities,
        })
    }

    pub(crate) fn with_rebound_source(
        &self,
        source: (AppliedAudioSource, u64, bool),
        selection: &AuthoredSelection,
    ) -> Result<Self, &'static str> {
        let mut projected = self.project_selection(selection)?;
        projected.source = source;
        projected.fence = None;
        Ok(projected)
    }
}

/// One private query in the existing fixed Root source pool. Poll through its
/// terminal even after cancellation; output closure retains disposal in Root.
#[must_use = "poll the canonical base query through its disposal terminal"]
pub struct CanonicalBasesCapture {
    scratch: Binding,
    original: Binding,
    warm_key: crate::candidate_custody::WarmScratchKey,
    warm_acquired: bool,
    fallback_to_cold: bool,
    cold_fallback: Option<Box<ColdFallback>>,
    identity: ProcessorInstanceIdentity,
    source: (AppliedAudioSource, u64, bool),
    fence: Option<StoppedEditFence>,
    canonical: Option<NativeDeviceState>,
    selection: AuthoredSelection,
    presentation: hex_plugin_host::protocol::PresentationPlan,
    prepared: PreparedForeignProcessor,
    sample_rate: u32,
    steps: [u32; MAX_FOREIGN_AUTOMATION_TARGETS],
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
        let warm_key = crate::candidate_custody::WarmScratchKey {
            original_binding: binding,
            identity,
            sample_rate_bits: f64::from(rate).to_bits(),
            input_channels: match original.profile {
                PluginProfile::Effect => 2,
                PluginProfile::Instrument => 0,
            },
            output_channels: 2,
            presentation: original.presentation,
        };
        let (scratch, warm_acquired, cold_fallback, phase) =
            if let Some(scratch) = output.acquire_warm_scratch_native_source_on_control(warm_key) {
                (
                    scratch,
                    true,
                    Some(Box::new(ColdFallback {
                        hosting: self.clone(),
                        metadata: original.metadata,
                        owner: original.identity.owner(),
                        device_id: original.identity.device_id(),
                        graph: binding.graph,
                        rate,
                        profile: original.profile,
                    })),
                    Phase::Restore,
                )
            } else {
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
                (scratch, false, None, Phase::Prepare)
            };
        Ok(CanonicalBasesCapture {
            scratch,
            original: binding,
            warm_key,
            warm_acquired,
            fallback_to_cold: false,
            cold_fallback,
            identity: original.identity,
            source: (source, epoch, playing),
            fence,
            canonical: Some(canonical.clone()),
            selection: selection.clone(),
            presentation: original.presentation,
            prepared: original.prepared,
            sample_rate: rate,
            steps: [0; MAX_FOREIGN_AUTOMATION_TARGETS],
            values: [None; MAX_FOREIGN_AUTOMATION_TARGETS],
            phase,
        })
    }
}

impl CanonicalBasesCapture {
    crate::include_vst3_sdk_tests! {
        pub(crate) fn awaiting_processing_approval(&self) -> bool {
            matches!(self.phase, Phase::Verify)
        }
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
            if self.warm_acquired && self.cold_fallback.is_some() {
                output.disable_warm_scratch_native_source_on_control(self.original);
                self.fallback_to_cold = true;
            }
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
            if self.fallback_to_cold {
                self.fallback_to_cold = false;
                if is_cancelled() || !self.source_matches(output) {
                    self.cold_fallback = None;
                    self.phase = Phase::Finished;
                    return Some(Err("native canonical base query cancelled".into()));
                }
                if let (Some(cold), Some(canonical)) =
                    (self.cold_fallback.take(), self.canonical.as_ref())
                {
                    match cold.hosting.begin_on_control(
                        output,
                        cold.metadata.selection(),
                        cold.owner,
                        cold.device_id,
                        cold.graph,
                        cold.rate,
                        cold.profile,
                        Some(canonical.clone()),
                        None,
                    ) {
                        Ok(scratch) => {
                            self.scratch = scratch;
                            self.warm_acquired = false;
                            self.steps = [0; MAX_FOREIGN_AUTOMATION_TARGETS];
                            self.values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
                            self.phase = Phase::Prepare;
                            return None;
                        }
                        Err(error) => {
                            self.phase = Phase::Finished;
                            return Some(Err(error));
                        }
                    }
                }
            }
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
        let status = std::sync::Arc::clone(&output.status);
        let source_desc = self.source;
        let fence = self.fence;
        let mut interrupted = false;
        let mut cancelled = || {
            if !interrupted {
                interrupted =
                    is_cancelled() || !status.canonical_source_matches(source_desc, fence);
            }
            interrupted
        };
        let next = match (&self.phase, progress) {
            (Phase::Prepare, Some(Progress::Facts(_))) => {
                output.complete_native_source_preparation(self.scratch, self.presentation)
            }
            (Phase::Prepare, Some(Progress::Prepared { .. })) => output
                .start_native_effect_with_authored_selection(
                    self.scratch,
                    (!self.selection.parameters().1.is_empty()).then_some(&self.selection),
                ),
            (Phase::Prepare, Some(Progress::Effect { prepared })) => {
                self.read_bases(output, prepared).map(|()| {
                    if !is_cancelled()
                        && self.source_matches(output)
                        && let Ok(quiesced) = output
                            .quiesce_native_source_preparation(self.scratch, self.scratch.graph)
                    {
                        self.scratch = quiesced;
                        if output
                            .park_warm_scratch_native_source_on_control(self.scratch, self.warm_key)
                        {
                            self.phase = Phase::Verify;
                            return;
                        }
                    }
                    self.phase = Phase::Retire(Ok(()));
                })
            }
            (Phase::Restore, Some(Progress::Facts(_))) => {
                let Some(canonical) = self.canonical.clone() else {
                    self.phase =
                        Phase::Retire(Err("native canonical base state is unavailable".into()));
                    return None;
                };
                output
                    .restore_native_source_state_on_control(self.scratch, canonical, &mut cancelled)
                    .and_then(|restored| {
                        let (facts, quantum, input_channels) =
                            restored.ok_or("native canonical base warm restore refused")?;
                        let sibling = prepared_from_quiescent_facts(
                            self.original,
                            quantum,
                            input_channels,
                            facts,
                        )?;
                        if sibling != self.prepared {
                            return Err("native canonical base processing metadata differs");
                        }
                        if self.selection.parameters().1.is_empty() {
                            if !interrupted
                                && !is_cancelled()
                                && self.source_matches(output)
                                && output.park_warm_scratch_native_source_on_control(
                                    self.scratch,
                                    self.warm_key,
                                )
                            {
                                self.phase = Phase::Verify;
                            } else {
                                self.phase = Phase::Retire(Ok(()));
                            }
                        } else {
                            self.phase = Phase::ReadQuiescent;
                        }
                        Ok(())
                    })
            }
            (Phase::ReadQuiescent, Some(Progress::Facts(_))) => {
                let metadata = {
                    let (class, ids) = self.selection.parameters();
                    output.read_quiescent_authored_metadata_on_control(
                        self.scratch,
                        class,
                        ids,
                        &mut cancelled,
                    )
                };
                metadata.and_then(|metadata| {
                    let metadata =
                        metadata.ok_or("native canonical base metadata is unavailable")?;
                    let (values, steps) =
                        extract_quiescent_bases(self.selection.parameters().1, &metadata)?;
                    self.values = values;
                    self.steps = steps;
                    if !interrupted
                        && !is_cancelled()
                        && self.source_matches(output)
                        && output
                            .park_warm_scratch_native_source_on_control(self.scratch, self.warm_key)
                    {
                        self.phase = Phase::Verify;
                    } else {
                        self.phase = Phase::Retire(Ok(()));
                    }
                    Ok(())
                })
            }
            _ => return None,
        };
        if interrupted {
            self.fallback_to_cold = false;
            self.phase = Phase::Retire(Err("native canonical base query cancelled".into()));
        } else if let Err(error) = next
            && !matches!(
                error,
                "native preparation is busy" | "native source is busy"
            )
        {
            if self.warm_acquired && self.cold_fallback.is_some() {
                output.disable_warm_scratch_native_source_on_control(self.original);
                self.fallback_to_cold = true;
            }
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
        // The private restored instance has already established the exact
        // class and compatible processing profile. An unlaned state has no
        // parameter capabilities to read or claim.
        if self.selection.parameters().1.is_empty() {
            return Ok(());
        }
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
                let mut steps = [0; MAX_FOREIGN_AUTOMATION_TARGETS];
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

fn prepared_from_quiescent_facts(
    binding: Binding,
    quantum: u16,
    input_channels: u8,
    facts: hex_plugin_host::protocol::Facts,
) -> Result<PreparedForeignProcessor, &'static str> {
    let prepared = PreparedForeignProcessor::new(
        quantum,
        facts.algorithmic_latency_frames,
        u32::from(quantum) * 2,
    )
    .map(|prepared| {
        let prepared = if input_channels == 0 {
            prepared.with_instrument_profile()
        } else {
            prepared
        };
        prepared.with_output_tail(if facts.tail_frames == u32::MAX {
            hex_lattice::OutputTail::Unbounded
        } else {
            hex_lattice::OutputTail::Finite {
                max_tail_frames: u64::from(facts.tail_frames) + u64::from(quantum) * 2,
            }
        })
    })
    .and_then(|prepared| prepared.with_binding(binding))
    .map_err(|_| "native canonical base latency overflow")?;
    if prepared.total_latency_frames() > hex_engine::compensation::MAX_COMPENSATION_FRAMES {
        return Err("native canonical base latency exceeds graph limit");
    }
    Ok(prepared)
}

fn extract_quiescent_bases(
    expected: &[u32],
    metadata: &hex_plugin_host::native::AuthoredParameterMetadata,
) -> Result<
    (
        automation::CanonicalParameterBases,
        [u32; MAX_FOREIGN_AUTOMATION_TARGETS],
    ),
    &'static str,
> {
    let mut values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
    let mut steps = [0; MAX_FOREIGN_AUTOMATION_TARGETS];
    if expected.len() > values.len() {
        return Err("native canonical base selection exceeds capacity");
    }
    for ((value_slot, step_slot), &id) in values.iter_mut().zip(steps.iter_mut()).zip(expected) {
        let value = metadata
            .initial_normalized(id)
            .ok_or("native canonical base value missing")?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err("native canonical base value differs");
        }
        let step_count = metadata
            .step_count(id)
            .ok_or("native canonical parameter capability missing")?;
        *value_slot = Some((id, value.to_bits()));
        *step_slot = step_count;
    }
    Ok((values, steps))
}

fn copy_values(
    expected: &[u32],
    mut values: impl Iterator<Item = (u32, f64)>,
) -> Result<automation::CanonicalParameterBases, &'static str> {
    let mut result = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
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

    #[test]
    fn project_selection_preserves_verified_bits_and_refuses_absent_or_mismatched_targets() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let class = hex_plugin_state::ClassId::new([7; 16]).unwrap();
        let precise = f64::from_bits(0x3fe0_0000_0000_0001);
        let mut values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
        values[0] = Some((1, precise.to_bits()));
        values[1] = Some((4, 0.25_f64.to_bits()));
        values[2] = Some((9, 1.0_f64.to_bits()));
        let mut steps = [0; MAX_FOREIGN_AUTOMATION_TARGETS];
        steps[0] = 0;
        steps[1] = 5;
        steps[2] = 1;
        let capabilities =
            automation::SourceCapabilities::from_canonical(class, &values, &steps).unwrap();
        let original_source = (
            AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                output_identity: 1,
                audio_revision: 3,
            },
            10,
            true,
        );
        let facts = CapturedCanonicalBases {
            binding,
            source: original_source,
            fence: None,
            canonical: NativeDeviceState::new(
                hex_lattice_contract::VST3_DEVICE_TYPE_ID,
                1,
                vec![1, 2, 3],
            )
            .unwrap(),
            prepared: PreparedForeignProcessor::new(8, 0, 16)
                .unwrap()
                .with_binding(binding)
                .unwrap(),
            sample_rate: 48_000,
            capabilities,
        };

        let subset = AuthoredSelection::from_targets([(4, Some([7; 16])), (9, Some([7; 16]))])
            .unwrap()
            .unwrap();
        let projected = facts.project_selection(&subset).unwrap();
        assert_eq!(projected.binding(), binding);
        assert_eq!(projected.sample_rate(), 48_000);
        assert_eq!(projected.source(), original_source);
        let params: Vec<_> = projected.parameters().collect();
        assert_eq!(params.len(), 2);
        assert_eq!((params[0].id, params[0].steps), (4, 5));
        assert_eq!(params[0].normalized_base_bits, 0.25_f64.to_bits());
        assert_eq!((params[1].id, params[1].steps), (9, 1));
        assert_eq!(params[1].normalized_base_bits, 1.0_f64.to_bits());

        let empty = AuthoredSelection::for_no_parameters([7; 16]).unwrap();
        assert_eq!(facts.project_selection(&empty).unwrap().values().count(), 0);

        let missing = AuthoredSelection::from_targets([(4, Some([7; 16])), (12, Some([7; 16]))])
            .unwrap()
            .unwrap();
        assert!(facts.project_selection(&missing).is_err());

        let wrong_class = AuthoredSelection::from_targets([(4, Some([8; 16]))])
            .unwrap()
            .unwrap();
        assert!(facts.project_selection(&wrong_class).is_err());

        let next_source = (
            AppliedAudioSource {
                audio_revision: 5,
                ..original_source.0
            },
            11,
            true,
        );
        let rebound = facts.with_rebound_source(next_source, &subset).unwrap();
        assert_eq!(rebound.source(), next_source);
        assert_eq!(rebound.parameters().count(), 2);
    }

    #[test]
    fn quiescent_facts_reconstruct_exact_prepared_profile_and_reject_overflow() {
        let binding = Binding {
            root: 1,
            instance: 20,
            graph: 2,
            epoch: 3,
        };
        let facts = hex_plugin_host::protocol::Facts {
            note_expression: Default::default(),
            algorithmic_latency_frames: 64,
            tail_frames: 128,
            presentation_available: true,
            restored: true,
        };
        let expected_instrument = PreparedForeignProcessor::new(64, 64, 128)
            .unwrap()
            .with_instrument_profile()
            .with_output_tail(hex_lattice::OutputTail::Finite {
                max_tail_frames: 128 + 128,
            })
            .with_binding(binding)
            .unwrap();
        assert_eq!(
            prepared_from_quiescent_facts(binding, 64, 0, facts).unwrap(),
            expected_instrument
        );
        assert_ne!(
            prepared_from_quiescent_facts(binding, 64, 2, facts).unwrap(),
            expected_instrument
        );

        let infinite = hex_plugin_host::protocol::Facts {
            tail_frames: u32::MAX,
            ..facts
        };
        let expected_infinite = PreparedForeignProcessor::new(64, 64, 128)
            .unwrap()
            .with_output_tail(hex_lattice::OutputTail::Unbounded)
            .with_binding(binding)
            .unwrap();
        assert_eq!(
            prepared_from_quiescent_facts(binding, 64, 2, infinite).unwrap(),
            expected_infinite
        );

        let excessive_latency = hex_plugin_host::protocol::Facts {
            algorithmic_latency_frames: hex_engine::compensation::MAX_COMPENSATION_FRAMES,
            ..facts
        };
        assert!(prepared_from_quiescent_facts(binding, 64, 2, excessive_latency).is_err());
    }
}
