use super::*;
use crate::{
    Command, Created, DeviceAutomationTarget, DeviceModulationRoute, DeviceVariant, Editor,
    ElementColor, ModulationAmount, ModulationOutputId, ModulationRange, Modulator,
};
use hex_lattice_contract::{DeviceAssetSlot, DeviceTypeId};

fn fixture(shift_ids: bool) -> (Project, DeviceId, AssetId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    if shift_ids {
        editor
            .execute(Command::AddMidiTrack {
                name: "unrelated".into(),
            })
            .unwrap();
    }
    let mut asset = |name: &str| {
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: name.into(),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        asset
    };
    let a = asset("not-portable-active.wav");
    let b = asset("not-portable-inactive.wav");
    let state = |asset: Option<AssetId>| {
        let state = NativeDeviceState::new(DeviceTypeId::from_static("test.unit"), 1, vec![3, 7])
            .unwrap()
            .with_output_gain_millidb(-1800)
            .unwrap();
        if let Some(asset) = asset {
            state
                .with_asset_references(vec![DeviceAssetReference {
                    slot: DeviceAssetSlot::new(1).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                }])
                .unwrap()
        } else {
            state
        }
    };
    let Created::Device(root) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(None),
        })
        .unwrap()
    else {
        panic!("root")
    };
    let Created::Device(child) = editor
        .execute(Command::InsertNestedDevice {
            parent: root,
            state: state(Some(a)),
        })
        .unwrap()
    else {
        panic!("child")
    };
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(None),
        })
        .unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device: child,
            bypassed: true,
        })
        .unwrap();

    let first = editor.project().snapshot().next_id;
    let macro_id = ModulationSourceId::new(first).unwrap();
    let lfo_id = ModulationSourceId::new(first + 1).unwrap();
    let macro_type = DeviceTypeId::from_static("hex.modulator.macro");
    let route = |id, source, destination| DeviceModulationRoute {
        id: ModulationRouteId::new(id).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(0.5).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    let modulation = DeviceModulation::new(
        vec![
            Modulator {
                id: macro_id,
                color: ElementColor::Cyan,
                enabled: true,
                state: NativeDeviceState::new(macro_type, 1, 0.25_f32.to_le_bytes().to_vec())
                    .unwrap(),
            },
            Modulator {
                id: lfo_id,
                color: ElementColor::default(),
                enabled: true,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("hex.modulator.lfo"),
                    1,
                    vec![1, 2],
                )
                .unwrap(),
            },
        ],
        vec![
            route(
                first + 2,
                macro_id,
                ModulationDestination::Device {
                    device_type: state(None).type_id(),
                    target: DeviceAutomationTarget::HostOutputGain,
                    vst3_class: None,
                },
            ),
            route(
                first + 3,
                lfo_id,
                ModulationDestination::Modulator {
                    source: macro_id,
                    module_type: macro_type,
                    parameter: hex_lattice_contract::ParameterId::from_static(1),
                },
            ),
        ],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: root,
            modulation,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.main_devices[1].comparison =
        Some(DeviceComparison::new(DeviceVariant::B, state(Some(b))));
    for (id, byte) in [(a, 1), (b, 2)] {
        let stored = snapshot
            .assets
            .iter_mut()
            .find(|asset| asset.id == id.value())
            .unwrap();
        stored.original_source =
            Some(OriginalSourceFacts::new(OriginalSourceFacts::LAW_VERSION, [byte; 32]).unwrap());
    }
    (Project::from_snapshot(snapshot).unwrap(), root, b)
}

#[test]
fn capture_preserves_whole_unit_without_project_or_media_identities() {
    let (project, root, _) = fixture(false);
    let (different_project, shifted_root, _) = fixture(true);
    assert_ne!(root, shifted_root);
    let unit = project.capture_portable_device_unit(root).unwrap();
    assert_eq!(
        unit,
        different_project
            .capture_portable_device_unit(shifted_root)
            .unwrap()
    );
    assert_eq!(unit.class(), DeviceUnitClass::Effect);
    assert_eq!(unit.nodes().len(), 2);
    let [first, child] = unit.nodes() else {
        panic!("two unit nodes")
    };
    assert_eq!(first.parent, None);
    assert_eq!(child.parent, Some(0));
    assert!(child.bypassed);
    assert_eq!(child.state.output_gain_millidb(), -1800);
    let bank = &first.modulation;
    assert_eq!(bank.sources()[0].color, ElementColor::Cyan);
    assert_eq!(bank.sources()[0].id.value(), 1);
    assert_eq!(bank.sources()[1].id.value(), 2);
    assert_eq!(bank.routes()[0].id.value(), 3);
    assert_eq!(bank.routes()[0].source, bank.sources()[0].id);
    assert_eq!(bank.routes()[1].id.value(), 4);
    assert_eq!(bank.routes()[1].source, bank.sources()[1].id);
    assert!(
        matches!(bank.routes()[1].destination, ModulationDestination::Modulator { source, .. }
        if source == bank.sources()[0].id)
    );
    assert_eq!(child.state.asset_references()[0].asset.value(), 1);
    let comparison = child.comparison.as_ref().unwrap();
    assert_eq!(comparison.active(), DeviceVariant::B);
    assert_eq!(comparison.inactive().asset_references()[0].asset.value(), 2);
    assert_eq!(unit.assets()[0].facts.content_digest(), [1; 32]);
    assert_eq!(unit.assets()[1].facts.content_digest(), [2; 32]);
    assert_eq!(unit.assets()[0].source_frames, 16);
}

#[test]
fn malformed_templates_and_unverified_inactive_assets_refuse() {
    let (mut project, root, inactive) = fixture(false);
    let unit = project.capture_portable_device_unit(root).unwrap();
    let mut nodes = unit.nodes().to_vec();
    nodes[1].parent = Some(1);
    assert!(PortableDeviceUnit::new(unit.class(), nodes, unit.assets().to_vec()).is_err());
    assert!(
        PortableDeviceUnit::new(unit.class(), unit.nodes().to_vec(), vec![unit.assets()[0]])
            .is_err()
    );
    project.assets.get_mut(&inactive).unwrap().original_source = None;
    assert_eq!(
        project.capture_portable_device_unit(root).unwrap_err(),
        Invalid("device asset has no verified original content identity")
    );
}

#[test]
fn retained_instrument_root_excludes_a_sibling_effect_and_reroots_its_child() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let state = crate::default_instrument_state().unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "retained".into(),
            devices: vec![state.clone(), state.clone()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let Created::Device(child) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state,
        })
        .unwrap()
    else {
        panic!("child")
    };
    let unit = editor
        .project()
        .capture_portable_device_unit(instrument)
        .unwrap();
    assert_eq!(unit.class(), DeviceUnitClass::Instrument);
    assert_eq!(unit.nodes().len(), 2);
    let child = editor
        .project()
        .capture_portable_device_unit(child)
        .unwrap();
    assert_eq!(child.class(), DeviceUnitClass::Effect);
    assert_eq!(child.nodes().len(), 1);
    assert_eq!(child.nodes()[0].parent, None);
}

#[test]
fn portable_unit_rejects_out_of_range_nesting_before_decoding_can_admit_it() {
    let (project, root, _) = fixture(false);
    let unit = project.capture_portable_device_unit(root).unwrap();
    let mut nodes = (0..crate::device_inventory::MAX_SERIAL_DEVICE_DEPTH)
        .map(|index| PortableDeviceNode {
            parent: index.checked_sub(1),
            ..unit.nodes()[0].clone()
        })
        .collect::<Vec<_>>();
    assert!(PortableDeviceUnit::new(unit.class(), nodes.clone(), Vec::new()).is_ok());
    nodes.push(PortableDeviceNode {
        parent: Some(nodes.len() - 1),
        ..unit.nodes()[0].clone()
    });
    assert_eq!(
        PortableDeviceUnit::new(unit.class(), nodes, Vec::new()).unwrap_err(),
        Invalid("portable device nesting exceeds its limit")
    );
}

#[test]
fn portable_capture_preserves_authored_vst3_bytes_but_drops_process_local_approval() {
    use hex_lattice_contract::{VST3_DEVICE_TYPE_ID, Vst3StateAuthority};
    let authority = Vst3StateAuthority::new_on_control().unwrap();
    let verifier = authority.verifier();
    let state = authority
        .approve_processing_on_control(
            NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![9, 2, 3]).unwrap(),
        )
        .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(root) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("main device");
    };
    let authored = editor.project().device(root).unwrap().state();
    assert!(verifier.approves_processing(authored));
    let unit = editor.project().capture_portable_device_unit(root).unwrap();
    let captured = &unit.nodes()[0].state;
    assert_eq!(captured, authored);
    assert!(!verifier.approves(captured));
    assert!(!verifier.approves_processing(captured));
    assert!(verifier.approves_processing(authored));
}
