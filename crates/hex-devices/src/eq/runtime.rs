use super::*;
use hex_lattice::{NativeInstrumentClock, NativeProcessorAudition};

mod automation;

const CADENCE: u64 = 64;

fn scaled_cadence(sample_rate: u32) -> u64 {
    ((u64::from(sample_rate) * CADENCE) / 48_000).clamp(16, 256)
}

#[derive(Clone, Copy)]
struct CompiledLane {
    node: usize,
    parameter: NodeParameter,
    start: usize,
    end: usize,
}
#[derive(Clone)]
struct Configuration {
    nodes: Vec<Node>,
    authored_stages: Vec<BandStages>,
    lanes: Vec<CompiledLane>,
    points: Vec<AutomationPoint>,
    node_lane_ranges: [(u8, u8); MAX_NODES],
    automated_nodes: u64,
    granted_nodes: u64,
    mode: u8,
    gain: f64,
    gain_alpha: f64,
    audition_step: f64,
    cadence: u64,
    parameters: Vec<automation::Grant>,
}
impl Configuration {
    fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.authored_stages.capacity() * std::mem::size_of::<BandStages>()
            + self.lanes.capacity() * std::mem::size_of::<CompiledLane>()
            + self.points.capacity() * std::mem::size_of::<AutomationPoint>()
            + self.parameters.capacity() * std::mem::size_of::<automation::Grant>()
    }
    fn node_at(&self, index: usize, beat: Option<u64>) -> Node {
        let mut node = self.nodes[index];
        if let Some(beat) = beat {
            let (start, end) = self.node_lane_ranges[index];
            for lane in &self.lanes[usize::from(start)..usize::from(end)] {
                let points = &self.points[lane.start..lane.end];
                let split = points.partition_point(|point| point.beat <= beat);
                let normalized = if split == 0 {
                    points[0].normalized
                } else if split >= points.len() {
                    points[points.len() - 1].normalized
                } else {
                    let prior = points[split - 1];
                    let next = points[split];
                    if lane.parameter.continuous() && beat > prior.beat {
                        prior.normalized
                            + (next.normalized - prior.normalized)
                                * ((beat - prior.beat) as f64 / (next.beat - prior.beat) as f64)
                                    as f32
                    } else {
                        prior.normalized
                    }
                };
                // All field/value pairs were validated on control. This match has
                // no fallible callback path and preserves legacy S metadata.
                match lane.parameter.value(normalized) {
                    ParameterValue::Bool(v) => node.enabled = v,
                    ParameterValue::Choice(v) => match lane.parameter {
                        NodeParameter::Kind => {
                            node.kind = match v {
                                0 => NodeKind::Bell,
                                1 => NodeKind::LowShelf,
                                2 => NodeKind::HighShelf,
                                3 => NodeKind::LowCut,
                                4 => NodeKind::HighCut,
                                _ => NodeKind::Notch,
                            }
                        }
                        NodeParameter::Slope => node.slope = v as u8,
                        NodeParameter::Bank => node.bank = v as u8,
                        _ => {}
                    },
                    ParameterValue::Float(v) => match lane.parameter {
                        NodeParameter::Frequency => node.frequency_hz = v,
                        NodeParameter::Gain => node.gain_db = v,
                        NodeParameter::Q => node.q = v,
                        _ => {}
                    },
                }
            }
        }
        node
    }
}

/// Validated control-side configuration shared immutably by its runtime pair.
pub struct PreparedEq {
    sample_rate: u32,
    configuration: Arc<Configuration>,
}
impl PreparedEq {
    pub(super) fn source_factory_bytes_bound() -> usize {
        // Source effects grant no host parameter slots. The retained historical
        // lanes and their point and node arrays have canonical state bounds.
        crate::source_effect_arc_bytes::<Self>() + Self::configuration_bytes_bound(0)
    }
    fn configuration_bytes_bound(parameters: usize) -> usize {
        crate::source_effect_arc_bytes::<Configuration>()
            + MAX_NODES * (std::mem::size_of::<Node>() + std::mem::size_of::<BandStages>())
            + MAX_LANES * std::mem::size_of::<CompiledLane>()
            + MAX_POINTS * std::mem::size_of::<AutomationPoint>()
            + parameters * std::mem::size_of::<automation::Grant>()
    }
    /// Bounds the factory's private backing for all supported saved states and grants.
    pub fn closed_configuration_bytes_bound() -> usize {
        Self::configuration_bytes_bound(hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE)
    }
    /// Private immutable backing charged once to the constructor that created this factory.
    pub fn configuration_storage_bytes(&self) -> usize {
        crate::source_effect_arc_bytes::<Configuration>() + self.configuration.heap_bytes()
    }
    pub(super) fn new(settings: EqState, sample_rate: u32) -> Result<Self, DeviceContractError> {
        Self::prepare(settings, sample_rate, &[])
    }
    pub(super) fn prepare(
        settings: EqState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        let parameters = automation::Grant::prepare(&settings, parameters)?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut stages = Vec::with_capacity(settings.nodes.len());
        for node in &settings.nodes {
            let coefficients = design(*node, sample_rate);
            let enabled_parameter = node.id.parameter(NodeParameter::Enabled)?;
            if (node.enabled
                || parameters
                    .iter()
                    .any(|grant| grant.descriptor.id == enabled_parameter)
                || settings
                    .lanes
                    .iter()
                    .any(|l| l.node == node.id && l.parameter == NodeParameter::Enabled))
                && !coefficients
                    .iter()
                    .all(|c| [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()))
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            stages.push(if node.enabled {
                coefficients
            } else {
                [Coefficients::IDENTITY; 4]
            });
        }
        let mut lanes = Vec::with_capacity(settings.lanes.len());
        let mut points = Vec::with_capacity(settings.lanes.iter().map(|l| l.points.len()).sum());
        for lane in &settings.lanes {
            let node = settings
                .nodes
                .iter()
                .position(|n| n.id == lane.node)
                .ok_or(DeviceContractError::UnknownParameter)?;
            let start = points.len();
            points.extend(&lane.points);
            lanes.push(CompiledLane {
                node,
                parameter: lane.parameter,
                start,
                end: points.len(),
            });
        }
        lanes.sort_by_key(|lane| lane.node);
        let mut node_lane_ranges = [(0_u8, 0_u8); MAX_NODES];
        let mut automated_nodes = 0_u64;
        let mut cursor = 0_usize;
        while cursor < lanes.len() {
            let node_index = lanes[cursor].node;
            let start = cursor;
            while cursor < lanes.len() && lanes[cursor].node == node_index {
                cursor += 1;
            }
            node_lane_ranges[node_index] = (start as u8, cursor as u8);
            automated_nodes |= 1_u64 << node_index;
        }
        let mut granted_nodes = 0_u64;
        for grant in &parameters {
            if let Some((index, _)) = grant.node_slot() {
                granted_nodes |= 1_u64 << index;
            }
        }
        // Rate-scaled exponential gain smoothing matching the 48 kHz 1/64 pole
        // bit-exact at 48 kHz and keeping constant time (~1.32 ms) across rates.
        let gain_alpha = 1.0 - (1.0 - 0.015625_f64).powf(48_000.0 / f64::from(sample_rate));
        let audition_step = 1.0 / f64::from(mode_fade_frames(sample_rate));
        let cadence = scaled_cadence(sample_rate);
        Ok(Self {
            sample_rate,
            configuration: Arc::new(Configuration {
                nodes: settings.nodes,
                authored_stages: stages,
                lanes,
                points,
                node_lane_ranges,
                automated_nodes,
                granted_nodes,
                mode: settings.mode,
                gain: f64::from(settings.output_gain),
                gain_alpha,
                audition_step,
                cadence,
                parameters,
            }),
        })
    }
}
impl hex_lattice::PreparedOwnedProcessorRuntime for PreparedEq {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.configuration
            .parameters
            .binary_search_by_key(&id, |grant| grant.descriptor.id)
            .is_ok()
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        (id == OUTPUT_GAIN && self.supports_parameter_automation(id))
            .then_some(OUTPUT_GAIN_SEMANTICS)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.configuration
            .parameters
            .binary_search_by_key(&id, |grant| grant.descriptor.id)
            .ok()
            .map(|index| self.configuration.parameters[index].descriptor)
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn supports_power_off(&self) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        4
    }
    fn declared_pair_bytes(&self) -> usize {
        EQ_RUNTIME_BYTES
            + 2 * self.configuration.nodes.len() * std::mem::size_of::<RuntimeBand>()
            + 2 * MAX_NODES * std::mem::size_of::<OutgoingBand>()
    }
    fn create_runtime(
        &self,
    ) -> Result<Box<dyn hex_lattice::OwnedProcessorRuntime>, DeviceContractError> {
        let bands: Box<[RuntimeBand]> = self
            .configuration
            .authored_stages
            .iter()
            .zip(&self.configuration.nodes)
            .map(|(&stages, node)| RuntimeBand::new(stages, node.kind))
            .collect();
        let rollback_bands = bands.clone();
        let effective_enabled = std::array::from_fn(|i| {
            self.configuration
                .nodes
                .get(i)
                .is_some_and(|node| node.enabled)
        });
        let effective_banks =
            std::array::from_fn(|i| self.configuration.nodes.get(i).map_or(0, |n| n.bank));
        let runtime = EqRuntime {
            sample_rate: self.sample_rate,
            configuration: Arc::clone(&self.configuration),
            bands,
            last_frame: None,
            fallback_frame: 0,
            refresh_target: false,
            audition: None,
            audition_request: None,
            audition_mix: 0.0,
            audition_history: [[[0.0; 2]; 4]; 2],
            gain: self.configuration.gain,
            mode: self.configuration.mode,
            effective_enabled,
            pristine: true,
            effective_banks,
            mode_fade: ModeFade::new(),
            rollback: EqRollback {
                open: false,
                committed_quiescent: true,
                dirty_bands: 0,
                dirty_flags: 0,
                bands: rollback_bands,
                last_frame: None,
                fallback_frame: 0,
                refresh_target: false,
                audition: None,
                audition_request: None,
                audition_mix: 0.0,
                audition_history: [[[0.0; 2]; 4]; 2],
                gain: self.configuration.gain,
                mode: self.configuration.mode,
                effective_enabled,
                pristine: true,
                effective_banks,
                mode_fade: ModeFade::new(),
            },
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn frequency_response_at_hz(&self, frequency: f32) -> Option<[f32; 2]> {
        if !frequency.is_finite() || frequency <= 0.0 || frequency >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        Some(std::array::from_fn(|channel| {
            let bank = bank(self.configuration.mode, channel);
            let mut magnitude = self.configuration.gain;
            for (node, stages) in self
                .configuration
                .nodes
                .iter()
                .zip(&self.configuration.authored_stages)
            {
                if node.enabled && node.bank == bank {
                    for stage in stages {
                        magnitude *= response_magnitude(
                            *stage,
                            f64::from(frequency),
                            f64::from(self.sample_rate),
                        );
                    }
                }
            }
            finite_f32(magnitude)
        }))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}

mod process;
mod traits;
mod transitions;
use process::{bank, filter, stages_used};
use traits::design;
use transitions::{
    ModeFade, OutgoingBand, Routing, RuntimeBand, coefficient_smooth_frames, mode_fade_frames,
};

const EQ_DIRTY_SCALARS: u8 = 1 << 0;
const EQ_DIRTY_AUDITION: u8 = 1 << 1;
const EQ_DIRTY_MODE_FADE: u8 = 1 << 2;

/// Preallocated bounded dirty-range rollback journal for `EqRuntime`.
struct EqRollback {
    open: bool,
    committed_quiescent: bool,
    dirty_bands: u64,
    dirty_flags: u8,
    bands: Box<[RuntimeBand]>,
    last_frame: Option<(u64, u64)>,
    fallback_frame: u64,
    refresh_target: bool,
    audition: Option<ProcessorAuditionSelection>,
    audition_request: Option<ProcessorAuditionSelection>,
    audition_mix: f64,
    audition_history: [[[f64; 2]; 4]; 2],
    gain: f64,
    mode: u8,
    effective_enabled: [bool; MAX_NODES],
    pristine: bool,
    effective_banks: [u8; MAX_NODES],
    mode_fade: ModeFade,
}

pub const EQ_RUNTIME_BYTES: usize = std::mem::size_of::<EqRuntime>();
/// Bounded sparse EQ histories, initialized on control and reused in playback.
pub struct EqRuntime {
    sample_rate: u32,
    configuration: Arc<Configuration>,
    bands: Box<[RuntimeBand]>,
    // Arrangement position for authored target refresh, then the independent
    // performance position for DSP continuity (legacy clocks use Arrangement).
    last_frame: Option<(u64, u64)>,
    fallback_frame: u64,
    refresh_target: bool,
    audition: Option<ProcessorAuditionSelection>,
    audition_request: Option<ProcessorAuditionSelection>,
    audition_mix: f64,
    audition_history: [[[f64; 2]; 4]; 2],
    gain: f64,
    mode: u8,
    effective_enabled: [bool; MAX_NODES],
    pristine: bool,
    effective_banks: [u8; MAX_NODES],
    mode_fade: ModeFade,
    rollback: EqRollback,
}

#[cfg(test)]
mod smoothing_tests;
