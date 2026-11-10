use super::*;
use hex_lattice::{NativeInstrumentClock, NativeProcessorAudition};

mod automation;

const CADENCE: u64 = 64;
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
    lanes: Vec<CompiledLane>,
    points: Vec<AutomationPoint>,
    mode: u8,
    gain: f64,
    parameters: Vec<automation::Grant>,
}
impl Configuration {
    fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.lanes.capacity() * std::mem::size_of::<CompiledLane>()
            + self.points.capacity() * std::mem::size_of::<AutomationPoint>()
            + self.parameters.capacity() * std::mem::size_of::<automation::Grant>()
    }
    fn node_at(&self, index: usize, beat: Option<u64>) -> Node {
        let mut node = self.nodes[index];
        if let Some(beat) = beat {
            for lane in self.lanes.iter().filter(|l| l.node == index) {
                let points = &self.points[lane.start..lane.end];
                let mut prior = points[0];
                let mut normalized = prior.normalized;
                for next in points {
                    if next.beat > beat {
                        if lane.parameter.continuous() && beat > prior.beat {
                            normalized = prior.normalized
                                + (next.normalized - prior.normalized)
                                    * ((beat - prior.beat) as f64 / (next.beat - prior.beat) as f64)
                                        as f32;
                        }
                        break;
                    }
                    prior = *next;
                    normalized = prior.normalized;
                }
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

pub(super) struct PreparedEq {
    sample_rate: u32,
    configuration: Arc<Configuration>,
    stages: Vec<BandStages>,
}
impl PreparedEq {
    pub(super) fn source_factory_bytes_bound() -> usize {
        // Source effects grant no host parameter slots. The retained historical
        // lanes and their point and node arrays have canonical state bounds.
        crate::source_effect_arc_bytes::<Self>()
            + crate::source_effect_arc_bytes::<Configuration>()
            + MAX_NODES * (std::mem::size_of::<Node>() + std::mem::size_of::<BandStages>())
            + MAX_LANES * std::mem::size_of::<CompiledLane>()
            + MAX_POINTS * std::mem::size_of::<AutomationPoint>()
    }
    pub(super) fn new(settings: EqState, sample_rate: u32) -> Result<Self, DeviceContractError> {
        Self::prepare(settings, sample_rate, &[])
    }
    fn prepare(
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
        Ok(Self {
            sample_rate,
            configuration: Arc::new(Configuration {
                nodes: settings.nodes,
                lanes,
                points,
                mode: settings.mode,
                gain: f64::from(settings.output_gain),
                parameters,
            }),
            stages,
        })
    }
}
impl PreparedNativeProcessor for PreparedEq {
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
            .iter()
            .find(|grant| grant.descriptor.id == id)
            .map(|grant| grant.descriptor)
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        4
    }
    fn declared_runtime_bytes(&self) -> usize {
        EQ_RUNTIME_BYTES
            + self.configuration.nodes.len() * std::mem::size_of::<RuntimeBand>()
            + MAX_NODES * std::mem::size_of::<OutgoingBand>()
            + std::mem::size_of::<Configuration>()
            + 2 * std::mem::size_of::<usize>()
            + self.configuration.heap_bytes()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        let mut bands =
            vec![RuntimeBand::default(); self.configuration.nodes.len()].into_boxed_slice();
        for (index, coefficients) in self.stages.iter().enumerate() {
            bands[index].current = *coefficients;
            bands[index].target = *coefficients;
            bands[index].stages_used = stages_used(coefficients);
            bands[index].shape = self.configuration.nodes[index].kind as u8;
        }
        Box::new(EqRuntime {
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
            effective_enabled: std::array::from_fn(|i| {
                self.configuration
                    .nodes
                    .get(i)
                    .is_some_and(|node| node.enabled)
            }),
            pristine: true,
            effective_banks: std::array::from_fn(|i| {
                self.configuration.nodes.get(i).map_or(0, |n| n.bank)
            }),
            mode_fade: ModeFade::new(),
        })
    }
    fn frequency_response_at_hz(&self, frequency: f32) -> Option<[f32; 2]> {
        if !frequency.is_finite() || frequency <= 0.0 || frequency >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        Some(std::array::from_fn(|channel| {
            let bank = bank(self.configuration.mode, channel);
            let mut magnitude = self.configuration.gain;
            for (node, stages) in self.configuration.nodes.iter().zip(&self.stages) {
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
#[derive(Clone, Copy)]
struct RuntimeBand {
    current: BandStages,
    target: BandStages,
    history: [[[f64; 2]; 4]; 2],
    remaining: u16,
    shape: u8,
    // A stage that has processed a non-identity filter remains in the bound
    // until its histories are reset, including through coefficient ramps.
    stages_used: u8,
}
impl Default for RuntimeBand {
    fn default() -> Self {
        Self {
            current: [Coefficients::IDENTITY; 4],
            target: [Coefficients::IDENTITY; 4],
            history: [[[0.0; 2]; 4]; 2],
            remaining: 0,
            shape: 0,
            stages_used: 1,
        }
    }
}
impl RuntimeBand {
    #[inline]
    fn filter_channel(&mut self, channel: usize, input: f64) -> f64 {
        let used = usize::from(self.stages_used);
        let history = &mut self.history[channel];
        let output = filter(input, &self.current[..used], history);
        // Even identity arithmetic can change history on a non-finite input.
        // Preserve that state until reset.
        if used < self.current.len() && !output.is_finite() {
            self.stages_used = self.current.len() as u8;
            filter(output, &self.current[used..], &mut history[used..])
        } else {
            output
        }
    }
}
/// A five-millisecond transition independent of device sample rate.
fn coefficient_smooth_frames(sample_rate: u32) -> u16 {
    u16::try_from(sample_rate.div_ceil(200).max(1)).unwrap_or(u16::MAX)
}
/// Frames for a channel-mode crossfade: 256 at 48 kHz, scaled by rate.
fn mode_fade_frames(sample_rate: u32) -> u32 {
    let frames = (u64::from(sample_rate) * 256).div_ceil(48_000).max(1);
    u32::try_from(frames).unwrap_or(u32::MAX)
}
/// One band of the outgoing routing: frozen effective coefficients, its
/// history and the bank it filtered.
#[derive(Clone, Copy)]
struct OutgoingBand {
    stages: BandStages,
    history: [[[f64; 2]; 4]; 2],
    bank: u8,
}
impl Default for OutgoingBand {
    fn default() -> Self {
        Self {
            stages: [Coefficients::IDENTITY; 4],
            history: [[[0.0; 2]; 4]; 2],
            bank: 0,
        }
    }
}
/// The outgoing routing's band audition: the auditioned band's frozen
/// coefficients and bank, its isolated history and the frozen audition mix.
#[derive(Clone, Copy)]
struct OutgoingAudition {
    stages: BandStages,
    history: [[[f64; 2]; 4]; 2],
    bank: u8,
    mix: f64,
}
/// Borrowed effective routing only; capture copies into the existing outgoing
/// owner and retains no borrow or resource custody.
struct Routing<'a> {
    mode: u8,
    nodes: &'a [Node],
    bands: &'a [RuntimeBand],
    banks: &'a [u8; MAX_NODES],
    audition: Option<ProcessorAuditionSelection>,
    audition_mix: f64,
    audition_history: &'a [[[f64; 2]; 4]; 2],
    gain: f64,
}
impl<'a> From<&'a EqRuntime> for Routing<'a> {
    fn from(source: &'a EqRuntime) -> Self {
        Self {
            mode: source.mode,
            nodes: &source.configuration.nodes,
            bands: &source.bands,
            banks: &source.effective_banks,
            audition: source.audition,
            audition_mix: source.audition_mix,
            audition_history: &source.audition_history,
            gain: source.gain,
        }
    }
}
/// The outgoing routing after a stereo/left-right/mid-side switch (Pulse
/// #4122, the same law as EQ8's #4106). Band histories cannot be reinterpreted
/// under a new routing, so the previous routing keeps filtering with its
/// frozen effective coefficients, history, gain and band audition (at its
/// frozen audition mix) while the output crossfades linearly into the new
/// routing, which starts from empty history. The crossfade ends exactly on the
/// new routing, then the outgoing state is cleared.
///
/// Capacity is one outgoing branch. Its band storage holds `MAX_NODES` bands,
/// the existing node limit, so any outgoing routing fits regardless of the
/// incoming node count (a mode switch combined with node deletion, such as a
/// state replacement, still crossfades). It is allocated with the runtime on
/// control; capturing, carrying and processing only copy into it. Same-mode
/// handoffs carry an active branch. A further mode change while the branch is
/// still fading is refused as transiently busy (`transition_busy_from`), so
/// the host defers it for at most one fade (256 frames at 48 kHz, 16/3 ms)
/// instead of dropping the older routing. A transport discontinuity or reset
/// ends the crossfade. Shared mode readings instead coalesce to the latest
/// target at the first frame after this branch finishes.
struct ModeFade {
    bands: Box<[OutgoingBand]>,
    len: usize,
    audition: Option<OutgoingAudition>,
    mode: u8,
    gain: f64,
    remaining: u32,
    total: u32,
}
impl ModeFade {
    fn new() -> Self {
        Self {
            bands: vec![OutgoingBand::default(); MAX_NODES].into_boxed_slice(),
            len: 0,
            audition: None,
            mode: 0,
            gain: 1.0,
            remaining: 0,
            total: 0,
        }
    }
    fn is_active(&self) -> bool {
        self.remaining > 0
    }
    fn clear(&mut self) {
        for band in &mut self.bands[..self.len] {
            band.history = [[[0.0; 2]; 4]; 2];
        }
        self.len = 0;
        self.audition = None;
        self.remaining = 0;
        self.total = 0;
    }
    /// Starts a crossfade out of `previous`'s routing.
    fn capture(&mut self, previous: Routing<'_>, sample_rate: u32) {
        self.clear();
        let mode = previous.mode;
        let active = |bank_id: u8| bank_id == bank(mode, 0) || bank_id == bank(mode, 1);
        let outgoing = previous
            .bands
            .iter()
            .zip(previous.banks)
            .filter(|(_, bank_id)| active(**bank_id));
        // `previous.bands` holds at most `MAX_NODES` bands, so every outgoing
        // band has a slot.
        for ((band, bank_id), slot) in outgoing.zip(self.bands.iter_mut()) {
            *slot = OutgoingBand {
                stages: band.current,
                history: band.history,
                bank: *bank_id,
            };
            self.len += 1;
        }
        self.audition = previous
            .audition
            .filter(|_| previous.audition_mix > 0.0)
            .and_then(|selection| {
                previous
                    .nodes
                    .iter()
                    .position(|node| node.id.value() == selection.index)
            })
            .and_then(|index| {
                Some(OutgoingAudition {
                    stages: previous.bands.get(index)?.current,
                    history: *previous.audition_history,
                    bank: *previous.banks.get(index)?,
                    mix: previous.audition_mix,
                })
            });
        self.mode = mode;
        self.gain = previous.gain;
        self.total = mode_fade_frames(sample_rate);
        self.remaining = self.total;
    }
    /// Carries an in-progress crossfade across a same-routing handoff, copying
    /// only the occupied bands.
    fn copy_from(&mut self, other: &ModeFade) {
        self.clear();
        if !other.is_active() {
            return;
        }
        self.bands[..other.len].copy_from_slice(&other.bands[..other.len]);
        self.len = other.len;
        self.audition = other.audition;
        self.mode = other.mode;
        self.gain = other.gain;
        self.remaining = other.remaining;
        self.total = other.total;
    }
    fn blend(
        &mut self,
        input: StereoFrame,
        current: [f64; 2],
        mapped_gain: Option<f64>,
    ) -> [f64; 2] {
        if !self.is_active() {
            return current;
        }
        let mut working = [f64::from(input[0]), f64::from(input[1])];
        if self.mode == 2 {
            working = [
                (working[0] + working[1]) * 0.5,
                (working[0] - working[1]) * 0.5,
            ];
        }
        let audition_input = working;
        for band in &mut self.bands[..self.len] {
            for (channel, value) in working.iter_mut().enumerate() {
                if band.bank == bank(self.mode, channel) {
                    *value = filter(*value, &band.stages, &mut band.history[channel]);
                }
            }
        }
        if let Some(audition) = &mut self.audition {
            for (channel, value) in working.iter_mut().enumerate() {
                let isolated = if audition.bank == bank(self.mode, channel) {
                    filter(
                        audition_input[channel],
                        &audition.stages,
                        &mut audition.history[channel],
                    )
                } else {
                    0.0
                };
                *value += (isolated - *value) * audition.mix;
            }
        }
        if self.mode == 2 {
            working = [working[0] + working[1], working[0] - working[1]];
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.clear();
            return current;
        }
        let weight = f64::from(self.remaining) / f64::from(self.total);
        let gain = mapped_gain.unwrap_or(self.gain);
        std::array::from_fn(|channel| {
            current[channel] + (working[channel] * gain - current[channel]) * weight
        })
    }
}
pub const EQ_RUNTIME_BYTES: usize = std::mem::size_of::<EqRuntime>();
struct EqRuntime {
    sample_rate: u32,
    configuration: Arc<Configuration>,
    bands: Box<[RuntimeBand]>,
    last_frame: Option<u64>,
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
impl EqRuntime {
    fn mode_is_automated(&self) -> bool {
        self.configuration
            .parameters
            .first()
            .is_some_and(|grant| grant.descriptor.id == MODE)
    }
    fn select_routing(&mut self, target: u8, discontinuity: bool) {
        if discontinuity || (self.pristine && self.mode_is_automated()) {
            if self.mode != target {
                self.cancel_immediately();
                self.refresh_target = true;
            }
            self.mode = target;
            if self.pristine && self.mode_is_automated() {
                self.mode_fade.clear();
            }
            return;
        }
        // One frozen outgoing branch. New readings coalesce while occupied;
        // the first frame after its fade samples the latest current target.
        if self.mode == target || self.mode_fade.is_active() {
            return;
        }
        self.mode_fade.capture(
            Routing {
                mode: self.mode,
                nodes: &self.configuration.nodes,
                bands: &self.bands,
                banks: &self.effective_banks,
                audition: self.audition,
                audition_mix: self.audition_mix,
                audition_history: &self.audition_history,
                gain: self.gain,
            },
            self.sample_rate,
        );
        self.mode = target;
        for band in &mut self.bands {
            band.history = [[[0.0; 2]; 4]; 2];
        }
        self.cancel_immediately();
        // Routing is independent of node coefficients. Keep their existing
        // target cadence and in-flight trajectories through a mode switch.
    }
    fn process(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        bypassed: bool,
    ) {
        self.process_at(
            frames,
            clock,
            bypassed,
            0,
            automation::Targets {
                gain: self.configuration.gain,
                mode: self.configuration.mode,
                mapped_gain: false,
            },
            None,
        );
    }
    // Callers pass either a whole block at offset zero or one frame at its
    // original block offset; clock and legacy lane positions stay unchanged.
    fn process_at(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        bypassed: bool,
        first_offset: usize,
        targets: automation::Targets,
        node_overrides: Option<&automation::Nodes>,
    ) {
        let automated = !self.configuration.lanes.is_empty() || node_overrides.is_some();
        for (offset, frame) in frames.iter_mut().enumerate() {
            let offset = first_offset + offset;
            let running = clock.is_some_and(|c| c.transport_is_running() == Some(true));
            // A stopped monitoring clock may repeat its arrangement origin.
            // Only running content positions define a transport discontinuity.
            let origin = if running {
                clock.and_then(|c| c.arrangement_frame_at_offset(offset))
            } else {
                None
            };
            let position = origin.unwrap_or(self.fallback_frame);
            let discontinuity = origin.is_some_and(|position| {
                self.last_frame
                    .is_none_or(|last| last.checked_add(1) != Some(position))
            });
            self.select_routing(targets.mode, discontinuity);
            if discontinuity {
                for band in &mut self.bands {
                    band.history = [[[0.0; 2]; 4]; 2];
                }
                self.audition_history = [[[0.0; 2]; 4]; 2];
                self.mode_fade.clear();
            }
            if self.refresh_target || discontinuity || (automated && position % CADENCE == 0) {
                self.refresh_target = false;
                let beat = if origin.is_some() && automated {
                    clock.and_then(|clock| clock.arrangement_beat_at_offset(offset))
                } else {
                    None
                };
                for index in 0..self.configuration.nodes.len() {
                    let mut node = self.configuration.node_at(index, beat);
                    if let Some(overrides) = node_overrides {
                        overrides.apply(index, &mut node);
                    }
                    let bank_changed = self.effective_banks[index] != node.bank;
                    self.effective_banks[index] = node.bank;
                    self.effective_enabled[index] = node.enabled;
                    let target = if node.enabled {
                        design(node, self.sample_rate)
                    } else {
                        [Coefficients::IDENTITY; 4]
                    };
                    let band = &mut self.bands[index];
                    if bank_changed || band.shape != node.kind as u8 {
                        band.history = [[[0.0; 2]; 4]; 2];
                        band.shape = node.kind as u8;
                    }
                    if discontinuity || (self.pristine && node_overrides.is_some()) {
                        band.target = target;
                        band.current = target;
                        band.remaining = 0;
                        band.stages_used = stages_used(&target);
                    } else if band.target != target {
                        band.target = target;
                        band.stages_used = band.stages_used.max(stages_used(&target));
                        band.remaining = if band.current == target {
                            0
                        } else {
                            coefficient_smooth_frames(self.sample_rate)
                        };
                    }
                }
            }
            let input = *frame;
            let mut working = [f64::from(frame[0]), f64::from(frame[1])];
            if self.mode == 2 {
                working = [
                    (working[0] + working[1]) * 0.5,
                    (working[0] - working[1]) * 0.5,
                ];
            }
            let audition_input = working;
            for (index, band) in self
                .bands
                .iter_mut()
                .take(self.configuration.nodes.len())
                .enumerate()
            {
                if band.remaining > 0 {
                    let t = 1.0 / f64::from(band.remaining);
                    for (current, target) in band.current.iter_mut().zip(&band.target) {
                        current.b0 += (target.b0 - current.b0) * t;
                        current.b1 += (target.b1 - current.b1) * t;
                        current.b2 += (target.b2 - current.b2) * t;
                        current.a1 += (target.a1 - current.a1) * t;
                        current.a2 += (target.a2 - current.a2) * t;
                    }
                    band.remaining -= 1;
                    if band.remaining == 0 {
                        band.current = band.target;
                    }
                }
                for (channel, value) in working.iter_mut().enumerate() {
                    if self.effective_banks[index] == bank(self.mode, channel) {
                        *value = band.filter_channel(channel, *value);
                    }
                }
            }
            if self.audition != self.audition_request {
                self.audition_mix = (self.audition_mix - 1.0 / 256.0).max(0.0);
                if self.audition_mix == 0.0 {
                    self.audition = self.audition_request;
                    self.audition_history = [[[0.0; 2]; 4]; 2];
                }
            } else if self.audition.is_some() {
                self.audition_mix = (self.audition_mix + 1.0 / 256.0).min(1.0);
            }
            if let Some(index) = self.audition.and_then(|selection| {
                self.configuration
                    .nodes
                    .iter()
                    .position(|node| node.id.value() == selection.index)
            }) {
                for channel in 0..2 {
                    let isolated = if self.effective_banks[index] == bank(self.mode, channel) {
                        filter(
                            audition_input[channel],
                            &self.bands[index].current,
                            &mut self.audition_history[channel],
                        )
                    } else {
                        0.0
                    };
                    working[channel] += (isolated - working[channel]) * self.audition_mix;
                }
            }
            if targets.mapped_gain {
                self.gain = targets.gain;
            } else {
                self.gain += (targets.gain - self.gain) * 0.015625;
            }
            if self.mode == 2 {
                working = [working[0] + working[1], working[0] - working[1]];
            }
            let output = self.mode_fade.blend(
                input,
                [working[0] * self.gain, working[1] * self.gain],
                targets.mapped_gain.then_some(self.gain),
            );
            *frame = if bypassed {
                input
            } else {
                [finite_f32(output[0]), finite_f32(output[1])]
            };
            self.last_frame = origin;
            self.fallback_frame = self.fallback_frame.wrapping_add(1);
            self.pristine = false;
        }
    }
}
fn stages_used(stages: &BandStages) -> u8 {
    // Always run one section so even an identity band applies the existing
    // finite-value and denormal law. Further untouched identity sections
    // have zero history and would repeat that same operation.
    stages
        .iter()
        .rposition(|stage| *stage != Coefficients::IDENTITY)
        .map_or(1, |index| (index + 1) as u8)
}

fn filter(mut input: f64, stages: &[Coefficients], history: &mut [[f64; 2]]) -> f64 {
    for (c, h) in stages.iter().zip(history) {
        let output = c.b0 * input + h[0];
        h[0] = quiet(c.b1 * input - c.a1 * output + h[1]);
        h[1] = quiet(c.b2 * input - c.a2 * output);
        input = quiet(output);
    }
    input
}
fn bank(mode: u8, channel: usize) -> u8 {
    match mode {
        1 => 1 + channel as u8,
        2 => 3 + channel as u8,
        _ => 0,
    }
}
impl NativeProcessorRuntime for EqRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.configuration
            .parameters
            .binary_search_by_key(&id, |grant| grant.descriptor.id)
            .is_ok()
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.process_automation(frames, clock, values, true)
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, None, false)
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, false)
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, clock, true)
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process(frames, None, true)
    }
    fn reset(&mut self) {
        for band in &mut self.bands {
            band.history = [[[0.0; 2]; 4]; 2];
            band.current = band.target;
            band.remaining = 0;
            band.stages_used = stages_used(&band.target);
        }
        self.last_frame = None;
        self.fallback_frame = 0;
        self.mode = self.configuration.mode;
        self.effective_banks = std::array::from_fn(|index| {
            self.configuration
                .nodes
                .get(index)
                .map_or(0, |node| node.bank)
        });
        self.effective_enabled = std::array::from_fn(|index| {
            self.configuration
                .nodes
                .get(index)
                .is_some_and(|node| node.enabled)
        });
        self.pristine = true;
        self.audition = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
        self.mode_fade.clear();
    }
    fn is_quiescent(&self) -> bool {
        !self.mode_fade.is_active()
            && self.audition.is_none()
            && self.audition_request.is_none()
            && self.audition_mix == 0.0
            && self
                .bands
                .iter()
                .all(|b| b.history.iter().flatten().flatten().all(|v| *v == 0.0))
            && self
                .audition_history
                .iter()
                .flatten()
                .flatten()
                .all(|v| *v == 0.0)
    }
    /// A channel-mode change still inherits and retargets without a reset:
    /// the previous routing crossfades out (see `ModeFade`). Retargeting keeps
    /// session playback from detaching the old suffix as a ringing tail beside
    /// a fresh chain.
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.sample_rate == self.sample_rate)
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(old) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|old| old.sample_rate == self.sample_rate)
        else {
            return;
        };
        if !self.mode_is_automated() && old.mode != self.configuration.mode {
            // The new routing starts from empty history; the old one fades out.
            self.reset();
            self.mode_fade.capture(Routing::from(old), self.sample_rate);
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = true;
            self.gain = old.gain;
            self.pristine = old.pristine;
            return;
        }
        self.mode = old.mode;
        self.mode_fade.copy_from(&old.mode_fade);
        if Arc::ptr_eq(&self.configuration, &old.configuration) {
            self.bands.copy_from_slice(&old.bands);
            self.effective_banks = old.effective_banks;
            self.effective_enabled = old.effective_enabled;
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = old.refresh_target;
            self.audition = old.audition;
            self.audition_request = old.audition_request;
            self.audition_mix = old.audition_mix;
            self.audition_history = old.audition_history;
            self.gain = old.gain;
            self.pristine = old.pristine;
            return;
        }
        // Stable identity owns effective DSP history. Canonical kind/bank can
        // differ behind an authored value; the first refresh compares effective
        // shape/bank and owns any genuine discontinuity exactly once.
        for (index, node) in self.configuration.nodes.iter().enumerate() {
            if let Some(prior) = old.configuration.nodes.iter().position(|n| n.id == node.id) {
                self.bands[index] = old.bands[prior];
                self.effective_banks[index] = old.effective_banks[prior];
                self.effective_enabled[index] = old.effective_enabled[prior];
            }
        }
        if old
            .audition
            .is_none_or(|selection| self.can_select(selection))
            && old
                .audition_request
                .is_none_or(|selection| self.can_select(selection))
        {
            self.audition = old.audition;
            self.audition_request = old.audition_request;
            self.audition_mix = old.audition_mix;
            self.audition_history = old.audition_history;
        }
        self.last_frame = old.last_frame;
        self.fallback_frame = old.fallback_frame;
        self.refresh_target = true;
        self.gain = old.gain;
        self.pristine = old.pristine;
    }
    /// The single `ModeFade` branch is full: a further channel-mode change
    /// waits until the previous switch has finished fading (Pulse #4122).
    fn transition_busy_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            old.sample_rate == self.sample_rate
                && !self.mode_is_automated()
                && old.mode != self.configuration.mode
                && old.mode_fade.is_active()
        })
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        Some(self)
    }
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
    }
}
impl NativeProcessorAudition for EqRuntime {
    fn can_select(&self, s: ProcessorAuditionSelection) -> bool {
        s.lane < 2
            && self
                .configuration
                .nodes
                .iter()
                .enumerate()
                .any(|(index, node)| {
                    node.id.value() == s.index
                        && self.effective_enabled[index]
                        && self.effective_banks[index] == bank(self.mode, usize::from(s.lane))
                })
    }
    fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
    }
    fn select(&mut self, s: Option<ProcessorAuditionSelection>) {
        self.audition_request = s;
    }
    fn cancel_immediately(&mut self) {
        self.audition = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
    }
}

/// Infallible after finite/range control validation. Every new shelf uses the
/// ordinary Q alpha law. Imported S shelves are preflighted and cannot automate
/// gain/Q/shape before explicitly converting to the new law.
fn design(node: Node, sample_rate: u32) -> BandStages {
    let frequency = f64::from(node.frequency_hz).min(f64::from(sample_rate) * 0.49);
    let omega = std::f64::consts::TAU * frequency / f64::from(sample_rate);
    let (sine, cosine) = omega.sin_cos();
    let q = f64::from(node.q);
    let gain = f64::from(node.gain_db);
    let mut result = [Coefficients::IDENTITY; 4];
    let norm = |b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64| Coefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    };
    match node.kind {
        NodeKind::Bell => {
            let a = 10.0_f64.powf(gain / 40.0);
            let alpha = sine / (2.0 * q);
            result[0] = norm(
                1.0 + alpha * a,
                -2.0 * cosine,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cosine,
                1.0 - alpha / a,
            );
        }
        NodeKind::Notch => {
            let alpha = sine / (2.0 * q);
            result[0] = norm(
                1.0,
                -2.0 * cosine,
                1.0,
                1.0 + alpha,
                -2.0 * cosine,
                1.0 - alpha,
            );
        }
        NodeKind::LowCut | NodeKind::HighCut => {
            let count = usize::from(node.slope) + 1;
            for (index, stage) in result.iter_mut().take(count).enumerate() {
                let section_q = if count == 1 {
                    q
                } else if count == 4 {
                    let values = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
                    values[index]
                        * if index + 1 == count {
                            q / f64::from(std::f32::consts::FRAC_1_SQRT_2)
                        } else {
                            1.0
                        }
                } else {
                    let butterworth = 1.0
                        / (2.0
                            * ((2 * index + 1) as f64 * std::f64::consts::PI / (4 * count) as f64)
                                .cos());
                    butterworth
                        * if index + 1 == count {
                            q / f64::from(std::f32::consts::FRAC_1_SQRT_2)
                        } else {
                            1.0
                        }
                };
                let alpha = sine / (2.0 * section_q);
                *stage = if node.kind == NodeKind::LowCut {
                    norm(
                        (1.0 + cosine) * 0.5,
                        -(1.0 + cosine),
                        (1.0 + cosine) * 0.5,
                        1.0 + alpha,
                        -2.0 * cosine,
                        1.0 - alpha,
                    )
                } else {
                    norm(
                        (1.0 - cosine) * 0.5,
                        1.0 - cosine,
                        (1.0 - cosine) * 0.5,
                        1.0 + alpha,
                        -2.0 * cosine,
                        1.0 - alpha,
                    )
                };
            }
        }
        NodeKind::LowShelf | NodeKind::HighShelf => {
            let a = 10.0_f64.powf(gain / 40.0);
            let alpha = if node.legacy_shelf {
                sine * 0.5 * (((a + 1.0 / a) * (1.0 / q - 1.0)) + 2.0).sqrt()
            } else {
                sine / (2.0 * q)
            };
            let beta = 2.0 * a.sqrt() * alpha;
            result[0] = if node.kind == NodeKind::LowShelf {
                norm(
                    a * ((a + 1.0) - (a - 1.0) * cosine + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cosine),
                    a * ((a + 1.0) - (a - 1.0) * cosine - beta),
                    (a + 1.0) + (a - 1.0) * cosine + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cosine),
                    (a + 1.0) + (a - 1.0) * cosine - beta,
                )
            } else {
                norm(
                    a * ((a + 1.0) + (a - 1.0) * cosine + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cosine),
                    a * ((a + 1.0) + (a - 1.0) * cosine - beta),
                    (a + 1.0) - (a - 1.0) * cosine + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * cosine),
                    (a + 1.0) - (a - 1.0) * cosine - beta,
                )
            };
        }
    }
    result
}

#[cfg(test)]
mod smoothing_tests {
    use super::*;
    use hex_lattice::EMPTY_DEVICE_RESOURCES;

    #[test]
    fn sparse_filter_matches_full_sections_with_residual_and_nonfinite_history() {
        let mut band = RuntimeBand::default();
        band.current[0].b0 = 0.75;
        band.current[0].b1 = 0.25;
        let mut full_history = [[0.0; 2]; 4];
        let mut compare = |band: &mut RuntimeBand, input| {
            let expected = filter(input, &band.current, &mut full_history);
            let actual = band.filter_channel(0, input);
            assert_eq!(actual.to_bits(), expected.to_bits());
            assert_eq!(band.history[0], full_history);
        };
        for input in [-0.0, 0.5, f64::MIN_POSITIVE, -0.25] {
            compare(&mut band, input);
            assert_eq!(band.stages_used, 1);
        }
        // The last stage's stored sample still matters after its coefficients
        // reach identity at the end of a ramp.
        band.current[3].b1 = 0.5;
        band.stages_used = stages_used(&band.current);
        compare(&mut band, 0.5);
        band.current[3] = Coefficients::IDENTITY;
        compare(&mut band, 0.0);
        compare(&mut band, 0.25);

        let mut malformed = RuntimeBand::default();
        let mut full = [[0.0; 2]; 4];
        for input in [f64::INFINITY, 0.5, f64::NAN] {
            let expected = filter(input, &malformed.current, &mut full);
            let actual = malformed.filter_channel(0, input);
            assert!(actual == expected || (actual.is_nan() && expected.is_nan()));
            assert_eq!(malformed.stages_used, 4);
        }
        assert!(
            malformed.history[0]
                .iter()
                .flatten()
                .all(|value| value.is_nan())
        );
    }

    fn runtime(rate: u32, gain: f32) -> Box<dyn NativeProcessorRuntime> {
        let mut state = EqState::default();
        let mut node = Node::new(NodeId::new(1).unwrap(), 750.0, gain, 0).unwrap();
        node.kind = NodeKind::LowShelf;
        state.add(node).unwrap();
        PreparedEq::new(state, rate).unwrap().create_runtime()
    }

    fn process(runtime: &mut dyn NativeProcessorRuntime, count: usize) -> Vec<StereoFrame> {
        let mut frames = vec![[0.25; 2]; count];
        runtime.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
        frames
    }

    fn remaining(runtime: &dyn NativeProcessorRuntime) -> u32 {
        u32::from(runtime.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].remaining)
    }

    #[test]
    fn unchanged_target_reaches_its_sample_rate_scaled_deadline() {
        for rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let mut old = runtime(rate, 0.0);
            process(old.as_mut(), 17);
            let mut changed = runtime(rate, 12.0);
            let initial = old.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current[0].b0;
            let target =
                changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].target[0].b0;
            changed.copy_history_from(old.as_ref());
            let duration = rate.div_ceil(200);
            for elapsed in 1..=duration {
                process(changed.as_mut(), 1);
                assert_eq!(
                    remaining(changed.as_ref()),
                    duration - elapsed,
                    "rate={rate}, elapsed={elapsed}"
                );
                let actual =
                    changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current[0].b0;
                let expected =
                    initial + (target - initial) * f64::from(elapsed) / f64::from(duration);
                assert!(
                    (actual - expected).abs() < 1e-12,
                    "rate={rate}, elapsed={elapsed}: {actual} vs {expected}"
                );
            }
            let band = &changed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0];
            assert_eq!(band.current, band.target, "rate={rate}");
        }
    }

    #[test]
    fn equivalent_publication_preserves_the_active_ramp_and_pcm() {
        let mut old = runtime(48_000, 0.0);
        process(old.as_mut(), 17);
        let mut reference = runtime(48_000, 12.0);
        reference.copy_history_from(old.as_ref());
        process(reference.as_mut(), 23);
        let mut republished: Vec<_> = [1, 17, 64]
            .into_iter()
            .map(|block| {
                let mut next = runtime(48_000, 12.0);
                next.copy_history_from(reference.as_ref());
                (next, block)
            })
            .collect();
        let expected = process(reference.as_mut(), 300);
        for (runtime, block) in &mut republished {
            let mut actual = vec![[0.25; 2]; 300];
            for chunk in actual.chunks_mut(*block) {
                runtime.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
            }
            for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert_eq!(
                    actual, expected,
                    "equivalent publication changed PCM at frame {index}, block {block}"
                );
            }
        }
    }

    #[test]
    fn changed_target_restarts_from_the_effective_coefficients() {
        let mut old = runtime(48_000, 0.0);
        process(old.as_mut(), 17);
        let mut first = runtime(48_000, 12.0);
        first.copy_history_from(old.as_ref());
        process(first.as_mut(), 23);
        let effective = first.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current;
        let mut reversed = runtime(48_000, -12.0);
        reversed.copy_history_from(first.as_ref());
        assert_eq!(
            reversed.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0].current,
            effective
        );
        process(reversed.as_mut(), 1);
        assert_eq!(remaining(reversed.as_ref()), 239);
    }

    #[test]
    fn extreme_frequency_and_q_retargets_remain_finite() {
        fn extreme(rate: u32, frequency: f32, gain: f32) -> Box<dyn NativeProcessorRuntime> {
            let mut state = EqState::default();
            let mut node = Node::new(NodeId::new(1).unwrap(), frequency, gain, 0).unwrap();
            node.q = 18.0;
            state.add(node).unwrap();
            PreparedEq::new(state, rate).unwrap().create_runtime()
        }
        for rate in [48_000, 192_000] {
            let mut old = extreme(rate, 20.0, 24.0);
            process(old.as_mut(), 128);
            let mut next = extreme(rate, 20_000.0, -24.0);
            next.copy_history_from(old.as_ref());
            let frames = process(next.as_mut(), 4096);
            assert!(
                frames
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite() && sample.abs() < 32.0),
                "rate={rate}"
            );
            assert!(frames.iter().flatten().any(|sample| sample.abs() > 1e-6));
            let band = &next.as_any().downcast_ref::<EqRuntime>().unwrap().bands[0];
            assert!(
                band.current
                    .iter()
                    .chain(&band.target)
                    .all(|c| [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()))
            );
            assert!(
                band.history
                    .iter()
                    .flatten()
                    .flatten()
                    .all(|sample| sample.is_finite())
            );
        }
    }

    #[test]
    fn moving_automation_is_independent_of_block_partitioning() {
        struct Clock(u64);
        impl NativeInstrumentClock for Clock {
            fn beat_at_offset(&self, offset: usize) -> Option<u64> {
                self.arrangement_beat_at_offset(offset)
            }
            fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
                Some(self.0 + offset as u64)
            }
            fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
                Some((self.0 + offset as u64) << 22)
            }
            fn transport_is_running(&self) -> Option<bool> {
                Some(true)
            }
        }
        fn render(block: usize) -> Vec<StereoFrame> {
            let mut state = EqState::default();
            let node = Node::new(NodeId::new(1).unwrap(), 750.0, 0.0, 0).unwrap();
            state.add(node).unwrap();
            state
                .set_lane(AutomationLane {
                    node: node.id,
                    parameter: NodeParameter::Gain,
                    points: vec![
                        AutomationPoint {
                            beat: 0,
                            normalized: 0.25,
                        },
                        AutomationPoint {
                            beat: 1 << 32,
                            normalized: 0.75,
                        },
                    ],
                })
                .unwrap();
            let mut runtime = PreparedEq::new(state, 48_000).unwrap().create_runtime();
            let mut frames = vec![[0.25; 2]; 1024];
            for (index, chunk) in frames.chunks_mut(block).enumerate() {
                runtime.process_block_with_clock(
                    chunk,
                    Some(&Clock((index * block) as u64)),
                    &EMPTY_DEVICE_RESOURCES,
                );
            }
            frames
        }
        let expected = render(1);
        for block in [17, 64, 256] {
            assert_eq!(render(block), expected, "block size {block}");
        }
    }
}
