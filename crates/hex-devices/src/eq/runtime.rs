use super::*;
use hex_lattice::{NativeInstrumentClock, NativeProcessorAudition};

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
}
impl Configuration {
    fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.lanes.capacity() * std::mem::size_of::<CompiledLane>()
            + self.points.capacity() * std::mem::size_of::<AutomationPoint>()
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
    pub(super) fn new(settings: EqState, sample_rate: u32) -> Result<Self, DeviceContractError> {
        settings.validate()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut stages = Vec::with_capacity(settings.nodes.len());
        for node in &settings.nodes {
            let coefficients = design(*node, sample_rate);
            if (node.enabled
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
            }),
            stages,
        })
    }
}
impl PreparedNativeProcessor for PreparedEq {
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }
    fn runtime_version(&self) -> u32 {
        1
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
}
impl Default for RuntimeBand {
    fn default() -> Self {
        Self {
            current: [Coefficients::IDENTITY; 4],
            target: [Coefficients::IDENTITY; 4],
            history: [[[0.0; 2]; 4]; 2],
            remaining: 0,
            shape: 0,
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
/// ends the crossfade.
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
    fn capture(&mut self, previous: &EqRuntime, sample_rate: u32) {
        self.clear();
        let mode = previous.configuration.mode;
        let active = |bank_id: u8| bank_id == bank(mode, 0) || bank_id == bank(mode, 1);
        let outgoing = previous
            .bands
            .iter()
            .zip(&previous.effective_banks)
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
                    .configuration
                    .nodes
                    .iter()
                    .position(|node| node.id.value() == selection.index)
            })
            .and_then(|index| {
                Some(OutgoingAudition {
                    stages: previous.bands.get(index)?.current,
                    history: previous.audition_history,
                    bank: *previous.effective_banks.get(index)?,
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
    fn blend(&mut self, input: StereoFrame, current: [f64; 2]) -> [f64; 2] {
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
        std::array::from_fn(|channel| {
            current[channel] + (working[channel] * self.gain - current[channel]) * weight
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
    effective_banks: [u8; MAX_NODES],
    mode_fade: ModeFade,
}
impl EqRuntime {
    fn process(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        bypassed: bool,
    ) {
        for (offset, frame) in frames.iter_mut().enumerate() {
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
            if discontinuity {
                for band in &mut self.bands {
                    band.history = [[[0.0; 2]; 4]; 2];
                }
                self.audition_history = [[[0.0; 2]; 4]; 2];
                self.mode_fade.clear();
            }
            if self.refresh_target || discontinuity || position % CADENCE == 0 {
                self.refresh_target = false;
                let beat = if origin.is_some() && !self.configuration.lanes.is_empty() {
                    clock.and_then(|clock| clock.arrangement_beat_at_offset(offset))
                } else {
                    None
                };
                for index in 0..self.configuration.nodes.len() {
                    let node = self.configuration.node_at(index, beat);
                    let bank_changed = self.effective_banks[index] != node.bank;
                    self.effective_banks[index] = node.bank;
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
                    if discontinuity {
                        band.target = target;
                        band.current = target;
                        band.remaining = 0;
                    } else if band.target != target {
                        band.target = target;
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
            if self.configuration.mode == 2 {
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
                    if self.effective_banks[index] == bank(self.configuration.mode, channel) {
                        *value = filter(*value, &band.current, &mut band.history[channel]);
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
                    let isolated =
                        if self.effective_banks[index] == bank(self.configuration.mode, channel) {
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
            self.gain += (self.configuration.gain - self.gain) * 0.015625;
            if self.configuration.mode == 2 {
                working = [working[0] + working[1], working[0] - working[1]];
            }
            let output = self
                .mode_fade
                .blend(input, [working[0] * self.gain, working[1] * self.gain]);
            *frame = if bypassed {
                input
            } else {
                [finite_f32(output[0]), finite_f32(output[1])]
            };
            self.last_frame = origin;
            self.fallback_frame = self.fallback_frame.wrapping_add(1);
        }
    }
}
fn filter(mut input: f64, stages: &BandStages, history: &mut [[f64; 2]; 4]) -> f64 {
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
        }
        self.last_frame = None;
        self.fallback_frame = 0;
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
        if old.configuration.mode != self.configuration.mode {
            // The new routing starts from empty history; the old one fades out.
            self.reset();
            self.mode_fade.capture(old, self.sample_rate);
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = true;
            self.gain = old.gain;
            return;
        }
        self.mode_fade.copy_from(&old.mode_fade);
        if Arc::ptr_eq(&self.configuration, &old.configuration) {
            self.bands.copy_from_slice(&old.bands);
            self.effective_banks = old.effective_banks;
            self.last_frame = old.last_frame;
            self.fallback_frame = old.fallback_frame;
            self.refresh_target = old.refresh_target;
            self.audition = old.audition;
            self.audition_request = old.audition_request;
            self.audition_mix = old.audition_mix;
            self.audition_history = old.audition_history;
            self.gain = old.gain;
            return;
        }
        for (index, node) in self.configuration.nodes.iter().enumerate() {
            if let Some(prior) = old
                .configuration
                .nodes
                .iter()
                .position(|n| n.id == node.id && n.bank == node.bank)
            {
                if old.configuration.nodes[prior].kind == node.kind {
                    self.bands[index] = old.bands[prior];
                } else {
                    self.bands[index].current = old.bands[prior].current;
                    self.bands[index].remaining = coefficient_smooth_frames(self.sample_rate);
                }
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
    }
    /// The single `ModeFade` branch is full: a further channel-mode change
    /// waits until the previous switch has finished fading (Pulse #4122).
    fn transition_busy_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            old.sample_rate == self.sample_rate
                && old.configuration.mode != self.configuration.mode
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
            && self.configuration.nodes.iter().any(|node| {
                node.id.value() == s.index
                    && node.enabled
                    && node.bank == bank(self.configuration.mode, usize::from(s.lane))
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
