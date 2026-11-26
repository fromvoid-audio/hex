use super::*;

// The current concrete direct-control path implements only these fixed
// IDs, with value-only/no-smoothing updates. Other families may explicitly
// implement structural/smoothed delivery without changing Binding metadata.
pub(super) fn runtime_parameter_automation_supported(parameter: ParameterId) -> bool {
    let implemented = parameter
        .value()
        .checked_sub(1)
        .is_some_and(|index| index < 14);
    let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
        return false;
    };
    implemented
        && binding.declares_arrangement_automation()
        && binding.semantics.normalization
            != hex_lattice_contract::ParameterNormalization::Unspecified
        && binding.semantics.stepping != hex_lattice_contract::ParameterStepping::Unspecified
        && binding.semantics.change_impact == hex_lattice_contract::ParameterChangeImpact::ValueOnly
        && binding.semantics.smoothing == hex_lattice_contract::ParameterSmoothing::None
}

impl NativeDeviceDefinition for UtilityDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedUtilityFilterRuntime>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode_state(state)?;
        let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
            return Ok(false);
        };
        binding.semantics.validate(binding.descriptor)?;
        Ok(runtime_parameter_automation_supported(parameter))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::inline_owned::prepared_owned_runtime(
            Self::prepare_owned_automated_filter_for_parameters(
                state,
                context.sample_rate,
                parameters,
            )?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Self::prepare_owned_filter(state, context.sample_rate)
            .map(|value| value.map(crate::inline_owned::owned_factory))
    }

    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let settings = decode_state(state)?;
        if settings.dc_remove || settings.bass_mono {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        PreparedUtility::new(settings)
            .map(|prepared| prepared.matrix())
            .map_err(|_| DeviceContractError::MalformedState)
    }

    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let mut settings = decode_state(state)?;
        // Stateful mute belongs to the processor, so its input matrix remains
        // compatible across mute handoffs and no audible old tail is detached.
        if settings.dc_remove || settings.bass_mono {
            settings.mute = false;
        }
        PreparedUtility::new(settings)
            .map(|prepared| prepared.matrix())
            .map_err(|_| DeviceContractError::MalformedState)
    }

    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        Self::prepare_owned_filter(state, sample_rate)
            .map(|value| value.map(|value| Box::new(value) as Box<dyn PreparedNativeProcessor>))
    }
}

/// The concrete fixed-inline Utility filter preparation. Its private fields
/// cannot contain caller-provided processor/storage implementations. Native
/// graph construction can retain this exact type's allocation declaration
/// before erasing the processor trait; this is not a generic provider receipt.
pub struct PreparedUtilityFilterRuntime(
    pub(super) crate::utility_filters::PreparedUtilityFilters,
    pub(super) Option<UtilityAutomation>,
);
impl PreparedUtilityFilterRuntime {
    pub(super) fn check_inline_ownership(&self) {
        let Self(filters, automation) = self;
        if let Some(automation) = automation {
            automation.check_inline();
        }
        filters.check_inline();
    }
}

impl UtilityDefinition {
    /// A source-local matrix can switch to its modulated provider only when
    /// authored settings and the route cannot activate either filter.
    pub fn supports_source_matrix_modulation(
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> bool {
        decode_state(state).is_ok_and(|settings| !settings.dc_remove && !settings.bass_mono)
            && parameter != parameter::DC_REMOVE
            && parameter != parameter::BASS_MONO
            && runtime_parameter_automation_supported(parameter)
    }

    /// Closed fixed-inline factory for the full Utility law. Automation always
    /// retains filters, including dormant histories, with an identity host matrix.
    pub fn prepare_owned_automated_filter(
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        Self::prepare_automated_with_mask(state, sample_rate, 0x3fff)
    }
    /// Exact admitted target inventory allows dormant filters to skip work only
    /// when this revision cannot enable them. The concrete owner stays inline.
    pub fn prepare_owned_automated_filter_for_parameters(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        let mut mask = 0u16;
        for parameter in parameters {
            let index = parameter
                .value()
                .checked_sub(1)
                .filter(|index| *index < 14)
                .ok_or(DeviceContractError::UnknownParameter)?;
            mask |= 1u16
                .checked_shl(index)
                .ok_or(DeviceContractError::UnknownParameter)?;
        }
        Self::prepare_automated_with_mask(state, sample_rate, mask)
    }
    pub(super) fn prepare_automated_with_mask(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: u16,
    ) -> Result<PreparedUtilityFilterRuntime, DeviceContractError> {
        state.require_asset_free_processing()?;
        for (index, binding) in BINDINGS.iter().enumerate() {
            if parameters & (1u16 << index) != 0 {
                binding.semantics.validate(binding.descriptor)?;
                if !runtime_parameter_automation_supported(binding.descriptor.id) {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
            }
        }
        let base = UtilityControls::decode(state)?;
        let mut automation = UtilityAutomation {
            controls: crate::parameter_binding::Controls::new(
                base,
                std::array::from_fn(|index| parameters & (1u16 << index) != 0),
            ),
            effective: base.settings(),
            matrix: StereoMatrix::identity(),
            pre_gain: StereoMatrix::identity(),
            post_gain: StereoMatrix::identity(),
            width_ramp: None,
        };
        let (matrix, filters) = automation.configuration(None, 0, sample_rate, None)?;
        let filters = filters.ok_or(DeviceContractError::InvalidDescriptor)?;
        let value = PreparedUtilityFilterRuntime(
            filters,
            Some(UtilityAutomation {
                matrix,
                ..automation
            }),
        );
        value.check_inline_ownership();
        Ok(value)
    }
    /// Prepare the same registered filter through its concrete constructor.
    /// Memoryless Utility settings return None, exactly as the device registry.
    pub fn prepare_owned_filter(
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<PreparedUtilityFilterRuntime>, DeviceContractError> {
        state.require_asset_free_processing()?;
        let settings = decode_state(state)?;
        if !settings.dc_remove && !settings.bass_mono {
            return Ok(None);
        }
        let prepared = crate::utility_filters::PreparedUtilityFilters::new(
            sample_rate,
            settings.dc_remove,
            settings.bass_mono,
            f64::from(settings.bass_mono_cutoff_hz),
            settings.bass_mono_audition,
        )
        .map_err(|_| DeviceContractError::MalformedState)?
        .with_output_mute(settings.mute);
        let value = PreparedUtilityFilterRuntime(prepared, None);
        value.check_inline_ownership();
        Ok(Some(value))
    }
}

impl PreparedNativeProcessor for PreparedUtilityFilterRuntime {
    fn supports_smoothed_bypass(&self) -> bool {
        // Static and automated filters advance private history during the
        // power transition so a reversal retains the original response.
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        self.1
            .filter(|automation| automation.supports(parameter))
            .and_then(|_| DESCRIPTORS.iter().find(|d| d.id == parameter).copied())
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.1
            .is_some_and(|automation| automation.supports(parameter))
    }
    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        if !self.supports_parameter_automation(parameter) {
            return None;
        }
        let binding = crate::parameter_binding::binding(&BINDINGS, parameter).ok()?;
        Some(hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            ..binding.semantics
        })
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<crate::utility_filters::UtilityFilterProcessor>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.0.algorithmic_latency_frames())
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedUtilityFilterRuntime {
    type Runtime = crate::utility_filters::UtilityFilterProcessor;
    fn create_inline_runtime(&self) -> Self::Runtime {
        crate::utility_filters::UtilityFilterProcessor::with_automation(self.0, self.1)
    }
}

/// Immutable authored controls plus the last effective matrix. No state codec,
/// allocation or metadata enumeration occurs during processing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UtilityAutomation {
    pub(super) controls: crate::parameter_binding::Controls<UtilityControls, 14>,
    pub(crate) effective: UtilitySettings,
    pub(crate) matrix: StereoMatrix,
    pub(super) pre_gain: StereoMatrix,
    pub(super) post_gain: StereoMatrix,
    pub(super) width_ramp: Option<UtilityRamp>,
}

/// Five-millisecond ramp for a direct Utility control retarget, backed by
/// [`hex_dsp::LinearRamp`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct UtilityRamp {
    pub(super) ramp: hex_dsp::LinearRamp,
}
impl UtilityRamp {
    pub(crate) fn new(from: f32, target: f32, sample_rate: u32) -> Option<Self> {
        if from == target {
            return None;
        }
        let total = hex_dsp::ramp_frames_for_ms(sample_rate, 5.0)
            .unwrap_or(1)
            .max(1);
        let mut ramp = hex_dsp::LinearRamp::settled(f64::from(from));
        ramp.retarget(f64::from(target), total);
        Some(Self { ramp })
    }
    pub(crate) fn check_inline(&self) {
        let Self { ramp } = self;
        let _: &hex_dsp::LinearRamp = ramp;
    }
    pub(crate) fn advance(&mut self) -> f32 {
        self.ramp.advance();
        self.value()
    }
    pub(crate) fn finished(self) -> bool {
        self.ramp.is_settled()
    }
    pub(crate) fn value(self) -> f32 {
        self.ramp.value() as f32
    }
}
impl UtilityAutomation {
    pub(crate) fn check_inline(&self) {
        let Self {
            controls,
            effective,
            matrix,
            pre_gain,
            post_gain,
            width_ramp,
        } = self;
        controls.check_inline(UtilityControls::check_inline);
        effective.check_inline();
        let _: (&StereoMatrix, &StereoMatrix, &StereoMatrix) = (matrix, pre_gain, post_gain);
        if let Some(ramp) = width_ramp {
            ramp.check_inline();
        }
    }
    pub(crate) fn inherit(&mut self, previous: &Self, sample_rate: u32) {
        let target_width = self.controls.base().settings().width;
        let previous_width = previous.effective.width;
        self.width_ramp = if self.controls.admitted(2) {
            None
        } else if !previous.controls.admitted(2)
            && target_width == previous.controls.base().settings().width
        {
            previous.width_ramp
        } else {
            UtilityRamp::new(previous_width, target_width, sample_rate)
        };
        self.controls.inherit(&previous.controls);
        self.matrix = previous.matrix;
        self.pre_gain = previous.pre_gain;
        self.post_gain = previous.post_gain;
        self.effective = previous.effective;
    }
    pub(crate) fn manual_mute_target(&self) -> Option<bool> {
        (!self.supports(parameter::MUTE)).then(|| self.controls.base().settings().mute)
    }
    pub(crate) fn supports(&self, parameter: ParameterId) -> bool {
        runtime_parameter_automation_supported(parameter)
            && self.controls.supports(&BINDINGS, parameter)
    }
    pub(crate) fn may_advance_dc(&self) -> bool {
        self.controls.base().settings.dc_remove || self.controls.admitted(10)
    }
    pub(crate) fn may_advance_bass(&self) -> bool {
        self.controls.base().settings.bass_mono || self.controls.admitted(11)
    }
    pub(crate) fn resolve(
        &self,
        values: &dyn hex_lattice::NativeParameterAutomation,
    ) -> Result<crate::parameter_binding::Resolved<14>, DeviceContractError> {
        self.controls.resolve(&BINDINGS, values)
    }
    pub(crate) fn configuration(
        &mut self,
        values: Option<(
            &dyn hex_lattice::NativeParameterAutomation,
            &mut crate::parameter_binding::Resolved<14>,
        )>,
        offset: usize,
        rate: u32,
        previous: Option<&crate::utility_filters::PreparedUtilityFilters>,
    ) -> Result<
        (
            StereoMatrix,
            Option<crate::utility_filters::PreparedUtilityFilters>,
        ),
        DeviceContractError,
    > {
        let reset = values.is_none();
        let pending = if let Some((values, resolved)) = values {
            self.controls.sample(&BINDINGS, resolved, values, offset)?
        } else {
            None
        };
        if !reset && pending.is_none() && previous.is_some() && self.width_ramp.is_none() {
            return Ok((self.matrix, None));
        }
        let mut settings = if reset {
            self.controls.base()
        } else {
            pending
                .as_ref()
                .map_or_else(|| self.controls.effective(), |next| next.effective())
        }
        .settings();
        let mut width_ramp = if reset { None } else { self.width_ramp };
        if let Some(ramp) = &mut width_ramp {
            settings.width = ramp.advance();
            if ramp.finished() {
                width_ramp = None;
            }
        }
        let filters = if previous.is_some()
            && settings.dc_remove == self.effective.dc_remove
            && settings.bass_mono == self.effective.bass_mono
            && settings.bass_mono_cutoff_hz == self.effective.bass_mono_cutoff_hz
            && settings.bass_mono_audition == self.effective.bass_mono_audition
            && settings.mute == self.effective.mute
        {
            None
        } else {
            Some(
                crate::utility_filters::PreparedUtilityFilters::new(
                    rate,
                    settings.dc_remove,
                    settings.bass_mono,
                    f64::from(settings.bass_mono_cutoff_hz),
                    settings.bass_mono_audition,
                )
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
                .with_output_mute(settings.mute),
            )
        };
        // Reuse only stages whose controls are unchanged. The gain stage stays
        // between the exact original prefix and balance compositions, retaining
        // every f32 matrix rounding/validation operation of the static law.
        let prefix_changed = previous.is_none()
            || settings.channel_mode != self.effective.channel_mode
            || settings.invert_left != self.effective.invert_left
            || settings.invert_right != self.effective.invert_right
            || settings.width != self.effective.width
            || settings.mid_side_balance != self.effective.mid_side_balance
            || settings.mono != self.effective.mono;
        let balance_changed = previous.is_none() || settings.balance != self.effective.balance;
        let pre_gain = if prefix_changed {
            pre_gain_matrix(settings).map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.pre_gain
        };
        let post_gain = if balance_changed {
            balance_matrix(settings.balance)
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.post_gain
        };
        let matrix = if prefix_changed || balance_changed || settings.gain != self.effective.gain {
            pre_gain
                .then(
                    scalar_matrix(settings.gain)
                        .map_err(|_| DeviceContractError::InvalidParameterValue)?,
                )
                .and_then(|m| m.then(post_gain))
                .map_err(|_| DeviceContractError::InvalidParameterValue)?
        } else {
            self.matrix
        };
        if reset {
            self.controls.reset();
        } else if let Some(pending) = pending {
            self.controls.install(pending);
        }
        self.effective = settings;
        self.matrix = matrix;
        self.pre_gain = pre_gain;
        self.post_gain = post_gain;
        self.width_ramp = width_ramp;
        Ok((matrix, filters))
    }
}
