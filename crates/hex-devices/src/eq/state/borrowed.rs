//! Validated borrowed current-format EQ codec. No owning EqState/Vec is built.
//! Node parsing/validation and lane semantics are shared with the owning codec.
use super::*;
const NODE_BYTES: usize = 20;

#[derive(Clone, Copy)]
pub struct EqStateRead<'a> {
    mode: u8,
    output_gain: f32,
    nodes: &'a [u8],
    lanes: &'a [u8],
    node_count: usize,
    lane_count: usize,
}
#[derive(Clone, Copy)]
struct LaneRead<'a> {
    node: NodeId,
    parameter: NodeParameter,
    count: usize,
    points: &'a [u8],
}
impl LaneRead<'_> {
    fn points(&self) -> impl Iterator<Item = Result<AutomationPoint, DeviceContractError>> + '_ {
        let mut reader = Reader {
            bytes: self.points,
            offset: 0,
        };
        (0..self.count).map(move |_| {
            Ok(AutomationPoint {
                beat: u64::from_le_bytes(reader.array()?),
                normalized: reader.float()?,
            })
        })
    }
}
fn lane<'a>(reader: &mut Reader<'a>) -> Result<LaneRead<'a>, DeviceContractError> {
    let node = NodeId::new(u64::from(reader.u32()?))?;
    let parameter = *NodeParameter::ALL
        .get(usize::from(reader.byte()?))
        .ok_or(DeviceContractError::MalformedState)?;
    let count = usize::from(reader.byte()?);
    let start = reader.offset;
    for _ in 0..count {
        let _: [u8; 8] = reader.array()?;
        let _ = reader.float()?;
    }
    Ok(LaneRead {
        node,
        parameter,
        count,
        points: reader
            .bytes
            .get(start..reader.offset)
            .ok_or(DeviceContractError::MalformedState)?,
    })
}
impl<'a> EqStateRead<'a> {
    pub fn from_native(state: &'a NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != EQ_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != EQ_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let mut reader = Reader {
            bytes: state.payload(),
            offset: 0,
        };
        let mode = reader.byte()?;
        let output_gain = reader.float()?;
        let node_count = usize::from(reader.byte()?);
        let lane_count = usize::from(reader.byte()?);
        let next = reader.u32()?;
        if node_count > MAX_NODES || lane_count > MAX_LANES {
            return Err(DeviceContractError::MalformedState);
        }
        let node_start = reader.offset;
        for _ in 0..node_count {
            reader.node()?;
        }
        let node_end = reader.offset;
        let mut points = 0_usize;
        for _ in 0..lane_count {
            // Preserve structural count refusal before reading the point bytes.
            let mut header = reader;
            let _ = NodeId::new(u64::from(header.u32()?))?;
            let _ = NodeParameter::ALL
                .get(usize::from(header.byte()?))
                .ok_or(DeviceContractError::MalformedState)?;
            points = points
                .checked_add(usize::from(header.byte()?))
                .ok_or(DeviceContractError::MalformedState)?;
            if points > MAX_POINTS {
                return Err(DeviceContractError::MalformedState);
            }
            lane(&mut reader)?;
        }
        if reader.offset != reader.bytes.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let result = Self {
            mode,
            output_gain,
            node_count,
            lane_count,
            nodes: reader
                .bytes
                .get(node_start..node_end)
                .ok_or(DeviceContractError::MalformedState)?,
            lanes: reader
                .bytes
                .get(node_end..reader.offset)
                .ok_or(DeviceContractError::MalformedState)?,
        };
        // Structural parsing completes before semantic validation, as in decode.
        if next == 0 || mode > 2 {
            return Err(DeviceContractError::MalformedState);
        }
        for index in 0..node_count {
            if result.node_at(index)?.id.value() >= next {
                return Err(DeviceContractError::MalformedState);
            }
        }
        DESCRIPTORS[201].validate(ParameterValue::Float(output_gain))?;
        for index in 0..node_count {
            let node = result.node_at(index)?;
            node.validate()?;
            for prior in 0..index {
                if result.node_at(prior)?.id == node.id {
                    return Err(DeviceContractError::MalformedState);
                }
            }
        }
        let mut lanes = Reader {
            bytes: result.lanes,
            offset: 0,
        };
        for index in 0..lane_count {
            let current = lane(&mut lanes)?;
            let node = result.node(current.node)?;
            let mut prior = Reader {
                bytes: result.lanes,
                offset: 0,
            };
            let mut duplicate = false;
            for _ in 0..index {
                let other = lane(&mut prior)?;
                duplicate |= other.node == current.node && other.parameter == current.parameter;
            }
            validate_lane_values(
                &node,
                current.parameter,
                current.count,
                duplicate,
                current.points(),
            )?;
        }
        Ok(result)
    }
    pub fn mode(self) -> u8 {
        self.mode
    }
    pub fn output_gain(self) -> f32 {
        self.output_gain
    }
    pub fn node_count(self) -> usize {
        self.node_count
    }
    pub fn lane_count(self) -> usize {
        self.lane_count
    }
    pub fn node_at(self, index: usize) -> Result<Node, DeviceContractError> {
        if index >= self.node_count {
            return Err(DeviceContractError::UnknownParameter);
        }
        let offset = index
            .checked_mul(NODE_BYTES)
            .ok_or(DeviceContractError::MalformedState)?;
        Reader {
            bytes: self
                .nodes
                .get(offset..)
                .ok_or(DeviceContractError::MalformedState)?,
            offset: 0,
        }
        .node()
    }
    pub fn node(self, id: NodeId) -> Result<Node, DeviceContractError> {
        for index in 0..self.node_count {
            let node = self.node_at(index)?;
            if node.id == id {
                return Ok(node);
            }
        }
        Err(DeviceContractError::UnknownParameter)
    }
    pub fn parameter(self, id: ParameterId) -> Result<ParameterValue, DeviceContractError> {
        if id == MODE {
            return Ok(ParameterValue::Choice(u32::from(self.mode)));
        }
        if id == OUTPUT_GAIN {
            return Ok(ParameterValue::Float(self.output_gain));
        }
        let (node, field) = NodeId::from_parameter(id)?;
        Ok(self.node(node)?.value(field))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> NativeDeviceState {
        let mut authored = EqState::default();
        for index in 1..=MAX_NODES {
            authored
                .add(
                    Node::new(
                        NodeId::new(index as u64).unwrap(),
                        100.0 + index as f32 * 100.0,
                        index as f32 / 8.0,
                        (index % 5) as u8,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        for index in 1..=MAX_LANES {
            authored
                .set_lane(AutomationLane {
                    node: NodeId::new(index as u64).unwrap(),
                    parameter: NodeParameter::Frequency,
                    points: vec![
                        AutomationPoint {
                            beat: 0,
                            normalized: 0.25,
                        },
                        AutomationPoint {
                            beat: 100,
                            normalized: 0.75,
                        },
                    ],
                })
                .unwrap();
        }
        authored.encode().unwrap()
    }

    #[test]
    fn borrowed_eq_matches_owning_current_codec_and_all_node_parameters() {
        let state = fixture();
        let owned = EqState::decode(&state).unwrap();
        let read = EqStateRead::from_native(&state).unwrap();
        assert_eq!(read.mode(), owned.mode);
        assert_eq!(read.output_gain(), owned.output_gain);
        assert_eq!(read.node_count(), owned.nodes.len());
        assert_eq!(read.lane_count(), owned.lanes.len());
        for (index, node) in owned.nodes.iter().enumerate() {
            assert_eq!(read.node_at(index).unwrap(), *node);
            for field in NodeParameter::ALL {
                assert_eq!(
                    read.parameter(node.id.parameter(field).unwrap()).unwrap(),
                    node.value(field)
                );
            }
        }
        assert!(read.node_at(MAX_NODES).is_err());
    }

    #[test]
    fn borrowed_eq_preserves_structural_and_semantic_refusals() {
        let state = fixture();
        for length in 0..state.payload().len() {
            let truncated = NativeDeviceState::new(
                EQ_TYPE_ID,
                EQ_STATE_VERSION,
                state.payload()[..length].to_vec(),
            )
            .unwrap();
            assert!(EqState::decode(&truncated).is_err());
            assert!(EqStateRead::from_native(&truncated).is_err());
        }
        // Exercise every stored byte with the same malformed-value discriminator;
        // accepted mutations must expose the exact same authored node facts.
        for offset in 0..state.payload().len() {
            let mut bytes = state.payload().to_vec();
            bytes[offset] ^= 0xff;
            let changed = NativeDeviceState::new(EQ_TYPE_ID, EQ_STATE_VERSION, bytes).unwrap();
            let owned = EqState::decode(&changed);
            let borrowed = EqStateRead::from_native(&changed);
            assert_eq!(
                owned.as_ref().err(),
                borrowed.as_ref().err(),
                "offset {offset}"
            );
            if let (Ok(owned), Ok(borrowed)) = (owned, borrowed) {
                for (index, node) in owned.nodes.iter().enumerate() {
                    assert_eq!(borrowed.node_at(index).unwrap(), *node);
                }
            }
        }
        let foreign = NativeDeviceState::new(
            DeviceTypeId::from_static("test.foreign"),
            EQ_STATE_VERSION,
            vec![],
        )
        .unwrap();
        assert!(matches!(
            EqStateRead::from_native(&foreign),
            Err(DeviceContractError::WrongDeviceType)
        ));
        let future = NativeDeviceState::new(EQ_TYPE_ID, EQ_STATE_VERSION + 1, vec![]).unwrap();
        assert!(matches!(
            EqStateRead::from_native(&future),
            Err(DeviceContractError::UnsupportedStateVersion)
        ));
    }
}
