//! Canonical bounded Rack macro mappings onto nested child parameter targets.
//!
//! Each macro carries a stable `macro_id`, normalized value in `0.0..=1.0`,
//! bypass switch, and up to `AUDIO_RACK_MAX_MAPPINGS_PER_MACRO` mappings
//! targeting stable `(rack_id, device_id, parameter_id)` coordinates inside
//! the owning Audio Rack tree.
//!
//! Target resolution happens strictly on control using `hex-lattice-contract`'s
//! canonical parameter semantics (`validated_state_descriptors`,
//! `validated_parameter_semantics`, `ParameterNormalization::denormalize`).
//! Read-only, structural, unknown, missing, or container targets are refused
//! explicitly without silently retargeting or mutating authored device states.

use super::{AUDIO_RACK_TYPE_ID, AudioRackError, AudioRackSpec};
use crate::NativeDeviceRegistry;
use hex_lattice_contract::{
    NativeDeviceState, ParameterAccess, ParameterChangeImpact, ParameterDescriptor, ParameterId,
    ParameterKind, ParameterNormalization, ParameterValue, validated_parameter_semantics,
    validated_state_descriptors,
};
use std::num::NonZeroU64;

/// Maximum macros admitted on one Audio Rack.
pub const AUDIO_RACK_MAX_MACROS: usize = 8;
/// Maximum parameter targets mapped to a single Rack macro.
pub const AUDIO_RACK_MAX_MAPPINGS_PER_MACRO: usize = 8;

/// Stable canonical target coordinate for a Rack macro mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioRackMacroTarget {
    /// Owning rack ID (outer rack or a depth-2 nested rack).
    pub rack_id: NonZeroU64,
    /// Stable child leaf device identity inside `rack_id`.
    pub device_id: NonZeroU64,
    /// Stable parameter identity on `device_id`.
    pub parameter_id: ParameterId,
}

/// Direction law applied when mapping a macro value into `[min_normalized, max_normalized]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioRackMacroPolarity {
    Normal,
    Inverted,
}

/// Precedence policy between a Rack macro and per-target parameter automation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioRackMacroPrecedence {
    MacroOverridesAuthored,
    AutomationOverridesMacro,
}

/// One bounded macro-to-parameter mapping entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioRackMacroMapping {
    pub target: AudioRackMacroTarget,
    pub min_normalized: f32,
    pub max_normalized: f32,
    pub polarity: AudioRackMacroPolarity,
    pub precedence: AudioRackMacroPrecedence,
    pub bypassed: bool,
}

impl AudioRackMacroMapping {
    /// Compute the effective normalized target position in `0.0..=1.0` for
    /// `macro_value` (`0.0..=1.0`) under explicit range and polarity laws.
    pub fn evaluate_normalized(&self, macro_value: f32) -> f32 {
        let clamped = macro_value.clamp(0.0, 1.0);
        let directed = match self.polarity {
            AudioRackMacroPolarity::Normal => clamped,
            AudioRackMacroPolarity::Inverted => 1.0 - clamped,
        };
        let mapped = f64::from(self.min_normalized)
            + (f64::from(self.max_normalized) - f64::from(self.min_normalized))
                * f64::from(directed);
        (mapped as f32).clamp(0.0, 1.0)
    }
}

/// Canonical Rack macro state and its bounded target mappings.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRackMacroSpec {
    pub macro_id: NonZeroU64,
    pub value: f32,
    pub bypassed: bool,
    pub mappings: Vec<AudioRackMacroMapping>,
}

/// Resolved control-side macro assignment produced before runtime preparation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedMacroAssignment {
    pub macro_id: NonZeroU64,
    pub target: AudioRackMacroTarget,
    pub normalized: f32,
    pub value: ParameterValue,
}

fn find_leaf_device_state(
    spec: &AudioRackSpec,
    target_rack_id: NonZeroU64,
    target_device_id: NonZeroU64,
) -> Result<Option<NativeDeviceState>, AudioRackError> {
    if spec.rack_id == target_rack_id {
        for branch in &spec.branches {
            for device in &branch.devices {
                if device.device_id == target_device_id {
                    if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                        return Err(AudioRackError::InvalidMacroTarget);
                    }
                    return Ok(Some(device.state.clone()));
                }
            }
        }
        return Ok(None);
    }
    for branch in &spec.branches {
        for device in &branch.devices {
            if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                let nested = AudioRackSpec::decode_state_raw(&device.state, true)?;
                if let Some(found) =
                    find_leaf_device_state(&nested, target_rack_id, target_device_id)?
                {
                    return Ok(Some(found));
                }
            }
        }
    }
    Ok(None)
}

fn patch_leaf_device_state(
    spec: &mut AudioRackSpec,
    target_rack_id: NonZeroU64,
    target_device_id: NonZeroU64,
    new_state: NativeDeviceState,
) -> Result<bool, AudioRackError> {
    if spec.rack_id == target_rack_id {
        for branch in &mut spec.branches {
            for device in &mut branch.devices {
                if device.device_id == target_device_id {
                    if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                        return Err(AudioRackError::InvalidMacroTarget);
                    }
                    device.state = new_state;
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    for branch in &mut spec.branches {
        for device in &mut branch.devices {
            if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                let mut nested = AudioRackSpec::decode_state_raw(&device.state, true)?;
                if patch_leaf_device_state(
                    &mut nested,
                    target_rack_id,
                    target_device_id,
                    new_state.clone(),
                )? {
                    device.state = nested.encode_state_version(super::AUDIO_RACK_STATE_VERSION)?;
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Validate structural macro constraints on `spec` (bounds, unique macro IDs,
/// finite values/ranges, unique targets, and live leaf target existence).
pub(super) fn validate_macros(spec: &AudioRackSpec) -> Result<(), AudioRackError> {
    if spec.macros.len() > AUDIO_RACK_MAX_MACROS {
        return Err(AudioRackError::MacroCapacityExceeded);
    }
    let mut seen_macros = Vec::with_capacity(spec.macros.len());
    let mut seen_targets: Vec<(NonZeroU64, ParameterId)> = Vec::new();
    for m in &spec.macros {
        if seen_macros.contains(&m.macro_id) {
            return Err(AudioRackError::DuplicateMacroId);
        }
        seen_macros.push(m.macro_id);
        if !m.value.is_finite() || !(0.0..=1.0).contains(&m.value) {
            return Err(AudioRackError::InvalidMacroValue);
        }
        if m.mappings.len() > AUDIO_RACK_MAX_MAPPINGS_PER_MACRO {
            return Err(AudioRackError::MacroMappingCapacityExceeded);
        }
        for mapping in &m.mappings {
            if !mapping.min_normalized.is_finite()
                || !mapping.max_normalized.is_finite()
                || !(0.0..=1.0).contains(&mapping.min_normalized)
                || !(0.0..=1.0).contains(&mapping.max_normalized)
                || mapping.min_normalized > mapping.max_normalized
            {
                return Err(AudioRackError::InvalidMacroRange);
            }
            if find_leaf_device_state(spec, mapping.target.rack_id, mapping.target.device_id)?
                .is_none()
            {
                return Err(AudioRackError::InvalidMacroTarget);
            }
            let key = (mapping.target.device_id, mapping.target.parameter_id);
            if seen_targets.contains(&key) {
                return Err(AudioRackError::DuplicateMacroTarget);
            }
            seen_targets.push(key);
        }
    }
    Ok(())
}

fn effective_normalization(
    descriptor: ParameterDescriptor,
    declared: ParameterNormalization,
) -> Result<ParameterNormalization, AudioRackError> {
    match declared {
        ParameterNormalization::Linear
        | ParameterNormalization::PositiveLogarithmic
        | ParameterNormalization::Discrete => Ok(declared),
        ParameterNormalization::Unspecified => match descriptor.kind {
            ParameterKind::Float { .. } => Ok(ParameterNormalization::Linear),
            ParameterKind::Bool | ParameterKind::Choice { .. } => {
                Ok(ParameterNormalization::Discrete)
            }
        },
    }
}

/// Resolve all active macro mappings on control against `registry`, verifying
/// every mapped parameter is known, writable, and non-structural, and return
/// both the effective prepared `AudioRackSpec` (authored `spec` is never
/// mutated) and the deterministic resolution ledger.
pub fn resolve_rack_macros_on_control(
    spec: &AudioRackSpec,
    registry: &NativeDeviceRegistry<'_>,
    automated_targets: &[(NonZeroU64, ParameterId)],
) -> Result<(AudioRackSpec, Vec<ResolvedMacroAssignment>), AudioRackError> {
    validate_macros(spec)?;
    if spec.macros.is_empty() {
        return Ok((spec.clone(), Vec::new()));
    }
    let mut effective = spec.clone();
    let mut ledger = Vec::new();
    for m in &spec.macros {
        for mapping in &m.mappings {
            let current_state = find_leaf_device_state(
                &effective,
                mapping.target.rack_id,
                mapping.target.device_id,
            )?
            .ok_or(AudioRackError::InvalidMacroTarget)?;
            let def = registry
                .definition(current_state.type_id())
                .ok_or(AudioRackError::UnsupportedMacroTarget)?;
            let migrated = def
                .migrate_state(&current_state)
                .map_err(|_| AudioRackError::UnsupportedMacroTarget)?;
            let descriptors = validated_state_descriptors(def, &migrated)
                .map_err(|_| AudioRackError::UnsupportedMacroTarget)?;
            let descriptor = descriptors
                .into_iter()
                .find(|d| d.id == mapping.target.parameter_id)
                .ok_or(AudioRackError::UnsupportedMacroTarget)?;
            let semantics = validated_parameter_semantics(def, &migrated, descriptor.id)
                .map_err(|_| AudioRackError::UnsupportedMacroTarget)?;
            if let Some(sem) = semantics
                && (sem.access == ParameterAccess::ReadOnly
                    || sem.change_impact == ParameterChangeImpact::Structural)
            {
                return Err(AudioRackError::UnsupportedMacroTarget);
            }
            let norm_law = effective_normalization(
                descriptor,
                semantics.map_or(ParameterNormalization::Unspecified, |s| s.normalization),
            )?;

            // Bypassed macro/mapping or active automation under AutomationOverridesMacro
            // leaves the target at its authored/automated state (still semantically validated above!).
            let automation_active = mapping.precedence
                == AudioRackMacroPrecedence::AutomationOverridesMacro
                && automated_targets
                    .contains(&(mapping.target.device_id, mapping.target.parameter_id));
            if m.bypassed || mapping.bypassed || automation_active {
                continue;
            }

            let normalized = mapping.evaluate_normalized(m.value);
            let value = norm_law
                .denormalize(descriptor, normalized)
                .map_err(|_| AudioRackError::UnsupportedMacroTarget)?;
            let patched = def
                .patch_parameter(&migrated, descriptor.id, value)
                .map_err(|_| AudioRackError::UnsupportedMacroTarget)?;
            if !patch_leaf_device_state(
                &mut effective,
                mapping.target.rack_id,
                mapping.target.device_id,
                patched,
            )? {
                return Err(AudioRackError::InvalidMacroTarget);
            }
            ledger.push(ResolvedMacroAssignment {
                macro_id: m.macro_id,
                target: mapping.target,
                normalized,
                value,
            });
        }
    }
    Ok((effective, ledger))
}
