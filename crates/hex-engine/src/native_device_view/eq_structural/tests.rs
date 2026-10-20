use super::*;
use hex_devices::{eq, eq8};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

fn legacy(mode: u32) -> NativeDeviceState {
    let definition = &eq8::EQ8_DEFINITION;
    let mut state = definition.default_state().unwrap();
    for bank in 0..5 {
        for slot in 0..8 {
            for (parameter, value) in [
                (
                    eq8::parameter::kind(bank, slot),
                    ParameterValue::Choice(slot as u32),
                ),
                (
                    eq8::parameter::enabled(bank, slot),
                    ParameterValue::Bool(slot != 7),
                ),
                (
                    eq8::parameter::frequency(bank, slot),
                    ParameterValue::Float(100.0 + (slot * 200 + bank * 75) as f32),
                ),
                (
                    eq8::parameter::gain(bank, slot),
                    ParameterValue::Float(if slot % 2 == 0 { 3.0 } else { -4.0 }),
                ),
                (eq8::parameter::q(bank, slot), ParameterValue::Float(0.7)),
            ] {
                state = definition
                    .patch_parameter(&state, parameter, value)
                    .unwrap();
            }
        }
    }
    state = definition
        .patch_parameter(&state, eq8::parameter::MODE, ParameterValue::Choice(mode))
        .unwrap();
    state = definition
        .patch_parameter(
            &state,
            eq8::parameter::OUTPUT_GAIN,
            ParameterValue::Float(0.75),
        )
        .unwrap();
    state
        .with_input_gain_millidb(-3000)
        .unwrap()
        .with_output_gain_millidb(4500)
        .unwrap()
}

fn node(id: u64) -> Node {
    Node::new(NodeId::new(id).unwrap(), 1800.0, -3.0, 4).unwrap()
}

fn assert_refused(current: &NativeDeviceState, cursor: u64, op: EqStructuralEdit) {
    let before = current.clone();
    assert!(eq_structural_on_control(current, cursor, op).is_err());
    assert_eq!(
        current, &before,
        "refusal preserves the actual caller payload and host metadata"
    );
}

#[test]
fn add_final_copy_preserves_all_banks_and_reserves_the_actual_preset_cursor_gap() {
    let old = legacy(2);
    let canonical = EqState::upgrade_legacy(&old, 100).unwrap();
    let current = canonical
        .encode()
        .unwrap()
        .with_input_gain_millidb(-3000)
        .unwrap()
        .with_output_gain_millidb(4500)
        .unwrap();
    let before = current.clone();
    for project_cursor in [50, 900] {
        let first = project_cursor.max(140);
        let mut final_node = node(first);
        final_node.kind = eq::NodeKind::HighCut;
        final_node.q = 1.25;
        final_node.slope = 3;
        final_node.enabled = false;
        let result = eq_structural_on_control(
            &current,
            project_cursor,
            EqStructuralEdit::AddNode(final_node),
        )
        .unwrap();
        let decoded = EqState::decode(&result.state).unwrap();
        assert_eq!(decoded.nodes[..40], canonical.nodes);
        assert_eq!(decoded.nodes[40], final_node);
        assert_eq!(decoded.mode, canonical.mode);
        assert_eq!(decoded.output_gain, canonical.output_gain);
        assert_eq!(decoded.lanes, canonical.lanes);
        assert_eq!(decoded.next_node_id, first as u32 + 1);
        assert_eq!(result.created_id, Some(final_node.id));
        assert_eq!(result.next_project_id, first + 1);
        assert_eq!(result.reserved_ids, first + 1 - project_cursor);
        assert_eq!(result.state.input_gain_millidb(), -3000);
        assert_eq!(result.state.output_gain_millidb(), 4500);
    }
    assert_eq!(current, before);
}

#[test]
fn removal_retires_only_associated_lanes_without_reusing_or_reserving_identity() {
    let mut authored = EqState::upgrade_legacy(&legacy(1), 100).unwrap();
    let removed = authored.nodes[8].id;
    let retained = authored.nodes[32].id;
    for id in [removed, retained] {
        authored
            .set_lane(eq::AutomationLane {
                node: id,
                parameter: eq::NodeParameter::Gain,
                points: vec![
                    eq::AutomationPoint {
                        beat: 0,
                        normalized: 0.25,
                    },
                    eq::AutomationPoint {
                        beat: 4 << 32,
                        normalized: 0.75,
                    },
                ],
            })
            .unwrap();
    }
    let current = authored.encode().unwrap();
    let result =
        eq_structural_on_control(&current, 17, EqStructuralEdit::RemoveNode(removed)).unwrap();
    let actual = EqState::decode(&result.state).unwrap();
    let mut expected = authored.clone();
    expected.remove(removed).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.nodes.len(), 39);
    assert_eq!(actual.lanes.len(), 1);
    assert_eq!(actual.lanes[0].node, retained);
    assert_eq!(actual.next_node_id, authored.next_node_id);
    assert_eq!(
        actual
            .node(retained)
            .unwrap()
            .id
            .parameter(eq::NodeParameter::Frequency),
        retained.parameter(eq::NodeParameter::Frequency)
    );
    assert_eq!(result.reserved_ids, 0);
    assert_eq!(result.next_project_id, 17);
    assert_eq!(result.created_id, None);
    assert_refused(&result.state, 17, EqStructuralEdit::RemoveNode(removed));
    assert_eq!(EqState::decode(&current).unwrap(), authored);
}

#[test]
fn upgrade_preserves_forty_authored_slots_all_modes_and_host_trims() {
    for mode in 0..3 {
        let current = legacy(mode);
        let before = current.clone();
        let result = eq_structural_on_control(&current, 400, EqStructuralEdit::UpgradeEq8).unwrap();
        let actual = EqState::decode(&result.state).unwrap();
        assert_eq!(actual, EqState::upgrade_legacy(&current, 400).unwrap());
        assert_eq!(actual.nodes.len(), 40);
        assert_eq!(actual.mode, mode as u8);
        assert_eq!(actual.output_gain, 0.75);
        for (index, entry) in actual.nodes.iter().enumerate() {
            let slot = index % 8;
            assert_eq!(entry.id.value(), 400 + index as u32);
            assert_eq!(entry.bank, (index / 8) as u8);
            assert_eq!(entry.enabled, slot != 7);
            assert_eq!(
                entry.frequency_hz,
                100.0 + (slot * 200 + index / 8 * 75) as f32
            );
            assert_eq!(entry.q, 0.7);
        }
        assert_eq!(result.reserved_ids, 40);
        assert_eq!(result.next_project_id, 440);
        assert_eq!(result.created_id, None);
        assert_eq!(result.state.type_id(), eq::EQ_TYPE_ID);
        assert_eq!(result.state.state_version(), eq::EQ_STATE_VERSION);
        assert_eq!(result.state.input_gain_millidb(), -3000);
        assert_eq!(result.state.output_gain_millidb(), 4500);
        assert_eq!(current, before);
    }
}

#[test]
fn full_stale_malformed_wrong_type_and_invalid_final_node_refuse_atomically() {
    let empty = EqState::default().encode().unwrap();
    assert_refused(&empty, 0, EqStructuralEdit::AddNode(node(1)));
    assert_refused(&empty, 1, EqStructuralEdit::AddNode(node(2)));
    assert_refused(&empty, 10, EqStructuralEdit::AddNode(node(1)));
    assert_refused(&empty, 1, EqStructuralEdit::UpgradeEq8);
    assert_refused(&legacy(0), 1, EqStructuralEdit::AddNode(node(1)));
    assert_refused(
        &legacy(0),
        1,
        EqStructuralEdit::RemoveNode(NodeId::new(1).unwrap()),
    );
    for field in 0..5 {
        let mut invalid = node(1);
        match field {
            0 => invalid.frequency_hz = f32::NAN,
            1 => invalid.gain_db = f32::INFINITY,
            2 => invalid.q = 0.0,
            3 => invalid.bank = 5,
            _ => invalid.slope = 4,
        }
        assert_refused(&empty, 1, EqStructuralEdit::AddNode(invalid));
    }
    let wrong_version =
        NativeDeviceState::new(eq::EQ_TYPE_ID, u32::MAX, empty.payload().to_vec()).unwrap();
    let truncated = NativeDeviceState::new(
        eq::EQ_TYPE_ID,
        eq::EQ_STATE_VERSION,
        empty.payload()[..3].to_vec(),
    )
    .unwrap();
    for invalid in [
        wrong_version,
        truncated,
        hex_devices::utility::UTILITY_DEFINITION
            .default_state()
            .unwrap(),
    ] {
        assert_refused(&invalid, 1, EqStructuralEdit::AddNode(node(1)));
    }
    let bound = empty
        .clone()
        .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
            slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
            asset: hex_lattice_contract::DeviceAssetKey::new(1).unwrap(),
        }])
        .unwrap();
    assert_refused(&bound, 1, EqStructuralEdit::AddNode(node(1)));
    let mut full = EqState::default();
    for id in 1..=64 {
        full.add(node(id)).unwrap();
    }
    assert_refused(
        &full.encode().unwrap(),
        65,
        EqStructuralEdit::AddNode(node(65)),
    );
}

#[test]
fn parameter_identity_boundary_and_exhausted_upgrade_never_wrap() {
    let maximum = u64::from((hex_lattice_contract::MAX_PARAMETER_ID - 1006) / 8);
    let current = EqState::default().encode().unwrap();
    let final_id = node(maximum);
    let result =
        eq_structural_on_control(&current, maximum, EqStructuralEdit::AddNode(final_id)).unwrap();
    assert_eq!(result.reserved_ids, 1);
    assert_eq!(result.next_project_id, maximum + 1);
    assert_refused(
        &result.state,
        maximum + 1,
        EqStructuralEdit::AddNode(final_id),
    );
    assert_refused(&current, u64::MAX, EqStructuralEdit::AddNode(node(1)));
    let old = legacy(0);
    let last = eq_structural_on_control(&old, maximum - 39, EqStructuralEdit::UpgradeEq8).unwrap();
    assert_eq!(
        EqState::decode(&last.state).unwrap().nodes[39].id,
        final_id.id
    );
    assert_refused(&old, maximum - 38, EqStructuralEdit::UpgradeEq8);
    assert_refused(&old, u64::MAX, EqStructuralEdit::UpgradeEq8);
}

#[test]
fn codec_result_does_not_replace_actual_rate_dsp_admission() {
    let definition = &eq8::EQ8_DEFINITION;
    let mut current = definition.default_state().unwrap();
    for (parameter, value) in [
        (eq8::parameter::kind(0, 0), ParameterValue::Choice(1)),
        (eq8::parameter::q(0, 0), ParameterValue::Float(18.0)),
        (eq8::parameter::gain(0, 0), ParameterValue::Float(24.0)),
        (eq8::parameter::enabled(0, 0), ParameterValue::Bool(true)),
    ] {
        current = definition
            .patch_parameter(&current, parameter, value)
            .unwrap();
    }
    let before = current.clone();
    let result = eq_structural_on_control(&current, 100, EqStructuralEdit::UpgradeEq8).unwrap();
    assert!(EqState::decode(&result.state).is_ok());
    assert!(crate::native_effect_frequency_response(&result.state, 48_000, &[1000.0]).is_err());
    assert_eq!(current, before);
}
