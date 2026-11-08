//! Sparse, bounded parametric EQ with stable authored node identities.
//! Editing and the codec run on control; the runtime owns bounded, control-allocated DSP histories.
use super::*;
mod runtime;
mod state;
pub use runtime::EQ_RUNTIME_BYTES;
pub use state::{AutomationLane, AutomationPoint, EqState, Node, NodeId, NodeKind, NodeParameter};

pub const EQ_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.eq");
pub const MAX_NODES: usize = 64;
pub const MAX_LANES: usize = 64;
pub const MAX_POINTS: usize = 160;
pub const EQ_STATE_VERSION: u32 = 1;
pub const MODE: ParameterId = ParameterId::from_static(201);
pub const OUTPUT_GAIN: ParameterId = ParameterId::from_static(202);
const KINDS: &[&str] = &[
    "bell",
    "low shelf",
    "high shelf",
    "low cut",
    "high cut",
    "notch",
];
const SLOPES: &[&str] = &["12 db/oct", "24 db/oct", "36 db/oct", "48 db/oct"];
const BANKS: &[&str] = &["stereo", "left", "right", "mid", "side"];

pub struct EqDefinition;
pub static EQ_DEFINITION: EqDefinition = EqDefinition;

impl NativeDeviceMetadata for EqDefinition {
    fn type_id(&self) -> DeviceTypeId {
        EQ_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("eq")
    }
    fn state_version(&self) -> u32 {
        EQ_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS[200..]
    }
    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        let settings = EqState::decode(state)?;
        let mut result = self.descriptors().to_vec();
        for node in settings.nodes {
            for field in NodeParameter::ALL {
                result.push(descriptor(node.id, field)?);
            }
        }
        Ok(result)
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        use hex_lattice_contract::{
            ParameterAccess as A, ParameterAutomation as E, ParameterChangeImpact as I,
            ParameterModulation as M, ParameterNormalization as N, ParameterSemantics,
            ParameterSmoothing as S, ParameterStepping as T,
        };
        let settings = EqState::decode(state)?;
        let (normalization, stepping, automation, smoothing) = if id == MODE {
            (N::Discrete, T::Choice, E::Unsupported, S::None)
        } else if id == OUTPUT_GAIN {
            (
                N::Linear,
                T::Continuous,
                E::DeviceArrangementEnvelope,
                S::DeviceManaged,
            )
        } else {
            let (node, field) = NodeId::from_parameter(id)?;
            let node = settings.node(node)?;
            let automation = if node.legacy_shelf
                && matches!(
                    field,
                    NodeParameter::Kind | NodeParameter::Gain | NodeParameter::Q
                ) {
                E::Unsupported
            } else {
                E::DeviceArrangementEnvelope
            };
            let (normalization, stepping) = match field {
                NodeParameter::Frequency | NodeParameter::Q => {
                    (N::PositiveLogarithmic, T::Continuous)
                }
                NodeParameter::Gain => (N::Linear, T::Continuous),
                NodeParameter::Enabled => (N::Discrete, T::Boolean),
                _ => (N::Discrete, T::Choice),
            };
            (normalization, stepping, automation, S::DeviceManaged)
        };
        Ok(Some(ParameterSemantics {
            access: A::ReadWrite,
            normalization,
            stepping,
            automation,
            modulation: M::Unsupported,
            smoothing,
            change_impact: if id == MODE {
                I::Structural
            } else {
                I::ValueOnly
            },
        }))
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        EqState::default().encode()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        EqState::decode(state)?.encode()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = EqState::decode(state)?;
        if id == MODE {
            return Ok(ParameterValue::Choice(settings.mode as u32));
        }
        if id == OUTPUT_GAIN {
            return Ok(ParameterValue::Float(settings.output_gain));
        }
        let (node, field) = NodeId::from_parameter(id)?;
        Ok(settings.node(node)?.value(field))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = EqState::decode(state)?;
        if id == MODE {
            DESCRIPTORS[200].validate(value)?;
            let ParameterValue::Choice(v) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.mode = v as u8;
        } else if id == OUTPUT_GAIN {
            DESCRIPTORS[201].validate(value)?;
            let ParameterValue::Float(v) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.output_gain = v;
        } else {
            let (node, field) = NodeId::from_parameter(id)?;
            descriptor(node, field)?.validate(value)?;
            settings.node_mut(node)?.set(field, value)?;
        }
        settings.encode()
    }
    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = EqState::decode(state)?;
        if id == MODE || id == OUTPUT_GAIN {
            return Ok(true);
        }
        let (node, field) = NodeId::from_parameter(id)?;
        let node = settings.node(node)?;
        Ok(match field {
            NodeParameter::Gain => matches!(
                node.kind,
                NodeKind::Bell | NodeKind::LowShelf | NodeKind::HighShelf
            ),
            NodeParameter::Slope => matches!(node.kind, NodeKind::LowCut | NodeKind::HighCut),
            _ => true,
        })
    }
}
impl EqDefinition {
    /// Control-only exact-grant component preparation. The returned processor
    /// uses the existing native checkpoint/runtime custody; no resource ownership
    /// transfers. Standard registry admission uses the same exact-grant component.
    /// Known node fields and output gain are accepted; mode and legacy-shelf
    /// kind/gain/q remain unsupported. Grants are sorted, unique and bounded by
    /// the neutral descriptor limit. No authored state or curves are rewritten.
    pub fn prepare_automation_component(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Arc<dyn PreparedNativeProcessor>, DeviceContractError> {
        state.require_asset_free_processing()?;
        Ok(Arc::new(runtime::PreparedEq::automated(
            EqState::decode(state)?,
            sample_rate,
            parameters,
        )?))
    }
}
impl NativeDeviceDefinition for EqDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let settings = EqState::decode(state)?;
        if parameter == OUTPUT_GAIN {
            return Ok(true);
        }
        let Ok((id, field)) = NodeId::from_parameter(parameter) else {
            return Ok(false);
        };
        let Ok(node) = settings.node(id) else {
            return Ok(false);
        };
        Ok(!node.legacy_shelf
            || !matches!(
                field,
                NodeParameter::Kind | NodeParameter::Gain | NodeParameter::Q
            ))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(hex_lattice::PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(hex_lattice::PreparedProcessorFactory::Legacy(
                Self::prepare_automation_component(state, context.sample_rate, parameters)?,
            )),
        })
    }

    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = EqState::decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        Ok(Some(Box::new(runtime::PreparedEq::new(
            EqState::decode(state)?,
            sample_rate,
        )?)))
    }
}

pub fn descriptor(
    node: NodeId,
    field: NodeParameter,
) -> Result<ParameterDescriptor, DeviceContractError> {
    let (label, kind, default, presentation) = match field {
        NodeParameter::Enabled => (
            "enabled",
            ParameterKind::Bool,
            ParameterValue::Bool(true),
            ParameterPresentation::Linear,
        ),
        NodeParameter::Kind => (
            "shape",
            ParameterKind::Choice { labels: KINDS },
            ParameterValue::Choice(0),
            ParameterPresentation::Linear,
        ),
        NodeParameter::Frequency => (
            "frequency",
            ParameterKind::Float {
                min: MIN_HZ,
                max: MAX_HZ,
                unit: Some("hz"),
            },
            ParameterValue::Float(1000.0),
            ParameterPresentation::Logarithmic,
        ),
        NodeParameter::Gain => (
            "gain",
            ParameterKind::Float {
                min: -MAX_GAIN_DB,
                max: MAX_GAIN_DB,
                unit: Some("db"),
            },
            ParameterValue::Float(0.0),
            ParameterPresentation::Linear,
        ),
        NodeParameter::Q => (
            "q",
            ParameterKind::Float {
                min: MIN_Q,
                max: MAX_Q,
                unit: None,
            },
            ParameterValue::Float(std::f32::consts::FRAC_1_SQRT_2),
            ParameterPresentation::Logarithmic,
        ),
        NodeParameter::Slope => (
            "slope",
            ParameterKind::Choice { labels: SLOPES },
            ParameterValue::Choice(0),
            ParameterPresentation::Linear,
        ),
        NodeParameter::Bank => (
            "channel",
            ParameterKind::Choice { labels: BANKS },
            ParameterValue::Choice(0),
            ParameterPresentation::Linear,
        ),
    };
    Ok(ParameterDescriptor {
        id: node.parameter(field)?,
        label,
        kind,
        default,
        presentation,
    })
}

#[cfg(test)]
mod mode_fade_tests;
#[cfg(test)]
mod steady_state_digest_tests;
#[cfg(test)]
mod tests;
