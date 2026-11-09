//! Opt-in parameter delivery inside the existing independent-twin transaction.
use super::*;
use crate::NativeParameterAutomation;
use hex_lattice_contract::{
    MAX_PARAMETERS_PER_DEVICE, ParameterDescriptor, ParameterId, ParameterModulation,
    ParameterSemantics, ParameterValue,
};

/// Maximum frames in one automated segment. Hosts split larger blocks without
/// resetting voices or automation; ordinary runtime entrypoints are unchanged.
pub const MAX_INSTRUMENT_AUTOMATION_FRAMES: usize = 4_096;

/// Borrowed view into the same mutable instrument, not a second voice owner.
/// Every method is bounded, allocates/frees nothing and retains no input borrow.
/// Descriptor grants are immutable, sorted and unique for the runtime's lifetime.
/// Automation IDs and readings must remain deterministic throughout the entire
/// preflight/process call; scalar and bulk readings must agree. Missing IDs and None select immutable prepared Base,
/// including after a previous segment supplied an authored value.
///
/// Values apply before MIDI events at the same offset. Providers own smoothing,
/// note-law discontinuities and coupled-value validation. They preserve note age,
/// gate, phase and unrelated history. Empty segments consume no control state.
/// History copy/reset must include all effective values and smoothing history.
pub trait InstrumentParameterRuntime {
    fn descriptors(&self) -> &[ParameterDescriptor];

    /// Same immutable per-parameter grant as the preparing factory. When a
    /// borrowed block reports a route for an admitted ID, the provider consumes
    /// its canonical value at each sample.
    fn parameter_modulation_semantics(
        &self,
        _parameter: ParameterId,
    ) -> Option<ParameterSemantics> {
        None
    }
    /// Copy every descriptor's last selected canonical target, resolving Base.
    /// This observes targets, not smoothed effective values or future curves;
    /// feeding the result back lets existing ramps finish without restarting.
    /// Success fills every exact-length slot with Some in descriptor order.
    /// Read-only opt-in promises this remains bounded, deterministic and free of
    /// allocation/deallocation through processing, reset and history transfer.
    fn copy_targets(
        &self,
        _output: &mut [Option<ParameterValue>],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation)
    }

    /// Last source-global input to the single occurrence-local route, before
    /// clamping. Detached release reads it together with the canonical targets.
    fn unclamped_modulation_target(&self) -> Option<(ParameterId, f64)> {
        None
    }

    /// Read-only coupled-value preflight, after generic typed-value, event,
    /// clock and occurrence-gain checks. Errors leave PCM and runtime unchanged.
    fn preflight(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[crate::NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError>;
    /// Infallible after successful complete preflight, adding audio to frames.
    fn process(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[crate::NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn crate::DeviceResources,
    );
}

pub(super) fn validate_grants(
    descriptors: &[ParameterDescriptor],
) -> Result<(), NativeInstrumentRuntimeError> {
    if descriptors.len() > MAX_PARAMETERS_PER_DEVICE
        || descriptors.windows(2).any(|pair| pair[0].id >= pair[1].id)
        || descriptors
            .iter()
            .any(|descriptor| descriptor.validate_descriptor().is_err())
    {
        return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
    }
    Ok(())
}

pub(super) fn matches_grants(
    runtime: &mut dyn MutableInstrumentRuntime,
    prepared: &dyn PreparedMutableInstrumentRuntime,
    descriptors: &[ParameterDescriptor],
) -> bool {
    match runtime.parameter_automation() {
        Some(view) => {
            !descriptors.is_empty()
                && view.descriptors() == descriptors
                && descriptors.iter().all(|&descriptor| {
                    let grant = prepared.parameter_modulation_semantics(descriptor.id);
                    grant == view.parameter_modulation_semantics(descriptor.id)
                        && grant.is_none_or(|semantics| {
                            semantics.modulation == ParameterModulation::SampleRate
                                && semantics.validate(descriptor).is_ok()
                        })
                })
        }
        None => descriptors.is_empty(),
    }
}

fn checked_targets(
    view: &dyn InstrumentParameterRuntime,
    targets: &mut [Option<ParameterValue>],
) -> Result<(), NativeInstrumentRuntimeError> {
    let descriptors = view.descriptors();
    validate_grants(descriptors)?;
    if targets.len() != descriptors.len() || descriptors.is_empty() {
        return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
    }
    view.copy_targets(targets)?;
    for (target, descriptor) in targets.iter().zip(descriptors) {
        let Some(value) = target else {
            return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
        };
        descriptor
            .validate(*value)
            .map_err(|_| NativeInstrumentRuntimeError::InvalidParameterAutomation)?;
    }
    Ok(())
}

pub(super) fn validate_target_views(
    primary: &dyn MutableInstrumentRuntime,
    scratch: &dyn MutableInstrumentRuntime,
    descriptors: &[ParameterDescriptor],
) -> Result<(), NativeInstrumentRuntimeError> {
    match (
        primary.parameter_automation_readonly(),
        scratch.parameter_automation_readonly(),
    ) {
        (None, None) => Ok(()),
        (Some(primary), Some(scratch))
            if !descriptors.is_empty()
                && primary.descriptors() == descriptors
                && scratch.descriptors() == descriptors =>
        {
            let mut targets = [None; MAX_PARAMETERS_PER_DEVICE];
            checked_targets(primary, &mut targets[..descriptors.len()])?;
            targets.fill(None);
            checked_targets(scratch, &mut targets[..descriptors.len()])
        }
        _ => Err(NativeInstrumentRuntimeError::IncompatibleTwins),
    }
}

fn validate_values(
    view: &dyn InstrumentParameterRuntime,
    values: &dyn NativeParameterAutomation,
    frames: usize,
) -> Result<(), NativeInstrumentRuntimeError> {
    let descriptors = view.descriptors();
    let invalid = NativeInstrumentRuntimeError::InvalidParameterAutomation;
    let ids = values.parameter_ids();
    if frames > MAX_INSTRUMENT_AUTOMATION_FRAMES
        || ids.len() > MAX_PARAMETERS_PER_DEVICE
        || ids.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid);
    }
    let mut bindings = [0_usize; MAX_PARAMETERS_PER_DEVICE];
    for (binding, id) in bindings.iter_mut().zip(ids) {
        *binding = descriptors
            .binary_search_by_key(id, |descriptor| descriptor.id)
            .map_err(|_| invalid)?;
        if values.has_sample_rate_modulation(*id)
            && view.parameter_modulation_semantics(*id).is_none()
        {
            return Err(NativeInstrumentRuntimeError::UnsupportedParameterModulation);
        }
    }
    let mut readings = [None; MAX_PARAMETERS_PER_DEVICE];
    let readings = &mut readings[..ids.len()];
    for offset in 0..frames {
        values.values_at(offset, readings).map_err(|_| invalid)?;
        for (index, reading) in readings.iter().enumerate() {
            if let Some(value) = reading {
                descriptors[bindings[index]]
                    .validate(*value)
                    .map_err(|_| invalid)?;
            }
        }
    }
    Ok(())
}

impl NativeInstrumentRuntimePair {
    /// Immutable opt-in validated against both twins during construction.
    pub fn supports_committed_parameter_targets(&self) -> bool {
        self.primary.parameter_automation_readonly().is_some()
    }

    /// Observe committed primary targets, including while a speculative block is
    /// open. Exact ordered factory IDs are required; output is unchanged on every
    /// refusal. No caller buffer or descriptor borrow is retained. Construction
    /// validates the immutable view/grants; callback queries allocate/free nothing.
    pub fn copy_committed_parameter_targets(
        &self,
        ids: &[ParameterId],
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let view = self
            .primary
            .parameter_automation_readonly()
            .ok_or(NativeInstrumentRuntimeError::UnsupportedParameterAutomation)?;
        let descriptors = view.descriptors();
        if ids.len() != descriptors.len()
            || output.len() != ids.len()
            || ids
                .iter()
                .zip(descriptors)
                .any(|(id, descriptor)| *id != descriptor.id)
        {
            return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
        }
        let mut targets = [None; MAX_PARAMETERS_PER_DEVICE];
        let targets = targets
            .get_mut(..descriptors.len())
            .ok_or(NativeInstrumentRuntimeError::InvalidParameterAutomation)?;
        checked_targets(view, targets)?;
        output.copy_from_slice(targets);
        Ok(())
    }

    /// The latest committed source-global input to the occurrence-local route.
    /// A finite value must name an actual granted sample-rate destination.
    pub fn committed_unclamped_modulation_target(
        &self,
    ) -> Result<Option<(ParameterId, f64)>, NativeInstrumentRuntimeError> {
        let view = self
            .primary
            .parameter_automation_readonly()
            .ok_or(NativeInstrumentRuntimeError::UnsupportedParameterAutomation)?;
        let target = view.unclamped_modulation_target();
        if let Some((parameter, value)) = target
            && (!value.is_finite()
                || view
                    .descriptors()
                    .binary_search_by_key(&parameter, |d| d.id)
                    .is_err()
                || !view
                    .parameter_modulation_semantics(parameter)
                    .is_some_and(|semantics| {
                        semantics.modulation == ParameterModulation::SampleRate
                    }))
        {
            return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
        }
        Ok(target)
    }

    pub const fn supports_parameter_automation(&self) -> bool {
        self.parameter_automation
    }

    /// Automated segment in the existing scratch transaction. Generic bounds and
    /// every sample's canonical values are validated before provider processing.
    /// Readings must remain stable across these checks and provider processing.
    /// Even an empty ID list reaches an opted-in provider to restore Base.
    pub fn process_block_with_clock_and_automation(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[crate::NativeInstrumentOccurrenceGain],
        values: &dyn NativeParameterAutomation,
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !self.block_started {
            return Err(NativeInstrumentRuntimeError::BlockNotStarted);
        }
        if !self.parameter_automation {
            if !values.parameter_ids().is_empty() {
                return Err(NativeInstrumentRuntimeError::UnsupportedParameterAutomation);
            }
            return self.process_block_with_clock_and_occurrence_gains(
                events, frames, clock, gains, resources,
            );
        }
        if frames.len() > MAX_INSTRUMENT_AUTOMATION_FRAMES {
            return Err(NativeInstrumentRuntimeError::InvalidParameterAutomation);
        }
        if !gains.is_empty() {
            if !self.occurrence_gain {
                return Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain);
            }
            let maximum = self
                .max_active_occurrences
                .checked_add(self.max_events_per_block)
                .ok_or(NativeInstrumentRuntimeError::RuntimeBudgetExceeded)?;
            crate::instrument_gain::validate_occurrence_gains(gains, frames.len(), maximum)?;
        }
        validate_event_batch(
            events,
            frames.len(),
            self.max_events_per_block,
            self.note_pressure,
        )?;
        self.scratch
            .preflight_events(events, frames.len(), resources)?;
        self.scratch
            .preflight_clock(clock, frames.len(), resources)?;
        if !gains.is_empty() {
            self.scratch.preflight_occurrence_gains(events, gains)?;
        }
        let view = self
            .scratch
            .parameter_automation()
            .ok_or(NativeInstrumentRuntimeError::IncompatibleTwins)?;
        validate_values(view, values, frames.len())?;
        view.preflight(events, frames.len(), clock, gains, values, resources)?;
        if !frames.is_empty() {
            view.process(events, frames, clock, gains, values, resources);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
