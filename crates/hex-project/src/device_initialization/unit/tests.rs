use super::*;
use crate::{
    DeviceId, DeviceModulation, DeviceModulationRoute, DeviceOwner, DeviceUnitClass, DeviceVariant,
    Editor, ElementColor, ModulationAmount, ModulationDestination, ModulationOutputId,
    ModulationRange, ModulationRouteId, ModulationSourceId, Modulator, OriginalSourceFacts,
    PortableDeviceAsset, PortableDeviceNode, default_instrument_state,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceAutomationTarget, DeviceTypeId,
};

fn effect(payload: u8) -> NativeDeviceState {
    NativeDeviceState::new(
        DeviceTypeId::from_static("test.complete.unit"),
        1,
        vec![payload],
    )
    .unwrap()
}

fn run_device(editor: &mut Editor, command: Command) -> DeviceId {
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("device")
    };
    device
}

fn add_track(editor: &mut Editor, name: &str) -> crate::TrackId {
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack { name: name.into() })
        .unwrap()
    else {
        panic!("track")
    };
    track
}

fn init(command: Command, unit: PortableDeviceUnit, assets: &[AssetId]) -> Command {
    Command::InitializeDevice(Box::new(
        super::super::DeviceInitialization::new_unit(command, unit, assets.to_vec()).unwrap(),
    ))
}

fn asset_facts(tag: u8) -> OriginalSourceFacts {
    OriginalSourceFacts::new(OriginalSourceFacts::LAW_VERSION, [tag; 32]).unwrap()
}

fn bound(state: NativeDeviceState, asset: u64) -> NativeDeviceState {
    state
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset).unwrap(),
        }])
        .unwrap()
}

fn root_with_media() -> PortableDeviceNode {
    let source = ModulationSourceId::new(1).unwrap();
    let state = effect(9).with_output_gain_millidb(-2400).unwrap();
    PortableDeviceNode {
        parent: None,
        state: bound(state.clone(), 1),
        comparison: Some(DeviceComparison::new(DeviceVariant::B, bound(state, 2))),
        bypassed: true,
        modulation: DeviceModulation::new(
            vec![Modulator {
                id: source,
                color: ElementColor::Cyan,
                enabled: true,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("hex.modulator.macro"),
                    1,
                    vec![1, 2],
                )
                .unwrap(),
            }],
            vec![DeviceModulationRoute {
                id: ModulationRouteId::new(2).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type: effect(9).type_id(),
                    target: DeviceAutomationTarget::HostOutputGain,
                    vst3_class: None,
                },
                amount: ModulationAmount::new(0.3).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            }],
        )
        .unwrap(),
    }
}

fn node(parent: Option<usize>, state: NativeDeviceState) -> PortableDeviceNode {
    PortableDeviceNode {
        parent,
        state,
        comparison: None,
        bypassed: false,
        modulation: DeviceModulation::default(),
    }
}

#[test]
fn restore_remaps_active_inactive_and_mapping_ids_in_one_edit() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let _unrelated = add_track(&mut editor, "unrelated");
    let assets = ["another-project-active.wav", "another-project-inactive.wav"].map(|path| {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: path.into(),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        id
    });
    let mut snapshot = editor.project().snapshot();
    for (asset, tag) in snapshot.assets.iter_mut().zip([7, 8]) {
        asset.original_source = Some(asset_facts(tag));
    }
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let unit = PortableDeviceUnit::new(
        DeviceUnitClass::Effect,
        vec![
            root_with_media(),
            node(Some(0), effect(6)),
            node(Some(1), effect(7)),
        ],
        [7, 8]
            .into_iter()
            .map(|tag| PortableDeviceAsset {
                source_rate: 48_000,
                source_frames: 16,
                facts: asset_facts(tag),
            })
            .collect(),
    )
    .unwrap();
    let before = editor.project().snapshot();
    let insert = || Command::InsertMainDevice {
        before: None,
        state: effect(0),
    };
    assert!(
        editor
            .execute(init(insert(), unit.clone(), &[assets[1], assets[0]]))
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    let refused = editor.execute_prepared(init(insert(), unit.clone(), &assets), |candidate| {
        assert_eq!(candidate.main_devices().len(), 3);
        Err::<(), _>(Invalid("prepared unit refused"))
    });
    assert_eq!(refused, Err(Invalid("prepared unit refused")));
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    let root = run_device(&mut editor, init(insert(), unit.clone(), &assets));
    let after = editor.project().snapshot();
    let project = editor.project();
    let restored = project.device(root).unwrap();
    assert_eq!(
        restored.state().asset_references()[0].asset.value(),
        assets[0].value()
    );
    assert_eq!(
        restored.comparison().unwrap().inactive().asset_references()[0]
            .asset
            .value(),
        assets[1].value()
    );
    assert_eq!(restored.comparison().unwrap().active(), DeviceVariant::B);
    assert_eq!(restored.state().output_gain_millidb(), -2400);
    assert!(restored.bypassed());
    let source = &restored.modulation().sources()[0];
    assert_eq!(source.color, ElementColor::Cyan);
    assert!(source.id.value() > root.value());
    assert_eq!(restored.modulation().routes()[0].source, source.id);
    assert_eq!(
        project.main_devices()[2].parent(),
        Some(project.main_devices()[1].id())
    );
    assert_eq!(project.capture_portable_device_unit(root).unwrap(), unit);
    let high_water = editor.project().snapshot().next_id;
    assert!(editor.undo());
    let mut expected = before;
    expected.next_id = high_water;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn replaces_retained_subtree_and_keeps_source_owner_and_unrelated_effect() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = add_track(&mut editor, "receiver");
    let other = add_track(&mut editor, "another contributor");
    let instrument = default_instrument_state().unwrap();
    let root = run_device(
        &mut editor,
        Command::InsertInstrument {
            track,
            state: instrument.clone(),
        },
    );
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    editor
        .execute(Command::BindTrackSource {
            track: other,
            source: Some(source),
        })
        .unwrap();
    let previous_child = run_device(
        &mut editor,
        Command::InsertNestedDevice {
            parent: root,
            state: effect(1),
        },
    );
    let sibling = run_device(
        &mut editor,
        Command::InsertDevice {
            track,
            before: None,
            state: effect(2),
        },
    );
    let target = DeviceAutomationTarget::HostOutputGain;
    let points = vec![crate::AutomationPoint {
        beat: crate::BeatTime::ZERO,
        value: crate::AutomationValue::new(0.5).unwrap(),
        outgoing: crate::AutomationInterpolation::Step,
    }];
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device: root,
            target,
            points: points.clone(),
        })
        .unwrap();
    let before = editor.project().snapshot();
    let template = PortableDeviceUnit::new(
        DeviceUnitClass::Instrument,
        vec![node(None, instrument.clone()), node(Some(0), effect(3))],
        Vec::new(),
    )
    .unwrap();
    editor
        .execute(init(
            Command::ReplaceDeviceState {
                device: root,
                state: instrument,
            },
            template.clone(),
            &[],
        ))
        .unwrap();
    let project = editor.project();
    let source_entry = project.native_source(source).unwrap();
    assert_eq!(source_entry.receiver(), Some(track));
    assert_eq!(source_entry.instrument().id(), root);
    assert_eq!(
        source_entry
            .instrument()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
    assert_eq!(project.track(other).unwrap().source_id(), Some(source));
    assert!(project.device(previous_child).is_none());
    assert_eq!(
        project.device_location(sibling).unwrap().owner,
        DeviceOwner::Track(track)
    );
    assert_eq!(
        project.capture_portable_device_unit(root).unwrap(),
        template
    );
    let child = source_entry.post_instrument_effects()[0].id();
    assert!(child.value() > previous_child.value());
    let high_water = editor.project().snapshot().next_id;
    assert!(editor.undo());
    let mut expected = before;
    expected.next_id = high_water;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
    assert!(editor.project().device(child).is_some());
}
