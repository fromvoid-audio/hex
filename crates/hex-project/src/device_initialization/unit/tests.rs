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

#[test]
fn replacement_unit_allows_cross_type_while_same_provider_unit_still_refuses() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = add_track(&mut editor, "fx");
    let provider_a =
        NativeDeviceState::new(DeviceTypeId::from_static("test.provider.a"), 1, vec![1]).unwrap();
    let root = run_device(
        &mut editor,
        Command::InsertDevice {
            track,
            before: None,
            state: provider_a,
        },
    );
    let old_child = run_device(
        &mut editor,
        Command::InsertNestedDevice {
            parent: root,
            state: effect(1),
        },
    );

    let unit_b = PortableDeviceUnit::new(
        DeviceUnitClass::Effect,
        vec![node(None, effect(42)), node(Some(0), effect(7))],
        Vec::new(),
    )
    .unwrap();

    // Same-provider preset load (`new_unit`) must still refuse replacing type A with unit B.
    let same_provider_cmd = Command::InitializeDevice(Box::new(
        super::super::DeviceInitialization::new_unit(
            Command::ReplaceDeviceState {
                device: root,
                state: effect(0),
            },
            unit_b.clone(),
            Vec::new(),
        )
        .unwrap(),
    ));
    assert_eq!(
        editor.execute(same_provider_cmd),
        Err(Invalid(
            "portable unit does not match the replacement target"
        ))
    );

    // Cross-type replacement default (`new_replacement_unit`) must still reject a class mismatch.
    let inst_unit = PortableDeviceUnit::new(
        DeviceUnitClass::Instrument,
        vec![node(None, effect(42))],
        Vec::new(),
    )
    .unwrap();
    let wrong_class_cmd = Command::InitializeDevice(Box::new(
        super::super::DeviceInitialization::new_replacement_unit(
            Command::ReplaceDeviceState {
                device: root,
                state: effect(0),
            },
            inst_unit,
            Vec::new(),
        )
        .unwrap(),
    ));
    assert_eq!(
        editor.execute(wrong_class_cmd),
        Err(Invalid(
            "portable unit does not match the replacement target"
        ))
    );

    // Cross-type replacement default (`new_replacement_unit`) succeeds when class matches.
    let before = editor.project().snapshot();
    let replacement_cmd = Command::InitializeDevice(Box::new(
        super::super::DeviceInitialization::new_replacement_unit(
            Command::ReplaceDeviceState {
                device: root,
                state: effect(0),
            },
            unit_b.clone(),
            Vec::new(),
        )
        .unwrap(),
    ));
    editor.execute(replacement_cmd).unwrap();
    assert_eq!(editor.project().device(root).unwrap().state(), &effect(42));
    assert!(editor.project().device(old_child).is_none());
    assert_eq!(
        editor.project().capture_portable_device_unit(root).unwrap(),
        unit_b
    );

    let high_water = editor.project().snapshot().next_id;
    assert!(editor.undo());
    let mut expected = before;
    expected.next_id = high_water;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
    assert_eq!(
        editor.project().capture_portable_device_unit(root).unwrap(),
        unit_b
    );
}

#[test]
fn registers_fresh_and_reuses_existing_originals_in_one_edit_with_reserved_ids_snapshot_and_undo() {
    use super::super::{RegisterUnitOriginal, ReservedUnitOriginal, UnitAssetResolution};
    use std::path::PathBuf;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let _clip_asset = {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("ordinary-clip.wav"),
                sample_rate: 48_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("clip asset")
        };
        id
    };
    let existing_id = {
        let Created::Asset(id) = editor
            .execute(Command::ImportDeviceAsset {
                path: PathBuf::from("existing-original.wav"),
                sample_rate: 44_100,
                frames: 24,
            })
            .unwrap()
        else {
            panic!("existing original")
        };
        id
    };
    let existing_facts = asset_facts(11);
    let fresh_facts = asset_facts(22);
    let mut snapshot = editor.project().snapshot();
    snapshot
        .assets
        .iter_mut()
        .find(|asset| asset.id == existing_id.value())
        .unwrap()
        .original_source = Some(existing_facts);
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();

    let existing_entry = PortableDeviceAsset {
        source_rate: 44_100,
        source_frames: 24,
        facts: existing_facts,
    };
    let fresh_entry = PortableDeviceAsset {
        source_rate: 96_000,
        source_frames: 32,
        facts: fresh_facts,
    };
    let unit = PortableDeviceUnit::new(
        DeviceUnitClass::Effect,
        vec![
            root_with_media(),
            // Child shares manifest entry 2 with the root's inactive B bank to prove deduplication.
            node(Some(0), bound(effect(6), 2)),
        ],
        vec![existing_entry, fresh_entry],
    )
    .unwrap();
    assert_eq!(
        editor.project().find_portable_original(existing_entry),
        Some(existing_id)
    );
    assert_eq!(editor.project().find_portable_original(fresh_entry), None);

    let resolutions = vec![
        UnitAssetResolution::Existing(existing_id),
        UnitAssetResolution::Register(RegisterUnitOriginal {
            path: PathBuf::from("preset-store/fresh-original.wav"),
            source_rate: 96_000,
            source_frames: 32,
            facts: fresh_facts,
        }),
    ];
    let insert = || Command::InsertMainDevice {
        before: None,
        state: effect(0),
    };
    let expected_fresh_id = AssetId(editor.project().next_identity());
    let initialization = super::super::DeviceInitialization::new_unit_with_originals(
        editor.project(),
        insert(),
        unit.clone(),
        resolutions,
    )
    .unwrap();
    assert_eq!(
        initialization.resolved_unit_assets(),
        &[existing_id, expected_fresh_id]
    );
    assert_eq!(
        initialization.reserved_unit_originals(),
        &[ReservedUnitOriginal {
            manifest_index: 1,
            asset: expected_fresh_id,
        }]
    );

    let before = editor.project().snapshot();
    // Refused audio preparation must leave project, assets, next_id and history untouched.
    let refused = editor.execute_prepared_transition(
        Command::InitializeDevice(Box::new(initialization.clone())),
        |old, candidate| {
            assert!(old.asset(expected_fresh_id).is_none());
            let staged = candidate.asset(expected_fresh_id).unwrap();
            assert_eq!(staged.original_source(), Some(fresh_facts));
            assert_eq!(staged.source_sample_rate(), 96_000);
            assert_eq!(staged.source_frames(), 32);
            Err::<(), _>(Invalid("prepared audio refused"))
        },
    );
    assert_eq!(refused, Err(Invalid("prepared audio refused")));
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());

    // Accepted audio preparation binds both A/B states and child to the exact destination IDs.
    let (Created::Device(root), prepared_fresh_id) = editor
        .execute_prepared_transition(
            Command::InitializeDevice(Box::new(initialization)),
            |old, candidate| {
                assert!(old.asset(expected_fresh_id).is_none());
                let staged = candidate.asset(expected_fresh_id).unwrap();
                assert_eq!(staged.original_source(), Some(fresh_facts));
                Ok::<_, Invalid>(staged.id())
            },
        )
        .unwrap()
    else {
        panic!("root device")
    };
    assert_eq!(prepared_fresh_id, expected_fresh_id);

    let after = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap(),
        *editor.project()
    );
    let project = editor.project();
    assert_eq!(project.assets().count(), 3);
    let fresh_asset = project.asset(expected_fresh_id).unwrap();
    assert_eq!(
        fresh_asset.path(),
        std::path::Path::new("preset-store/fresh-original.wav")
    );
    assert_eq!(fresh_asset.source_sample_rate(), 96_000);
    assert_eq!(fresh_asset.source_frames(), 32);
    assert_eq!(fresh_asset.original_source(), Some(fresh_facts));
    assert_eq!(
        project.find_portable_original(fresh_entry),
        Some(expected_fresh_id)
    );

    let restored = project.device(root).unwrap();
    assert_eq!(
        restored.state().asset_references()[0].asset.value(),
        existing_id.value()
    );
    assert_eq!(
        restored.comparison().unwrap().inactive().asset_references()[0]
            .asset
            .value(),
        expected_fresh_id.value()
    );
    let child = &project.main_devices()[1];
    assert_eq!(child.parent(), Some(root));
    assert_eq!(
        child.state().asset_references()[0].asset.value(),
        expected_fresh_id.value()
    );
    assert_eq!(project.capture_portable_device_unit(root).unwrap(), unit);

    // One undo removes both the installed unit and the freshly registered original.
    let high_water = after.next_id;
    assert!(editor.undo());
    let mut expected_undone = before;
    expected_undone.next_id = high_water;
    assert_eq!(editor.project().snapshot(), expected_undone);
    assert!(editor.project().asset(expected_fresh_id).is_none());
    assert!(editor.project().asset(existing_id).is_some());

    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn refuses_invalid_duplicate_wrong_original_and_stale_reserved_unit_originals() {
    use super::super::{RegisterUnitOriginal, UnitAssetResolution};
    use std::path::PathBuf;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let clip_id = {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("ordinary-clip.wav"),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("clip asset")
        };
        id
    };
    let existing_id = {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("stamped-original.wav"),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("stamped original")
        };
        id
    };
    let mut snapshot = editor.project().snapshot();
    snapshot
        .assets
        .iter_mut()
        .find(|asset| asset.id == existing_id.value())
        .unwrap()
        .original_source = Some(asset_facts(1));
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let before = editor.project().snapshot();

    let unit = PortableDeviceUnit::new(
        DeviceUnitClass::Effect,
        vec![root_with_media()],
        vec![
            PortableDeviceAsset {
                source_rate: 48_000,
                source_frames: 16,
                facts: asset_facts(1),
            },
            PortableDeviceAsset {
                source_rate: 48_000,
                source_frames: 16,
                facts: asset_facts(2),
            },
        ],
    )
    .unwrap();
    let insert = || Command::InsertMainDevice {
        before: None,
        state: effect(0),
    };
    let valid_register = || {
        UnitAssetResolution::Register(RegisterUnitOriginal {
            path: PathBuf::from("fresh.wav"),
            source_rate: 48_000,
            source_frames: 16,
            facts: asset_facts(2),
        })
    };

    // Ordinary clip asset (no original_source) cannot satisfy Existing.
    assert_eq!(
        super::super::DeviceInitialization::new_unit_with_originals(
            editor.project(),
            insert(),
            unit.clone(),
            vec![UnitAssetResolution::Existing(clip_id), valid_register()],
        )
        .unwrap_err(),
        Invalid("destination original audio does not match the preset")
    );

    // Duplicate registration of an original already in the project is refused.
    assert_eq!(
        super::super::DeviceInitialization::new_unit_with_originals(
            editor.project(),
            insert(),
            unit.clone(),
            vec![
                UnitAssetResolution::Register(RegisterUnitOriginal {
                    path: PathBuf::from("dup.wav"),
                    source_rate: 48_000,
                    source_frames: 16,
                    facts: asset_facts(1),
                }),
                valid_register(),
            ],
        )
        .unwrap_err(),
        Invalid("destination original audio is already registered")
    );

    // Empty path or mismatched metadata/digest is refused.
    assert_eq!(
        super::super::DeviceInitialization::new_unit_with_originals(
            editor.project(),
            insert(),
            unit.clone(),
            vec![
                UnitAssetResolution::Existing(existing_id),
                UnitAssetResolution::Register(RegisterUnitOriginal {
                    path: PathBuf::new(),
                    source_rate: 48_000,
                    source_frames: 16,
                    facts: asset_facts(2),
                }),
            ],
        )
        .unwrap_err(),
        Invalid("media needs a path and frames")
    );
    assert_eq!(
        super::super::DeviceInitialization::new_unit_with_originals(
            editor.project(),
            insert(),
            unit.clone(),
            vec![
                UnitAssetResolution::Existing(existing_id),
                UnitAssetResolution::Register(RegisterUnitOriginal {
                    path: PathBuf::from("wrong.wav"),
                    source_rate: 48_000,
                    source_frames: 16,
                    facts: asset_facts(99),
                }),
            ],
        )
        .unwrap_err(),
        Invalid("registered original audio does not match the preset")
    );

    // Stale reserved ID refuses if another edit advances next_id before execution.
    let stale = super::super::DeviceInitialization::new_unit_with_originals(
        editor.project(),
        insert(),
        unit,
        vec![UnitAssetResolution::Existing(existing_id), valid_register()],
    )
    .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());

    let _ = add_track(&mut editor, "advances next_id");
    let after_track = editor.project().snapshot();
    assert_eq!(
        editor.execute(Command::InitializeDevice(Box::new(stale))),
        Err(Invalid("reserved original asset identity is stale"))
    );
    assert_eq!(editor.project().snapshot(), after_track);
}
