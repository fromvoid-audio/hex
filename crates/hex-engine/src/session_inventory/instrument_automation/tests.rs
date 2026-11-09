use super::*;
use hex_lattice::{NativeInstrumentClock, NativeParameterAutomation};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue, BeatTime,
    Command, Created, Editor, Project,
};
fn fixture() -> (Editor, hex_project::DeviceId) {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: registry
                .default_state(hex_devices::fm4::device::FM4_TYPE_ID)
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    for operator in 1..=4 {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: hex_devices::fm4::device::parameter::carrier(operator),
                points: vec![point(BeatTime::ZERO, 0.0), point(BeatTime::ONE, 1.0)],
            })
            .unwrap();
    }
    (editor, device)
}
fn point(beat: BeatTime, value: f32) -> AutomationPoint {
    AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    }
}
fn compile(editor: &Editor, id: hex_project::DeviceId) -> Result<Option<Program>, Invalid> {
    let device = editor.project().device(id).unwrap();
    let registry = hex_devices::native_instrument_registry().unwrap();
    let definition = registry.definition(device.state().type_id()).unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let instrument = registry
        .prepare_instrument(
            device.state(),
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let factory = instrument.prepare_mutable_runtime().unwrap().unwrap();
    prepare(device, definition, factory.as_ref())
}
struct Clock;
impl NativeInstrumentClock for Clock {
    fn tempo_micro_bpm_at_offset(&self, _: usize) -> Option<u32> {
        Some(120_000_000)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some(offset as u64 * (BeatTime::ONE.raw() / 8))
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.beat_at_offset(offset)
    }
}
#[test]
fn four_carrier_program_uses_canonical_mapping_touch_base_and_content_clock() {
    let (mut editor, id) = fixture();
    let carrier = hex_devices::fm4::device::parameter::carrier(1);
    // The neutral canonical lane geometry can be compiled independently.
    // Project source-touch admission intentionally stays closed in this PR.
    let mut lanes = editor.project().device(id).unwrap().automation().to_vec();
    lanes[0].touch = vec![hex_project::DeviceAutomationTouchSpan {
        start: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
        end: BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 4),
        steps: vec![hex_project::DeviceAutomationTouchStep {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
            value: AutomationValue::new(0.875).unwrap(),
        }],
    }];
    let registry = hex_devices::native_instrument_registry().unwrap();
    let state = editor.project().device(id).unwrap().state();
    let tuning = hex_lattice::TuningTable::default();
    let prepared = registry
        .prepare_instrument(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap();
    let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
    let program = prepare_lanes(
        id,
        state,
        &lanes,
        registry.definition(state.type_id()).unwrap(),
        factory.as_ref(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(program.device, id);
    let block = program.values.block(Some(&Clock), 9);
    assert_eq!(
        block.parameter_ids(),
        &[29, 30, 31, 32].map(ParameterId::from_static)
    );
    assert_eq!(
        block.value_at(carrier, 0),
        Some(ParameterValue::Float(-1.0))
    );
    assert_eq!(
        block.value_at(carrier, 2),
        Some(ParameterValue::Float(0.75))
    );
    assert_eq!(block.value_at(carrier, 6), Some(ParameterValue::Float(0.5)));
    assert_eq!(block.value_at(carrier, 8), Some(ParameterValue::Float(1.0)));
    assert_eq!(program.values.block(None, 9).value_at(carrier, 2), None);
    editor
        .execute(Command::SetDeviceAutomationReadSelection {
            device: id,
            target: DeviceAutomationTarget::Parameter(carrier),
            selection: AutomationReadSelection::Base,
        })
        .unwrap();
    let base = compile(&editor, id).unwrap().unwrap();
    let block = base.values.block(Some(&Clock), 9);
    assert_eq!(block.parameter_ids().len(), 4);
    assert_eq!(block.value_at(carrier, 2), None);
    assert_eq!(
        block.value_at(ParameterId::from_static(30), 2),
        Some(ParameterValue::Float(-0.5))
    );
    assert!(crate::project_device_automation_target_supported(
        editor.project(),
        id,
        DeviceAutomationTarget::Parameter(carrier)
    ));
    crate::device_automation::validate_project(editor.project()).unwrap();
}
#[test]
fn retained_storage_fits_quote_and_visits_once_independently_of_inventory_marker() {
    let (editor, id) = fixture();
    let program = compile(&editor, id).unwrap().unwrap();
    let (retained, overlap) = quote(editor.project().device(id).unwrap()).unwrap();
    let actual = program.values.storage_bytes().unwrap();
    assert!(actual <= retained);
    assert!(overlap > retained);
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 8,
            handle_rows: 0,
            root_rows: 4,
            max_retained_bytes: 1024 * 1024,
            max_construction_peak_bytes: 1024 * 1024,
        })
        .unwrap();
    let inventory_marker = ();
    assert!(ledger.native_payload_once(&inventory_marker).unwrap());
    program.visit(&mut ledger).unwrap();
    let first = ledger.summary().unwrap();
    assert_eq!(first.data_bytes, actual);
    program.visit(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap(), first);
    assert!(add_budget(usize::MAX, 1).is_err());
    assert!(add_budget(MAX_PROGRAM_BYTES, 1).is_err());
}
#[test]
fn real_factory_grants_refuse_other_parameters_and_host_targets() {
    for target in [
        DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut editor, id) = fixture();
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device: id,
                target,
                points: vec![point(BeatTime::ZERO, 0.5)],
            })
            .unwrap();
        assert!(compile(&editor, id).is_err());
    }
}

#[test]
fn shared_contributors_compile_only_the_receiver_and_unbound_tracks_compile_nothing() {
    let (mut editor, id) = fixture();
    let first = editor.project().tracks()[0].id();
    let source = editor.project().track(first).unwrap().source_id().unwrap();
    let Created::Track(second) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: second,
            source: Some(source),
        })
        .unwrap();
    let registry = hex_devices::native_instrument_registry().unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let factory = registry
        .prepare_instrument(
            editor.project().device(id).unwrap().state(),
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &tuning,
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_mutable_runtime()
        .unwrap()
        .unwrap();
    let mut bytes = 0;
    let mut programs = Vec::new();
    for track in editor.project().tracks() {
        if let Some(program) =
            prepare_for_receiver(track, Some(factory.as_ref()), &registry, None, &mut bytes)
                .unwrap()
        {
            programs.push(program);
        }
    }
    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].device, id);
    assert_eq!(bytes, programs[0].values.storage_bytes().unwrap());
    for track in [first, second] {
        editor
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    assert!(
        editor.project().native_source(source).is_some(),
        "disconnected canonical source remains retained"
    );
    let mut bytes = 0;
    for track in editor.project().tracks() {
        assert!(
            prepare_for_receiver(track, Some(factory.as_ref()), &registry, None, &mut bytes)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(bytes, 0);
}

#[test]
fn program_visitor_requires_one_root_and_four_data_rows_including_empty_touch() {
    let (editor, id) = fixture();
    let program = compile(&editor, id).unwrap().unwrap();
    assert_eq!(program.values.touch_storage().capacity(), 0);
    for (roots, rows, accepted) in [(1, 4, true), (0, 4, false), (1, 3, false)] {
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: rows,
                handle_rows: 0,
                root_rows: roots,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            },
        )
        .unwrap();
        assert_eq!(program.visit(&mut ledger).is_ok(), accepted);
        if accepted {
            let first = ledger.summary().unwrap();
            program.visit(&mut ledger).unwrap();
            assert_eq!(ledger.summary().unwrap(), first);
        }
    }
}
#[test]
fn ordinary_construction_quote_counts_shared_receiving_program_once() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let (mut editor, device) = fixture();
            let receiver = editor.project().tracks()[0].id();
            let source = editor
                .project()
                .track(receiver)
                .unwrap()
                .source_id()
                .unwrap();
            let expected = construction_bytes(editor.project(), None).unwrap();
            let full = crate::PreparedRevision::device_handoff_metadata_construction_bytes(
                editor.project(),
            )
            .unwrap();
            let prepare = |editor: &Editor| {
                crate::PreparedRevision::prepare(
                    editor.project(),
                    &std::collections::BTreeMap::new(),
                    std::sync::Arc::new(hex_devices::SineTable::new()),
                    None,
                )
                .unwrap()
            };
            let revision = prepare(&editor);
            assert!(revision.session().instrument_automation_storage_bytes() > 0);
            assert!(revision.session().instrument_automation_storage_bytes() <= expected);
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::BindTrackSource {
                    track: contributor,
                    source: Some(source),
                })
                .unwrap();
            assert_eq!(
                construction_bytes(editor.project(), None).unwrap(),
                expected
            );
            let shared = prepare(&editor);
            assert_eq!(
                shared.session().instrument_automation_storage_bytes(),
                revision.session().instrument_automation_storage_bytes()
            );
            assert!(
                shared
                    .session()
                    .instrument_automation(receiver, device)
                    .is_some()
            );
            assert!(
                shared
                    .session()
                    .instrument_automation(contributor, device)
                    .is_none()
            );
            // The existing predecessor evidence quote is unchanged when only curves
            // disappear. Native construction contributes exactly its separate peak.
            let (mut plain, id) = fixture();
            for operator in 1..=4 {
                plain
                    .execute(Command::ReplaceDeviceAutomation {
                        device: id,
                        parameter: hex_devices::fm4::device::parameter::carrier(operator),
                        points: vec![],
                    })
                    .unwrap();
            }
            assert!(
                crate::AudioCandidateReservationQuote::for_project(plain.project(), 0).is_err(),
                "ordinary allowance does not widen closed FM4 admission"
            );
            let without = crate::PreparedRevision::device_handoff_metadata_construction_bytes(
                plain.project(),
            )
            .unwrap();
            assert_eq!(full.checked_sub(without), Some(expected));
            let limits = reservation();
            assert_eq!(limits.root_rows, hex_project::MAX_TRACKS);
            assert_eq!(limits.data_rows, hex_project::MAX_TRACKS * 4);
            assert_eq!(limits.handle_rows, 0);
            assert!(expected <= limits.max_construction_peak_bytes);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn native_quote_does_not_charge_opaque_foreign_source_programs() {
    let (mut editor, device) = fixture();
    for operator in 1..=4 {
        editor
            .execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: hex_devices::fm4::device::parameter::carrier(operator),
                points: vec![],
            })
            .unwrap();
    }
    // Unknown foreign payloads remain generic project state. The foreign owner
    // separately validates/quotes them; native quotation must not reinterpret it.
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::VST3_DEVICE_TYPE_ID,
                1,
                vec![1],
            )
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(29),
            points: vec![point(BeatTime::ZERO, 0.5)],
        })
        .unwrap();
    assert_eq!(construction_bytes(editor.project(), None).unwrap(), 0);
}
