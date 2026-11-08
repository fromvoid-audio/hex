//! Opt-in parameter delivery inside the existing independent-twin transaction.
use super::*;
use crate::NativeParameterAutomation;
use hex_lattice_contract::{MAX_PARAMETERS_PER_DEVICE, ParameterDescriptor};

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
    descriptors: &[ParameterDescriptor],
) -> bool {
    match runtime.parameter_automation() {
        Some(view) => !descriptors.is_empty() && view.descriptors() == descriptors,
        None => descriptors.is_empty(),
    }
}

fn validate_values(
    descriptors: &[ParameterDescriptor],
    values: &dyn NativeParameterAutomation,
    frames: usize,
) -> Result<(), NativeInstrumentRuntimeError> {
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
        validate_event_batch(events, frames.len(), self.max_events_per_block)?;
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
        validate_values(view.descriptors(), values, frames.len())?;
        view.preflight(events, frames.len(), clock, gains, values, resources)?;
        if !frames.is_empty() {
            view.process(events, frames, clock, gains, values, resources);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
