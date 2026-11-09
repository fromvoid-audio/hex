//! Direct prepared component probe before registry node opt-in. All construction,
//! reference rendering, admission and final destruction are outside measured scopes.
use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation};
use hex_lattice_contract::{ParameterId, ParameterValue};
struct Values {
    ids: Vec<ParameterId>,
    start: usize,
    bad: bool,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        let i = self.start + offset;
        if id == hex_engine::eq::MODE {
            return Some(ParameterValue::Choice(if self.bad && offset == 17 {
                3
            } else {
                (i / 61 % 3) as u32
            }));
        }
        if id == hex_engine::eq::OUTPUT_GAIN {
            return Some(ParameterValue::Float(0.5 + (i % 31) as f32 * 0.01));
        }
        let (node, field) = NodeId::from_parameter(id).unwrap();
        if self.bad && offset == 17 && node.value() == 1 && field == NodeParameter::Gain {
            return Some(ParameterValue::Float(f32::NAN));
        }
        if (120..150).contains(&(i % 257)) {
            return None;
        }
        Some(match field {
            NodeParameter::Enabled => {
                ParameterValue::Bool((i / 211 + node.value() as usize).is_multiple_of(2))
            }
            NodeParameter::Kind => ParameterValue::Choice((i / 257 % 6) as u32),
            NodeParameter::Frequency => {
                ParameterValue::Float(200.0 + node.value() as f32 * 40.0 + (i % 200) as f32)
            }
            NodeParameter::Gain => ParameterValue::Float(-1.0 + (i % 41) as f32 * 0.05),
            NodeParameter::Q => ParameterValue::Float(0.7 + (i % 13) as f32 * 0.05),
            NodeParameter::Slope => ParameterValue::Choice((i / 137 % 4) as u32),
            NodeParameter::Bank => ParameterValue::Choice((i / 191 % 5) as u32),
        })
    }
}
fn zero(info: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            info.count_total,
            info.bytes_total,
            info.count_current,
            info.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn dense_shared_nodes_checkpoint_retry_base_bypass_and_reset_allocate_nothing() {
    dense_scope(false);
}
#[test]
fn dense_mode_and_nodes_checkpoint_retry_base_bypass_and_reset_allocate_nothing() {
    dense_scope(true);
}
fn dense_scope(mode_grant: bool) {
    let mut state = EqState::default();
    let mut ids = if mode_grant {
        vec![hex_engine::eq::MODE, hex_engine::eq::OUTPUT_GAIN]
    } else {
        vec![hex_engine::eq::OUTPUT_GAIN]
    };
    for index in 1..=64 {
        let id = NodeId::new(index).unwrap();
        state.add(Node::new(id, 1000.0, 0.0, 0).unwrap()).unwrap();
        for field in NodeParameter::ALL {
            ids.push(id.parameter(field).unwrap());
        }
    }
    let factory = hex_engine::eq::EqDefinition::prepare_automation_component(
        &state.encode().unwrap(),
        48000,
        &ids,
    )
    .unwrap();
    let mut actual = factory.create_runtime();
    let mut reference = factory.create_runtime();
    let mut values = Values {
        ids,
        start: 0,
        bad: false,
    };
    let mut output = [[0.001, -0.0005]; 1024];
    let mut expected = output;
    for (i, block) in expected.chunks_mut(32).enumerate() {
        values.start = i * 32;
        reference
            .process_block_with_automation(block, None, &values, &R)
            .unwrap();
    }
    let mut failed = false;
    zero(allocation_counter::measure(|| {
        for (i, block) in output.chunks_mut(32).enumerate() {
            values.start = i * 32;
            failed |= actual
                .process_block_with_automation(block, None, &values, &R)
                .is_err();
        }
    }));
    assert!(!failed);
    assert_eq!(output, expected);
    values.start = 1024;
    values.bad = true;
    let mut rejected = [[0.001, -0.0005]; 32];
    let mut refused = false;
    zero(allocation_counter::measure(|| {
        refused = actual
            .process_block_with_automation(&mut rejected, None, &values, &R)
            .is_err();
        actual.copy_history_from(reference.as_ref());
    }));
    assert!(refused);
    values.bad = false;
    let mut a = [[0.001, -0.0005]; 32];
    let mut b = a;
    reference
        .process_block_with_automation(&mut b, None, &values, &R)
        .unwrap();
    zero(allocation_counter::measure(|| {
        failed = actual
            .process_block_with_automation(&mut a, None, &values, &R)
            .is_err();
    }));
    assert!(!failed);
    assert_eq!(a, b);
    let base = Values {
        ids: Vec::new(),
        start: 0,
        bad: false,
    };
    let mut dry = [[0.001, -0.0005]; 37];
    let mut discarded = dry;
    reference
        .process_block_with_automation(&mut discarded, None, &base, &R)
        .unwrap();
    zero(allocation_counter::measure(|| {
        failed = actual
            .process_bypassed_block_with_automation(&mut dry, None, &base, &R)
            .is_err();
    }));
    assert!(!failed);
    assert_eq!(dry, [[0.001, -0.0005]; 37]);
    let mut a = [[0.001, -0.0005]; 64];
    let mut b = a;
    reference
        .process_block_with_automation(&mut b, None, &base, &R)
        .unwrap();
    zero(allocation_counter::measure(|| {
        failed = actual
            .process_block_with_automation(&mut a, None, &base, &R)
            .is_err();
        actual.reset();
        reference.reset();
    }));
    assert!(!failed);
    assert_eq!(a, b);
}
