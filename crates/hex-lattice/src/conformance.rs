//! Opt-in, control-side authoring checks. These allocate and must never run on audio.
//!
//! Fixture agreement detects contract violations; it does not establish sound
//! parity, native interaction, physical deadlines or complete feature coverage.
use crate::{
    DeviceMeter, NativeDeviceDefinition, NativeDeviceRegistry, NativeRuntimeChain,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterKind,
    ParameterValue,
};
use std::{fmt, sync::Arc};

/// The exact stage and authored type that failed an authoring fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceFailure {
    pub device: String,
    pub stage: &'static str,
}
impl fmt::Display for ConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.device, self.stage)
    }
}
impl std::error::Error for ConformanceFailure {}

fn fail<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    stage: &'static str,
) -> ConformanceFailure {
    ConformanceFailure {
        device: match definition.type_id().as_str() {
            Ok(id) => id.to_owned(),
            Err(_) => "invalid type id".to_owned(),
        },
        stage,
    }
}
fn contract<T, D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    stage: &'static str,
    result: Result<T, DeviceContractError>,
) -> Result<T, ConformanceFailure> {
    result.map_err(|_| fail(definition, stage))
}

/// Check a current or legacy fixture through the actual codec and descriptors.
/// Each device supplies its meaningful legacy/malformed fixtures separately.
/// Parameter edits start from the same state, avoiding order-dependent patches.
pub fn check_metadata<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    fixture: &NativeDeviceState,
) -> Result<usize, ConformanceFailure> {
    let state = contract(
        definition,
        "fixture migration",
        definition.migrate_state(fixture),
    )?;
    if state.type_id() != definition.type_id()
        || state.state_version() != definition.state_version()
    {
        return Err(fail(definition, "migrated identity/version"));
    }
    let again = contract(
        definition,
        "repeat migration",
        definition.migrate_state(&state),
    )?;
    if state != again {
        return Err(fail(definition, "migration idempotence"));
    }
    let descriptors = contract(
        definition,
        "state descriptors",
        hex_lattice_contract::validated_state_parameters(definition, &state),
    )?;
    for (descriptor, semantics) in &descriptors {
        let current = contract(
            definition,
            "read parameter",
            definition.read_parameter(&state, descriptor.id),
        )?;
        contract(
            definition,
            "read value domain",
            descriptor.validate(current),
        )?;
        let enabled = contract(
            definition,
            "parameter availability",
            definition.parameter_enabled(&state, descriptor.id),
        )?;
        if semantics.is_some_and(|s| s.access == hex_lattice_contract::ParameterAccess::ReadOnly) {
            if definition
                .patch_parameter(&state, descriptor.id, current)
                .is_ok()
            {
                return Err(fail(definition, "read-only parameter accepted"));
            }
            continue;
        }
        if enabled {
            let patched = contract(
                definition,
                "same-value patch",
                definition.patch_parameter(&state, descriptor.id, current),
            )?;
            let readback = contract(
                definition,
                "patch readback",
                definition.read_parameter(&patched, descriptor.id),
            )?;
            if readback != current {
                return Err(fail(definition, "same-value readback"));
            }
        }
        let invalid = match descriptor.kind {
            ParameterKind::Float { .. } => ParameterValue::Float(f32::NAN),
            ParameterKind::Bool => ParameterValue::Float(0.0),
            ParameterKind::Choice { labels } => ParameterValue::Choice(labels.len() as u32),
        };
        if definition
            .patch_parameter(&state, descriptor.id, invalid)
            .is_ok()
        {
            return Err(fail(definition, "invalid parameter accepted"));
        }
    }
    let future = contract(
        definition,
        "future fixture construction",
        NativeDeviceState::new(definition.type_id(), u32::MAX, state.payload().to_vec()),
    )?;
    if definition.state_version() != u32::MAX && definition.migrate_state(&future).is_ok() {
        return Err(fail(definition, "future version accepted"));
    }
    let wrong = contract(
        definition,
        "wrong-type fixture construction",
        NativeDeviceState::new(
            DeviceTypeId::from_static("conformance.wrong-type"),
            state.state_version(),
            state.payload().to_vec(),
        ),
    )?;
    if definition.migrate_state(&wrong).is_ok() {
        return Err(fail(definition, "wrong type accepted"));
    }
    Ok(descriptors.len())
}

fn chain(
    definition: &dyn NativeDeviceDefinition,
    state: &NativeDeviceState,
    rate: u32,
    bypassed: bool,
) -> Result<NativeRuntimeChain, ConformanceFailure> {
    let definitions = [definition];
    let registry = contract(
        definition,
        "effect registry",
        NativeDeviceRegistry::new(&definitions),
    )?;
    let prepared = contract(
        definition,
        "effect preparation",
        registry.prepare_runtime_device(
            state,
            &crate::DevicePreparationContext {
                sample_rate: rate,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        ),
    )?;
    contract(
        definition,
        "runtime admission",
        NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
            modulation: None,
            automation: Default::default(),
            meter: Arc::new(DeviceMeter::default()),
            owner: ProcessorChainOwner::Track(1),
            device_id: 1,
            device_type: definition.type_id(),
            matrix: prepared.matrix,
            bypassed,
            sample_rate: rate,
            configuration: Some(Arc::new(state.clone())),
            definition: prepared.processor,
        }]),
    )
}
fn render(
    definition: &dyn NativeDeviceDefinition,
    chain: &mut NativeRuntimeChain,
    input: &[StereoFrame],
    partitions: &[usize],
) -> Result<Vec<StereoFrame>, ConformanceFailure> {
    let mut output = input.to_vec();
    let mut offset = 0;
    for &length in partitions {
        let end = offset + length;
        chain.begin_block();
        contract(
            definition,
            "effect processing",
            chain.process_block(&mut output[offset..end]),
        )?;
        chain.commit_block();
        offset = end;
    }
    Ok(output)
}
fn same(left: &[StereoFrame], right: &[StereoFrame], tolerance: f32) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.iter()
                .zip(b)
                .all(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() <= tolerance)
        })
}

/// Exercise the real transactional effect chain at one explicit valid rate/state.
/// Independent musical/transfer oracles belong alongside device implementations.
/// Covers finite asymmetric input, irregular partitions, reset, bypass, abandoned
/// speculation, history inheritance and the actual declared-storage admission.
pub fn check_effect(
    definition: &dyn NativeDeviceDefinition,
    state: &NativeDeviceState,
    sample_rate: u32,
) -> Result<(), ConformanceFailure> {
    let mut whole = chain(definition, state, sample_rate, false)?;
    let mut chunks = chain(definition, state, sample_rate, false)?;
    let input: Vec<StereoFrame> = (0..513)
        .map(|i| {
            if i == 0 {
                [1.0, -0.25]
            } else {
                [
                    ((i % 31) as f32 - 15.0) / 64.0,
                    ((i % 17) as f32 - 8.0) / 47.0,
                ]
            }
        })
        .collect();
    let expected = render(definition, &mut whole, &input, &[513])?;
    let partitioned = render(definition, &mut chunks, &input, &[1, 7, 31, 3, 257, 214])?;
    if !same(&expected, &partitioned, 1.0e-6) {
        return Err(fail(definition, "finite partition agreement"));
    }
    chunks.reset();
    if !same(
        &expected,
        &render(definition, &mut chunks, &input, &[513])?,
        0.0,
    ) {
        return Err(fail(definition, "reset reproducibility"));
    }
    let mut bypass = chain(definition, state, sample_rate, true)?;
    // Ordinary bypass has zero effective latency; opted-in fixed-latency
    // bypass retains its declared dry delay. Check every sample exactly,
    // including zero startup history, rather than skipping aligned providers.
    let latency = contract(
        definition,
        "bypass latency query",
        bypass.algorithmic_latency_frames_for_entry(0),
    )?
    .ok_or_else(|| fail(definition, "unknown bypass latency"))?;
    let latency = usize::try_from(latency).map_err(|_| fail(definition, "bypass latency range"))?;
    let delayed_dry: Vec<_> = (0..input.len())
        .map(|i| {
            i.checked_sub(latency)
                .and_then(|source| input.get(source))
                .copied()
                .unwrap_or_default()
        })
        .collect();
    if !same(
        &delayed_dry,
        &render(definition, &mut bypass, &input, &[513])?,
        0.0,
    ) {
        return Err(fail(definition, "bypass changes samples"));
    }
    let mut abandoned = input.clone();
    chunks.begin_block();
    contract(
        definition,
        "abandoned effect processing",
        chunks.process_block(&mut abandoned),
    )?; // deliberately no commit
    let continuation = render(definition, &mut whole, &input, &[513])?;
    if !same(
        &continuation,
        &render(definition, &mut chunks, &input, &[513])?,
        0.0,
    ) {
        return Err(fail(
            definition,
            "abandoned render changed committed history",
        ));
    }
    let mut inherited = chain(definition, state, sample_rate, false)?;
    if !inherited.can_inherit_from(&whole) {
        return Err(fail(definition, "unchanged history compatibility"));
    }
    inherited.inherit_from(&whole);
    if !same(
        &render(definition, &mut whole, &input, &[513])?,
        &render(definition, &mut inherited, &input, &[513])?,
        0.0,
    ) {
        return Err(fail(definition, "history inheritance changes continuation"));
    }
    Ok(())
}

/// The processing capability actually exercised by the instrument fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentClass {
    Analytic,
    SharedAnalytic,
    Mutable,
}

/// Check one real instrument configuration without a project, audio backend or UI.
/// The caller supplies admission limits; the harness never raises host budgets.
/// Mutable instruments receive occurrence-aware onset/release events. Analytic
/// classes sample the same absolute ages in different caller traversal orders.
pub fn check_instrument(
    definition: &dyn crate::NativeInstrumentDefinition,
    state: &NativeDeviceState,
    sample_rate: u32,
    limits: crate::NativeInstrumentRuntimeLimits,
) -> Result<InstrumentClass, ConformanceFailure> {
    let prepared = contract(
        definition,
        "instrument preparation",
        definition.prepare_instrument(
            state,
            &crate::DevicePreparationContext {
                sample_rate,
                tuning: &crate::TuningTable::default(),
                resources: &crate::EMPTY_DEVICE_RESOURCES,
            },
        ),
    )?;
    if contract(
        definition,
        "owned capability query",
        prepared.prepare_owned_mutable_runtime(),
    )?
    .is_some()
    {
        return Err(fail(
            definition,
            "owned-pair instrument fixture not supported yet",
        ));
    }
    if let Some(factory) = contract(
        definition,
        "mutable capability query",
        prepared.prepare_mutable_runtime(),
    )? {
        let mut whole = crate::NativeInstrumentRuntimePair::new(factory.as_ref(), limits)
            .map_err(|_| fail(definition, "mutable runtime admission"))?;
        let mut chunks = crate::NativeInstrumentRuntimePair::new(factory.as_ref(), limits)
            .map_err(|_| fail(definition, "mutable runtime admission"))?;
        let expected = render_instrument(definition, &mut whole, &[513])?;
        let partitioned = render_instrument(definition, &mut chunks, &[1, 7, 31, 3, 257, 214])?;
        if !same(&expected, &partitioned, 1.0e-6) {
            return Err(fail(definition, "instrument partition agreement"));
        }
        chunks.reset();
        if !same(
            &expected,
            &render_instrument(definition, &mut chunks, &[513])?,
            0.0,
        ) {
            return Err(fail(definition, "instrument reset reproducibility"));
        }
        return Ok(InstrumentClass::Mutable);
    }
    if let Some(context) = contract(
        definition,
        "shared capability query",
        prepared.prepare_shared_analytic_context(),
    )? {
        let note = contract(
            definition,
            "shared note preparation",
            context.prepare_note(60, 100),
        )?;
        let a: Vec<_> = (0..513)
            .map(|age| context.sample(&note, age, 257, &crate::EMPTY_DEVICE_RESOURCES))
            .collect();
        let mut b: Vec<_> = (0..513)
            .rev()
            .map(|age| context.sample(&note, age, 257, &crate::EMPTY_DEVICE_RESOURCES))
            .collect();
        b.reverse();
        if !same(&a, &b, 0.0) {
            return Err(fail(definition, "shared analytic finite determinism"));
        }
        return Ok(InstrumentClass::SharedAnalytic);
    }
    let note = contract(
        definition,
        "analytic note preparation",
        prepared.prepare_note(60, 100),
    )?;
    let a: Vec<_> = (0..513)
        .map(|age| note.sample(age, 257, &crate::EMPTY_DEVICE_RESOURCES))
        .collect();
    let mut b: Vec<_> = (0..513)
        .rev()
        .map(|age| note.sample(age, 257, &crate::EMPTY_DEVICE_RESOURCES))
        .collect();
    b.reverse();
    if !same(&a, &b, 0.0) {
        return Err(fail(definition, "analytic finite determinism"));
    }
    Ok(InstrumentClass::Analytic)
}

fn render_instrument<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    pair: &mut crate::NativeInstrumentRuntimePair,
    partitions: &[usize],
) -> Result<Vec<StereoFrame>, ConformanceFailure> {
    use crate::NativeInstrumentEvent;
    let mut output = vec![[0.0; 2]; 513];
    let mut offset = 0;
    for &length in partitions {
        let end = offset + length;
        let mut events = Vec::new();
        if offset == 0 {
            events.push(NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            });
        }
        if (offset..end).contains(&257) {
            events.push(NativeInstrumentEvent::NoteOff {
                frame_offset: 257 - offset,
                occurrence_id: 1,
            });
        }
        pair.begin_block();
        pair.process_block(
            &events,
            &mut output[offset..end],
            &crate::EMPTY_DEVICE_RESOURCES,
        )
        .map_err(|_| fail(definition, "instrument event processing"))?;
        pair.commit_block();
        offset = end;
    }
    Ok(output)
}

#[cfg(test)]
mod tests;

/// Registered original fixture for owned-effect host proofs, never a browser device.
pub mod owned_delay;
pub mod resource_pcm;
/// Original pass-through fixture for bounded live-transition admission.
pub mod transition_fixture;

/// Small original aux-key runtime for host contract and allocation probes.
pub mod auxiliary;
