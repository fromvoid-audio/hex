use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_project::{BeatTime, Command, Created};

#[test]
fn persistent_sampler_midi_retirement_preserves_each_move_capacity_and_live_alias() {
    for routed in [false, true] {
        verify_retirement(routed, false);
    }
}

#[test]
fn fresh_compiler_maps_and_copied_maps_retire_without_invented_copy_transfers() {
    for fresh_maps in [false, true] {
        for routed in [false, true] {
            verify_retirement(routed, fresh_maps);
        }
    }
}

fn verify_retirement(routed: bool, fresh_maps: bool) {
    let Some((bank, _, mut editor)) = crate::shared_analytic::sampler_tests::canonical_fixture()
    else {
        eprintln!("unsupported persistent sampler MIDI retirement");
        return;
    };
    eprintln!(
        "SUPPORTED persistent sampler MIDI retirement routed={routed} fresh_maps={fresh_maps}"
    );
    let track = editor.project().tracks()[0].id();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "sample".into(),
            source_length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("cell")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    if routed {
        let state = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone();
        let Created::Track(destination) = editor
            .execute(Command::AddMidiTrack {
                name: "destination".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = editor
            .project()
            .track(destination)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        editor
            .execute(Command::SetMidiSource {
                destination,
                source: Some(track),
            })
            .unwrap();
    }
    let registry = hex_devices::native_instrument_registry().unwrap();
    let contexts = crate::shared_analytic::SharedAnalyticContextTable::prepare(
        editor.project(),
        &registry,
        None,
        Some(crate::native_device_resources::DeviceResourcePreparation {
            bank: &bank,
            max_scope_metadata_bytes: 65_536,
        }),
    )
    .unwrap();
    let mut inventory = PreparedSessionInventory::prepare_with_media_and_shared_contexts(
        editor.project(),
        &BTreeMap::new(),
        crate::SharedSineTable::new_on_control(),
        None,
        &registry,
        contexts,
    )
    .unwrap();
    let project_meter = editor.project().meter_map().points().as_ptr();
    let project_tempo = editor.project().tempo_map().points().as_ptr();
    assert_ne!(inventory.meter_map.points().as_ptr(), project_meter);
    let expected = [
        bytes(&inventory.midi_clips).unwrap(),
        bytes(&inventory.routed_midi_clips).unwrap(),
        bytes(&inventory.midi_source_roots).unwrap(),
        bytes(&inventory.mutable_instruments).unwrap(),
    ];
    let pointer = inventory.midi_clips.as_ptr();
    let rows = inventory.midi_clips.len() + inventory.routed_midi_clips.len();
    let factories = inventory.mutable_instruments.len();
    let mut assembly = SessionAssembly {
        phase: AssemblyPhase::Installed,
        ..Default::default()
    };
    let mut tempo = SessionTempoConstruction::default();
    if !fresh_maps {
        // The copied case uses actual transfers, not a phase/receipt invented
        // for the compiler's independent maps. Setup overlap is not a peak proof.
        assembly
            .meter_copy
            .prepare_from(editor.project().meter_map())
            .unwrap();
        let mut copied_meter = None;
        assembly
            .meter_copy
            .transfer_into(&mut copied_meter)
            .unwrap();
        let copied_meter = copied_meter.unwrap();
        drop(std::mem::replace(&mut inventory.meter_map, copied_meter));
        tempo.reserve(editor.project()).unwrap();
        tempo.construct().unwrap();
        let mut copied_timing = None;
        assert!(tempo.commit_inventory_transfer(&mut copied_timing));
        drop(std::mem::replace(
            &mut inventory.timing,
            copied_timing.unwrap(),
        ));
    }
    assert_eq!(
        inventory.timing.beat_to_frame(BeatTime::ONE).unwrap(),
        editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::ONE)
            .unwrap()
    );
    let mut source = Some(inventory);
    assert!(assembly.can_restore_revision());
    assembly.restore_revision(&mut source);
    assert!(source.is_none());
    assert!(assembly.can_transfer_revision());
    assembly.transfer_revision(&mut source);
    assert_eq!(source.as_ref().unwrap().midi_clips.as_ptr(), pointer);
    assert!(assembly.can_restore_revision());
    assembly.restore_revision(&mut source);
    let budget = AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 512,
        root_rows: 128,
        max_retained_bytes: 2_000_000,
        max_construction_peak_bytes: 2_000_000,
    };
    let inspect =
        |assembly: &SessionAssembly, tempo: &SessionTempoConstruction, omit_pending: bool| {
            let c = assembly.capacity(false).unwrap();
            assert_eq!(
                [
                    c.midi_bytes,
                    c.routed_midi_bytes,
                    c.midi_source_root_bytes,
                    c.mutable_bytes
                ],
                if assembly.phase == AssemblyPhase::Vacant {
                    [0; 4]
                } else {
                    expected
                }
            );
            let mut l = AudioRootStorageLedger::try_new_on_control(budget).unwrap();
            l.root(assembly, AudioStorageRole::PendingReclaim).unwrap();
            l.root(tempo, AudioStorageRole::PendingReclaim).unwrap();
            tempo.visit_audio_storage(&mut l).unwrap();
            l.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
                .unwrap();
            l.borrow_editor_midi_sources_on_control(&editor).unwrap();
            l.borrow_editor_native_states_on_control(&editor).unwrap();
            if omit_pending {
                // Independent omitted-physical-field oracle: visit all the
                // remaining rows/factories and the real context owner, while
                // deliberately leaving out the newly occupied pending slot.
                visit_midi_rows(
                    &assembly.unused_midi,
                    &mut l,
                    AudioStorageRole::PendingReclaim,
                )
                .unwrap();
                visit_midi_rows(
                    &assembly.unused_routed_midi,
                    &mut l,
                    AudioStorageRole::PendingReclaim,
                )
                .unwrap();
                for row in &assembly.unused_mutable {
                    row.factory.visit_storage(&mut l).unwrap();
                }
            } else {
                assembly
                    .visit_audio_storage(&mut l, None, AudioStorageRole::PendingReclaim)
                    .unwrap();
            }
            if let Some(contexts) = assembly
                .inventory
                .as_ref()
                .map(|i| &i.shared_contexts)
                .or(assembly.contexts.as_ref())
            {
                crate::shared_analytic::resource_storage::visit_table(
                    contexts,
                    &mut l,
                    AudioStorageRole::PendingReclaim,
                )
                .unwrap();
            }
            if omit_pending {
                assert_eq!(
                    l.validate_selected_shared_handles_on_control(),
                    Err(AudioStorageError::UnknownStorage)
                );
            } else {
                l.validate_selected_shared_handles_on_control().unwrap();
            }
            assert!(l.summary().unwrap().require_complete().is_err());
        };
    inspect(&assembly, &tempo, false);
    let mut pending_rows = 0;
    let mut pending_factories = 0;
    for _ in 0..128 {
        if !assembly.has_next_disposal() {
            break;
        }
        assembly.dispose_next().unwrap();
        pending_rows += usize::from(assembly.pending_midi.is_some());
        pending_factories += usize::from(assembly.pending_mutable.is_some());
        assert!(!assembly.can_restore_revision());
        inspect(&assembly, &tempo, false);
        if assembly.pending_midi.is_some() || assembly.pending_mutable.is_some() {
            inspect(&assembly, &tempo, true);
        }
    }
    assert_eq!(assembly.phase, AssemblyPhase::Complete);
    assert_eq!(pending_rows, rows);
    assert_eq!(pending_factories, factories);
    assert!(assembly.unused_midi.is_empty());
    assert!(assembly.unused_routed_midi.is_empty());
    assert!(assembly.unused_mutable.is_empty());
    // Actual parent return/retirement, not direct fixture destruction.
    assert_eq!(assembly.clear_disposed(), Err(AudioStorageError::Capacity));
    assembly.return_timing(&mut tempo).unwrap();
    assert_eq!(assembly.timing_disposal, Step::Complete);
    assert_eq!(assembly.meter_disposal, Step::Complete);
    assert_eq!(tempo.prepared.is_some(), !fresh_maps);
    inspect(&assembly, &tempo, false);
    tempo.cleanup().unwrap();
    assembly.clear_disposed().unwrap();
    inspect(&assembly, &tempo, false);
    assert_eq!(assembly.phase, AssemblyPhase::Vacant);
    assert!(editor.project().session_midi_clip(clip).is_some());
    assert_eq!(
        editor.project().meter_map().points().as_ptr(),
        project_meter
    );
    assert_eq!(
        editor.project().tempo_map().points().as_ptr(),
        project_tempo
    );
    assert_eq!(
        editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::ONE)
            .unwrap(),
        22_050
    );
    assert!(crate::audio_preparation::validate_constructor_project(editor.project()).is_err());
}
