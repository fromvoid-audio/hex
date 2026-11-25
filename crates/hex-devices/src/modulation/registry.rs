//! Immutable built-in modulator registry covering all 16 native modulator codecs.
//!
//! `hex-engine` resolves modulator validation, output preparation, identity-range
//! rules, and source-to-source target parameters through `native_modulator_registry()`
//! instead of hardcoded `hex.modulator.*` type-ID match tables.

use super::{
    adsr, ahd_release, audio_rate, button, buttons, envelope_follower, lfo, macro_four,
    macro_value, math, mix, note_counter, note_pressure, ramp, random_hold, steps,
};
use hex_lattice::{
    ModulationSourceId, ModulatorPreparationContext, ModulatorPreparedOutput,
    NativeModulatorDefinition, NativeModulatorRegistry, PreparedModulationSource,
    SourceModulationParameter,
};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, ModulationOutputId, NativeDeviceState, ParameterId,
};

struct AdsrModulator;
impl NativeModulatorDefinition for AdsrModulator {
    fn type_id(&self) -> DeviceTypeId {
        adsr::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "adsr"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        adsr::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        adsr::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = adsr::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct AhdReleaseModulator;
impl NativeModulatorDefinition for AhdReleaseModulator {
    fn type_id(&self) -> DeviceTypeId {
        ahd_release::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "ahd release"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        ahd_release::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        ahd_release::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = ahd_release::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct AudioRateModulator;
impl NativeModulatorDefinition for AudioRateModulator {
    fn type_id(&self) -> DeviceTypeId {
        audio_rate::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "audio rate"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        audio_rate::default_state()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        audio_rate::prepare(state, ModulationSourceId::new(1)?, 48_000).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = audio_rate::prepare(state, id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct ButtonModulator;
impl NativeModulatorDefinition for ButtonModulator {
    fn type_id(&self) -> DeviceTypeId {
        button::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "button"
    }
    fn uses_identity_range(&self) -> bool {
        true
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        button::encode(false)
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        button::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = button::prepare(state, id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct ButtonsModulator;
impl NativeModulatorDefinition for ButtonsModulator {
    fn type_id(&self) -> DeviceTypeId {
        buttons::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "buttons"
    }
    fn output_count(&self) -> usize {
        buttons::OUTPUT_COUNT
    }
    fn uses_identity_range(&self) -> bool {
        true
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        buttons::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        buttons::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let settings = buttons::Settings::decode(state)?;
        for number in 1..=buttons::OUTPUT_COUNT as u32 {
            let output = ModulationOutputId::new(number)?;
            let source = settings.prepare(id, context.sample_rate, output)?;
            out.push(ModulatorPreparedOutput { output, source });
        }
        Ok(())
    }
}

struct EnvelopeFollowerModulator;
impl NativeModulatorDefinition for EnvelopeFollowerModulator {
    fn type_id(&self) -> DeviceTypeId {
        envelope_follower::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "envelope follower"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        envelope_follower::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        envelope_follower::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source =
            envelope_follower::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct LfoModulator;
impl NativeModulatorDefinition for LfoModulator {
    fn type_id(&self) -> DeviceTypeId {
        lfo::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "lfo"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        lfo::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        lfo::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = lfo::Settings::decode(state)?.prepare(
            id,
            context.sample_rate,
            context.fallback_bpm,
            context.maximum_bpm,
        )?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
    fn resolve_target_parameter(
        &self,
        parameter: ParameterId,
        target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        if parameter != lfo::RATE_PARAMETER {
            return None;
        }
        let target_param = if target.is_some_and(|source| {
            source.supports_modulation_parameter(SourceModulationParameter::TempoLfoRate)
        }) {
            SourceModulationParameter::TempoLfoRate
        } else {
            SourceModulationParameter::FreeLfoRate
        };
        Some((ModulationOutputId::MAIN, target_param))
    }
}

struct MacroFourModulator;
impl NativeModulatorDefinition for MacroFourModulator {
    fn type_id(&self) -> DeviceTypeId {
        macro_four::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "macro 4"
    }
    fn output_count(&self) -> usize {
        macro_four::VALUE_PARAMETERS.len()
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        macro_four::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        macro_four::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let settings = macro_four::Settings::decode(state)?;
        for parameter in macro_four::VALUE_PARAMETERS {
            let output = macro_four::output_for_parameter(parameter).ok_or(Error::InvalidId)?;
            let source = settings.prepare(id, context.sample_rate, output)?;
            out.push(ModulatorPreparedOutput { output, source });
        }
        Ok(())
    }
    fn resolve_target_parameter(
        &self,
        parameter: ParameterId,
        _target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        let output = macro_four::output_for_parameter(parameter)?;
        Some((output, SourceModulationParameter::MacroValue))
    }
}

struct MacroValueModulator;
impl NativeModulatorDefinition for MacroValueModulator {
    fn type_id(&self) -> DeviceTypeId {
        macro_value::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "macro"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        macro_value::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        macro_value::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = macro_value::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
    fn resolve_target_parameter(
        &self,
        parameter: ParameterId,
        _target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        (parameter == macro_value::VALUE_PARAMETER).then_some((
            ModulationOutputId::MAIN,
            SourceModulationParameter::MacroValue,
        ))
    }
}

struct MathModulator;
impl NativeModulatorDefinition for MathModulator {
    fn type_id(&self) -> DeviceTypeId {
        math::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "math"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        math::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        math::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = math::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
    fn resolve_target_parameter(
        &self,
        parameter: ParameterId,
        _target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        let target_param = if parameter == math::INPUT_A {
            SourceModulationParameter::MathA
        } else if parameter == math::INPUT_B {
            SourceModulationParameter::MathB
        } else {
            return None;
        };
        Some((ModulationOutputId::MAIN, target_param))
    }
}

struct MixModulator;
impl NativeModulatorDefinition for MixModulator {
    fn type_id(&self) -> DeviceTypeId {
        mix::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "mix"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        mix::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        mix::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = mix::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
    fn resolve_target_parameter(
        &self,
        parameter: ParameterId,
        _target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        let target_param = if parameter == mix::INPUT_A {
            SourceModulationParameter::MixA
        } else if parameter == mix::INPUT_B {
            SourceModulationParameter::MixB
        } else if parameter == mix::BLEND {
            SourceModulationParameter::MixBlend
        } else {
            return None;
        };
        Some((ModulationOutputId::MAIN, target_param))
    }
}

struct NoteCounterModulator;
impl NativeModulatorDefinition for NoteCounterModulator {
    fn type_id(&self) -> DeviceTypeId {
        note_counter::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "note counter"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        note_counter::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        note_counter::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = note_counter::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct NotePressureModulator;
impl NativeModulatorDefinition for NotePressureModulator {
    fn type_id(&self) -> DeviceTypeId {
        note_pressure::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "note pressure"
    }
    fn output_count(&self) -> usize {
        0
    }
    fn is_occurrence_owned(&self) -> bool {
        true
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        note_pressure::default_state()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        note_pressure::validate(state)
    }
    fn prepare_outputs(
        &self,
        _id: ModulationSourceId,
        state: &NativeDeviceState,
        _context: ModulatorPreparationContext,
        _out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        note_pressure::validate(state)
    }
}

struct RampModulator;
impl NativeModulatorDefinition for RampModulator {
    fn type_id(&self) -> DeviceTypeId {
        ramp::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "ramp"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        ramp::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        ramp::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = ramp::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct RandomHoldModulator;
impl NativeModulatorDefinition for RandomHoldModulator {
    fn type_id(&self) -> DeviceTypeId {
        random_hold::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "random"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        random_hold::Settings {
            seed: 1,
            rate_hz: 1.0,
        }
        .encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        random_hold::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let source = random_hold::Settings::decode(state)?.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

struct StepsModulator;
impl NativeModulatorDefinition for StepsModulator {
    fn type_id(&self) -> DeviceTypeId {
        steps::TYPE_ID
    }
    fn display_name(&self) -> &'static str {
        "steps"
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        steps::Settings::default().encode()
    }
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), Error> {
        steps::Settings::decode(state).map(|_| ())
    }
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), Error> {
        let settings = steps::Settings::decode(state)?;
        if context.instrument_events && matches!(settings.timing, steps::Timing::Arrangement { .. })
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        let source = settings.prepare(id, context.sample_rate)?;
        out.push(ModulatorPreparedOutput {
            output: ModulationOutputId::MAIN,
            source,
        });
        Ok(())
    }
}

static BUILTIN_MODULATORS: [&dyn NativeModulatorDefinition; 16] = [
    &AdsrModulator,
    &AhdReleaseModulator,
    &AudioRateModulator,
    &ButtonModulator,
    &ButtonsModulator,
    &EnvelopeFollowerModulator,
    &LfoModulator,
    &MacroFourModulator,
    &MacroValueModulator,
    &MathModulator,
    &MixModulator,
    &NoteCounterModulator,
    &NotePressureModulator,
    &RampModulator,
    &RandomHoldModulator,
    &StepsModulator,
];

/// The immutable catalog of all 16 built-in native modulators.
pub fn native_modulator_registry() -> Result<NativeModulatorRegistry<'static>, Error> {
    NativeModulatorRegistry::new(&BUILTIN_MODULATORS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::modulator::BUILTIN_MODULATOR_TYPES;

    #[test]
    fn registry_covers_all_sixteen_canonical_modulators_and_prepares_default_states() {
        let registry = native_modulator_registry().expect("built-in modulator registry valid");
        assert_eq!(registry.definitions().len(), BUILTIN_MODULATOR_TYPES.len());
        let id = ModulationSourceId::new(1).unwrap();
        let context = ModulatorPreparationContext {
            sample_rate: 48_000,
            fallback_bpm: 120.0,
            maximum_bpm: 180.0,
            instrument_events: true,
        };
        for type_id in BUILTIN_MODULATOR_TYPES {
            let def = registry
                .definition(type_id)
                .unwrap_or_else(|| panic!("missing modulator {:?}", type_id.as_str()));
            let state = registry.default_state(type_id).unwrap();
            assert_eq!(state.type_id(), type_id);
            registry.validate_state(&state).unwrap();
            let mut outputs = Vec::new();
            registry
                .prepare_outputs(id, &state, context, &mut outputs)
                .unwrap();
            assert_eq!(outputs.len(), def.output_count());
        }
        assert!(registry.uses_identity_range(button::TYPE_ID));
        assert!(registry.uses_identity_range(buttons::TYPE_ID));
        assert!(!registry.uses_identity_range(lfo::TYPE_ID));
        assert!(registry.is_occurrence_owned(note_pressure::TYPE_ID));
        assert_eq!(registry.output_count(macro_four::TYPE_ID), Some(4));
        assert_eq!(registry.output_count(buttons::TYPE_ID), Some(2));
        assert_eq!(registry.output_count(note_pressure::TYPE_ID), Some(0));
    }

    #[test]
    fn registry_resolves_modulator_target_parameters_without_engine_match_tables() {
        let registry = native_modulator_registry().unwrap();
        assert_eq!(
            registry.resolve_target_parameter(
                macro_value::TYPE_ID,
                macro_value::VALUE_PARAMETER,
                None
            ),
            Some((
                ModulationOutputId::MAIN,
                SourceModulationParameter::MacroValue
            ))
        );
        assert_eq!(
            registry.resolve_target_parameter(
                macro_four::TYPE_ID,
                macro_four::VALUE_PARAMETERS[2],
                None
            ),
            Some((
                ModulationOutputId::new(3).unwrap(),
                SourceModulationParameter::MacroValue
            ))
        );
        assert_eq!(
            registry.resolve_target_parameter(math::TYPE_ID, math::INPUT_A, None),
            Some((ModulationOutputId::MAIN, SourceModulationParameter::MathA))
        );
        assert_eq!(
            registry.resolve_target_parameter(mix::TYPE_ID, mix::BLEND, None),
            Some((
                ModulationOutputId::MAIN,
                SourceModulationParameter::MixBlend
            ))
        );
        let tempo_lfo = lfo::Settings {
            rate: lfo::Rate::BeatsPerCycle(1.0),
            ..Default::default()
        }
        .prepare(ModulationSourceId::new(1).unwrap(), 48_000, 120.0, 180.0)
        .unwrap();
        assert_eq!(
            registry.resolve_target_parameter(lfo::TYPE_ID, lfo::RATE_PARAMETER, Some(&tempo_lfo)),
            Some((
                ModulationOutputId::MAIN,
                SourceModulationParameter::TempoLfoRate
            ))
        );
        assert_eq!(
            registry.resolve_target_parameter(lfo::TYPE_ID, lfo::RATE_PARAMETER, None),
            Some((
                ModulationOutputId::MAIN,
                SourceModulationParameter::FreeLfoRate
            ))
        );
    }
}
