//! Original stereo cutoff filter. Version 1 stores thirteen 32-bit little-endian
//! words in descriptor order. The TPT kernel is shared with Operator. The
//! per-channel cutoff is modulated by a free-running oscillator and a linked
//! peak envelope. Core continuous parameters admit shared host automation and
//! sample-rate modulation through the closed inline parameter binding table;
//! all authored values are checked before runtime construction.

mod runtime;

use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, PreparedNativeProcessor,
    PreparedRuntimeDevice, ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterAutomation, ParameterDescriptor, ParameterId, ParameterKind, ParameterModulation,
    ParameterNormalization, ParameterPresentation, ParameterSemantics, ParameterSmoothing,
    ParameterValue,
};
pub use runtime::{FilterRuntime, PreparedFilter};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.filter");
const VERSION: u32 = 1;
const COUNT: usize = 13;
const WORD_BYTES: usize = size_of::<u32>();

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const TYPE: ParameterId = ParameterId::from_static(1);
    pub const SLOPE: ParameterId = ParameterId::from_static(2);
    pub const CUTOFF: ParameterId = ParameterId::from_static(3);
    pub const RESONANCE: ParameterId = ParameterId::from_static(4);
    pub const MORPH: ParameterId = ParameterId::from_static(5);
    pub const MIX: ParameterId = ParameterId::from_static(6);
    pub const LFO_AMOUNT: ParameterId = ParameterId::from_static(7);
    pub const LFO_RATE: ParameterId = ParameterId::from_static(8);
    pub const LFO_SHAPE: ParameterId = ParameterId::from_static(9);
    pub const LFO_PHASE: ParameterId = ParameterId::from_static(10);
    pub const ENV_AMOUNT: ParameterId = ParameterId::from_static(11);
    pub const ENV_RELEASE: ParameterId = ParameterId::from_static(12);
    pub const ENV_ATTACK: ParameterId = ParameterId::from_static(13);
}

const TYPE: usize = 0;
const SLOPE: usize = 1;
const CUTOFF: usize = 2;
const RESONANCE: usize = 3;
const MORPH: usize = 4;
const MIX: usize = 5;
const LFO_AMOUNT: usize = 6;
const LFO_RATE: usize = 7;
const LFO_SHAPE: usize = 8;
const LFO_PHASE: usize = 9;
const ENV_AMOUNT: usize = 10;
const ENV_RELEASE: usize = 11;
const ENV_ATTACK: usize = 12;

const fn choice(
    index: usize,
    label: &'static str,
    labels: &'static [&'static str],
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Choice { labels },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    }
}

const fn value(
    index: usize,
    label: &'static str,
    unit: &'static str,
    min: f32,
    max: f32,
    default: f32,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Float {
            min,
            max,
            unit: Some(unit),
        },
        default: ParameterValue::Float(default),
        presentation,
    }
}

use ParameterPresentation::{Linear, Logarithmic};

const DESCRIPTORS: [ParameterDescriptor; COUNT] = [
    choice(TYPE, "type", crate::operator_filter::MODES),
    choice(SLOPE, "slope", &["12 db", "24 db"]),
    value(CUTOFF, "cutoff", "hz", 20.0, 20_000.0, 1000.0, Logarithmic),
    value(RESONANCE, "resonance", "%", 0.0, 100.0, 0.0, Linear),
    value(MORPH, "morph", "%", 0.0, 100.0, 50.0, Linear),
    value(MIX, "dry/wet", "%", 0.0, 100.0, 100.0, Linear),
    value(LFO_AMOUNT, "lfo amount", "oct", 0.0, 4.0, 0.0, Linear),
    value(LFO_RATE, "lfo rate", "hz", 0.02, 30.0, 0.5, Logarithmic),
    choice(
        LFO_SHAPE,
        "lfo shape",
        &["sine", "triangle", "saw up", "saw down", "square"],
    ),
    value(LFO_PHASE, "lfo stereo phase", "°", 0.0, 360.0, 0.0, Linear),
    value(ENV_AMOUNT, "envelope amount", "oct", -4.0, 4.0, 0.0, Linear),
    value(
        ENV_RELEASE,
        "envelope release",
        "ms",
        5.0,
        2000.0,
        120.0,
        Logarithmic,
    ),
    value(
        ENV_ATTACK,
        "envelope attack",
        "ms",
        0.1,
        200.0,
        5.0,
        Logarithmic,
    ),
];

const STATIC_CHOICE: ParameterSemantics = ParameterSemantics {
    automation: ParameterAutomation::Unsupported,
    modulation: ParameterModulation::Unsupported,
    smoothing: ParameterSmoothing::None,
    ..crate::parameter_binding::CHOICE_ENVELOPE
};

const MODULATED_LINEAR: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const MODULATED_LOG: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    ..MODULATED_LINEAR
};

const BINDINGS: [crate::parameter_binding::Binding<Settings>; COUNT] = [
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[TYPE],
        STATIC_CHOICE,
        |s| s.0[TYPE],
        |s, v| {
            s.0[TYPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[SLOPE],
        STATIC_CHOICE,
        |s| s.0[SLOPE],
        |s, v| {
            s.0[SLOPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[CUTOFF],
        MODULATED_LOG,
        |s| s.f32_at(CUTOFF),
        |s, v| s.set_f32(CUTOFF, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[RESONANCE],
        MODULATED_LINEAR,
        |s| s.f32_at(RESONANCE),
        |s, v| s.set_f32(RESONANCE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[MORPH],
        MODULATED_LINEAR,
        |s| s.f32_at(MORPH),
        |s, v| s.set_f32(MORPH, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[MIX],
        MODULATED_LINEAR,
        |s| s.f32_at(MIX),
        |s, v| s.set_f32(MIX, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_AMOUNT],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_AMOUNT),
        |s, v| s.set_f32(LFO_AMOUNT, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_RATE],
        MODULATED_LOG,
        |s| s.f32_at(LFO_RATE),
        |s, v| s.set_f32(LFO_RATE, v),
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[LFO_SHAPE],
        STATIC_CHOICE,
        |s| s.0[LFO_SHAPE],
        |s, v| {
            s.0[LFO_SHAPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_PHASE],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_PHASE),
        |s, v| s.set_f32(LFO_PHASE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_AMOUNT],
        MODULATED_LINEAR,
        |s| s.f32_at(ENV_AMOUNT),
        |s, v| s.set_f32(ENV_AMOUNT, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_RELEASE],
        MODULATED_LOG,
        |s| s.f32_at(ENV_RELEASE),
        |s, v| s.set_f32(ENV_RELEASE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_ATTACK],
        MODULATED_LOG,
        |s| s.f32_at(ENV_ATTACK),
        |s, v| s.set_f32(ENV_ATTACK, v),
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings([u32; COUNT]);

impl Settings {
    fn default_values() -> Result<Self, DeviceContractError> {
        let mut settings = Self([0; COUNT]);
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            settings.write(index, descriptor.default)?;
        }
        Ok(settings)
    }

    fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if state.payload().len() != COUNT * WORD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut settings = Self([0; COUNT]);
        for (word, bytes) in settings
            .0
            .iter_mut()
            .zip(state.payload().chunks_exact(WORD_BYTES))
        {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            descriptor.validate(settings.read(index))?;
        }
        Ok(settings)
    }

    fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(
            TYPE_ID,
            VERSION,
            self.0.into_iter().flat_map(u32::to_le_bytes).collect(),
        )
    }

    fn f32_at(self, index: usize) -> f32 {
        f32::from_bits(self.0[index])
    }

    fn set_f32(&mut self, index: usize, value: f32) {
        self.0[index] = value.to_bits();
    }

    fn float(self, index: usize) -> f64 {
        f64::from(f32::from_bits(self.0[index]))
    }

    fn read(self, index: usize) -> ParameterValue {
        if matches!(DESCRIPTORS[index].kind, ParameterKind::Choice { .. }) {
            ParameterValue::Choice(self.0[index])
        } else {
            ParameterValue::Float(f32::from_bits(self.0[index]))
        }
    }

    fn write(&mut self, index: usize, value: ParameterValue) -> Result<(), DeviceContractError> {
        DESCRIPTORS[index].validate(value)?;
        self.0[index] = match value {
            ParameterValue::Float(value) => value.to_bits(),
            ParameterValue::Choice(value) => value,
            ParameterValue::Bool(_) => return Err(DeviceContractError::InvalidParameterValue),
        };
        Ok(())
    }
}

fn parameter_index(id: ParameterId) -> Result<usize, DeviceContractError> {
    let index = id
        .value()
        .checked_sub(1)
        .ok_or(DeviceContractError::UnknownParameter)? as usize;
    if index < COUNT {
        Ok(index)
    } else {
        Err(DeviceContractError::UnknownParameter)
    }
}

/// Built-in effect metadata and validated control-side construction.
pub struct FilterDefinition;

pub static DEFINITION: FilterDefinition = FilterDefinition;

impl FilterDefinition {
    /// Sealed inline preparation for track, source and bus effect chains.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedFilter, DeviceContractError> {
        state.require_asset_free_processing()?;
        if parameters.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut admitted = [false; COUNT];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            if !binding.declares_arrangement_automation() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            admitted[parameter_index(id)?] = true;
        }
        PreparedFilter::new(Settings::decode(state)?, rate, admitted)
    }
}

impl NativeDeviceMetadata for FilterDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("filter")
    }
    fn state_version(&self) -> u32 {
        VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(
            &BINDINGS,
            state,
            id,
            Settings::decode,
        )
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::default_values()?.encode()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::decode(state)?.encode()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Ok(Settings::decode(state)?.read(parameter_index(id)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = Settings::decode(state)?;
        settings.write(parameter_index(id)?, value)?;
        settings.encode()
    }
}

impl NativeDeviceDefinition for FilterDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedFilter>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        Settings::decode(state)?;
        Ok(crate::parameter_binding::binding(&BINDINGS, parameter)
            .is_ok_and(|binding| binding.declares_arrangement_automation()))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::parameter_binding::prepared_legacy_runtime(
            Self::prepare_owned(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

/// Static wet response before LFO and envelope modulation. Coordinates are
/// normalized across 20 Hz to the project rate's supported upper cutoff and
/// -48 to +48 dB. The provider's prepared kernel owns these response values.
pub fn response(
    state: &NativeDeviceState,
    rate: u32,
) -> Result<[[f32; 2]; 129], DeviceContractError> {
    let prepared = FilterDefinition::prepare_owned(state, rate, &[])?;
    let upper = 20_000.0_f64.min(f64::from(rate) * 0.45);
    let frame = prepared.frame.with_full_wet();
    let compact = crate::operator_filter::Prepared { frame, rate };
    Ok(std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let frequency = 20.0 * (upper / 20.0).powf(x);
        let magnitude = compact.magnitude_at_hz(frequency as f32);
        let y = ((20.0 * magnitude.log10() + 48.0) / 96.0).clamp(0.0, 1.0);
        [x as f32, y]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, OutputTail};

    struct RampAutomation {
        ids: Vec<ParameterId>,
        start: f32,
        end: f32,
        length: usize,
        offset_base: usize,
    }

    impl NativeParameterAutomation for RampAutomation {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, frame_offset: usize) -> Option<ParameterValue> {
            if !self.ids.contains(&id) {
                return None;
            }
            let frame = (self.offset_base + frame_offset).min(self.length);
            let t = frame as f32 / self.length.max(1) as f32;
            Some(ParameterValue::Float(
                self.start + (self.end - self.start) * t,
            ))
        }
    }

    fn parameter(index: usize) -> ParameterId {
        DESCRIPTORS[index].id
    }

    fn patch(state: &NativeDeviceState, index: usize, value: ParameterValue) -> NativeDeviceState {
        DEFINITION
            .patch_parameter(state, parameter(index), value)
            .unwrap()
    }

    fn signal(frequency: f64) -> Vec<[f32; 2]> {
        (0..8192)
            .map(|sample| {
                let sample =
                    (std::f64::consts::TAU * frequency * f64::from(sample) / 48_000.0).sin() as f32;
                [sample; 2]
            })
            .collect()
    }

    fn render(state: &NativeDeviceState, signal: &mut [[f32; 2]], block: usize) {
        let mut runtime = FilterDefinition::prepare_owned(state, 48_000, &[])
            .unwrap()
            .create_runtime();
        for block in signal.chunks_mut(block) {
            runtime.process_block(block, &EMPTY_DEVICE_RESOURCES);
        }
    }

    fn power(signal: &[[f32; 2]]) -> f32 {
        signal[4096..].iter().map(|frame| frame[0] * frame[0]).sum()
    }

    #[test]
    fn cutoff_type_and_slope_follow_the_audio_response_and_dry_is_transparent() {
        let low = DEFINITION.default_state().unwrap();
        let high = patch(&low, TYPE, ParameterValue::Choice(1));
        let steep = patch(&low, SLOPE, ParameterValue::Choice(1));
        let dry = patch(&low, MIX, ParameterValue::Float(0.0));
        let mut pass = signal(4000.0);
        let input = pass.clone();
        let mut stop = input.clone();
        let mut steep_stop = input.clone();
        let mut dry_output = input.clone();
        render(&high, &mut pass, 127);
        render(&low, &mut stop, 127);
        render(&steep, &mut steep_stop, 127);
        render(&dry, &mut dry_output, 127);
        assert!(power(&pass) > 0.95 * power(&input));
        assert!(power(&stop) < power(&input) * 0.005);
        assert!(power(&steep_stop) < power(&stop) * 0.005);
        assert_eq!(dry_output, input);
        assert!(response(&low, 48_000).unwrap()[128][1] < response(&high, 48_000).unwrap()[128][1]);
    }

    #[test]
    fn cutoff_modulation_respects_polarity_stereo_phase_and_block_partition() {
        let state = DEFINITION.default_state().unwrap();
        let positive = patch(&state, ENV_AMOUNT, ParameterValue::Float(3.0));
        let negative = patch(&state, ENV_AMOUNT, ParameterValue::Float(-3.0));
        let lfo = patch(&state, LFO_AMOUNT, ParameterValue::Float(2.0));
        let lfo = patch(&lfo, LFO_PHASE, ParameterValue::Float(180.0));
        let input = signal(2000.0);
        let mut closed = input.clone();
        let mut open = input.clone();
        let mut whole = input.clone();
        let mut partitioned = input.clone();
        render(&negative, &mut closed, 256);
        render(&positive, &mut open, 256);
        render(&lfo, &mut whole, 8192);
        render(&lfo, &mut partitioned, 113);
        assert!(power(&open) > power(&closed) * 10.0);
        assert_eq!(whole, partitioned);
        assert!(whole.iter().any(|frame| (frame[0] - frame[1]).abs() > 0.1));
    }

    #[test]
    fn codec_round_trips_and_refuses_unvalidated_values_and_formats() {
        let original = DEFINITION.default_state().unwrap();
        let changed = patch(&original, CUTOFF, ParameterValue::Float(100.0));
        assert_eq!(DEFINITION.migrate_state(&changed).unwrap(), changed);
        assert_eq!(
            DEFINITION.read_parameter(&changed, parameter(CUTOFF)),
            Ok(ParameterValue::Float(100.0))
        );
        assert!(
            DEFINITION
                .patch_parameter(&changed, parameter(CUTOFF), ParameterValue::Float(f32::NAN))
                .is_err()
        );
        for version in [VERSION + 1, VERSION] {
            let broken = NativeDeviceState::new(TYPE_ID, version, vec![0; 4]).unwrap();
            assert!(DEFINITION.migrate_state(&broken).is_err());
        }
    }

    #[test]
    fn continuous_parameters_declare_automation_and_sample_rate_modulation_and_choices_refuse() {
        let state = DEFINITION.default_state().unwrap();
        let continuous = [
            parameter::CUTOFF,
            parameter::RESONANCE,
            parameter::MORPH,
            parameter::MIX,
            parameter::LFO_AMOUNT,
            parameter::LFO_RATE,
            parameter::LFO_PHASE,
            parameter::ENV_AMOUNT,
            parameter::ENV_RELEASE,
            parameter::ENV_ATTACK,
        ];
        for id in continuous {
            assert_eq!(
                DEFINITION.supports_parameter_automation(&state, id),
                Ok(true)
            );
            let semantics = DEFINITION
                .parameter_semantics_for_state(&state, id)
                .unwrap()
                .expect("semantics declared");
            assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
            assert_eq!(
                semantics.automation,
                ParameterAutomation::DeviceArrangementEnvelope
            );
            let prepared = FilterDefinition::prepare_owned(&state, 48_000, &[id]).unwrap();
            assert!(prepared.supports_parameter_automation(id));
            assert_eq!(prepared.parameter_modulation_semantics(id), Some(semantics));
            assert!(prepared.parameter_automation_descriptor(id).is_some());
        }

        for discrete in [parameter::TYPE, parameter::SLOPE, parameter::LFO_SHAPE] {
            assert_eq!(
                DEFINITION.supports_parameter_automation(&state, discrete),
                Ok(false)
            );
            assert_eq!(
                FilterDefinition::prepare_owned(&state, 48_000, &[discrete]).err(),
                Some(DeviceContractError::UnsupportedProcessingClass)
            );
        }

        let unknown = ParameterId::from_static(99);
        assert_eq!(
            DEFINITION.supports_parameter_automation(&state, unknown),
            Ok(false)
        );
        assert_eq!(
            FilterDefinition::prepare_owned(&state, 48_000, &[unknown]).err(),
            Some(DeviceContractError::UnknownParameter)
        );
    }

    #[test]
    fn automated_cutoff_sweep_matches_across_block_partitions_and_preserves_bypass() {
        let state = DEFINITION.default_state().unwrap();
        let prepared =
            FilterDefinition::prepare_owned(&state, 48_000, &[parameter::CUTOFF]).unwrap();
        let input = signal(4000.0);

        let mut whole = input.clone();
        let mut rt_whole = prepared.create_runtime();
        let auto_whole = RampAutomation {
            ids: vec![parameter::CUTOFF],
            start: 150.0,
            end: 12_000.0,
            length: input.len(),
            offset_base: 0,
        };
        rt_whole
            .process_block_with_automation(&mut whole, None, &auto_whole, &EMPTY_DEVICE_RESOURCES)
            .unwrap();

        let mut partitioned = input.clone();
        let mut rt_part = prepared.create_runtime();
        let mut offset = 0;
        for chunk in partitioned.chunks_mut(113) {
            let auto = RampAutomation {
                ids: vec![parameter::CUTOFF],
                start: 150.0,
                end: 12_000.0,
                length: input.len(),
                offset_base: offset,
            };
            rt_part
                .process_block_with_automation(chunk, None, &auto, &EMPTY_DEVICE_RESOURCES)
                .unwrap();
            offset += chunk.len();
        }

        assert_eq!(whole, partitioned);
        let early_power: f32 = whole[256..1024].iter().map(|f| f[0] * f[0]).sum();
        let late_power: f32 = whole[7168..7936].iter().map(|f| f[0] * f[0]).sum();
        assert!(
            late_power > early_power * 20.0,
            "automated cutoff opening must raise high-frequency power ({early_power} -> {late_power})"
        );

        let mut bypassed = input.clone();
        let mut rt_bypass = prepared.create_runtime();
        rt_bypass
            .process_bypassed_block_with_automation(
                &mut bypassed,
                None,
                &auto_whole,
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(bypassed, input);
    }

    #[test]
    fn finite_output_tail_drains_deterministically_even_at_extreme_resonance() {
        let state = DEFINITION.default_state().unwrap();
        let extreme = patch(&state, CUTOFF, ParameterValue::Float(20.0));
        let extreme = patch(&extreme, RESONANCE, ParameterValue::Float(100.0));
        let extreme = patch(&extreme, SLOPE, ParameterValue::Choice(1));
        let extreme = patch(&extreme, LFO_AMOUNT, ParameterValue::Float(1.0));
        let extreme = patch(&extreme, ENV_AMOUNT, ParameterValue::Float(2.0));

        let prepared = FilterDefinition::prepare_owned(&extreme, 48_000, &[]).unwrap();
        let OutputTail::Finite { max_tail_frames } = prepared.output_tail() else {
            panic!("hex.filter must declare a finite OutputTail");
        };
        assert!(max_tail_frames > 0 && max_tail_frames <= 48_000 * 6);

        let mut runtime = prepared.create_runtime();
        assert!(runtime.is_quiescent(), "fresh runtime starts quiescent");

        let mut impulse = [[0.0_f32; 2]; 256];
        impulse[0] = [1.0, -1.0];
        runtime.process_block(&mut impulse, &EMPTY_DEVICE_RESOURCES);
        assert!(
            !runtime.is_quiescent(),
            "resonant impulse must excite filter history"
        );

        let mut silence = vec![[0.0_f32; 2]; max_tail_frames as usize];
        for chunk in silence.chunks_mut(256) {
            runtime.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
        }
        assert!(
            runtime.is_quiescent(),
            "filter must become quiescent within max_tail_frames"
        );
        assert!(silence.iter().flatten().all(|s| s.is_finite()));

        let mut after = [[0.0_f32; 2]; 128];
        runtime.process_block(&mut after, &EMPTY_DEVICE_RESOURCES);
        assert_eq!(after, [[0.0_f32; 2]; 128]);
    }
}
