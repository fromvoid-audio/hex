use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, DeviceComparisonAction, DeviceVariant, Editor, Project};

fn legacy() -> (Editor, hex_project::DeviceId) {
    let mut e = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "legacy".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let state = hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap();
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
    (e, device)
}
#[test]
fn eq8_upgrade_refuses_a_retained_comparison_before_emission() {
    let (mut e, device) = legacy();
    e.execute(Command::CompareDevice {
        device,
        expected: DeviceVariant::A,
        action: DeviceComparisonAction::Switch,
    })
    .unwrap();
    let state = hex_devices::eq8::EQ8_DEFINITION
        .patch_parameter(
            e.project().device(device).unwrap().state(),
            hex_engine::eq::OUTPUT_GAIN,
            ParameterValue::Float(0.5),
        )
        .unwrap();
    e.execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let before = e.project().snapshot();
    let entry = e.project().device(device).unwrap();
    let target = EqState::upgrade_legacy(entry.state(), e.project().next_identity())
        .unwrap()
        .encode()
        .unwrap();
    assert!(
        prepare(entry, entry.state(), target).is_err(),
        "conversion must retain both authored a/b states rather than let provider replacement clear one"
    );
    assert_eq!(e.project().snapshot(), before);
}

use hex_lattice_contract::{DeviceAutomationTarget as Target, ParameterId};
use hex_project::{
    AutomationInterpolation as Curve, AutomationPoint, AutomationReadSelection as Read,
    AutomationValue, BeatTime, DeviceAutomation, DeviceAutomationReplacement,
    DeviceAutomationTouchSpan, DeviceAutomationTouchStep,
};

fn row(target: Target, kind: bool) -> DeviceAutomation {
    let values = if kind {
        vec![3.0 / 7.0, 4.0 / 7.0, 5.0 / 7.0, 6.0 / 7.0, 1.0]
    } else {
        vec![0.125, 0.875]
    };
    DeviceAutomation {
        target,
        vst3_class: None,
        read_selection: if target
            .provider_parameter()
            .is_some_and(|id| id.value() % 2 == 0)
        {
            Read::Base
        } else {
            Read::Authored
        },
        points: values
            .into_iter()
            .enumerate()
            .map(|(i, value)| AutomationPoint {
                beat: BeatTime::from_quarters(i as u64).unwrap(),
                value: AutomationValue::new(value).unwrap(),
                outgoing: if kind { Curve::Step } else { Curve::Linear },
            })
            .collect(),
        touch: vec![DeviceAutomationTouchSpan {
            start: BeatTime::from_ratio(1, 4).unwrap(),
            end: BeatTime::from_ratio(3, 4).unwrap(),
            steps: vec![
                DeviceAutomationTouchStep {
                    beat: BeatTime::from_ratio(1, 4).unwrap(),
                    value: AutomationValue::new(if kind { 3.0 / 7.0 } else { 0.25 }).unwrap(),
                },
                DeviceAutomationTouchStep {
                    beat: BeatTime::from_ratio(1, 2).unwrap(),
                    value: AutomationValue::new(if kind { 4.0 / 7.0 } else { 0.75 }).unwrap(),
                },
            ],
        }],
    }
}
fn command(
    e: &Editor,
    device: hex_project::DeviceId,
    state: NativeDeviceState,
    automation: Option<DeviceAutomationReplacement>,
    reserved_ids: u64,
) -> Command {
    let d = e.project().device(device).unwrap();
    Command::ReplaceDeviceStateWithIdentityReservation {
        device,
        expected_state: d.state().clone(),
        expected_variant: d.active_variant(),
        state,
        automation,
        expected_next_id: e.project().next_identity(),
        reserved_ids,
    }
}
fn install(e: &mut Editor, device: hex_project::DeviceId, mut rows: Vec<DeviceAutomation>) {
    rows.sort_by_key(|row| row.target);
    let d = e.project().device(device).unwrap();
    let command = command(
        e,
        device,
        d.state().clone(),
        Some(DeviceAutomationReplacement::new(d.automation().to_vec(), rows).unwrap()),
        0,
    );
    e.execute(command).unwrap();
}
fn upgrade(e: &Editor, device: hex_project::DeviceId) -> Result<Command, Invalid> {
    let d = e.project().device(device).unwrap();
    let state = EqState::upgrade_legacy(d.state(), e.project().next_identity())
        .unwrap()
        .encode()
        .unwrap();
    let (state, automation) = prepare(d, d.state(), state)?;
    Ok(command(e, device, state, automation, 40))
}
fn bells(e: &mut Editor, device: hex_project::DeviceId) {
    let mut state = e.project().device(device).unwrap().state().clone();
    for slot in 0..40 {
        state = hex_devices::eq8::EQ8_DEFINITION
            .patch_parameter(
                &state,
                ParameterId::new(slot * 5 + 2).unwrap(),
                ParameterValue::Choice(0),
            )
            .unwrap();
    }
    state = state
        .with_input_gain_millidb(6123)
        .unwrap()
        .with_output_gain_millidb(-3456)
        .unwrap();
    e.execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
}
#[test]
fn every_bank_slot_field_host_metadata_touch_and_history_converts_atomically() {
    let (mut e, device) = legacy();
    bells(&mut e, device);
    let mut rows: Vec<_> = (1..=202)
        .map(|raw| {
            row(
                Target::Parameter(ParameterId::new(raw).unwrap()),
                raw <= 200 && (raw - 1) % 5 == 1,
            )
        })
        .collect();
    rows.extend([
        row(Target::HostInputGain, false),
        row(Target::HostOutputGain, false),
    ]);
    install(&mut e, device, rows);
    let before = e.project().snapshot();
    let first = e.project().next_identity();
    let original = e.project().device(device).unwrap().automation().to_vec();
    let original_state = e.project().device(device).unwrap().state().clone();
    let command = upgrade(&e, device).unwrap();
    e.execute_prepared(command, |p| {
        hex_engine::PreparedRevision::prepare(
            p,
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .map(|_| ())
        .map_err(|_| Invalid("converted graph refused"))
    })
    .unwrap();
    let d = e.project().device(device).unwrap();
    assert_eq!(d.automation().len(), 244);
    assert_eq!(d.state().input_gain_millidb(), 6123);
    assert_eq!(d.state().output_gain_millidb(), -3456);
    assert_eq!(e.project().next_identity(), first + 40);
    let target_view = hex_engine::native_device_view(d.state()).unwrap();
    let source_view = hex_engine::native_device_view(&original_state).unwrap();
    for old in &original {
        let Some(id) = old.target.provider_parameter() else {
            assert!(d.automation().contains(old));
            continue;
        };
        if id.value() > 200 {
            assert!(d.automation().contains(old));
            continue;
        }
        let slot = (id.value() - 1) / 5;
        let field = [
            NodeParameter::Enabled,
            NodeParameter::Kind,
            NodeParameter::Frequency,
            NodeParameter::Gain,
            NodeParameter::Q,
        ][((id.value() - 1) % 5) as usize];
        let node = hex_engine::eq::NodeId::new(first + u64::from(slot)).unwrap();
        let target = node.parameter(field).unwrap();
        let mapped = d
            .automation()
            .iter()
            .find(|r| r.target == Target::Parameter(target))
            .unwrap();
        if field != NodeParameter::Kind {
            let mut expected = old.clone();
            expected.target = Target::Parameter(target);
            assert_eq!(mapped, &expected);
            let a = source_view
                .parameters
                .iter()
                .find(|p| p.descriptor.id == id)
                .unwrap();
            let b = target_view
                .parameters
                .iter()
                .find(|p| p.descriptor.id == target)
                .unwrap();
            for n in [0.0, 0.13, 0.5, 0.91, 1.0] {
                assert_eq!(
                    a.semantics.unwrap().denormalize(a.descriptor, n).unwrap(),
                    b.semantics.unwrap().denormalize(b.descriptor, n).unwrap()
                );
            }
        } else {
            let slope_id = node.parameter(NodeParameter::Slope).unwrap();
            let slope = d
                .automation()
                .iter()
                .find(|r| r.target == Target::Parameter(slope_id))
                .unwrap();
            assert_eq!(mapped.read_selection, old.read_selection);
            assert_eq!(slope.read_selection, old.read_selection);
            for ((a, b), (kind, slope_value)) in mapped.points.iter().zip(&slope.points).zip([
                (3, 0),
                (3, 3),
                (4, 0),
                (4, 3),
                (5, 0),
            ]) {
                assert_eq!(a.value.get(), kind as f32 / 5.0);
                assert_eq!(b.value.get(), slope_value as f32 / 3.0);
                assert_eq!(a.outgoing, Curve::Step);
            }
            assert_eq!(mapped.touch[0].start, old.touch[0].start);
            assert_eq!(mapped.touch[0].end, old.touch[0].end);
            assert_eq!(mapped.touch[0].steps[0].value.get(), 3.0 / 5.0);
            assert_eq!(slope.touch[0].steps[1].value.get(), 1.0);
        }
    }
    let after = e.project().snapshot();
    let path =
        std::env::temp_dir().join(format!("hex-eq8-shared-upgrade-{}.hex", std::process::id()));
    hex_storage::save_new(&path, e.project()).unwrap();
    let loaded = hex_storage::load(&path);
    std::fs::remove_file(path).unwrap();
    assert_eq!(&loaded.unwrap(), e.project());
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}
#[test]
fn ambiguous_kind_shelf_unknown_and_changed_mapping_refuse_without_mutation() {
    for case in 0..5 {
        let (mut e, device) = legacy();
        bells(&mut e, device);
        let mut lane = row(Target::Parameter(ParameterId::from_static(2)), true);
        match case {
            0 => {
                lane.points[0].outgoing = Curve::Linear;
            }
            1 => {
                lane.points[0].value = AutomationValue::new(1.0 / 7.0).unwrap();
            }
            2 => {
                lane.target = Target::Parameter(ParameterId::from_static(999));
            }
            3 => {
                let old = e.project().device(device).unwrap().state();
                let state = hex_devices::eq8::EQ8_DEFINITION
                    .patch_parameter(old, ParameterId::from_static(2), ParameterValue::Choice(1))
                    .unwrap();
                e.execute(Command::ReplaceDeviceState { device, state })
                    .unwrap();
                lane = row(Target::Parameter(ParameterId::from_static(5)), false);
            }
            _ => {}
        }
        install(&mut e, device, vec![lane]);
        let before = e.project().snapshot();
        let d = e.project().device(device).unwrap();
        let mut target = EqState::upgrade_legacy(d.state(), e.project().next_identity()).unwrap();
        if case == 4 {
            target.output_gain = 0.75;
        }
        assert!(
            prepare(d, d.state(), target.encode().unwrap()).is_err(),
            "case {case}"
        );
        assert_eq!(e.project().snapshot(), before);
    }
}
#[test]
fn equal_kind_buckets_keep_linear_flags_and_mode_output_only_still_replace_inventory() {
    let (mut e, device) = legacy();
    bells(&mut e, device);
    let mut lane = row(Target::Parameter(ParameterId::from_static(2)), true);
    lane.points.truncate(2);
    lane.points[0].value = AutomationValue::new(0.4).unwrap();
    lane.points[1].value = AutomationValue::new(0.45).unwrap();
    lane.points[0].outgoing = Curve::Linear;
    install(&mut e, device, vec![lane]);
    e.execute(upgrade(&e, device).unwrap()).unwrap();
    let d = e.project().device(device).unwrap();
    assert_eq!(d.automation()[0].points[0].outgoing, Curve::Linear);
    let (mut e, device) = legacy();
    install(
        &mut e,
        device,
        vec![
            row(Target::Parameter(hex_engine::eq::MODE), false),
            row(Target::Parameter(hex_engine::eq::OUTPUT_GAIN), false),
        ],
    );
    let before = e.project().snapshot();
    e.execute(upgrade(&e, device).unwrap()).unwrap();
    assert_eq!(e.project().device(device).unwrap().automation().len(), 2);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}
#[test]
fn prepare_refusal_retains_original_state_inventory_and_allocator() {
    let (mut e, device) = legacy();
    bells(&mut e, device);
    install(
        &mut e,
        device,
        vec![row(Target::Parameter(ParameterId::from_static(3)), false)],
    );
    let before = e.project().snapshot();
    assert!(
        e.execute_prepared(upgrade(&e, device).unwrap(), |_| Err::<(), _>(Invalid(
            "injected preparation refusal"
        )))
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}
