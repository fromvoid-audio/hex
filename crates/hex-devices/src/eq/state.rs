use super::*;
mod borrowed;
pub use borrowed::EqStateRead;

/// Project-allocated monotonic identity. Parameter slots never depend on order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct NodeId(u32);
impl NodeId {
    pub fn new(value: u64) -> Result<Self, DeviceContractError> {
        let value = u32::try_from(value).map_err(|_| DeviceContractError::InvalidId)?;
        let node = Self(value);
        node.parameter(NodeParameter::Bank)?;
        Ok(node)
    }
    pub const fn value(self) -> u32 {
        self.0
    }
    pub fn parameter(self, field: NodeParameter) -> Result<ParameterId, DeviceContractError> {
        let id = self
            .0
            .checked_mul(8)
            .and_then(|v| v.checked_add(1000 + field as u32))
            .ok_or(DeviceContractError::InvalidId)?;
        if self.0 == 0 {
            return Err(DeviceContractError::InvalidId);
        }
        ParameterId::new(id)
    }
    pub fn from_parameter(id: ParameterId) -> Result<(Self, NodeParameter), DeviceContractError> {
        let raw = id
            .value()
            .checked_sub(1000)
            .ok_or(DeviceContractError::UnknownParameter)?;
        let field = *NodeParameter::ALL
            .get((raw % 8) as usize)
            .ok_or(DeviceContractError::UnknownParameter)?;
        Ok((Self::new(u64::from(raw / 8))?, field))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeParameter {
    Enabled,
    Kind,
    Frequency,
    Gain,
    Q,
    Slope,
    Bank,
}
impl NodeParameter {
    pub const ALL: [Self; 7] = [
        Self::Enabled,
        Self::Kind,
        Self::Frequency,
        Self::Gain,
        Self::Q,
        Self::Slope,
        Self::Bank,
    ];
    pub fn normalized(self, value: ParameterValue) -> Result<f32, DeviceContractError> {
        descriptor(NodeId::new(1)?, self)?.validate(value)?;
        Ok(match (self, value) {
            (Self::Enabled, ParameterValue::Bool(v)) => u8::from(v) as f32,
            (Self::Kind, ParameterValue::Choice(v)) => v as f32 / 5.0,
            (Self::Slope, ParameterValue::Choice(v)) => v as f32 / 3.0,
            (Self::Bank, ParameterValue::Choice(v)) => v as f32 / 4.0,
            (Self::Frequency, ParameterValue::Float(v)) => {
                (v / MIN_HZ).ln() / (MAX_HZ / MIN_HZ).ln()
            }
            (Self::Q, ParameterValue::Float(v)) => (v / MIN_Q).ln() / (MAX_Q / MIN_Q).ln(),
            (Self::Gain, ParameterValue::Float(v)) => (v + MAX_GAIN_DB) / (2.0 * MAX_GAIN_DB),
            _ => return Err(DeviceContractError::InvalidParameterValue),
        })
    }
    pub fn value(self, normalized: f32) -> ParameterValue {
        let n = normalized.clamp(0.0, 1.0);
        match self {
            Self::Enabled => ParameterValue::Bool(n >= 0.5),
            Self::Kind => ParameterValue::Choice((n * 5.0).round() as u32),
            Self::Slope => ParameterValue::Choice((n * 3.0).round() as u32),
            Self::Bank => ParameterValue::Choice((n * 4.0).round() as u32),
            Self::Frequency => ParameterValue::Float(
                (MIN_HZ * ((MAX_HZ / MIN_HZ).ln() * n).exp()).clamp(MIN_HZ, MAX_HZ),
            ),
            Self::Gain => ParameterValue::Float(n * (2.0 * MAX_GAIN_DB) - MAX_GAIN_DB),
            Self::Q => ParameterValue::Float(
                (MIN_Q * ((MAX_Q / MIN_Q).ln() * n).exp()).clamp(MIN_Q, MAX_Q),
            ),
        }
    }
    pub const fn continuous(self) -> bool {
        matches!(self, Self::Frequency | Self::Gain | Self::Q)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
}
impl NodeKind {
    fn from_u8(value: u8) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Bell),
            1 => Ok(Self::LowShelf),
            2 => Ok(Self::HighShelf),
            3 => Ok(Self::LowCut),
            4 => Ok(Self::HighCut),
            5 => Ok(Self::Notch),
            _ => Err(DeviceContractError::MalformedState),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub enabled: bool,
    pub kind: NodeKind,
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
    /// 0,1,2,3 mean 12,24,36,48 db/octave.
    pub slope: u8,
    pub bank: u8,
    /// Imported shelves retain EQ8's historical S law until shape/Q is edited.
    pub(super) legacy_shelf: bool,
}
impl Node {
    pub fn new(
        id: NodeId,
        frequency_hz: f32,
        gain_db: f32,
        bank: u8,
    ) -> Result<Self, DeviceContractError> {
        let node = Self {
            id,
            enabled: true,
            kind: NodeKind::Bell,
            frequency_hz,
            gain_db,
            q: std::f32::consts::FRAC_1_SQRT_2,
            slope: 0,
            bank,
            legacy_shelf: false,
        };
        node.validate()?;
        Ok(node)
    }
    fn validate(self) -> Result<(), DeviceContractError> {
        for field in NodeParameter::ALL {
            descriptor(self.id, field)?.validate(self.value(field))?;
        }
        Ok(())
    }
    pub fn value(self, field: NodeParameter) -> ParameterValue {
        match field {
            NodeParameter::Enabled => ParameterValue::Bool(self.enabled),
            NodeParameter::Kind => ParameterValue::Choice(self.kind as u32),
            NodeParameter::Frequency => ParameterValue::Float(self.frequency_hz),
            NodeParameter::Gain => ParameterValue::Float(self.gain_db),
            NodeParameter::Q => ParameterValue::Float(self.q),
            NodeParameter::Slope => ParameterValue::Choice(u32::from(self.slope)),
            NodeParameter::Bank => ParameterValue::Choice(u32::from(self.bank)),
        }
    }
    pub(super) fn set(
        &mut self,
        field: NodeParameter,
        value: ParameterValue,
    ) -> Result<(), DeviceContractError> {
        match (field, value) {
            (NodeParameter::Enabled, ParameterValue::Bool(v)) => self.enabled = v,
            (NodeParameter::Kind, ParameterValue::Choice(v)) => {
                self.kind = NodeKind::from_u8(v as u8)?;
                self.legacy_shelf = false;
            }
            (NodeParameter::Frequency, ParameterValue::Float(v)) => self.frequency_hz = v,
            (NodeParameter::Gain, ParameterValue::Float(v)) => self.gain_db = v,
            (NodeParameter::Q, ParameterValue::Float(v)) => {
                self.q = v;
                self.legacy_shelf = false;
            }
            (NodeParameter::Slope, ParameterValue::Choice(v)) => self.slope = v as u8,
            (NodeParameter::Bank, ParameterValue::Choice(v)) => self.bank = v as u8,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationPoint {
    /// Q32.32 quarter-note beats from arrangement origin.
    pub beat: u64,
    /// Descriptor-normalized value; frequency and Q interpolate logarithmically.
    pub normalized: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AutomationLane {
    pub node: NodeId,
    pub parameter: NodeParameter,
    pub points: Vec<AutomationPoint>,
}
impl AutomationLane {
    pub fn value_at(&self, beat: u64) -> Option<ParameterValue> {
        let first = *self.points.first()?;
        let mut prior = first;
        for next in &self.points {
            if next.beat > beat {
                let value = if self.parameter.continuous() && beat > prior.beat {
                    prior.normalized
                        + (next.normalized - prior.normalized)
                            * ((beat - prior.beat) as f64 / (next.beat - prior.beat) as f64) as f32
                } else {
                    prior.normalized
                };
                return Some(self.parameter.value(value));
            }
            prior = *next;
        }
        Some(self.parameter.value(prior.normalized))
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct EqState {
    /// Never-used node cursor, retained by presets as well as project history.
    pub next_node_id: u32,
    pub mode: u8,
    pub output_gain: f32,
    pub nodes: Vec<Node>,
    pub lanes: Vec<AutomationLane>,
}
impl Default for EqState {
    fn default() -> Self {
        Self {
            next_node_id: 1,
            mode: 0,
            output_gain: 1.0,
            nodes: Vec::new(),
            lanes: Vec::new(),
        }
    }
}
impl EqState {
    pub fn node(&self, id: NodeId) -> Result<&Node, DeviceContractError> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or(DeviceContractError::UnknownParameter)
    }
    pub(super) fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, DeviceContractError> {
        self.nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(DeviceContractError::UnknownParameter)
    }
    pub fn add(&mut self, node: Node) -> Result<(), DeviceContractError> {
        if self.nodes.len() >= MAX_NODES || self.nodes.iter().any(|n| n.id == node.id) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        node.validate()?;
        if node.id.value() < self.next_node_id {
            return Err(DeviceContractError::InvalidId);
        }
        self.next_node_id = node
            .id
            .value()
            .checked_add(1)
            .ok_or(DeviceContractError::InvalidId)?;
        self.nodes.push(node);
        Ok(())
    }
    pub fn remove(&mut self, id: NodeId) -> Result<(), DeviceContractError> {
        let index = self
            .nodes
            .iter()
            .position(|n| n.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        self.nodes.remove(index);
        self.lanes.retain(|lane| lane.node != id);
        Ok(())
    }
    pub fn set_lane(&mut self, lane: AutomationLane) -> Result<(), DeviceContractError> {
        let mut candidate = self.clone();
        candidate
            .lanes
            .retain(|l| l.node != lane.node || l.parameter != lane.parameter);
        if !lane.points.is_empty() {
            candidate.lanes.push(lane);
        }
        candidate.validate()?;
        let _ = candidate.encode()?;
        *self = candidate;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if self.next_node_id == 0
            || self.nodes.iter().any(|n| n.id.value() >= self.next_node_id)
            || self.mode > 2
            || self.nodes.len() > MAX_NODES
            || self.lanes.len() > MAX_LANES
            || self.lanes.iter().map(|l| l.points.len()).sum::<usize>() > MAX_POINTS
        {
            return Err(DeviceContractError::MalformedState);
        }
        DESCRIPTORS[201].validate(ParameterValue::Float(self.output_gain))?;
        for (i, node) in self.nodes.iter().enumerate() {
            node.validate()?;
            if self.nodes[..i].iter().any(|n| n.id == node.id) {
                return Err(DeviceContractError::MalformedState);
            }
        }
        for (i, lane) in self.lanes.iter().enumerate() {
            let node = self.node(lane.node)?;
            validate_lane_values(
                node,
                lane.parameter,
                lane.points.len(),
                self.lanes[..i]
                    .iter()
                    .any(|l| l.node == lane.node && l.parameter == lane.parameter),
                lane.points.iter().copied().map(Ok),
            )?;
        }
        Ok(())
    }
    /// Lossless five-bank conversion, including all disabled authored slots.
    /// Caller reserves forty project IDs atomically with the state replacement.
    pub fn upgrade_legacy(
        state: &NativeDeviceState,
        first_id: u64,
    ) -> Result<Self, DeviceContractError> {
        let old = decode(state)?;
        let mut result = Self {
            mode: match old.mode {
                ChannelMode::Stereo => 0,
                ChannelMode::LeftRight => 1,
                ChannelMode::MidSide => 2,
            },
            output_gain: old.output_gain,
            ..Self::default()
        };
        for (bank, bands) in old.banks.iter().enumerate() {
            for (slot, band) in bands.iter().enumerate() {
                let id = first_id
                    .checked_add((bank * BAND_COUNT + slot) as u64)
                    .ok_or(DeviceContractError::InvalidId)?;
                let (kind, slope) = match band.kind {
                    0 => (NodeKind::Bell, 0),
                    1 => (NodeKind::LowShelf, 0),
                    2 => (NodeKind::HighShelf, 0),
                    3 => (NodeKind::LowCut, 0),
                    4 => (NodeKind::LowCut, 3),
                    5 => (NodeKind::HighCut, 0),
                    6 => (NodeKind::HighCut, 3),
                    7 => (NodeKind::Notch, 0),
                    _ => return Err(DeviceContractError::MalformedState),
                };
                result.add(Node {
                    id: NodeId::new(id)?,
                    enabled: band.enabled,
                    kind,
                    frequency_hz: band.frequency_hz,
                    gain_db: band.gain_db,
                    q: band.q,
                    slope,
                    bank: bank as u8,
                    legacy_shelf: matches!(kind, NodeKind::LowShelf | NodeKind::HighShelf),
                })?;
            }
        }
        Ok(result)
    }
    pub fn encode(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut b = Vec::with_capacity(4096);
        b.push(self.mode);
        b.extend(self.output_gain.to_le_bytes());
        b.push(self.nodes.len() as u8);
        b.push(self.lanes.len() as u8);
        b.extend(self.next_node_id.to_le_bytes());
        for n in &self.nodes {
            b.extend(n.id.value().to_le_bytes());
            b.push(u8::from(n.enabled) | (u8::from(n.legacy_shelf) << 1));
            b.push(n.kind as u8);
            b.push(n.slope);
            b.push(n.bank);
            b.extend(n.frequency_hz.to_le_bytes());
            b.extend(n.gain_db.to_le_bytes());
            b.extend(n.q.to_le_bytes());
        }
        for l in &self.lanes {
            b.extend(l.node.value().to_le_bytes());
            b.push(l.parameter as u8);
            b.push(l.points.len() as u8);
            for p in &l.points {
                b.extend(p.beat.to_le_bytes());
                b.extend(p.normalized.to_le_bytes());
            }
        }
        NativeDeviceState::new(EQ_TYPE_ID, EQ_STATE_VERSION, b)
    }
    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != EQ_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != EQ_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let mut r = Reader {
            bytes: state.payload(),
            offset: 0,
        };
        let mode = r.byte()?;
        let output_gain = r.float()?;
        let count = usize::from(r.byte()?);
        let lane_count = usize::from(r.byte()?);
        let next_node_id = r.u32()?;
        if count > MAX_NODES || lane_count > MAX_LANES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut nodes = Vec::with_capacity(count);
        for _ in 0..count {
            nodes.push(r.node()?);
        }
        let mut lanes = Vec::with_capacity(lane_count);
        let mut total = 0;
        for _ in 0..lane_count {
            let node = NodeId::new(u64::from(r.u32()?))?;
            let parameter = *NodeParameter::ALL
                .get(usize::from(r.byte()?))
                .ok_or(DeviceContractError::MalformedState)?;
            let count = usize::from(r.byte()?);
            total += count;
            if total > MAX_POINTS {
                return Err(DeviceContractError::MalformedState);
            }
            let mut points = Vec::with_capacity(count);
            for _ in 0..count {
                points.push(AutomationPoint {
                    beat: u64::from_le_bytes(r.array()?),
                    normalized: r.float()?,
                });
            }
            lanes.push(AutomationLane {
                node,
                parameter,
                points,
            });
        }
        if r.offset != r.bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let result = Self {
            next_node_id,
            mode,
            output_gain,
            nodes,
            lanes,
        };
        result.validate()?;
        Ok(result)
    }
}
#[derive(Clone, Copy)]
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn node(&mut self) -> Result<Node, DeviceContractError> {
        let id = NodeId::new(u64::from(self.u32()?))?;
        let flags = self.byte()?;
        if flags > 3 {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(Node {
            id,
            enabled: flags & 1 != 0,
            legacy_shelf: flags & 2 != 0,
            kind: NodeKind::from_u8(self.byte()?)?,
            slope: self.byte()?,
            bank: self.byte()?,
            frequency_hz: self.float()?,
            gain_db: self.float()?,
            q: self.float()?,
        })
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DeviceContractError> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or(DeviceContractError::MalformedState)?;
        let raw = self
            .bytes
            .get(self.offset..end)
            .ok_or(DeviceContractError::MalformedState)?;
        let mut result = [0; N];
        result.copy_from_slice(raw);
        self.offset = end;
        Ok(result)
    }
    fn byte(&mut self) -> Result<u8, DeviceContractError> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, DeviceContractError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn float(&mut self) -> Result<f32, DeviceContractError> {
        Ok(f32::from_le_bytes(self.array()?))
    }
}

// One semantic law shared by owned editing validation and borrowed UI decoding.
fn validate_lane_values(
    node: &Node,
    parameter: NodeParameter,
    count: usize,
    duplicate: bool,
    points: impl Iterator<Item = Result<AutomationPoint, DeviceContractError>>,
) -> Result<(), DeviceContractError> {
    if count == 0 || count > 64 || duplicate {
        return Err(DeviceContractError::MalformedState);
    }
    let mut prior = None;
    for point in points {
        let point = point?;
        if prior.is_some_and(|beat| beat >= point.beat)
            || !point.normalized.is_finite()
            || !(0.0..=1.0).contains(&point.normalized)
        {
            return Err(DeviceContractError::MalformedState);
        }
        prior = Some(point.beat);
    }
    if node.legacy_shelf
        && matches!(
            parameter,
            NodeParameter::Kind | NodeParameter::Gain | NodeParameter::Q
        )
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}
