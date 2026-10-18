use super::super::{Row, TrackAssembly};
use super::*;
use crate::{AudioStorageLedgerReservation, AudioStorageRole, owned_shared::OwnedShared};
use hex_project::{BeatTime, Command, Created};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn persistent_arrangement_sampler_fields_move_without_inline_double_charge_or_hidden_aliases() {
    for routed in [false, true] {
        verify(routed, false);
    }
}
#[test]
fn persistent_arrangement_sampler_disposal_panic_keeps_pending_and_parent_failure() {
    verify(false, true);
}
fn verify(routed: bool, panic: bool) {
    let Some((bank, identity, mut editor)) =
        crate::shared_analytic::sampler_tests::canonical_fixture()
    else {
        eprintln!("unsupported persistent Arrangement sampler fields");
        return;
    };
    drop(identity);
    eprintln!("SUPPORTED persistent Arrangement sampler fields routed={routed} panic={panic}");
    let source = editor.project().tracks()[0].id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let destination = if routed {
        let state = editor
            .project()
            .track(source)
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
                source: Some(source),
            })
            .unwrap();
        destination
    } else {
        source
    };
    let revision = crate::PreparedRevision::prepare_with_audio_transforms_and_device_resources(
        editor.project(),
        &BTreeMap::new(),
        crate::SharedSineTable::new_on_control(),
        None,
        &crate::audio_transform::AudioTransformPreparation::default(),
        crate::native_device_resources::DeviceResourcePreparation {
            bank: &bank,
            max_scope_metadata_bytes: 65_536,
        },
    )
    .unwrap();
    let arrangement = Arc::clone(revision.arrangement());
    drop(revision);
    let Ok(mut remainder) = Arc::try_unwrap(arrangement) else {
        panic!("owned Arrangement fixture")
    };
    let source_track = remainder
        .tracks
        .iter_mut()
        .find(|track| track.track_id == Some(destination))
        .unwrap();
    // Extract only the actual already-paired instrument and canonical state.
    // Unqualified legacy meters/chains stay outside the selected native row.
    let mut instrument = source_track.instrument.take().unwrap();
    instrument.spans.try_reserve_exact(7).unwrap();
    instrument.nodes.try_reserve_exact(5).unwrap();
    let expected_spans =
        instrument.spans.capacity() * std::mem::size_of::<crate::midi_compile::CompiledNoteSpan>();
    let expected_nodes =
        instrument.nodes.capacity() * std::mem::size_of::<crate::midi_compile::SpanNode>();
    let spans_pointer = instrument.spans.as_ptr();
    let nodes_pointer = instrument.nodes.as_ptr();
    let state = source_track.instrument_state.take().unwrap();
    let (meter, meter_birth) =
        OwnedShared::new_owned(hex_lattice::DeviceMeter::default()).into_paired_parts_on_control();
    let (chain, chain_birth) =
        OwnedShared::<[crate::PreparedEffectInstance]>::new_owned_empty_slice()
            .into_paired_parts_on_control();
    let mut track =
        crate::track_output::CompiledTrackMix::prepare(1.0, false, 0.0, 0, None, None, None, false)
            .unwrap()
            .assemble(Vec::new(), Some(instrument), chain, meter);
    track.track_id = Some(destination);
    track.instrument_state = Some(state);
    track.instrument_id = editor
        .project()
        .track(destination)
        .unwrap()
        .instrument()
        .map(hex_project::Device::id);
    track.mutable_instrument = true;
    track.midi_source_index = routed.then_some(0);
    let mut row = Row {
        id: Some(destination),
        ordinal: 0,
        installed_ordinal: Some(0),
        track: Some(track),
        meter_declaration: Some(meter_birth),
        chain_declaration: Some(chain_birth),
        handles: [
            crate::audio_only_inputs::handle().unwrap(),
            crate::audio_only_inputs::handle().unwrap(),
        ],
        ..Default::default()
    };
    row.midi.panic_after_instrument_take = panic;
    let mut assembly = TrackAssembly {
        rows: vec![row],
        active: 1,
        phase: super::super::super::Phase::Installed,
        ..Default::default()
    };
    let limits = AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 4096,
        root_rows: 512,
        max_retained_bytes: 16 * 1024 * 1024,
        max_construction_peak_bytes: 16 * 1024 * 1024,
    };
    let inspect = |assembly: &TrackAssembly, omit: bool, budget: usize| {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                max_retained_bytes: budget,
                max_construction_peak_bytes: budget,
                ..limits
            })?;
        ledger.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)?;
        ledger.borrow_editor_native_states_on_control(&editor)?;
        crate::shared_analytic::resource_storage::visit_table(
            &remainder.shared_contexts,
            &mut ledger,
            AudioStorageRole::Caller,
        )?;
        for track in &remainder.tracks {
            if let Some(instrument) = &track.instrument {
                crate::midi_template::storage::visit_arrangement_instrument(
                    instrument,
                    &mut ledger,
                    AudioStorageRole::Caller,
                )?;
            }
            if let Some(state) = &track.instrument_state {
                ledger.borrow_canonical_native_state_on_control(state)?;
            }
        }
        ledger.root(assembly, AudioStorageRole::PendingReclaim)?;
        if !omit {
            ledger.mutate(|ledger| assembly.visit_audio_storage(ledger, &[]))?;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        ledger.summary()
    };
    let capacity = assembly.capacity(false).unwrap();
    assert_eq!(capacity.midi_span_bytes, expected_spans);
    assert_eq!(capacity.midi_node_bytes, expected_nodes);
    let before = inspect(&assembly, false, limits.max_retained_bytes).unwrap();
    assert!(before.require_complete().is_err());
    assert_eq!(
        inspect(&assembly, false, before.construction_peak_bytes).unwrap(),
        before
    );
    assert_eq!(
        inspect(&assembly, false, before.retained_bytes - 1),
        Err(AudioStorageError::ReservationExceeded)
    );
    assert!(inspect(&assembly, true, limits.max_retained_bytes).is_err());
    assembly.dispose_next(&mut []).unwrap(); // Whole preflight, then fixed owner moves.
    assert!(assembly.rows[0].track.is_none());
    let staged = assembly.rows[0].midi.instrument.as_ref().unwrap();
    assert_eq!(staged.spans.as_ptr(), spans_pointer);
    assert_eq!(staged.nodes.as_ptr(), nodes_pointer);
    assert_eq!(
        inspect(&assembly, false, limits.max_retained_bytes).unwrap(),
        before
    ); // Inline body charged by Row only.
    if panic {
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        custody.arrangement_clips.tracks = assembly;
        custody.arrangement_clips.phase = super::super::super::Phase::Installed;
        assert!(!custody.dispose_arrangement_clips_on_control());
        assert_eq!(
            custody.phase,
            crate::AudioPreparationPhase::FailedIncomplete
        );
        assert!(custody.first_panic.is_some());
        let failed = &mut custody.arrangement_clips.tracks;
        assert_eq!(failed.rows[0].midi.instrument_disposal, Disposal::Pending);
        assert!(failed.rows[0].midi.has_next());
        assert!(failed.rows[0].midi.instrument.is_none());
        assert_eq!(
            failed.dispose_next(&mut []),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            inspect(failed, false, limits.max_retained_bytes),
            Err(AudioStorageError::UnknownStorage)
        );
    } else {
        while assembly.has_next() {
            assembly.dispose_next(&mut []).unwrap();
            inspect(&assembly, false, limits.max_retained_bytes).unwrap();
        }
        assembly.clear_disposed().unwrap();
        assert!(assembly.rows[0].midi.vacant());
        assert_eq!(assembly.capacity(false).unwrap().midi_span_bytes, 0);
        assert_eq!(assembly.capacity(false).unwrap().midi_node_bytes, 0);
    }
    assert!(crate::audio_preparation::validate_constructor_project(editor.project()).is_err());
}
