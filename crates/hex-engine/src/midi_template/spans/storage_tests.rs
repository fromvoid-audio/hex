use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole,
    owned_shared::OwnedShared,
};
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 32,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn templates() -> (
    OwnedShared<PreparedMidiSourceTemplate>,
    OwnedShared<PreparedMidiInstrumentTemplate>,
) {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "template storage".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
    let clip = &project.track(track).unwrap().midi_clips()[0];
    let mut source = PreparedMidiSourceTemplate::prepare(track, clip, &timing).unwrap();
    let mut instrument =
        PreparedMidiInstrumentTemplate::prepare_with_instrument(&source, &SilentInstrument)
            .unwrap();
    // Nonzero spare backing on empty and populated arrays is real storage.
    source.intro.reserve_exact(3);
    source.repeats.reserve_exact(3);
    source.intro_spans.reserve_exact(3);
    source.repeat_spans.reserve_exact(3);
    instrument.intro.reserve_exact(3);
    instrument.repeats.reserve_exact(3);
    instrument.intro_nodes.reserve_exact(3);
    instrument.repeat_nodes.reserve_exact(3);
    (
        OwnedShared::new_owned(source),
        OwnedShared::new_owned(instrument),
    )
}
#[test]
fn actual_template_receipts_cover_all_spare_buffers_and_require_every_owning_alias() {
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let (source, instrument) = templates();
    let source_alias = source.clone_on_control();
    let instrument_alias = instrument.clone_on_control();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .mutate(|ledger| {
            crate::midi_template::storage::visit_source(
                &source,
                ledger,
                AudioStorageRole::Current,
            )?;
            crate::midi_template::storage::visit_instrument(
                &instrument,
                ledger,
                AudioStorageRole::Current,
            )
        })
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .mutate(|ledger| {
            crate::midi_template::storage::visit_source(
                &source_alias,
                ledger,
                AudioStorageRole::Archive,
            )?;
            crate::midi_template::storage::visit_instrument(
                &instrument_alias,
                ledger,
                AudioStorageRole::Archive,
            )
        })
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 13);
    assert_eq!(summary.data_allocations, before.data_allocations);
    assert_eq!(
        summary.data_bytes - before.data_bytes,
        std::mem::size_of_val(&source_alias) + std::mem::size_of_val(&instrument_alias)
    );
    let source_parts = source.parts().unwrap();
    let instrument_parts = instrument.parts().unwrap();
    let context_parts = instrument.shared_contexts.parts().unwrap();
    let expected = 2 * (std::mem::size_of_val(&source) + std::mem::size_of_val(&instrument))
        + source_parts.0
        + source_parts.1
        + instrument_parts.0
        + instrument_parts.1
        + source.intro.capacity() * std::mem::size_of::<super::super::SourceEdge>()
        + source.repeats.capacity() * std::mem::size_of::<super::super::SourceEdge>()
        + source.intro_spans.capacity() * std::mem::size_of::<SourceSpan>()
        + source.repeat_spans.capacity() * std::mem::size_of::<SourceSpan>()
        + instrument.intro.capacity() * std::mem::size_of::<VoiceSpan>()
        + instrument.repeats.capacity() * std::mem::size_of::<VoiceSpan>()
        + instrument.intro_nodes.capacity() * std::mem::size_of::<IntervalNode>()
        + instrument.repeat_nodes.capacity() * std::mem::size_of::<IntervalNode>()
        + instrument.bank.storage_bytes().unwrap()
        - std::mem::size_of_val(&instrument.bank)
        + context_parts.0
        + context_parts.1;
    assert_eq!(summary.data_bytes, expected);
    drop(ledger);
    let inventory = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        ledger.mutate(|ledger| {
            crate::midi_template::storage::visit_source(
                &source,
                ledger,
                AudioStorageRole::Current,
            )?;
            crate::midi_template::storage::visit_instrument(
                &instrument,
                ledger,
                AudioStorageRole::Current,
            )?;
            crate::midi_template::storage::visit_source(
                &source_alias,
                ledger,
                AudioStorageRole::Archive,
            )?;
            crate::midi_template::storage::visit_instrument(
                &instrument_alias,
                ledger,
                AudioStorageRole::Archive,
            )
        })?;
        ledger.validate_closed_shared_handles()?;
        ledger.summary()
    };
    assert_eq!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: summary.retained_bytes,
            max_construction_peak_bytes: summary.construction_peak_bytes,
            ..reservation()
        })
        .unwrap(),
        summary
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: summary.retained_bytes - 1,
            ..reservation()
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_construction_peak_bytes: summary.construction_peak_bytes - 1,
            ..reservation()
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            handle_rows: 1,
            ..reservation()
        })
        .is_err()
    );
}
#[test]
fn raw_outer_template_arcs_cannot_adopt_legitimate_nested_bank_or_context_declarations() {
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let (source, instrument) = templates();
    let (arc, _) = source.into_paired_parts_on_control();
    let raw_source = OwnedShared::untracked(arc);
    let (arc, _) = instrument.into_paired_parts_on_control();
    let raw_instrument = OwnedShared::untracked(arc);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert!(
        ledger
            .mutate(|ledger| crate::midi_template::storage::visit_source(
                &raw_source,
                ledger,
                AudioStorageRole::Current
            ))
            .is_err()
    );
    assert!(ledger.summary().is_err());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert!(
        ledger
            .mutate(|ledger| crate::midi_template::storage::visit_instrument(
                &raw_instrument,
                ledger,
                AudioStorageRole::Current
            ))
            .is_err()
    );
    assert!(ledger.summary().is_err());
}
