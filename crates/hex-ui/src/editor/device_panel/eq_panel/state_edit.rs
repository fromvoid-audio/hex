//! Control-side EQ topology edit adapter. Stable removed node identities prune
//! only their seven known parameter targets; all other authored rows survive.
//! Automation remains device-global across A/B. The model owns the exact
//! inventory/state/variant/allocator comparison and one-step history publication.
//! No DSP, curve evaluation, resource custody or window interaction lives here.
use hex_engine::eq::{EQ_TYPE_ID, EqState, NodeParameter};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Device, DeviceAutomationReplacement, Invalid};

#[path = "state_edit/legacy.rs"]
mod legacy;

const EQ8_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.eq8");

pub(super) fn prepare(
    entry: &Device,
    expected: &NativeDeviceState,
    state: NativeDeviceState,
) -> Result<(NativeDeviceState, Option<DeviceAutomationReplacement>), Invalid> {
    if entry.state() != expected {
        return Err(Invalid("eq source changed; refresh and retry the edit"));
    }
    if state.type_id() != EQ_TYPE_ID || ![EQ_TYPE_ID, EQ8_TYPE_ID].contains(&expected.type_id()) {
        return Err(Invalid("eq edit has an unsupported provider"));
    }
    expected
        .require_asset_free_processing()
        .and_then(|_| state.require_asset_free_processing())
        .map_err(|_| Invalid("eq edit cannot discard unsupported asset bindings"))?;
    let next = EqState::decode(&state).map_err(|_| Invalid("eq edit state is malformed"))?;
    let automation = if expected.type_id() == EQ8_TYPE_ID {
        legacy::prepare(entry, expected, &state, &next)?
    } else {
        let prior =
            EqState::decode(expected).map_err(|_| Invalid("eq source state is malformed"))?;
        let mut removed = Vec::new();
        for node in prior.nodes {
            if next.nodes.iter().any(|candidate| candidate.id == node.id) {
                continue;
            }
            for field in NodeParameter::ALL {
                removed.push(
                    node.id
                        .parameter(field)
                        .map_err(|_| Invalid("eq node parameter identity is invalid"))?,
                );
            }
        }
        removed.sort_unstable();
        let is_removed = |row: &hex_project::DeviceAutomation| {
            row.target
                .provider_parameter()
                .is_some_and(|id| removed.binary_search(&id).is_ok())
        };
        if entry.automation().iter().any(is_removed) {
            Some(DeviceAutomationReplacement::new(
                entry.automation().to_vec(),
                entry
                    .automation()
                    .iter()
                    .filter(|row| !is_removed(row))
                    .cloned()
                    .collect(),
            )?)
        } else {
            None
        }
    };
    // Both supported EQ formats are asset-free. Copy neutral host trims exactly,
    // including an EQ8 upgrade where same-provider metadata copying is unavailable.
    let state = state
        .with_input_gain_millidb(expected.input_gain_millidb())
        .and_then(|state| state.with_output_gain_millidb(expected.output_gain_millidb()))
        .map_err(|_| Invalid("eq host gain metadata is invalid"))?;
    Ok((state, automation))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_engine::eq::{Node, NodeId, OUTPUT_GAIN};
    use hex_lattice_contract::{
        DeviceAutomationTarget as Target, NativeDeviceMetadata, ParameterId,
    };
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
        BeatTime, Command, Created, Editor, Project,
    };
    fn fixture() -> (Editor, hex_project::DeviceId, EqState) {
        let mut e = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Track(track) = e.execute(Command::AddTrack { name: "eq".into() }).unwrap()
        else {
            panic!("track")
        };
        let mut eq = EqState::default();
        for id in [7, 11] {
            eq.add(Node::new(NodeId::new(id).unwrap(), 1000.0, 0.0, 0).unwrap())
                .unwrap();
        }
        let state = eq
            .encode()
            .unwrap()
            .with_input_gain_millidb(6123)
            .unwrap()
            .with_output_gain_millidb(-3456)
            .unwrap();
        let Created::Device(device) = e
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device")
        };
        (e, device, eq)
    }
    fn lane(e: &mut Editor, device: hex_project::DeviceId, target: Target) {
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.4).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    }
    #[test]
    fn opaque_state_reencoding_preserves_host_trims_without_float_rounding() {
        let (e, device, eq) = fixture();
        let entry = e.project().device(device).unwrap();
        let (state, automation) = prepare(entry, entry.state(), eq.encode().unwrap()).unwrap();
        assert_eq!(state.input_gain_millidb(), 6123);
        assert_eq!(state.output_gain_millidb(), -3456);
        assert!(automation.is_none());
    }
    #[test]
    fn removed_node_prunes_only_known_targets_and_undo_restores_complete_inventory() {
        let (mut e, device, mut eq) = fixture();
        let removed: Vec<_> = NodeParameter::ALL
            .into_iter()
            .map(|field| NodeId::new(7).unwrap().parameter(field).unwrap())
            .collect();
        for &id in &removed {
            lane(&mut e, device, Target::Parameter(id));
        }
        let retained = NodeId::new(11)
            .unwrap()
            .parameter(NodeParameter::Gain)
            .unwrap();
        let unknown = ParameterId::from_static(1063); // Reserved eighth slot is not a known field.
        for target in [
            Target::Parameter(retained),
            Target::Parameter(unknown),
            Target::Parameter(OUTPUT_GAIN),
            Target::HostInputGain,
        ] {
            lane(&mut e, device, target);
        }
        e.execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: Target::Parameter(retained),
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
        let before = e.project().snapshot();
        let entry = e.project().device(device).unwrap();
        let expected = entry.state().clone();
        let variant = entry.active_variant();
        let inventory = entry.automation().to_vec();
        eq.remove(NodeId::new(7).unwrap()).unwrap();
        let (state, change) = prepare(entry, &expected, eq.encode().unwrap()).unwrap();
        let change = change.unwrap();
        assert_eq!(change.expected(), inventory);
        let kept: Vec<_> = inventory
            .into_iter()
            .filter(|row| {
                !row.target
                    .provider_parameter()
                    .is_some_and(|id| removed.contains(&id))
            })
            .collect();
        assert_eq!(change.replacement(), kept);
        e.execute(Command::ReplaceDeviceStateWithIdentityReservation {
            automation: Some(change),
            device,
            expected_state: expected,
            expected_variant: variant,
            state,
            expected_next_id: e.project().next_identity(),
            reserved_ids: 0,
        })
        .unwrap();
        assert_eq!(e.project().device(device).unwrap().automation(), kept);
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
    }
    #[test]
    fn no_op_creation_preserve_inventory_and_legacy_upgrade_is_explicit() {
        let (mut e, device, mut eq) = fixture();
        lane(
            &mut e,
            device,
            Target::Parameter(
                NodeId::new(7)
                    .unwrap()
                    .parameter(NodeParameter::Gain)
                    .unwrap(),
            ),
        );
        let entry = e.project().device(device).unwrap();
        assert!(
            prepare(entry, entry.state(), eq.encode().unwrap())
                .unwrap()
                .1
                .is_none()
        );
        eq.add(Node::new(NodeId::new(29).unwrap(), 1500.0, 0.0, 0).unwrap())
            .unwrap();
        assert!(
            prepare(entry, entry.state(), eq.encode().unwrap())
                .unwrap()
                .1
                .is_none()
        );
        let Created::Track(track) = e
            .execute(Command::AddTrack {
                name: "legacy".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let old = hex_devices::eq8::EQ8_DEFINITION
            .default_state()
            .unwrap()
            .with_input_gain_millidb(6123)
            .unwrap()
            .with_output_gain_millidb(-3456)
            .unwrap();
        let Created::Device(legacy) = e
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: old,
            })
            .unwrap()
        else {
            panic!("device")
        };
        lane(&mut e, legacy, Target::Parameter(OUTPUT_GAIN));
        let entry = e.project().device(legacy).unwrap();
        let upgraded = EqState::upgrade_legacy(entry.state(), 1000)
            .unwrap()
            .encode()
            .unwrap();
        let (state, change) = prepare(entry, entry.state(), upgraded).unwrap();
        let change = change.unwrap();
        assert_eq!(change.expected(), entry.automation());
        assert_eq!(change.replacement(), entry.automation());
        assert_eq!(EqState::decode(&state).unwrap().nodes.len(), 40);
        assert_eq!(state.input_gain_millidb(), 6123);
        assert_eq!(state.output_gain_millidb(), -3456);
    }
    #[test]
    fn stale_or_malformed_sources_refuse_without_changing_authored_data() {
        let (e, device, eq) = fixture();
        let before = e.project().snapshot();
        let entry = e.project().device(device).unwrap();
        let stale = entry.state().clone().with_input_gain_db(9.0).unwrap();
        assert!(prepare(entry, &stale, eq.encode().unwrap()).is_err());
        let malformed = NativeDeviceState::new(EQ_TYPE_ID, 1, vec![0]).unwrap();
        assert!(prepare(entry, entry.state(), malformed).is_err());
        assert_eq!(e.project().snapshot(), before);
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[path = "state_edit/legacy_tests.rs"]
mod legacy_tests;
